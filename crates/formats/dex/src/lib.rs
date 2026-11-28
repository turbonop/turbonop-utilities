//! Android DEX files: Dalvik executables, versions 035 to 041.
//!
//! - [`Dex`]: one DEX file, or one of the DEX files of a version 041
//!   container ([`Dex::parse_all`]): its [`Header`] (the Adler-32 checksum
//!   and SHA-1 signature verified, [`Dex::verify`]), the map list
//!   ([`map`]), and the id tables: strings (MUTF-8, [`mutf8`]), types,
//!   prototypes, fields, methods, class definitions, call sites, method
//!   handles; type lists.
//! - [`class_data`]: a class's fields and methods; [`code`]: method code
//!   items (registers, tries and their catch handlers); [`debug`]: debug
//!   information, run to line numbers and local variables.
//! - [`annotations`]: annotation directories, sets, and items, and
//!   [`value`]'s encoded values (annotation elements, static field
//!   values, call sites' arguments).
//! - [`hiddenapi`]: hidden-API flags of the platform's fields and methods.
//! - [`insn`]: Dalvik bytecode decoded instruction by instruction, every
//!   opcode and format and the switch and array payloads; [`text`]:
//!   instructions, values, and types rendered as `dexdump` writes them,
//!   indices resolved to names; [`names`]: types and members as Java
//!   writes them.
//! - [`size`]: each class's and method's bytes, for size by package,
//!   class, and method.
//!
//! Everything is read on demand from the borrowed bytes, and every read is
//! checked: damaged data gives errors, never a panic.

#![warn(missing_docs)]

#[cfg(test)]
use turbonop_global_alloc as _;

pub mod annotations;
pub mod class_data;
pub mod code;
pub mod debug;
pub mod header;
pub mod hiddenapi;
pub mod insn;
pub mod map;
pub mod mutf8;
pub mod names;
pub mod reader;
pub mod size;
pub mod text;
pub mod value;

use turbonop_core::{Error, ErrorKind, Result};

pub use class_data::{ClassData, EncodedField, EncodedMethod};
pub use code::CodeItem;
pub use header::Header;
pub use map::{MapItem, MapItemType};
pub use value::EncodedValue;

use reader::{Cursor, u16_at, u32_at};

/// "No index": a class without a superclass or source file.
pub const NO_INDEX: u32 = 0xFFFF_FFFF;

/// A prototype (`proto_id_item`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct ProtoId {
    /// Its shorty descriptor (a string index).
    pub shorty_idx: u32,
    /// Its return type (a type index).
    pub return_type_idx: u32,
    /// Where its parameters' type list is (0: none).
    pub parameters_off: u32,
}

/// A field reference (`field_id_item`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct FieldId {
    /// Its class (a type index).
    pub class_idx: u16,
    /// Its type (a type index).
    pub type_idx: u16,
    /// Its name (a string index).
    pub name_idx: u32,
}

/// A method reference (`method_id_item`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct MethodId {
    /// Its class (a type index).
    pub class_idx: u16,
    /// Its prototype (a proto index).
    pub proto_idx: u16,
    /// Its name (a string index).
    pub name_idx: u32,
}

/// A class definition (`class_def_item`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct ClassDef {
    /// The class (a type index).
    pub class_idx: u32,
    /// Its access flags.
    pub access_flags: u32,
    /// Its superclass (a type index), or [`NO_INDEX`].
    pub superclass_idx: u32,
    /// Where its interfaces' type list is (0: none).
    pub interfaces_off: u32,
    /// Its source file's name (a string index), or [`NO_INDEX`].
    pub source_file_idx: u32,
    /// Where its annotations directory is (0: none).
    pub annotations_off: u32,
    /// Where its class data is (0: none, a marker interface say).
    pub class_data_off: u32,
    /// Where its static fields' initial values are (0: none).
    pub static_values_off: u32,
}

/// A method handle (`method_handle_item`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct MethodHandleItem {
    /// Its kind ([`MethodHandleKind`]'s value).
    pub kind: u16,
    /// The field (for the field kinds) or method it refers to.
    pub field_or_method_idx: u16,
}

/// What a method handle does.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum MethodHandleKind {
    /// Sets a static field.
    StaticPut,
    /// Gets a static field.
    StaticGet,
    /// Sets an instance field.
    InstancePut,
    /// Gets an instance field.
    InstanceGet,
    /// Calls a static method.
    InvokeStatic,
    /// Calls an instance method.
    InvokeInstance,
    /// Calls a constructor.
    InvokeConstructor,
    /// Calls a method directly.
    InvokeDirect,
    /// Calls an interface method.
    InvokeInterface,
}

impl MethodHandleKind {
    /// The kind of value `v`, if it's one.
    pub fn from_u16(v: u16) -> Option<Self> {
        Some(match v {
            0 => Self::StaticPut,
            1 => Self::StaticGet,
            2 => Self::InstancePut,
            3 => Self::InstanceGet,
            4 => Self::InvokeStatic,
            5 => Self::InvokeInstance,
            6 => Self::InvokeConstructor,
            7 => Self::InvokeDirect,
            8 => Self::InvokeInterface,
            _ => return None,
        })
    }

    /// Whether it accesses a field (or else calls a method).
    pub fn is_field(self) -> bool {
        matches!(
            self,
            Self::StaticPut | Self::StaticGet | Self::InstancePut | Self::InstanceGet
        )
    }

    /// Its name as dexdump writes it (`put-static`, `invoke-interface`).
    pub fn name(self) -> &'static str {
        match self {
            Self::StaticPut => "put-static",
            Self::StaticGet => "get-static",
            Self::InstancePut => "put-instance",
            Self::InstanceGet => "get-instance",
            Self::InvokeStatic => "invoke-static",
            Self::InvokeInstance => "invoke-instance",
            Self::InvokeConstructor => "invoke-constructor",
            Self::InvokeDirect => "invoke-direct",
            Self::InvokeInterface => "invoke-interface",
        }
    }
}

/// A list of types (`type_list`): a prototype's parameters, a class's
/// interfaces.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct TypeList<'d> {
    /// The entries' bytes (two each).
    entries: &'d [u8],
    /// Where it is (0: an empty list that isn't in the file).
    pub offset: u32,
}

impl<'d> TypeList<'d> {
    /// How many types.
    pub fn len(&self) -> usize {
        self.entries.len() / 2
    }

    /// Whether there are none.
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// The `i`th type (a type index).
    pub fn get(&self, i: usize) -> Option<u16> {
        u16_at(self.entries, i.checked_mul(2)?)
    }

    /// The types (type indices).
    pub fn iter(&self) -> impl ExactSizeIterator<Item = u16> + 'd {
        self.entries
            .as_chunks::<2>()
            .0
            .iter()
            .map(|b| u16::from_le_bytes(*b))
    }

    /// Its size in the file: the count and the entries.
    pub fn byte_size(&self) -> usize {
        if self.offset == 0 {
            0
        } else {
            4 + self.entries.len()
        }
    }
}

/// Whether a checksum and a signature are what the header says.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Verification {
    /// The Adler-32 of the bytes after the checksum.
    pub checksum: u32,
    /// The SHA-1 of the bytes after the signature.
    pub signature: [u8; 20],
    /// Whether the checksum is the header's.
    pub checksum_ok: bool,
    /// Whether the signature is the header's.
    pub signature_ok: bool,
}

/// A DEX file (or one DEX file of a version 041 container).
#[derive(Clone, Debug)]
pub struct Dex<'d> {
    /// The whole file: in a container, every offset is from its start.
    data: &'d [u8],
    /// Where this file's header is.
    offset: usize,
    header: Header,
    string_ids: &'d [u8],
    type_ids: &'d [u8],
    proto_ids: &'d [u8],
    field_ids: &'d [u8],
    method_ids: &'d [u8],
    class_defs: &'d [u8],
    map: std::result::Result<Vec<MapItem>, Error>,
    call_site_ids: &'d [u8],
    method_handles: &'d [u8],
    hiddenapi_off: Option<u32>,
}

/// The `size` items of `item_size` bytes at `off` of `data` (none if
/// `size` is 0, wherever `off` points).
fn table<'d>(
    data: &'d [u8],
    off: u32,
    size: u32,
    item_size: usize,
    what: &'static str,
) -> Result<&'d [u8]> {
    if size == 0 {
        return Ok(&[]);
    }
    let start = off as usize;
    (size as usize)
        .checked_mul(item_size)
        .and_then(|n| start.checked_add(n))
        .and_then(|end| data.get(start..end))
        .ok_or(Error::new(ErrorKind::Truncated, u64::from(off), what))
}

impl<'d> Dex<'d> {
    /// Parses `data` as a DEX file; in a version 041 container, the first
    /// of its DEX files.
    pub fn parse(data: &'d [u8]) -> Result<Self> {
        Self::parse_at(data, 0)
    }

    /// Parses `data` as a DEX file, or as a container of them: every DEX
    /// file in it, in order (one, for a version before 041).
    pub fn parse_all(data: &'d [u8]) -> Result<Vec<Self>> {
        let first = Self::parse_at(data, 0)?;
        let Some(container) = first.header.container_size else {
            return Ok(vec![first]);
        };
        let end = (container as usize).min(data.len());
        let mut all = Vec::new();
        let mut at = 0usize;
        let mut next = Some(first);
        loop {
            let dex = match next.take() {
                Some(d) => d,
                None => Self::parse_at(data, at)?,
            };
            let size = dex.header.file_size as usize;
            all.push(dex);
            // (Every file is at least its header: no loop forever.)
            at = at
                .checked_add(size.max(header::HEADER_SIZE_V41 as usize))
                .ok_or(Error::new(ErrorKind::Overflow, at as u64, "file_size"))?;
            if at >= end {
                return Ok(all);
            }
        }
    }

    /// Parses the DEX file whose header is at `offset` of `data` (a
    /// container's, for one after the first).
    pub fn parse_at(data: &'d [u8], offset: usize) -> Result<Self> {
        let header = Header::parse(data, offset)?;
        if let Some(h) = header.header_offset
            && h as usize != offset
        {
            return Err(Error::new(
                ErrorKind::Malformed,
                offset as u64 + 0x74,
                "header_offset",
            ));
        }
        let h = &header;
        let string_ids = table(data, h.string_ids_off, h.string_ids_size, 4, "string_ids")?;
        let type_ids = table(data, h.type_ids_off, h.type_ids_size, 4, "type_ids")?;
        let proto_ids = table(data, h.proto_ids_off, h.proto_ids_size, 12, "proto_ids")?;
        let field_ids = table(data, h.field_ids_off, h.field_ids_size, 8, "field_ids")?;
        let method_ids = table(data, h.method_ids_off, h.method_ids_size, 8, "method_ids")?;
        let class_defs = table(data, h.class_defs_off, h.class_defs_size, 32, "class_defs")?;
        let map = map::parse(data, h.map_off);
        let (mut call_site_ids, mut method_handles, mut hiddenapi_off) = (&[][..], &[][..], None);
        if let Ok(items) = &map {
            for item in items {
                match item.kind {
                    MapItemType::CallSiteIdItem => {
                        call_site_ids = table(data, item.offset, item.size, 4, "call_site_ids")?;
                    }
                    MapItemType::MethodHandleItem => {
                        method_handles = table(data, item.offset, item.size, 8, "method_handles")?;
                    }
                    MapItemType::HiddenapiClassDataItem => hiddenapi_off = Some(item.offset),
                    _ => {}
                }
            }
        }
        Ok(Self {
            data,
            offset,
            header,
            string_ids,
            type_ids,
            proto_ids,
            field_ids,
            method_ids,
            class_defs,
            map,
            call_site_ids,
            method_handles,
            hiddenapi_off,
        })
    }

    /// The header.
    pub fn header(&self) -> &Header {
        &self.header
    }

    /// The version (35 to 41).
    pub fn version(&self) -> u32 {
        self.header.version
    }

    /// The whole file's bytes (a container's, for version 041): every
    /// offset is from its start.
    pub fn data(&self) -> &'d [u8] {
        self.data
    }

    /// Where this DEX file's header is (in a container, after the first,
    /// not 0).
    pub fn offset(&self) -> usize {
        self.offset
    }

    /// This DEX file's own bytes: from its header, `file_size` of them (as
    /// many as there are).
    pub fn bytes(&self) -> &'d [u8] {
        let end = self
            .offset
            .saturating_add(self.header.file_size as usize)
            .min(self.data.len());
        &self.data[self.offset.min(end)..end]
    }

    /// The checksum and signature computed, and whether they're the
    /// header's: each over this file's bytes after it (`file_size` of
    /// them from the header; in a container, the file's own). The two run
    /// at once, and the checksum on every core.
    pub fn verify(&self) -> Verification {
        let bytes = self.bytes();
        let adler = || header::adler32(bytes.get(12..).unwrap_or_default());
        let sha = || {
            use sha1::Digest;
            let digest = sha1::Sha1::digest(bytes.get(32..).unwrap_or_default());
            let mut s = [0u8; 20];
            s.copy_from_slice(&digest);
            s
        };
        // (A thread costs more than hashing a small file.)
        let (checksum, signature) = if bytes.len() < 1 << 20 {
            (adler(), sha())
        } else {
            turbonop_par::join(adler, sha)
        };
        let complete = bytes.len() == self.header.file_size as usize;
        Verification {
            checksum,
            signature,
            checksum_ok: complete && checksum == self.header.checksum,
            signature_ok: complete && signature == self.header.signature,
        }
    }

    /// The map list.
    pub fn map(&self) -> Result<&[MapItem]> {
        self.map.as_deref().map_err(|e| *e)
    }

    fn index_error(what: &'static str, i: u32) -> Error {
        Error::new(ErrorKind::Malformed, u64::from(i), what)
    }

    // Strings.

    /// How many strings.
    pub fn string_count(&self) -> u32 {
        self.header.string_ids_size
    }

    /// Where string `i`'s data (`string_data_item`) is.
    pub fn string_data_offset(&self, i: u32) -> Result<u32> {
        u32_at(self.string_ids, (i as usize).saturating_mul(4))
            .ok_or_else(|| Self::index_error("string index", i))
    }

    /// String `i`'s UTF-16 length and MUTF-8 bytes (without the NUL that
    /// ends them).
    pub fn string_data(&self, i: u32) -> Result<(u32, &'d [u8])> {
        let off = self.string_data_offset(i)?;
        let mut c = Cursor::at(self.data, off as usize);
        let utf16_size = c.uleb128("string length")?;
        let start = c.pos();
        // At most three bytes a UTF-16 unit: a damaged string can't make
        // every string's read scan the whole file.
        let most = (utf16_size as usize).saturating_mul(3).saturating_add(1);
        let rest = self.data.get(start..).unwrap_or_default();
        let rest = &rest[..rest.len().min(most)];
        let len = rest.iter().position(|&b| b == 0).ok_or(Error::new(
            ErrorKind::Truncated,
            start as u64,
            "string data",
        ))?;
        Ok((utf16_size, &rest[..len]))
    }

    /// String `i`'s MUTF-8 bytes (without the NUL).
    pub fn string_bytes(&self, i: u32) -> Result<&'d [u8]> {
        Ok(self.string_data(i)?.1)
    }

    /// String `i`, decoded (unpaired surrogates as U+FFFD).
    pub fn string(&self, i: u32) -> Result<String> {
        let (_, bytes) = self.string_data(i)?;
        mutf8::decode(bytes).map_err(|e| {
            Error::new(
                ErrorKind::Malformed,
                u64::from(self.string_data_offset(i).unwrap_or(0)) + e.offset as u64,
                "MUTF-8 string",
            )
        })
    }

    // Types.

    /// How many types.
    pub fn type_count(&self) -> u32 {
        self.header.type_ids_size
    }

    /// Type `i`'s descriptor (a string index).
    pub fn type_descriptor_idx(&self, i: u32) -> Result<u32> {
        u32_at(self.type_ids, (i as usize).saturating_mul(4))
            .ok_or_else(|| Self::index_error("type index", i))
    }

    /// Type `i`'s descriptor (`Ljava/lang/String;`), its MUTF-8 bytes.
    pub fn type_descriptor(&self, i: u32) -> Result<&'d [u8]> {
        self.string_bytes(self.type_descriptor_idx(i)?)
    }

    /// Type `i` as Java writes it (`java.lang.String[]`).
    pub fn type_name(&self, i: u32) -> Result<String> {
        Ok(names::descriptor_to_java(&mutf8::decode_lossy(
            self.type_descriptor(i)?,
        )))
    }

    // Prototypes.

    /// How many prototypes.
    pub fn proto_count(&self) -> u32 {
        self.header.proto_ids_size
    }

    /// Prototype `i`.
    pub fn proto(&self, i: u32) -> Result<ProtoId> {
        let at = (i as usize).saturating_mul(12);
        match (
            u32_at(self.proto_ids, at),
            u32_at(self.proto_ids, at.saturating_add(4)),
            u32_at(self.proto_ids, at.saturating_add(8)),
        ) {
            (Some(shorty_idx), Some(return_type_idx), Some(parameters_off)) => Ok(ProtoId {
                shorty_idx,
                return_type_idx,
                parameters_off,
            }),
            _ => Err(Self::index_error("proto index", i)),
        }
    }

    /// Prototype `i`'s parameters.
    pub fn proto_parameters(&self, i: u32) -> Result<TypeList<'d>> {
        self.type_list(self.proto(i)?.parameters_off)
    }

    /// The type list at `off` (empty for 0).
    pub fn type_list(&self, off: u32) -> Result<TypeList<'d>> {
        if off == 0 {
            return Ok(TypeList::default());
        }
        if !off.is_multiple_of(4) {
            return Err(Error::new(
                ErrorKind::Malformed,
                u64::from(off),
                "type list alignment",
            ));
        }
        let mut c = Cursor::at(self.data, off as usize);
        let n = c.u32("type list size")?;
        let n = c.check_count(n, 2, "type list")?;
        Ok(TypeList {
            entries: c.bytes(n * 2, "type list")?,
            offset: off,
        })
    }

    // Fields and methods.

    /// How many field references.
    pub fn field_count(&self) -> u32 {
        self.header.field_ids_size
    }

    /// Field reference `i`.
    pub fn field(&self, i: u32) -> Result<FieldId> {
        let at = (i as usize).saturating_mul(8);
        match (
            u16_at(self.field_ids, at),
            u16_at(self.field_ids, at.saturating_add(2)),
            u32_at(self.field_ids, at.saturating_add(4)),
        ) {
            (Some(class_idx), Some(type_idx), Some(name_idx)) => Ok(FieldId {
                class_idx,
                type_idx,
                name_idx,
            }),
            _ => Err(Self::index_error("field index", i)),
        }
    }

    /// How many method references.
    pub fn method_count(&self) -> u32 {
        self.header.method_ids_size
    }

    /// Method reference `i`.
    pub fn method(&self, i: u32) -> Result<MethodId> {
        let at = (i as usize).saturating_mul(8);
        match (
            u16_at(self.method_ids, at),
            u16_at(self.method_ids, at.saturating_add(2)),
            u32_at(self.method_ids, at.saturating_add(4)),
        ) {
            (Some(class_idx), Some(proto_idx), Some(name_idx)) => Ok(MethodId {
                class_idx,
                proto_idx,
                name_idx,
            }),
            _ => Err(Self::index_error("method index", i)),
        }
    }

    // Classes.

    /// How many class definitions.
    pub fn class_def_count(&self) -> u32 {
        self.header.class_defs_size
    }

    /// Class definition `i`.
    pub fn class_def(&self, i: u32) -> Result<ClassDef> {
        let at = (i as usize).saturating_mul(32);
        let b = self
            .class_defs
            .get(at..at.saturating_add(32))
            .ok_or_else(|| Self::index_error("class definition index", i))?;
        let f = |k: usize| u32::from_le_bytes([b[k], b[k + 1], b[k + 2], b[k + 3]]);
        Ok(ClassDef {
            class_idx: f(0),
            access_flags: f(4),
            superclass_idx: f(8),
            interfaces_off: f(12),
            source_file_idx: f(16),
            annotations_off: f(20),
            class_data_off: f(24),
            static_values_off: f(28),
        })
    }

    /// The class definitions, in order.
    pub fn class_defs(&self) -> impl ExactSizeIterator<Item = ClassDef> + '_ {
        (0..self.class_defs.len() / 32).map(|i| {
            #[allow(clippy::cast_possible_truncation)]
            self.class_def(i as u32).expect("in the table")
        })
    }

    /// A class's data (fields and methods), if it has any.
    pub fn class_data(&self, class: &ClassDef) -> Result<Option<ClassData>> {
        if class.class_data_off == 0 {
            return Ok(None);
        }
        ClassData::parse(self.data, class.class_data_off).map(Some)
    }

    /// A class's interfaces.
    pub fn interfaces(&self, class: &ClassDef) -> Result<TypeList<'d>> {
        self.type_list(class.interfaces_off)
    }

    /// A class's static fields' initial values (an encoded array: the
    /// first fields', in order; the rest are zero or null).
    pub fn static_values(&self, class: &ClassDef) -> Result<Vec<EncodedValue>> {
        if class.static_values_off == 0 {
            return Ok(Vec::new());
        }
        value::parse_array_at(self.data, class.static_values_off)
    }

    /// The code item at `off`.
    pub fn code_item(&self, off: u32) -> Result<CodeItem<'d>> {
        CodeItem::parse(self.data, off)
    }

    // Call sites and method handles.

    /// How many call sites.
    pub fn call_site_count(&self) -> u32 {
        #[allow(clippy::cast_possible_truncation)]
        let n = (self.call_site_ids.len() / 4) as u32;
        n
    }

    /// Where call site `i` (its encoded array) is.
    pub fn call_site_offset(&self, i: u32) -> Result<u32> {
        u32_at(self.call_site_ids, (i as usize).saturating_mul(4))
            .ok_or_else(|| Self::index_error("call site index", i))
    }

    /// Call site `i`: its bootstrap method handle, method name, method
    /// type, then the bootstrap's other arguments.
    pub fn call_site(&self, i: u32) -> Result<Vec<EncodedValue>> {
        value::parse_array_at(self.data, self.call_site_offset(i)?)
    }

    /// How many method handles.
    pub fn method_handle_count(&self) -> u32 {
        #[allow(clippy::cast_possible_truncation)]
        let n = (self.method_handles.len() / 8) as u32;
        n
    }

    /// Method handle `i`.
    pub fn method_handle(&self, i: u32) -> Result<MethodHandleItem> {
        let at = (i as usize).saturating_mul(8);
        match (
            u16_at(self.method_handles, at),
            u16_at(self.method_handles, at.saturating_add(4)),
        ) {
            (Some(kind), Some(field_or_method_idx)) => Ok(MethodHandleItem {
                kind,
                field_or_method_idx,
            }),
            _ => Err(Self::index_error("method handle index", i)),
        }
    }

    /// Where the hidden-API flags are, if there are any.
    pub fn hiddenapi_offset(&self) -> Option<u32> {
        self.hiddenapi_off
    }

    /// The hidden-API flags, if there are any.
    pub fn hiddenapi(&self) -> Result<Option<hiddenapi::HiddenApi<'d>>> {
        match self.hiddenapi_off {
            None => Ok(None),
            Some(off) => {
                hiddenapi::HiddenApi::parse(self.data, off, self.class_def_count()).map(Some)
            }
        }
    }
}

#[cfg(test)]
mod tests;
