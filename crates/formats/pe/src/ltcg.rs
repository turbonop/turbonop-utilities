//! Visual C++ link-time code generation objects (`cl /GL`): an anonymous
//! object header (version 1, class ID [`CLASS_ID`]) around a COFF object
//! whose sections hold the compiler's intermediate language, which
//! `link /LTCG` hands to the back end (the c2.dll that `.drectve` names).
//!
//! The IL is undocumented. What is read here was worked out from
//! objects MSVC 14.5x writes, and checked against what the tools show of
//! them (lib.exe's symbol index, the sources' SHA-256 digests, the
//! CodeView records' framing):
//!
//! - `.drectve`: `-compiler:"<path of c2.dll>"`.
//! - `.cil$fg`: the back end's command line (a count, then strings).
//! - `.cil$gl`: the global symbol table: a header; the source files (their
//!   paths and checksums); the functions, variables, and the like, each
//!   with an id, flags, a decorated name, and attributes; and last the
//!   linker directives (`#pragma comment(lib)` and `(linker)`,
//!   `detect_mismatch`).
//! - `.cil$db`: the CodeView type and id records (a header, then records
//!   each framed by a tag and a compressed length).
//! - `.cil$ex` (function bodies), `.cil$sy` (their local symbols),
//!   `.cil$in` (initializers): their sizes only.
//!
//! Symbols are found by their records' heads (tag, id, flags, name, and
//! the attributes that follow): the records' full grammar is not known,
//! so they are not walked one after another. A function with a body
//! (tag 0x0E) has more after its attributes, worked out from objects
//! that differ by a function, a call, or a body's length:
//!
//! - two 32-bit fields, the second the kind of function (0 a free
//!   function, 1 a virtual one, 2 a constructor, 3 a destructor), and a
//!   few bytes more for a member function;
//! - its type's index and its id's (`.cil$db`'s `LF_PROCEDURE` or
//!   `LF_MFUNCTION`, and its `LF_FUNC_ID`), a compressed value and 32
//!   bits;
//! - its IL's offset in `.cil$ex` and its local symbols' in `.cil$sy`
//!   (compressed; the bodies follow one another in the records' order,
//!   after `.cil$ex`'s header, so each one's size is the distance to the
//!   next);
//! - a byte that grows with the body (what it counts is not known), and
//!   eight bytes;
//! - its parameter count (`this` counted), and what it refers to: a
//!   count, then each symbol's id and how (1 directly: a call, or a
//!   variable read or written; 2 its address taken; 0 a virtual call,
//!   through the vtable).
//!
//! (Checked against `tools/coff/ltcg.cpp`: `exported` calls what it
//! calls, reads `counter`, `banner`, and `elsewhere`, takes its string
//! literals' addresses, and calls `area` virtually.)

use alloc::vec::Vec;
use hexcavator_core::{Error, ErrorKind, Result};

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
    streams: Vec<Stream<'data>>,
    symbols: Vec<CoffSymbol<'data>>,
}

fn u16_at(d: &[u8], o: usize) -> Option<u16> {
    Some(u16::from_le_bytes(d.get(o..o + 2)?.try_into().ok()?))
}

fn u32_at(d: &[u8], o: usize) -> Option<u32> {
    Some(u32::from_le_bytes(d.get(o..o + 4)?.try_into().ok()?))
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
                string(u32::from_le_bytes(raw[4..8].try_into().unwrap()) as usize)
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
            let at = symtab + i * 18;
            let Some(e) = il.get(at..at + 18) else { break };
            symbols.push(CoffSymbol {
                name: short(&e[..8]),
                value: u32_at(e, 8).unwrap_or(0),
                section: u16_at(e, 12).unwrap_or(0) as i16,
                storage_class: e[16],
            });
            i += 1 + usize::from(e[17]);
        }
        Ok(Self {
            version: 1,
            machine: u16_at(data, 6).unwrap_or(0),
            time_date_stamp: u32_at(data, 8).unwrap_or(0),
            class_id: data[12..28].try_into().unwrap(),
            size_of_data,
            il_version,
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
}

/// A source file the object was compiled from (the main file and every
/// header), with its checksum.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SourceFile<'data> {
    pub id: u32,
    pub path: &'data [u8],
    /// CodeView's checksum kind: 1 MD5, 2 SHA-1, 3 SHA-256.
    pub checksum_kind: u8,
    pub checksum: &'data [u8],
}

/// A linker directive, from `#pragma comment(lib)` or `(linker)`, or
/// `#pragma detect_mismatch`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Directive<'data> {
    pub kind: DirectiveKind,
    pub text: &'data [u8],
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DirectiveKind {
    /// A library to search (`#pragma comment(lib)`: `/DEFAULTLIB`).
    Library,
    /// A linker option (`#pragma comment(linker)`, `detect_mismatch`'s
    /// `/FAILIFMISMATCH`).
    Linker,
    Other(u8),
}

impl DirectiveKind {
    fn of(b: u8) -> Self {
        match b {
            0x04 => Self::Library,
            0x0A => Self::Linker,
            other => Self::Other(other),
        }
    }
}

/// How a symbol is defined or declared.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Storage {
    /// Defined here, with external linkage.
    Defined,
    /// Defined here and `__declspec(dllexport)`.
    Exported,
    /// Declared only (defined elsewhere).
    Declared,
    /// Declared `__declspec(dllimport)`.
    Imported,
    /// Defined here, internal linkage (`static`).
    Static,
    /// A C tentative definition (`int x;`): a common symbol.
    Common,
    Unknown(u8),
}

impl Storage {
    /// Whether the symbol is defined here, visible to other objects.
    pub fn is_public_definition(self) -> bool {
        matches!(self, Self::Defined | Self::Exported | Self::Common)
    }

    pub fn name(self) -> &'static str {
        match self {
            Self::Defined => "defined",
            Self::Exported => "dllexport",
            Self::Declared => "declared",
            Self::Imported => "dllimport",
            Self::Static => "static",
            Self::Common => "common",
            Self::Unknown(_) => "unknown",
        }
    }
}

/// A function's attributes ([`SymbolKind::Function`]'s `attributes`).
pub mod function {
    /// The callee pops its arguments (`__stdcall`, x86 `__thiscall`).
    pub const CALLEE_POPS: u8 = 0x10;
    pub const INLINE: u8 = 0x20;
    /// It takes variable arguments.
    pub const VARARGS: u8 = 0x40;
    pub const NORETURN: u8 = 0x80;
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SymbolKind {
    Function {
        storage: Storage,
        /// [`function`]'s bits.
        attributes: u8,
        /// Whether its record has a body (tag 0x0E; 0x04 is declared,
        /// 0x10 a virtual function declared).
        body: bool,
    },
    Data {
        storage: Storage,
        writable: bool,
        /// Its size, as the IL sees it (pointers that will become RVAs
        /// counted at pointer size).
        size: u64,
    },
}

/// What a function with a body (tag 0x0E) records of it: see the module's
/// documentation.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct FunctionIl<'data> {
    /// Its type's index in `.cil$db` (an `LF_PROCEDURE` or `LF_MFUNCTION`).
    pub type_index: u32,
    /// Its id record's index in `.cil$db` (an `LF_FUNC_ID` or
    /// `LF_MFUNC_ID`).
    pub id_index: u32,
    /// Where its IL starts in `.cil$ex`.
    pub il_offset: u32,
    /// Where its local symbols start in `.cil$sy`.
    pub symbols_offset: u32,
    /// How many parameters it takes (`this` counted).
    pub params: u8,
    /// What it refers to: each symbol's id, and how.
    refs: &'data [u8],
    ref_count: u8,
}

/// How a function refers to a symbol.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RefKind {
    /// Directly: it calls the function, or reads or writes the variable.
    Direct,
    /// It takes its address.
    Address,
    /// It calls the virtual function through a vtable.
    Virtual,
    Other(u8),
}

impl RefKind {
    fn of(b: u8) -> Self {
        match b {
            0 => Self::Virtual,
            1 => Self::Direct,
            2 => Self::Address,
            other => Self::Other(other),
        }
    }
}

impl FunctionIl<'_> {
    /// What it refers to: each symbol's id (as [`IlSymbol::id`]), and how.
    pub fn refs(&self) -> impl Iterator<Item = (u32, RefKind)> + '_ {
        let mut r = Reader { d: self.refs, p: 0 };
        (0..self.ref_count).map_while(move |_| {
            let id = r.id()?;
            let kind = r.u8()?;
            Some((id, RefKind::of(kind)))
        })
    }
}

/// A function or variable in the global symbol table.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct IlSymbol<'data> {
    /// The record's offset in `.cil$gl`.
    pub offset: usize,
    pub id: u32,
    /// The byte before the name: 0x26 for special members, RTTI, and
    /// vftables the compiler made, 0x25 for string literals, 0x24 for
    /// variables of internal linkage, 0 for most (what its bits mean is
    /// not known).
    pub flags: u8,
    /// Its decorated name (x86 C names without the leading underscore).
    pub name: &'data [u8],
    pub kind: SymbolKind,
    /// A function with a body: its IL, type, and what it refers to.
    pub il: Option<FunctionIl<'data>>,
}

impl IlSymbol<'_> {
    pub fn storage(&self) -> Storage {
        match self.kind {
            SymbolKind::Function { storage, .. } | SymbolKind::Data { storage, .. } => storage,
        }
    }
}

/// `.cil$gl`, read.
#[derive(Clone, Debug)]
pub struct Globals<'data> {
    pub source_files: Vec<SourceFile<'data>>,
    pub symbols: Vec<IlSymbol<'data>>,
    pub directives: Vec<Directive<'data>>,
}

impl Globals<'_> {
    /// Each symbol's IL size (a function with a body's; `None` for the
    /// rest), in the order of [`Globals::symbols`]: from its offset in
    /// `.cil$ex` (`il_len` bytes long) to the next body's, or the end.
    pub fn il_sizes(&self, il_len: usize) -> Vec<Option<u32>> {
        let mut at: Vec<(u32, usize)> = self
            .symbols
            .iter()
            .enumerate()
            .filter_map(|(i, s)| Some((s.il?.il_offset, i)))
            .collect();
        at.sort_unstable();
        let mut out = alloc::vec![None; self.symbols.len()];
        let end = u32::try_from(il_len).unwrap_or(u32::MAX);
        for (k, &(offset, i)) in at.iter().enumerate() {
            let next = at.get(k + 1).map_or(end, |n| n.0);
            out[i] = next.checked_sub(offset);
        }
        out
    }
}

/// The size of `.cil$gl`'s header.
const GLOBALS_HEADER: usize = 30;

struct Reader<'a> {
    d: &'a [u8],
    p: usize,
}

impl<'a> Reader<'a> {
    fn u8(&mut self) -> Option<u8> {
        let b = *self.d.get(self.p)?;
        self.p += 1;
        Some(b)
    }

    fn u16(&mut self) -> Option<u16> {
        let v = u16_at(self.d, self.p)?;
        self.p += 2;
        Some(v)
    }

    fn u32(&mut self) -> Option<u32> {
        let v = u32_at(self.d, self.p)?;
        self.p += 4;
        Some(v)
    }

    /// An id: a 16-bit unit, and another when the first's top bit is set.
    fn id(&mut self) -> Option<u32> {
        let w = self.u16()?;
        if w & 0x8000 == 0 {
            return Some(w.into());
        }
        let hi = self.u16()?;
        Some(u32::from(w & 0x7FFF) | (u32::from(hi) << 15))
    }

    /// A compressed 16-bit value: a byte below 0x80, else 0x80 and the
    /// value.
    fn c16(&mut self) -> Option<u16> {
        match self.u8()? {
            b if b < 0x80 => Some(b.into()),
            0x80 => self.u16(),
            _ => None,
        }
    }

    /// A compressed 32-bit value.
    fn c32(&mut self) -> Option<u32> {
        match self.u8()? {
            b if b < 0x80 => Some(b.into()),
            0x80 => self.u32(),
            _ => None,
        }
    }

    fn sz(&mut self) -> Option<&'a [u8]> {
        let rest = self.d.get(self.p..)?;
        let n = rest.iter().position(|&b| b == 0)?;
        self.p += n + 1;
        Some(&rest[..n])
    }
}

/// Whether `name` could be a symbol's name: of the characters decorated
/// and C names are made of, starting as they do.
fn is_name(name: &[u8]) -> bool {
    matches!(
        name.first(),
        Some(b'?' | b'_' | b'$' | b'@' | b'a'..=b'z' | b'A'..=b'Z')
    ) && name.iter().all(|&b| {
        b.is_ascii_alphanumeric() || matches!(b, b'?' | b'@' | b'$' | b'_' | b'<' | b'>' | b'-')
    })
}

impl<'data> Globals<'data> {
    pub fn parse(g: &'data [u8]) -> Result<Self> {
        let bad = |at: usize, what: &'static str| Error::new(ErrorKind::Malformed, at as u64, what);
        if g.len() < GLOBALS_HEADER + 4 {
            return Err(Error::new(ErrorKind::Truncated, 0, ".cil$gl header"));
        }
        let mut r = Reader {
            d: g,
            p: GLOBALS_HEADER,
        };
        // (Records of three compressed values, of predefined things.)
        while g.get(r.p) == Some(&0x18) {
            r.p += 1;
            for _ in 0..3 {
                r.c16().ok_or(bad(r.p, ".cil$gl record 0x18"))?;
            }
        }
        let mut source_files = Vec::new();
        while let Some((file, end)) = Self::file_at(g, r.p) {
            source_files.push(file);
            r.p = end;
        }
        let files_end = r.p;
        let (directives, directives_at) = Self::directives(g, files_end);
        let symbols = Self::symbols(&g[..directives_at], files_end, &mut source_files);
        Ok(Self {
            source_files,
            symbols,
            directives,
        })
    }

    /// The directives record (tag 0x0A, two zero bytes, a count, and
    /// (kind, string) pairs), which ends the table; and where it starts.
    fn directives(g: &'data [u8], from: usize) -> (Vec<Directive<'data>>, usize) {
        // (The last place one parses to the very end; searched for from
        // the end, as the symbols before it are not walked.)
        let mut at = g.len().saturating_sub(4);
        while at >= from {
            if g[at] == 0x0A
                && g.get(at + 1..at + 3) == Some(&[0, 0])
                && let Some(d) = Self::directives_at(g, at)
            {
                return (d, at);
            }
            if at == 0 {
                break;
            }
            at -= 1;
        }
        (Vec::new(), g.len())
    }

    fn directives_at(g: &'data [u8], at: usize) -> Option<Vec<Directive<'data>>> {
        let mut r = Reader { d: g, p: at + 3 };
        let count = r.u8()?;
        let mut out = Vec::with_capacity(count.into());
        for _ in 0..count {
            let kind = r.u8()?;
            let text = r.sz()?;
            if text.is_empty() || !text.iter().all(|&b| b >= 0x20) {
                return None;
            }
            out.push(Directive {
                kind: DirectiveKind::of(kind),
                text,
            });
        }
        (r.p == g.len()).then_some(out)
    }

    /// The functions and variables: each record found by its head, from
    /// the first that starts at or after `from`, none overlapping the
    /// one before it.
    fn symbols(
        g: &'data [u8],
        from: usize,
        files: &mut Vec<SourceFile<'data>>,
    ) -> Vec<IlSymbol<'data>> {
        let mut out = Vec::new();
        let mut p = from;
        while p < g.len() {
            // (Source files come among the symbols too: those first
            // included where they are.)
            if let Some((file, end)) = Self::file_at(g, p) {
                files.push(file);
                p = end;
                continue;
            }
            match Self::symbol_at(g, p) {
                Some((mut s, end)) => {
                    // (A record head that starts a few bytes later and
                    // ends at the same name has the longer name: it is
                    // the record, the first a run of bytes before it,
                    // its id's, that happened to look like one.)
                    for q in p + 1..(p + 8).min(end) {
                        if let Some((t, e)) = Self::symbol_at(g, q)
                            && e == end
                            && t.name.len() > s.name.len()
                        {
                            s = t;
                        }
                    }
                    out.push(s);
                    p = end;
                }
                None => p += 1,
            }
        }
        out
    }

    /// The source file whose record (tag 0x12, id, path, checksum's
    /// length and kind, checksum) starts at `p`, and where it ends.
    fn file_at(g: &'data [u8], p: usize) -> Option<(SourceFile<'data>, usize)> {
        if *g.get(p)? != 0x12 {
            return None;
        }
        let mut r = Reader { d: g, p: p + 1 };
        let id = r.id()?;
        let path = r.sz()?;
        let len = usize::from(r.u8()?);
        let checksum_kind = r.u8()?;
        let plausible = path.len() >= 2
            && path.iter().all(|&b| b >= 0x20)
            && (path.contains(&b'\\') || path.contains(&b'/'))
            && matches!((checksum_kind, len), (1, 16) | (2, 20) | (3, 32) | (0, 0));
        if !plausible {
            return None;
        }
        let checksum = g.get(r.p..r.p + len)?;
        Some((
            SourceFile {
                id,
                path,
                checksum_kind,
                checksum,
            },
            r.p + len,
        ))
    }

    /// The symbol whose record starts at `p`, and where its name ends.
    fn symbol_at(g: &'data [u8], p: usize) -> Option<(IlSymbol<'data>, usize)> {
        let tag = *g.get(p)?;
        let function = matches!(tag, 0x0E | 0x04 | 0x10);
        let data = matches!(tag, 0x01..=0x03);
        if !function && !data {
            return None;
        }
        let mut r = Reader { d: g, p: p + 1 };
        if data && r.u8()? != 0 {
            return None;
        }
        let id = r.id()?;
        let flags = r.u8()?;
        if flags > 0x3F {
            return None;
        }
        let name = r.sz()?;
        if !is_name(name) {
            return None;
        }
        let end = r.p;
        let attrs_at = r.p;
        let a = g.get(r.p..r.p + 6)?;
        // (The first attribute: the value's machine type, 0x80-0x8F.)
        if a[0] & 0xF0 != 0x80 {
            return None;
        }
        let kind = if function {
            let storage = match a[2] {
                0x03 => Storage::Static,
                0x04 => Storage::Declared,
                0x05 => Storage::Defined,
                0x09 => Storage::Exported,
                0x0A => Storage::Imported,
                _ => return None,
            };
            SymbolKind::Function {
                storage,
                attributes: a[4],
                body: tag == 0x0E,
            }
        } else {
            if a[2] != 0 {
                return None;
            }
            let writable = match a[3] {
                0x02 => true,
                0x04 | 0x08 => false,
                _ => return None,
            };
            let storage = match a[4] {
                0x01 => Storage::Defined,
                0x02 => Storage::Declared,
                0x03 => Storage::Common,
                0x04 => Storage::Static,
                0x07 => Storage::Exported,
                _ => return None,
            };
            r.p += 5;
            let size = r.c32()?;
            SymbolKind::Data {
                storage,
                writable,
                size: size.into(),
            }
        };
        let il = if tag == 0x0E {
            Self::function_il(g, attrs_at + 6)
        } else {
            None
        };
        Some((
            IlSymbol {
                offset: p,
                id,
                flags,
                name,
                kind,
                il,
            },
            end,
        ))
    }

    /// A function body's record after its attributes (at `p`): see the
    /// module's documentation. The bytes a member function has before its
    /// type are skipped to its type's index and its id's: a compressed
    /// type index (`0x80`, then 32 bits) and an id index after it, both
    /// past `.cil$db`'s first index and the id past the type.
    fn function_il(g: &'data [u8], p: usize) -> Option<FunctionIl<'data>> {
        let first = Types::FIRST_INDEX;
        let at = (p + 8..p + 24).find(|&q| {
            g.get(q) == Some(&0x80)
                && matches!(
                    (u32_at(g, q + 1), u32_at(g, q + 5)),
                    (Some(t), Some(i)) if t >= first && t < 0x0100_0000 && i > t && i < 0x0100_0000
                )
        })?;
        let mut r = Reader { d: g, p: at };
        let type_index = r.c32()?;
        let id_index = r.u32()?;
        let il_offset = r.c32()?;
        let symbols_offset = r.c32()?;
        r.p += 1 + 8;
        let params = r.u8()?;
        let ref_count = r.u8()?;
        let start = r.p;
        for _ in 0..ref_count {
            r.id()?;
            r.u8()?;
        }
        Some(FunctionIl {
            type_index,
            id_index,
            il_offset,
            symbols_offset,
            params,
            refs: g.get(start..r.p)?,
            ref_count,
        })
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
        let mut r = Reader { d, p: 10 };
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
                    let rec = d
                        .get(r.p..r.p + len)
                        .filter(|rec| rec.len() >= 2)
                        .ok_or(bad(at, ".cil$db record"))?;
                    r.p += len;
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
mod tests {
    use super::*;

    #[test]
    fn readers() {
        let mut r = Reader {
            d: &[
                0x2d, 0x1f, 0xaf, 0x97, 0x01, 0x00, 0x74, 0x80, 0x05, 0x10, 0, 0, 0x80, 0xff, 0xff,
            ],
            p: 0,
        };
        assert_eq!(r.id(), Some(0x1f2d));
        assert_eq!(r.id(), Some(0x97af));
        assert_eq!(r.c32(), Some(0x74));
        assert_eq!(r.c32(), Some(0x1005));
        assert_eq!(r.c16(), Some(0xffff));
        assert_eq!(r.u8(), None);
    }

    #[test]
    fn not_ltcg() {
        assert!(!LtcgObject::is_ltcg(&[0; 40]));
        let mut h = [0u8; 40];
        h[2] = 0xff;
        h[3] = 0xff;
        h[4] = 1;
        h[12..28].copy_from_slice(&CLASS_ID);
        assert!(LtcgObject::is_ltcg(&h));
        // (No IL object after it.)
        assert!(LtcgObject::parse(&h[..32]).is_err());
    }
}
