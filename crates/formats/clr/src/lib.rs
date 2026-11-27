//! .NET assemblies: ECMA-335 CLI metadata, in a PE image or a portable
//! PDB.
//!
//! - [`CliImage`]: a PE image's CLI header (data directory 14,
//!   `COM_DESCRIPTOR`), its metadata, method bodies, managed resources,
//!   strong-name signature, v-table fixups, and ReadyToRun header.
//! - [`Metadata`]: the metadata root and its streams: the tables (`#~`, or
//!   `#-` with its `Ptr` tables), `#Strings`, `#US`, `#GUID`, `#Blob`, and a
//!   portable PDB's `#Pdb`. Every table of II.22 and of the portable PDB
//!   has a typed row ([`tables`]), and the lookups between them (a type's
//!   fields, methods, events, properties, nested types, generic
//!   parameters; a token's custom attributes, constant, marshalling) are
//!   methods.
//! - [`signature`]: method, field, property, local, type and method spec
//!   signatures; [`names`]: types, signatures, and members as IL spells
//!   them.
//! - [`body`] and [`il`]: method bodies (tiny and fat headers, exception
//!   clauses) and their IL, decoded with every CIL opcode, tokens resolved
//!   to names.
//! - [`attributes`]: custom attribute values, permission sets,
//!   marshalling descriptors; [`resources`]: `.resources` files; [`pdb`]:
//!   document names and sequence points; [`r2r`]: ReadyToRun headers.
//!
//! Everything is read on demand from the borrowed bytes, and every read is
//! checked: damaged data gives errors, never a panic.

#![warn(missing_docs)]

#[cfg(test)]
use turbonop_global_alloc as _;

pub mod attributes;
pub mod body;
pub mod header;
pub mod heaps;
pub mod il;
pub mod metadata;
pub mod names;
pub mod pdb;
pub mod r2r;
pub mod reader;
pub mod resources;
pub mod signature;
pub mod tables;
#[cfg(test)]
mod testutil;

use turbonop_core::{Error, ErrorKind, Result, Rva};
use turbonop_pe::PeFile;
use turbonop_pe::consts::directory;

pub use body::MethodBody;
pub use header::{Cor20Header, Directory};
pub use heaps::{BlobIndex, Guid, GuidIndex, StringIndex};
pub use metadata::{Metadata, Stream, Version};
pub use r2r::ReadyToRunHeader;
pub use tables::{CodedIndex, CodedIndexKind, Row, TableId, Tables, Token};

use tables::{ManifestResourceRow, MethodDefRow};

/// A managed PE image: its CLI header and metadata.
#[derive(Debug)]
pub struct CliImage<'d> {
    pe: PeFile<'d>,
    header: Cor20Header,
    metadata: Metadata<'d>,
}

/// What the entry point is.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum EntryPoint {
    /// None.
    None,
    /// A method (or, in a multi-module assembly, a File) token.
    Token(Token),
    /// A native entry point's RVA.
    Native(u32),
}

/// An embedded manifest resource.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct EmbeddedResource<'d> {
    /// Its ManifestResource row.
    pub rid: u32,
    /// Its offset in the resources directory (its length's).
    pub offset: u32,
    /// Where its data is in the file.
    pub file_offset: u64,
    /// Its data (after the length).
    pub data: &'d [u8],
}

/// The strong-name signature.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct StrongNameSignature<'d> {
    /// Its directory.
    pub directory: Directory,
    /// Where it is in the file.
    pub file_offset: u64,
    /// Its bytes.
    pub data: &'d [u8],
}

impl StrongNameSignature<'_> {
    /// Whether it's all zeros: space reserved (delay or public signing)
    /// but no signature.
    pub fn is_zero(&self) -> bool {
        self.data.iter().all(|&b| b == 0)
    }
}

impl<'d> CliImage<'d> {
    /// The CLI image `pe` is, or `None` if it has no CLI header (data
    /// directory 14 is empty).
    pub fn parse(pe: &PeFile<'d>) -> Result<Option<Self>> {
        let Some(dir) = pe.data_directory(directory::COM_DESCRIPTOR) else {
            return Ok(None);
        };
        let (rva, size) = (dir.virtual_address.get(), dir.size.get());
        if rva == 0 || size == 0 {
            return Ok(None);
        }
        let bytes = pe.bytes_at_rva(Rva(rva), size.max(header::COR20_HEADER_SIZE))?;
        let base = pe.rva_to_offset(Rva(rva)).unwrap_or(0);
        let header = Cor20Header::parse(bytes, base)?;
        let md = pe
            .bytes_at_rva(Rva(header.metadata.rva), header.metadata.size)
            .map_err(|e| Error::new(e.kind, e.offset, "CLI metadata"))?;
        let md_base = pe.rva_to_offset(Rva(header.metadata.rva)).unwrap_or(0);
        let metadata =
            Metadata::parse(md).map_err(|e| Error::new(e.kind, e.offset + md_base, e.what))?;
        Ok(Some(Self {
            pe: *pe,
            header,
            metadata,
        }))
    }

    /// Parses `data` as a PE image, and it as a CLI image.
    pub fn parse_bytes(data: &'d [u8]) -> Result<Option<Self>> {
        Self::parse(&PeFile::parse_image(data)?)
    }

    /// The PE image.
    pub fn pe(&self) -> &PeFile<'d> {
        &self.pe
    }

    /// The CLI header.
    pub fn header(&self) -> &Cor20Header {
        &self.header
    }

    /// The metadata.
    pub fn metadata(&self) -> &Metadata<'d> {
        &self.metadata
    }

    /// The entry point.
    pub fn entry_point(&self) -> EntryPoint {
        match self.header.entry_point {
            0 => EntryPoint::None,
            e if self.header.flags & header::flags::NATIVE_ENTRYPOINT != 0 => EntryPoint::Native(e),
            e => EntryPoint::Token(Token(e)),
        }
    }

    /// The file's bytes from `rva` to the end of its section's raw data,
    /// and the file offset of `rva`.
    pub fn bytes_from_rva(&self, rva: u32) -> Result<(&'d [u8], u64)> {
        let bad = || Error::new(ErrorKind::Malformed, u64::from(rva), "RVA not in file");
        let offset = self.pe.rva_to_offset(Rva(rva)).ok_or_else(bad)?;
        let data = self.pe.data();
        let end = self
            .pe
            .sections()
            .iter()
            .find(|s| {
                let start = s.virtual_address.get();
                let raw = s.size_of_raw_data.get();
                rva >= start && rva - start < raw
            })
            .map_or(
                self.pe
                    .optional_header()
                    .map_or(0, |o| u64::from(o.size_of_headers())),
                |s| u64::from(s.pointer_to_raw_data.get()) + u64::from(s.size_of_raw_data.get()),
            )
            .min(data.len() as u64);
        let bytes = data
            .get(
                usize::try_from(offset).map_err(|_| bad())?
                    ..usize::try_from(end).map_err(|_| bad())?,
            )
            .ok_or_else(bad)?;
        Ok((bytes, offset))
    }

    /// The method body at `rva`.
    pub fn method_body_at(&self, rva: u32) -> Result<MethodBody<'d>> {
        let (bytes, offset) = self.bytes_from_rva(rva)?;
        body::parse_method_body(bytes, offset)
    }

    /// Method `rid`'s body, if it has one (an RVA).
    pub fn method_body(&self, rid: u32) -> Result<Option<MethodBody<'d>>> {
        let m: MethodDefRow = self.metadata.row(rid).ok_or(Error::new(
            ErrorKind::Malformed,
            u64::from(rid),
            "MethodDef",
        ))?;
        if m.rva == 0 {
            return Ok(None);
        }
        self.method_body_at(m.rva).map(Some)
    }

    /// Every method's body (those with an RVA), by rid, parsed on every
    /// core.
    pub fn method_bodies(&self) -> Vec<(u32, Result<MethodBody<'d>>)>
    where
        Self: Sync,
    {
        let n = self.metadata.tables().rows(TableId::MethodDef) as usize;
        turbonop_par::map_range(n, 512, |i| {
            let rid = u32::try_from(i + 1).unwrap_or(u32::MAX);
            self.method_body(rid).transpose().map(|b| (rid, b))
        })
        .into_iter()
        .flatten()
        .collect()
    }

    /// The managed resource at `offset` in the resources directory: a
    /// 4-byte length, then the data.
    pub fn resource_at(&self, offset: u32) -> Result<(&'d [u8], u64)> {
        let dir = self.header.resources;
        let bad = |what| Error::new(ErrorKind::Malformed, u64::from(offset), what);
        if offset.checked_add(4).is_none_or(|end| end > dir.size) {
            return Err(bad("resource offset past the resources"));
        }
        let all = self.pe.bytes_at_rva(Rva(dir.rva), dir.size)?;
        let at = offset as usize;
        let len = u32::from_le_bytes([all[at], all[at + 1], all[at + 2], all[at + 3]]) as usize;
        let data = all
            .get(at + 4..)
            .and_then(|r| r.get(..len))
            .ok_or_else(|| bad("resource past the resources"))?;
        let base = self.pe.rva_to_offset(Rva(dir.rva)).unwrap_or(0);
        Ok((data, base + at as u64 + 4))
    }

    /// The resources embedded in this file (ManifestResource rows with no
    /// Implementation), each with its data or why it can't be read.
    pub fn embedded_resources(&self) -> Vec<Result<EmbeddedResource<'d>>> {
        self.metadata
            .tables()
            .iter::<ManifestResourceRow>()
            .filter(|(_, r)| r.implementation.is_null())
            .map(|(rid, r)| {
                let (data, file_offset) = self.resource_at(r.offset)?;
                Ok(EmbeddedResource {
                    rid,
                    offset: r.offset,
                    file_offset,
                    data,
                })
            })
            .collect()
    }

    /// The strong-name signature, if the header has one.
    pub fn strong_name_signature(&self) -> Option<Result<StrongNameSignature<'d>>> {
        let dir = self.header.strong_name_signature;
        if dir.is_empty() {
            return None;
        }
        Some(
            self.pe
                .bytes_at_rva(Rva(dir.rva), dir.size)
                .map(|data| StrongNameSignature {
                    directory: dir,
                    file_offset: self.pe.rva_to_offset(Rva(dir.rva)).unwrap_or(0),
                    data,
                }),
        )
    }

    /// The ReadyToRun header, if ManagedNativeHeader points to one.
    pub fn ready_to_run(&self) -> Result<Option<ReadyToRunHeader>> {
        let dir = self.header.managed_native_header;
        if dir.is_empty() {
            return Ok(None);
        }
        let bytes = self.pe.bytes_at_rva(Rva(dir.rva), dir.size)?;
        if bytes.get(..4) != Some(&r2r::READYTORUN_SIGNATURE.to_le_bytes()) {
            return Ok(None);
        }
        let (all, _) = self.bytes_from_rva(dir.rva)?;
        ReadyToRunHeader::parse(all).map(Some)
    }

    /// A ReadyToRun image's compiler identifier (its CompilerIdentifier
    /// section's string).
    pub fn compiler_identifier(&self) -> Option<String> {
        let r2r = self.ready_to_run().ok()??;
        let s = r2r.section(100)?;
        let b = self.pe.bytes_at_rva(Rva(s.rva), s.size).ok()?;
        let b = &b[..b.iter().position(|&c| c == 0).unwrap_or(b.len())];
        Some(String::from_utf8_lossy(b).into_owned())
    }

    /// The v-table fixups.
    pub fn vtable_fixups(&self) -> Result<Vec<header::VTableFixup>> {
        let dir = self.header.vtable_fixups;
        if dir.is_empty() {
            return Ok(Vec::new());
        }
        let bytes = self.pe.bytes_at_rva(Rva(dir.rva), dir.size)?;
        header::parse_vtable_fixups(bytes, self.pe.rva_to_offset(Rva(dir.rva)).unwrap_or(0))
    }
}

#[cfg(test)]
#[allow(clippy::cast_possible_truncation)]
mod tests {
    #[test]
    fn types_are_send_and_sync() {
        fn check<T: Send + Sync>() {}
        check::<crate::CliImage<'_>>();
        check::<crate::Metadata<'_>>();
    }
}
