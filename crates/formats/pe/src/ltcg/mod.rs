//! Visual C++ link-time code generation objects (`cl /GL`): an anonymous
//! object header (version 1, class ID [`CLASS_ID`]) around a COFF object
//! whose sections hold the compiler's intermediate language, which
//! `link /LTCG` hands to the back end (the c2.dll that `.drectve` names).
//!
//! The IL is undocumented. What is read here was worked out from objects
//! MSVC 14.5x writes (small sources varied one thing at a time, and some
//! four hundred objects built from this repository's test sources for
//! x64, x86, and ARM64), and checked against what the tools show of them
//! (lib.exe's symbol index, the sources' SHA-256 digests, the CodeView
//! records' framing) and against the sections' agreement with one
//! another. The format, field by field with the evidence for each, is in
//! docs/ltcg-il.md.
//!
//! - `.drectve`: `-compiler:"<path of c2.dll>"`.
//! - `.cil$fg`: the back end's command line (a count, then strings).
//! - `.cil$gl` ([`Globals`]): the global symbol table, a header and then
//!   records one after another, each led by a tag: source files,
//!   sections, namespaces, named types, attributes, constants, variables,
//!   functions (declared, and with a body), and last the linker
//!   directives. Every record is walked in turn.
//! - `.cil$ex` ([`IlUnits`]): each function's IL, one unit after another
//!   (the bytecode itself is not decoded).
//! - `.cil$sy` ([`Locals`]): each function's scopes and local symbols, a
//!   stream per IL unit, in the same order.
//! - `.cil$in` ([`Initializers`]): the variables' initial values.
//! - `.cil$db` ([`Types`]): the CodeView type and id records (a header,
//!   then records each framed by a tag and a compressed length).

use alloc::vec::Vec;
use turbonop_core::{Error, ErrorKind, Result};

mod code;
mod globals;
mod init;
mod locals;

pub use code::IlUnits;
pub use globals::*;
pub use init::{InitItem, Initializer, Initializers};
pub use locals::{FunctionLocals, Local, LocalItem, LocalKind, Locals, Scope};

/// The class ID of an LTCG object, `{0CB3FE38-D9A5-4DAB-AC9B-D6B6222653C2}`,
/// as stored.
pub const CLASS_ID: [u8; 16] = [
    0x38, 0xFE, 0xB3, 0x0C, 0xA5, 0xD9, 0xAB, 0x4D, 0xAC, 0x9B, 0xD6, 0xB6, 0x22, 0x26, 0x53, 0xC2,
];

/// The size of the anonymous object header (version 1).
pub const HEADER_SIZE: usize = 32;

/// A section of the IL object.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Stream<'data> {
    pub name: &'data [u8],
    /// Its offset in the file.
    pub offset: u64,
    pub data: &'data [u8],
}

/// One of the IL object's COFF symbols (`@comp.id`, `@feat.00`, and its
/// sections').
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CoffSymbol<'data> {
    pub name: &'data [u8],
    pub value: u32,
    pub section: i16,
    pub storage_class: u8,
}

/// An LTCG object.
#[derive(Clone, Debug)]
pub struct LtcgObject<'data> {
    /// The anonymous header's version (1).
    pub version: u16,
    pub machine: u16,
    pub time_date_stamp: u32,
    pub class_id: [u8; 16],
    /// The size of what follows the header (the IL object).
    pub size_of_data: u32,
    /// The IL object's file header's machine field, which is not a
    /// machine (0x0C13 from MSVC 14.5x): the IL's version, it seems.
    pub il_version: u16,
    /// The IL object's size of optional header and section headers' end:
    /// where its first section's data may start (its headers' extent).
    pub headers_end: usize,
    /// Where the IL object's COFF symbol table starts, and its size with
    /// its string table (offsets in the file).
    pub symbol_table: (u64, u64),
    streams: Vec<Stream<'data>>,
    symbols: Vec<CoffSymbol<'data>>,
}

fn u16_at(d: &[u8], o: usize) -> Option<u16> {
    Some(u16::from_le_bytes(
        d.get(o..o.checked_add(2)?)?.try_into().ok()?,
    ))
}

fn u32_at(d: &[u8], o: usize) -> Option<u32> {
    Some(u32::from_le_bytes(
        d.get(o..o.checked_add(4)?)?.try_into().ok()?,
    ))
}

impl<'data> LtcgObject<'data> {
    /// Whether `data` starts with an LTCG object's header.
    pub fn is_ltcg(data: &[u8]) -> bool {
        data.len() >= HEADER_SIZE
            && data[0..4] == [0, 0, 0xFF, 0xFF]
            && u16_at(data, 4) == Some(1)
            && data[12..28] == CLASS_ID
    }

    pub fn parse(data: &'data [u8]) -> Result<Self> {
        if !Self::is_ltcg(data) {
            return Err(Error::new(ErrorKind::BadMagic, 0, "LTCG object header"));
        }
        let truncated =
            |at: usize, what: &'static str| Error::new(ErrorKind::Truncated, at as u64, what);
        let size_of_data = u32_at(data, 28).unwrap_or(0);
        let end = HEADER_SIZE
            .saturating_add(usize::try_from(size_of_data).unwrap_or(usize::MAX))
            .min(data.len());
        let il = &data[HEADER_SIZE..end];
        let il_version = u16_at(il, 0).ok_or(truncated(HEADER_SIZE, "IL object header"))?;
        let count = usize::from(u16_at(il, 2).ok_or(truncated(HEADER_SIZE, "IL object header"))?);
        let symtab = u32_at(il, 8).ok_or(truncated(HEADER_SIZE, "IL object header"))? as usize;
        let nsyms = u32_at(il, 12).ok_or(truncated(HEADER_SIZE, "IL object header"))? as usize;
        let optional = usize::from(u16_at(il, 16).unwrap_or(0));
        let strings_at = symtab.saturating_add(nsyms.saturating_mul(18));
        let string = |offset: usize| -> &'data [u8] {
            let s = il.get(strings_at.saturating_add(offset)..).unwrap_or(&[]);
            &s[..s.iter().position(|&b| b == 0).unwrap_or(s.len())]
        };
        let short = |raw: &'data [u8]| -> &'data [u8] {
            if raw[..4] == [0, 0, 0, 0] {
                string(u32::from_le_bytes([raw[4], raw[5], raw[6], raw[7]]) as usize)
            } else {
                &raw[..raw.iter().position(|&b| b == 0).unwrap_or(8)]
            }
        };
        let mut streams = Vec::with_capacity(count);
        for i in 0..count {
            let at = 20 + optional + i * 40;
            let h = il
                .get(at..at + 40)
                .ok_or(truncated(HEADER_SIZE + at, "IL section headers"))?;
            let raw_name = &h[..8];
            let name = if raw_name[0] == b'/' {
                // ("/<offset>": a long name in the string table.)
                core::str::from_utf8(&raw_name[1..])
                    .ok()
                    .and_then(|s| s.trim_end_matches('\0').parse::<usize>().ok())
                    .map_or(raw_name, &string)
            } else {
                &raw_name[..raw_name.iter().position(|&b| b == 0).unwrap_or(8)]
            };
            let size = u32_at(h, 16).unwrap_or(0) as usize;
            let ptr = u32_at(h, 20).unwrap_or(0) as usize;
            let body = il
                .get(ptr..ptr.saturating_add(size))
                .ok_or(truncated(HEADER_SIZE + ptr, "IL section"))?;
            streams.push(Stream {
                name,
                offset: (HEADER_SIZE + ptr) as u64,
                data: body,
            });
        }
        let mut symbols = Vec::new();
        let mut i = 0;
        while i < nsyms {
            let Some(at) = i.checked_mul(18).and_then(|o| o.checked_add(symtab)) else {
                break;
            };
            let Some(e) = il.get(at..at.saturating_add(18)) else {
                break;
            };
            symbols.push(CoffSymbol {
                name: short(&e[..8]),
                value: u32_at(e, 8).unwrap_or(0),
                section: i16::from_le_bytes([e[12], e[13]]),
                storage_class: e[16],
            });
            i += 1 + usize::from(e[17]);
        }
        // (The string table: its size leads it.)
        let strings = il
            .get(strings_at..)
            .and_then(|s| u32_at(s, 0))
            .map_or(0, |n| (n as usize).min(il.len().saturating_sub(strings_at)));
        let symbol_table = if symtab == 0 || symtab >= il.len() {
            (0, 0)
        } else {
            let len = strings_at.min(il.len()).saturating_sub(symtab) + strings;
            ((HEADER_SIZE + symtab) as u64, len as u64)
        };
        Ok(Self {
            version: 1,
            machine: u16_at(data, 6).unwrap_or(0),
            time_date_stamp: u32_at(data, 8).unwrap_or(0),
            class_id: CLASS_ID,
            size_of_data,
            il_version,
            headers_end: (HEADER_SIZE + 20 + optional + count * 40).min(data.len()),
            symbol_table,
            streams,
            symbols,
        })
    }

    /// The IL object's sections, in order.
    pub fn streams(&self) -> &[Stream<'data>] {
        &self.streams
    }

    /// The section named `name`'s bytes.
    pub fn stream(&self, name: &[u8]) -> Option<&'data [u8]> {
        self.streams.iter().find(|s| s.name == name).map(|s| s.data)
    }

    /// The section named `name`'s offset in the file.
    pub fn stream_offset(&self, name: &[u8]) -> Option<u64> {
        self.streams
            .iter()
            .find(|s| s.name == name)
            .map(|s| s.offset)
    }

    /// The IL object's COFF symbols (other than auxiliary records).
    pub fn coff_symbols(&self) -> &[CoffSymbol<'data>] {
        &self.symbols
    }

    /// `@comp.id`'s value: the compiler's product id (high half) and build.
    pub fn comp_id(&self) -> Option<u32> {
        self.symbols
            .iter()
            .find(|s| s.name == b"@comp.id")
            .map(|s| s.value)
    }

    /// `@feat.00`'s value (its bits: `/GS`, `/guard:cf`, `/sdl`, ...).
    pub fn feat_00(&self) -> Option<u32> {
        self.symbols
            .iter()
            .find(|s| s.name == b"@feat.00")
            .map(|s| s.value)
    }

    /// The back end the linker is to load: `.drectve`'s `-compiler:` path.
    pub fn compiler(&self) -> Option<&'data [u8]> {
        let d = self.stream(b".drectve")?;
        let key = b"-compiler:";
        let at = d.windows(key.len()).position(|w| w == key)? + key.len();
        let rest = &d[at..];
        Some(match rest.strip_prefix(b"\"") {
            Some(q) => &q[..q.iter().position(|&b| b == b'"').unwrap_or(q.len())],
            None => &rest[..rest.iter().position(|&b| b == b' ').unwrap_or(rest.len())],
        })
    }

    /// The back end's command line (`.cil$fg`): its arguments.
    pub fn backend_args(&self) -> Vec<&'data [u8]> {
        let Some(d) = self.stream(b".cil$fg") else {
            return Vec::new();
        };
        let count = u32_at(d, 0).unwrap_or(0) as usize;
        d.get(4..)
            .unwrap_or(&[])
            .split(|&b| b == 0)
            .take(count)
            .collect()
    }

    /// The global symbol table (`.cil$gl`).
    pub fn globals(&self) -> Result<Globals<'data>> {
        let g = self.stream(b".cil$gl").ok_or(Error::new(
            ErrorKind::Malformed,
            0,
            "no .cil$gl section",
        ))?;
        Globals::parse(g)
    }

    /// The CodeView type and id records (`.cil$db`), in index order.
    pub fn types(&self) -> Result<Types<'data>> {
        let d = self.stream(b".cil$db").ok_or(Error::new(
            ErrorKind::Malformed,
            0,
            "no .cil$db section",
        ))?;
        Types::parse(d)
    }

    /// The functions' scopes and local symbols (`.cil$sy`), a stream per
    /// IL unit (none if the object has no such section).
    pub fn locals(&self) -> Result<Locals<'data>> {
        Locals::parse(self.stream(b".cil$sy").unwrap_or(&[]))
    }

    /// The variables' initial values (`.cil$in`; none if the object has no
    /// such section).
    pub fn initializers(&self) -> Result<Initializers<'data>> {
        Initializers::parse(self.stream(b".cil$in").unwrap_or(&[]))
    }

    /// Where each function's IL starts and ends in `.cil$ex`.
    pub fn il_units(&self) -> IlUnits {
        IlUnits::find(self.stream(b".cil$ex").unwrap_or(&[]))
    }
}

/// A reader of the IL's encodings, every read bounds-checked.
#[derive(Clone)]
pub(crate) struct Reader<'a> {
    pub(crate) d: &'a [u8],
    pub(crate) p: usize,
}

impl<'a> Reader<'a> {
    pub(crate) fn new(d: &'a [u8], p: usize) -> Self {
        Self { d, p }
    }

    pub(crate) fn u8(&mut self) -> Option<u8> {
        let b = *self.d.get(self.p)?;
        self.p += 1;
        Some(b)
    }

    pub(crate) fn u16(&mut self) -> Option<u16> {
        let v = u16_at(self.d, self.p)?;
        self.p += 2;
        Some(v)
    }

    pub(crate) fn u32(&mut self) -> Option<u32> {
        let v = u32_at(self.d, self.p)?;
        self.p += 4;
        Some(v)
    }

    pub(crate) fn take(&mut self, n: usize) -> Option<&'a [u8]> {
        let s = self.d.get(self.p..self.p.checked_add(n)?)?;
        self.p += n;
        Some(s)
    }

    /// An id: a 16-bit unit, and another when the first's top bit is set.
    pub(crate) fn id(&mut self) -> Option<u32> {
        let w = self.u16()?;
        if w & 0x8000 == 0 {
            return Some(w.into());
        }
        let hi = self.u16()?;
        Some(u32::from(w & 0x7FFF) | (u32::from(hi) << 15))
    }

    /// A compressed 16-bit value: a byte below 0x80, else 0x80 and the
    /// value.
    pub(crate) fn c16(&mut self) -> Option<u16> {
        match self.u8()? {
            b if b < 0x80 => Some(b.into()),
            0x80 => self.u16(),
            _ => None,
        }
    }

    /// A compressed 32-bit value.
    pub(crate) fn c32(&mut self) -> Option<u32> {
        match self.u8()? {
            b if b < 0x80 => Some(b.into()),
            0x80 => self.u32(),
            _ => None,
        }
    }

    /// An unsigned LEB128 value (at most 32 bits).
    pub(crate) fn leb(&mut self) -> Option<u32> {
        let mut v = 0u32;
        for shift in [0, 7, 14, 21, 28] {
            let b = self.u8()?;
            let bits = u32::from(b & 0x7F);
            if shift == 28 && bits > 0x0F {
                return None;
            }
            v |= bits << shift;
            if b < 0x80 {
                return Some(v);
            }
        }
        None
    }

    /// A NUL-terminated string (without its NUL).
    pub(crate) fn sz(&mut self) -> Option<&'a [u8]> {
        let rest = self.d.get(self.p..)?;
        let n = rest.iter().position(|&b| b == 0)?;
        self.p += n + 1;
        Some(&rest[..n])
    }

    /// A string led by its compressed length.
    pub(crate) fn lstr(&mut self) -> Option<&'a [u8]> {
        let n = self.c16()?;
        self.take(n.into())
    }
}

/// `.cil$db`'s CodeView records.
#[derive(Clone, Debug)]
pub struct Types<'data> {
    /// The type records (numbered from 0x1000), then the id records
    /// (numbered on from the types), each its leaf kind and the bytes
    /// after it.
    pub records: Vec<(u16, &'data [u8])>,
    /// How many of `records` are types (the rest are ids).
    pub type_count: usize,
}

impl<'data> Types<'data> {
    /// The first record's index.
    pub const FIRST_INDEX: u32 = 0x1000;

    pub fn parse(d: &'data [u8]) -> Result<Self> {
        let bad = |at: usize, what: &'static str| Error::new(ErrorKind::Malformed, at as u64, what);
        let mut r = Reader::new(d, 10);
        if d.len() < 10 {
            return Err(Error::new(ErrorKind::Truncated, 0, ".cil$db header"));
        }
        let mut records = Vec::new();
        let mut type_count = None;
        while r.p < d.len() {
            let at = r.p;
            match r.u8() {
                Some(0x0B) => {
                    let len = usize::from(r.c16().ok_or(bad(at, ".cil$db record"))?);
                    let rec = r
                        .take(len)
                        .filter(|rec| rec.len() >= 2)
                        .ok_or(bad(at, ".cil$db record"))?;
                    records.push((u16::from_le_bytes([rec[0], rec[1]]), &rec[2..]));
                }
                // Four lists (tags 0x18, 0x1C, 0x1D, 0x1E) of ids, between
                // the types and the ids.
                Some(0x18) if type_count.is_none() => {
                    type_count = Some(records.len());
                    r.p = at;
                    for tag in [0x18, 0x1C, 0x1D, 0x1E] {
                        if r.u8() != Some(tag) {
                            return Err(bad(r.p - 1, ".cil$db id lists"));
                        }
                        let n = r.c16().ok_or(bad(r.p, ".cil$db id lists"))?;
                        for _ in 0..n {
                            r.id().ok_or(bad(r.p, ".cil$db id lists"))?;
                        }
                    }
                }
                // (What follows the ids is not read.)
                _ => break,
            }
        }
        let type_count = type_count.unwrap_or(records.len());
        Ok(Self {
            records,
            type_count,
        })
    }
}

#[cfg(test)]
mod tests;
