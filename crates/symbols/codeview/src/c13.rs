//! C13 debug subsections (`DEBUG_S_*`), read into editable forms and
//! written back byte for byte.
//!
//! A COFF object's `.debug$S` is [`CV_SIGNATURE_C13`] and then
//! subsections, each a kind, a length, the contents, and zeros to the next
//! multiple of 4 ([`subsections`], [`push_subsection`]). A PDB module
//! stream's C13 part is the same subsections without the signature.
//!
//! [`Contents`] reads every kind cvinfo.h defines: symbols, lines, the
//! string table, file checksums, frame data, inlinee lines (both
//! signatures), cross-scope imports and exports, IL lines, the function
//! and type metadata-token maps, merged assembly input, and COFF symbol
//! RVAs. Others, and kinds marked `DEBUG_S_IGNORE`, are kept as their
//! bytes. [`fields`] finds where each kind refers outside itself: type and
//! id indices, addresses, string-table and file-checksum offsets (the same
//! [`Field`]s as symbol records').

use crate::lines::subsection as kind;
use crate::lines::{CV_LINES_HAVE_COLUMNS, FileChecksum};
use crate::symbol_refs::{self, Field, FieldError};
use crate::symbols::SymbolRecord;
use turbonop_core::Reader;

/// What `.debug$S` and `.debug$T` start with: C13 CodeView.
pub const CV_SIGNATURE_C13: u32 = 4;

/// Why subsections could not be read.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Error {
    /// The data ends inside something: a subsection of `kind` (`u32::MAX`
    /// for the subsections' framing), at `at` in its contents.
    Truncated {
        /// The subsection's kind.
        kind: u32,
        /// Where, in its contents.
        at: usize,
    },
    /// A value the format doesn't allow: a signature, padding that isn't
    /// zero, bytes past a subsection's last entry, a count that doesn't
    /// fit.
    Malformed {
        /// The subsection's kind.
        kind: u32,
        /// Where, in its contents.
        at: usize,
    },
    /// A symbol record's fields: at `at` in its symbols subsection.
    Symbol {
        /// Where the record starts, in the subsection's contents.
        at: usize,
        /// What was wrong with it.
        error: FieldError,
    },
}

impl core::fmt::Display for Error {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Error::Truncated { kind, at } => {
                write!(f, "subsection {kind:#x} truncated at {at:#x}")
            }
            Error::Malformed { kind, at } => {
                write!(f, "subsection {kind:#x} malformed at {at:#x}")
            }
            Error::Symbol { at, error } => write!(f, "symbol at {at:#x}: {error}"),
        }
    }
}

/// The kind used in errors about the framing of subsections.
const FRAMING: u32 = u32::MAX;

/// One subsection: its kind and its contents (without the padding after
/// them).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Subsection<'a> {
    /// `DEBUG_S_*`, with `DEBUG_S_IGNORE` if it is set.
    pub kind: u32,
    /// The contents.
    pub data: &'a [u8],
}

/// How many zeros pad contents of `len` bytes to 4.
fn padding(len: usize) -> usize {
    len.wrapping_neg() & 3
}

/// The subsections of `c13` (a `.debug$S` after its signature, or a PDB
/// module stream's C13 part). Each must be padded to 4 bytes with zeros,
/// the last too.
pub fn subsections(c13: &[u8]) -> Result<Vec<Subsection<'_>>, Error> {
    let mut r = Reader::new(c13);
    let mut out = Vec::new();
    while !r.is_empty() {
        let at = r.position();
        let truncated = Error::Truncated { kind: FRAMING, at };
        let kind = r.u32_le().map_err(|_| truncated)?;
        let len = r.u32_le().map_err(|_| truncated)? as usize;
        let data = r.bytes(len).map_err(|_| Error::Truncated {
            kind,
            at: r.remaining().len(),
        })?;
        let pad_at = r.position();
        let pad = r.bytes(padding(len)).map_err(|_| Error::Truncated {
            kind: FRAMING,
            at: pad_at,
        })?;
        if pad.iter().any(|&b| b != 0) {
            return Err(Error::Malformed {
                kind: FRAMING,
                at: pad_at,
            });
        }
        out.push(Subsection { kind, data });
    }
    Ok(out)
}

/// The subsections of a `.debug$S` section (its [`CV_SIGNATURE_C13`]
/// first).
pub fn section_subsections(section: &[u8]) -> Result<Vec<Subsection<'_>>, Error> {
    let malformed = Error::Malformed {
        kind: FRAMING,
        at: 0,
    };
    let signature = section.get(..4).ok_or(Error::Truncated {
        kind: FRAMING,
        at: 0,
    })?;
    if signature != CV_SIGNATURE_C13.to_le_bytes() {
        return Err(malformed);
    }
    subsections(&section[4..])
}

/// Appends a subsection: its kind, length, contents, and zeros to 4 bytes
/// (from the subsection's start, which must itself be at a multiple of 4).
pub fn push_subsection(out: &mut Vec<u8>, kind: u32, data: &[u8]) {
    out.extend_from_slice(&kind.to_le_bytes());
    out.extend_from_slice(&(data.len() as u32).to_le_bytes());
    out.extend_from_slice(data);
    out.resize(out.len() + padding(data.len()), 0);
}

/// A lines subsection (`DEBUG_S_LINES`, and `DEBUG_S_IL_LINES`, which
/// cvdump reads alike): the code it covers, and a block per file.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Lines {
    /// The code's offset in its section (relocated: `SECREL`).
    pub offset: u32,
    /// The code's section (relocated: `SECTION`).
    pub segment: u16,
    /// `CV_LINES_HAVE_COLUMNS`, ...
    pub flags: u16,
    /// The code's size.
    pub code_size: u32,
    /// The files' blocks.
    pub blocks: Vec<LineBlock>,
}

/// One file's lines in a [`Lines`].
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct LineBlock {
    /// The file: an offset in the file checksums.
    pub file: u32,
    /// The lines (`CV_Line_t`): an offset from the code's start, and the
    /// line, end delta, and statement flag.
    pub lines: Vec<(u32, u32)>,
    /// Their columns (`CV_Column_t`), if the subsection has them: start
    /// and end.
    pub columns: Vec<(u16, u16)>,
}

impl Lines {
    /// Reads a lines subsection's contents.
    pub fn parse(data: &[u8]) -> Result<Self, Error> {
        let mut r = Reader::new(data);
        let t = |r: &Reader<'_>| Error::Truncated {
            kind: kind::DEBUG_S_LINES,
            at: r.position(),
        };
        let offset = r.u32_le().map_err(|_| t(&r))?;
        let segment = r.u16_le().map_err(|_| t(&r))?;
        let flags = r.u16_le().map_err(|_| t(&r))?;
        let code_size = r.u32_le().map_err(|_| t(&r))?;
        let columns = flags & CV_LINES_HAVE_COLUMNS != 0;
        let mut blocks = Vec::new();
        while !r.is_empty() {
            let start = r.position();
            let file = r.u32_le().map_err(|_| t(&r))?;
            let count = r.u32_le().map_err(|_| t(&r))? as usize;
            let size = r.u32_le().map_err(|_| t(&r))? as usize;
            let mut block = LineBlock {
                file,
                lines: Vec::with_capacity(count.min(r.remaining().len() / 8)),
                columns: Vec::new(),
            };
            for _ in 0..count {
                let o = r.u32_le().map_err(|_| t(&r))?;
                let l = r.u32_le().map_err(|_| t(&r))?;
                block.lines.push((o, l));
            }
            if columns {
                block.columns.reserve(count);
                for _ in 0..count {
                    let s = r.u16_le().map_err(|_| t(&r))?;
                    let e = r.u16_le().map_err(|_| t(&r))?;
                    block.columns.push((s, e));
                }
            }
            // The block's size is what was read: nothing else is in it.
            if r.position() - start != size {
                return Err(Error::Malformed {
                    kind: kind::DEBUG_S_LINES,
                    at: start + 8,
                });
            }
            blocks.push(block);
        }
        Ok(Self {
            offset,
            segment,
            flags,
            code_size,
            blocks,
        })
    }

    /// Appends the contents.
    pub fn write(&self, out: &mut Vec<u8>) {
        out.extend_from_slice(&self.offset.to_le_bytes());
        out.extend_from_slice(&self.segment.to_le_bytes());
        out.extend_from_slice(&self.flags.to_le_bytes());
        out.extend_from_slice(&self.code_size.to_le_bytes());
        let columns = self.flags & CV_LINES_HAVE_COLUMNS != 0;
        for b in &self.blocks {
            let n = b.lines.len();
            let size = 12 + 8 * n + if columns { 4 * n } else { 0 };
            out.extend_from_slice(&b.file.to_le_bytes());
            out.extend_from_slice(&(n as u32).to_le_bytes());
            out.extend_from_slice(&(size as u32).to_le_bytes());
            for &(o, l) in &b.lines {
                out.extend_from_slice(&o.to_le_bytes());
                out.extend_from_slice(&l.to_le_bytes());
            }
            if columns {
                for i in 0..n {
                    let (s, e) = b.columns.get(i).copied().unwrap_or_default();
                    out.extend_from_slice(&s.to_le_bytes());
                    out.extend_from_slice(&e.to_le_bytes());
                }
            }
        }
    }
}

/// The entries of a file checksums subsection (`DEBUG_S_FILECHKSMS`), each
/// at its offset (what lines and inlinee lines name files by).
pub fn file_checksums(data: &[u8]) -> Result<Vec<(u32, FileChecksum<'_>)>, Error> {
    let mut r = Reader::new(data);
    let mut out = Vec::new();
    while !r.is_empty() {
        let at = r.position();
        let t = Error::Truncated {
            kind: kind::DEBUG_S_FILECHKSMS,
            at,
        };
        let name = r.u32_le().map_err(|_| t)?;
        let len = usize::from(r.u8().map_err(|_| t)?);
        let checksum_kind = r.u8().map_err(|_| t)?;
        let checksum = r.bytes(len).map_err(|_| t)?;
        let pad = r.bytes(padding(6 + len)).map_err(|_| t)?;
        if pad.iter().any(|&b| b != 0) {
            return Err(Error::Malformed {
                kind: kind::DEBUG_S_FILECHKSMS,
                at,
            });
        }
        out.push((
            at as u32,
            FileChecksum {
                name,
                kind: checksum_kind,
                checksum,
            },
        ));
    }
    Ok(out)
}

/// Appends a file checksum entry, padded to 4 bytes; its offset in `out`
/// (the entry's offset in the subsection, if `out` holds only its
/// contents).
pub fn push_file_checksum(out: &mut Vec<u8>, entry: &FileChecksum<'_>) -> u32 {
    let at = out.len() as u32;
    out.extend_from_slice(&entry.name.to_le_bytes());
    out.push(entry.checksum.len() as u8);
    out.push(entry.kind);
    out.extend_from_slice(entry.checksum);
    out.resize(out.len() + padding(6 + entry.checksum.len()), 0);
    at
}

/// The strings of a string table (`DEBUG_S_STRINGTABLE`, a PDB's
/// `/names`' buffer), each at its offset; a last string without its zero
/// is an error.
pub fn strings(table: &[u8]) -> Result<Vec<(u32, &[u8])>, Error> {
    let mut out = Vec::new();
    let mut at = 0;
    while at < table.len() {
        let Some(end) = table[at..].iter().position(|&b| b == 0) else {
            return Err(Error::Truncated {
                kind: kind::DEBUG_S_STRINGTABLE,
                at,
            });
        };
        out.push((at as u32, &table[at..at + end]));
        at += end + 1;
    }
    Ok(out)
}

/// Builds a string table: each string once, at the offset of its first
/// insertion. It starts with the empty string at 0, as compilers' do.
#[derive(Clone, Debug)]
pub struct StringTableBuilder {
    bytes: Vec<u8>,
    offsets: std::collections::HashMap<Vec<u8>, u32>,
}

impl Default for StringTableBuilder {
    fn default() -> Self {
        Self::new()
    }
}

impl StringTableBuilder {
    /// A table holding the empty string, at 0.
    pub fn new() -> Self {
        let mut offsets = std::collections::HashMap::new();
        offsets.insert(Vec::new(), 0);
        Self {
            bytes: vec![0],
            offsets,
        }
    }

    /// The offset of `s` (which must not hold a zero), added if it isn't
    /// in the table yet.
    pub fn insert(&mut self, s: &[u8]) -> u32 {
        debug_assert!(!s.contains(&0));
        if let Some(&at) = self.offsets.get(s) {
            return at;
        }
        let at = self.bytes.len() as u32;
        self.bytes.extend_from_slice(s);
        self.bytes.push(0);
        self.offsets.insert(s.to_vec(), at);
        at
    }

    /// The table's bytes.
    pub fn bytes(&self) -> &[u8] {
        &self.bytes
    }
}

/// `DEBUG_S_INLINEELINES`'s signatures: entries without extra files
/// (`CV_INLINEE_SOURCE_LINE_SIGNATURE`), and with (`..._EX`).
pub const CV_INLINEE_SOURCE_LINE_SIGNATURE: u32 = 0;
/// See [`CV_INLINEE_SOURCE_LINE_SIGNATURE`].
pub const CV_INLINEE_SOURCE_LINE_SIGNATURE_EX: u32 = 1;

/// An inlinee lines subsection (`DEBUG_S_INLINEELINES`): where each inlined
/// function's source starts.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct InlineeLines {
    /// Whether entries list extra files (`..._SIGNATURE_EX`).
    pub extended: bool,
    /// The entries.
    pub entries: Vec<InlineeSource>,
}

/// One inlined function's source (`InlineeSourceLine`, `...Ex`).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct InlineeSource {
    /// The function: an id (`LF_FUNC_ID`, `LF_MFUNC_ID`).
    pub inlinee: u32,
    /// Its file: an offset in the file checksums.
    pub file: u32,
    /// The line its definition starts at.
    pub line: u32,
    /// Other files its code comes from (extended entries only): offsets in
    /// the file checksums.
    pub extra_files: Vec<u32>,
}

impl InlineeLines {
    /// Reads an inlinee lines subsection's contents.
    pub fn parse(data: &[u8]) -> Result<Self, Error> {
        let k = kind::DEBUG_S_INLINEELINES;
        let mut r = Reader::new(data);
        let t = |r: &Reader<'_>| Error::Truncated {
            kind: k,
            at: r.position(),
        };
        let extended = match r.u32_le().map_err(|_| t(&r))? {
            CV_INLINEE_SOURCE_LINE_SIGNATURE => false,
            CV_INLINEE_SOURCE_LINE_SIGNATURE_EX => true,
            _ => return Err(Error::Malformed { kind: k, at: 0 }),
        };
        let mut entries = Vec::new();
        while !r.is_empty() {
            let mut e = InlineeSource {
                inlinee: r.u32_le().map_err(|_| t(&r))?,
                file: r.u32_le().map_err(|_| t(&r))?,
                line: r.u32_le().map_err(|_| t(&r))?,
                extra_files: Vec::new(),
            };
            if extended {
                let n = r.u32_le().map_err(|_| t(&r))? as usize;
                if n > r.remaining().len() / 4 {
                    return Err(t(&r));
                }
                for _ in 0..n {
                    e.extra_files.push(r.u32_le().map_err(|_| t(&r))?);
                }
            }
            entries.push(e);
        }
        Ok(Self { extended, entries })
    }

    /// Appends the contents.
    pub fn write(&self, out: &mut Vec<u8>) {
        let signature = if self.extended {
            CV_INLINEE_SOURCE_LINE_SIGNATURE_EX
        } else {
            CV_INLINEE_SOURCE_LINE_SIGNATURE
        };
        out.extend_from_slice(&signature.to_le_bytes());
        for e in &self.entries {
            for v in [e.inlinee, e.file, e.line] {
                out.extend_from_slice(&v.to_le_bytes());
            }
            if self.extended {
                out.extend_from_slice(&(e.extra_files.len() as u32).to_le_bytes());
                for f in &e.extra_files {
                    out.extend_from_slice(&f.to_le_bytes());
                }
            }
        }
    }
}

/// One function's frame (`FRAMEDATA`), in a frame data subsection: x86's
/// FPO data.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Frame {
    /// The code's start, relative to the subsection's base (`ulRvaStart`).
    pub rva_start: u32,
    /// The code's size (`cbBlock`).
    pub code_size: u32,
    /// Bytes of locals (`cbLocals`).
    pub locals_size: u32,
    /// Bytes of parameters (`cbParams`).
    pub params_size: u32,
    /// The most stack used (`cbStkMax`).
    pub max_stack: u32,
    /// The frame's program: an offset in the string table (`frameFunc`).
    pub frame_func: u32,
    /// The prolog's size (`cbProlog`).
    pub prolog_size: u16,
    /// Bytes of saved registers (`cbSavedRegs`).
    pub saved_regs_size: u16,
    /// `fHasSEH`, `fHasEH`, `fIsFunctionStart`.
    pub flags: u32,
}

/// A frame's size in a frame data subsection.
const FRAME_SIZE: usize = 32;

/// A frame data subsection (`DEBUG_S_FRAMEDATA`).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct FrameData {
    /// The address frames are relative to, which an object relocates
    /// (`ADDR32NB`). A PDB's subsections may lack it: LLVM reads it when
    /// the contents aren't a whole number of frames, as here.
    pub base: Option<u32>,
    /// The frames.
    pub frames: Vec<Frame>,
}

impl FrameData {
    /// Reads a frame data subsection's contents.
    pub fn parse(data: &[u8]) -> Result<Self, Error> {
        let k = kind::DEBUG_S_FRAMEDATA;
        let mut r = Reader::new(data);
        let base = if data.len() % FRAME_SIZE == 4 {
            Some(
                r.u32_le()
                    .map_err(|_| Error::Truncated { kind: k, at: 0 })?,
            )
        } else {
            None
        };
        if !r.remaining().len().is_multiple_of(FRAME_SIZE) {
            return Err(Error::Malformed {
                kind: k,
                at: r.position(),
            });
        }
        let mut frames = Vec::with_capacity(r.remaining().len() / FRAME_SIZE);
        while !r.is_empty() {
            let at = r.position();
            let t = |_| Error::Truncated { kind: k, at };
            frames.push(Frame {
                rva_start: r.u32_le().map_err(t)?,
                code_size: r.u32_le().map_err(t)?,
                locals_size: r.u32_le().map_err(t)?,
                params_size: r.u32_le().map_err(t)?,
                max_stack: r.u32_le().map_err(t)?,
                frame_func: r.u32_le().map_err(t)?,
                prolog_size: r.u16_le().map_err(t)?,
                saved_regs_size: r.u16_le().map_err(t)?,
                flags: r.u32_le().map_err(t)?,
            });
        }
        Ok(Self { base, frames })
    }

    /// Appends the contents.
    pub fn write(&self, out: &mut Vec<u8>) {
        if let Some(b) = self.base {
            out.extend_from_slice(&b.to_le_bytes());
        }
        for f in &self.frames {
            for v in [
                f.rva_start,
                f.code_size,
                f.locals_size,
                f.params_size,
                f.max_stack,
                f.frame_func,
            ] {
                out.extend_from_slice(&v.to_le_bytes());
            }
            out.extend_from_slice(&f.prolog_size.to_le_bytes());
            out.extend_from_slice(&f.saved_regs_size.to_le_bytes());
            out.extend_from_slice(&f.flags.to_le_bytes());
        }
    }
}

/// One module's ids that this one uses (`CrossScopeReferences`), in a
/// cross-scope imports subsection.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct CrossScopeImport {
    /// The other module: its name's offset in the string table
    /// (`externalScope`).
    pub module: u32,
    /// The ids, the other module's (not indices into this one's).
    pub ids: Vec<u32>,
}

/// Reads a cross-scope imports subsection's contents
/// (`DEBUG_S_CROSSSCOPEIMPORTS`).
pub fn cross_scope_imports(data: &[u8]) -> Result<Vec<CrossScopeImport>, Error> {
    let k = kind::DEBUG_S_CROSSSCOPEIMPORTS;
    let mut r = Reader::new(data);
    let mut out = Vec::new();
    while !r.is_empty() {
        let t = |r: &Reader<'_>| Error::Truncated {
            kind: k,
            at: r.position(),
        };
        let module = r.u32_le().map_err(|_| t(&r))?;
        let n = r.u32_le().map_err(|_| t(&r))? as usize;
        if n > r.remaining().len() / 4 {
            return Err(t(&r));
        }
        let mut ids = Vec::with_capacity(n);
        for _ in 0..n {
            ids.push(r.u32_le().map_err(|_| t(&r))?);
        }
        out.push(CrossScopeImport { module, ids });
    }
    Ok(out)
}

/// Appends cross-scope imports.
pub fn write_cross_scope_imports(imports: &[CrossScopeImport], out: &mut Vec<u8>) {
    for i in imports {
        out.extend_from_slice(&i.module.to_le_bytes());
        out.extend_from_slice(&(i.ids.len() as u32).to_le_bytes());
        for id in &i.ids {
            out.extend_from_slice(&id.to_le_bytes());
        }
    }
}

/// Reads pairs of 4-byte values: cross-scope exports
/// (`DEBUG_S_CROSSSCOPEEXPORTS`: each a local id, an index in this
/// module, and the global id it is exported as).
pub fn pairs(kind: u32, data: &[u8]) -> Result<Vec<(u32, u32)>, Error> {
    if !data.len().is_multiple_of(8) {
        return Err(Error::Malformed {
            kind,
            at: data.len() / 8 * 8,
        });
    }
    Ok(data
        .as_chunks::<8>()
        .0
        .iter()
        .map(|c| {
            (
                u32::from_le_bytes([c[0], c[1], c[2], c[3]]),
                u32::from_le_bytes([c[4], c[5], c[6], c[7]]),
            )
        })
        .collect())
}

fn write_pairs(pairs: &[(u32, u32)], out: &mut Vec<u8>) {
    for &(a, b) in pairs {
        out.extend_from_slice(&a.to_le_bytes());
        out.extend_from_slice(&b.to_le_bytes());
    }
}

/// A metadata-token map (`DEBUG_S_FUNC_MDTOKEN_MAP`,
/// `DEBUG_S_TYPE_MDTOKEN_MAP`): a count, that many pairs, and data after
/// them (which cvdump prints as raw bytes).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct TokenMap<'a> {
    /// The pairs: an id or offset, and a token (its top bit set) or an
    /// offset.
    pub entries: Vec<(u32, u32)>,
    /// What follows them.
    pub rest: &'a [u8],
}

impl<'a> TokenMap<'a> {
    /// Reads a token map's contents.
    pub fn parse(kind: u32, data: &'a [u8]) -> Result<Self, Error> {
        let mut r = Reader::new(data);
        let n = r.u32_le().map_err(|_| Error::Truncated { kind, at: 0 })? as usize;
        let len = n
            .checked_mul(8)
            .filter(|&l| l <= r.remaining().len())
            .ok_or(Error::Truncated { kind, at: 4 })?;
        let entries = pairs(kind, &data[4..4 + len])?;
        Ok(Self {
            entries,
            rest: &data[4 + len..],
        })
    }

    /// Appends the contents.
    pub fn write(&self, out: &mut Vec<u8>) {
        out.extend_from_slice(&(self.entries.len() as u32).to_le_bytes());
        write_pairs(&self.entries, out);
        out.extend_from_slice(self.rest);
    }
}

/// One assembly in a merged assembly input subsection
/// (`DEBUG_S_MERGED_ASSEMBLYINPUT`), as cvdump reads it.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct MergedAssembly<'a> {
    /// Its time stamp.
    pub timestamp: u32,
    /// Its index, with the top bit set if it has a PDB.
    pub index: u32,
    /// Its version's bytes (after their 2-byte size, which counts itself).
    pub version: &'a [u8],
    /// Its name.
    pub name: &'a [u8],
}

/// Reads a merged assembly input subsection's contents: entries, each
/// padded to 4 bytes with zeros.
pub fn merged_assemblies(data: &[u8]) -> Result<Vec<MergedAssembly<'_>>, Error> {
    let k = kind::DEBUG_S_MERGED_ASSEMBLYINPUT;
    let mut r = Reader::new(data);
    let mut out = Vec::new();
    while !r.is_empty() {
        let at = r.position();
        let t = |_| Error::Truncated { kind: k, at };
        let timestamp = r.u32_le().map_err(t)?;
        let index = r.u32_le().map_err(t)?;
        let size = usize::from(r.u16_le().map_err(t)?);
        let version = r
            .bytes(
                size.checked_sub(2)
                    .ok_or(Error::Malformed { kind: k, at })?,
            )
            .map_err(t)?;
        let name = r.cstr().map_err(t)?;
        let pad = r.bytes(padding(r.position() - at)).map_err(t)?;
        if pad.iter().any(|&b| b != 0) {
            return Err(Error::Malformed { kind: k, at });
        }
        out.push(MergedAssembly {
            timestamp,
            index,
            version,
            name,
        });
    }
    Ok(out)
}

/// Appends merged assembly input.
pub fn write_merged_assemblies(entries: &[MergedAssembly<'_>], out: &mut Vec<u8>) {
    for e in entries {
        let start = out.len();
        out.extend_from_slice(&e.timestamp.to_le_bytes());
        out.extend_from_slice(&e.index.to_le_bytes());
        out.extend_from_slice(&((e.version.len() + 2) as u16).to_le_bytes());
        out.extend_from_slice(e.version);
        out.extend_from_slice(e.name);
        out.push(0);
        out.resize(out.len() + padding(out.len() - start), 0);
    }
}

/// Reads a symbols subsection's contents: its records, each at its offset
/// in them. Bytes after the last record are an error.
pub fn symbol_records(data: &[u8]) -> Result<Vec<SymbolRecord<'_>>, Error> {
    let k = kind::DEBUG_S_SYMBOLS;
    let mut out = Vec::new();
    let mut at = 0;
    while at < data.len() {
        let t = Error::Truncated { kind: k, at };
        let len = data
            .get(at..at + 2)
            .map(|b| usize::from(u16::from_le_bytes([b[0], b[1]])))
            .ok_or(t)?;
        if len < 2 {
            return Err(Error::Malformed { kind: k, at });
        }
        let body = data.get(at + 2..at + 2 + len).ok_or(t)?;
        out.push(SymbolRecord {
            offset: at as u32,
            kind: u16::from_le_bytes([body[0], body[1]]),
            data: &body[2..],
        });
        at += 2 + len;
    }
    Ok(out)
}

/// A subsection's contents, read.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Contents<'a> {
    /// `DEBUG_S_SYMBOLS`: symbol records (written back unpadded, as they
    /// were: each record's padding is part of its body).
    Symbols(Vec<SymbolRecord<'a>>),
    /// `DEBUG_S_LINES`.
    Lines(Lines),
    /// `DEBUG_S_IL_LINES`: laid out as lines.
    IlLines(Lines),
    /// `DEBUG_S_STRINGTABLE`: zero-terminated strings, by offset.
    StringTable(&'a [u8]),
    /// `DEBUG_S_FILECHKSMS`.
    FileChecksums(Vec<FileChecksum<'a>>),
    /// `DEBUG_S_FRAMEDATA`.
    FrameData(FrameData),
    /// `DEBUG_S_INLINEELINES`.
    InlineeLines(InlineeLines),
    /// `DEBUG_S_CROSSSCOPEIMPORTS`.
    CrossScopeImports(Vec<CrossScopeImport>),
    /// `DEBUG_S_CROSSSCOPEEXPORTS`: (local id, global id) pairs.
    CrossScopeExports(Vec<(u32, u32)>),
    /// `DEBUG_S_FUNC_MDTOKEN_MAP`.
    FuncTokenMap(TokenMap<'a>),
    /// `DEBUG_S_TYPE_MDTOKEN_MAP`.
    TypeTokenMap(TokenMap<'a>),
    /// `DEBUG_S_MERGED_ASSEMBLYINPUT`.
    MergedAssemblyInput(Vec<MergedAssembly<'a>>),
    /// `DEBUG_S_COFF_SYMBOL_RVA`: RVAs (LLVM's reading: undocumented in
    /// cvinfo.h).
    CoffSymbolRva(Vec<u32>),
    /// Any other kind, or one marked `DEBUG_S_IGNORE`: its bytes.
    Other(&'a [u8]),
}

impl<'a> Contents<'a> {
    /// Reads the contents of a subsection of `kind`.
    pub fn parse(kind: u32, data: &'a [u8]) -> Result<Self, Error> {
        Ok(match kind {
            kind::DEBUG_S_SYMBOLS => Contents::Symbols(symbol_records(data)?),
            kind::DEBUG_S_LINES => Contents::Lines(Lines::parse(data)?),
            kind::DEBUG_S_IL_LINES => {
                Contents::IlLines(Lines::parse(data).map_err(|e| match e {
                    Error::Truncated { at, .. } => Error::Truncated { kind, at },
                    Error::Malformed { at, .. } => Error::Malformed { kind, at },
                    e => e,
                })?)
            }
            kind::DEBUG_S_STRINGTABLE => Contents::StringTable(data),
            kind::DEBUG_S_FILECHKSMS => {
                Contents::FileChecksums(file_checksums(data)?.into_iter().map(|(_, c)| c).collect())
            }
            kind::DEBUG_S_FRAMEDATA => Contents::FrameData(FrameData::parse(data)?),
            kind::DEBUG_S_INLINEELINES => Contents::InlineeLines(InlineeLines::parse(data)?),
            kind::DEBUG_S_CROSSSCOPEIMPORTS => {
                Contents::CrossScopeImports(cross_scope_imports(data)?)
            }
            kind::DEBUG_S_CROSSSCOPEEXPORTS => Contents::CrossScopeExports(pairs(kind, data)?),
            kind::DEBUG_S_FUNC_MDTOKEN_MAP => Contents::FuncTokenMap(TokenMap::parse(kind, data)?),
            kind::DEBUG_S_TYPE_MDTOKEN_MAP => Contents::TypeTokenMap(TokenMap::parse(kind, data)?),
            kind::DEBUG_S_MERGED_ASSEMBLYINPUT => {
                Contents::MergedAssemblyInput(merged_assemblies(data)?)
            }
            kind::DEBUG_S_COFF_SYMBOL_RVA => {
                if !data.len().is_multiple_of(4) {
                    return Err(Error::Malformed {
                        kind,
                        at: data.len() / 4 * 4,
                    });
                }
                Contents::CoffSymbolRva(
                    data.as_chunks::<4>()
                        .0
                        .iter()
                        .map(|&c| u32::from_le_bytes(c))
                        .collect(),
                )
            }
            _ => Contents::Other(data),
        })
    }

    /// Appends the contents (without the subsection's header or padding).
    pub fn write(&self, out: &mut Vec<u8>) {
        match self {
            Contents::Symbols(records) => {
                for r in records {
                    // (A record read is never too long to write.)
                    let _ = crate::write::push_symbol(out, r.kind, r.data, 1);
                }
            }
            Contents::Lines(l) | Contents::IlLines(l) => l.write(out),
            Contents::StringTable(b) | Contents::Other(b) => out.extend_from_slice(b),
            Contents::FileChecksums(entries) => {
                for e in entries {
                    push_file_checksum(out, e);
                }
            }
            Contents::FrameData(f) => f.write(out),
            Contents::InlineeLines(i) => i.write(out),
            Contents::CrossScopeImports(i) => write_cross_scope_imports(i, out),
            Contents::CrossScopeExports(p) => write_pairs(p, out),
            Contents::FuncTokenMap(m) | Contents::TypeTokenMap(m) => m.write(out),
            Contents::MergedAssemblyInput(m) => write_merged_assemblies(m, out),
            Contents::CoffSymbolRva(rvas) => {
                for r in rvas {
                    out.extend_from_slice(&r.to_le_bytes());
                }
            }
        }
    }
}

/// Calls `visit` with each field of a subsection of `kind` whose contents
/// are `data`, at its offset in them: for symbols, each record's
/// [`symbol_refs`] fields; for lines, the code's address and each block's
/// file; for file checksums, each file's name; for inlinee lines, each
/// function's id and files; for frame data, its base ([`Field::Rva`]) and
/// each frame's program; for cross-scope imports, each module's name, and
/// for exports each local id. Other kinds have none (token maps hold
/// tokens and offsets this doesn't know the meaning of; the string table,
/// merged assembly input, and COFF symbol RVAs refer to nothing).
pub fn fields(kind: u32, data: &[u8], mut visit: impl FnMut(Field)) -> Result<(), Error> {
    let at = |v: usize| v as u32;
    match kind {
        kind::DEBUG_S_SYMBOLS => {
            for r in symbol_records(data)? {
                let base = r.offset + 4;
                symbol_refs::fields(r.kind, r.data, |f| visit(f.shifted(base))).map_err(
                    |error| Error::Symbol {
                        at: r.offset as usize,
                        error,
                    },
                )?;
            }
        }
        kind::DEBUG_S_LINES | kind::DEBUG_S_IL_LINES => {
            let lines = Lines::parse(data)?;
            visit(Field::Address {
                offset: 0,
                section: 4,
            });
            let columns = lines.flags & CV_LINES_HAVE_COLUMNS != 0;
            let mut pos = 12;
            for b in &lines.blocks {
                visit(Field::Checksum(at(pos)));
                pos += 12 + b.lines.len() * if columns { 12 } else { 8 };
            }
        }
        kind::DEBUG_S_FILECHKSMS => {
            for (offset, _) in file_checksums(data)? {
                visit(Field::String(offset));
            }
        }
        kind::DEBUG_S_INLINEELINES => {
            let lines = InlineeLines::parse(data)?;
            let mut pos = 4;
            for e in &lines.entries {
                visit(Field::Id(at(pos)));
                visit(Field::Checksum(at(pos + 4)));
                pos += 12;
                if lines.extended {
                    pos += 4;
                    for _ in &e.extra_files {
                        visit(Field::Checksum(at(pos)));
                        pos += 4;
                    }
                }
            }
        }
        kind::DEBUG_S_FRAMEDATA => {
            let f = FrameData::parse(data)?;
            let mut pos = 0;
            if f.base.is_some() {
                visit(Field::Rva(0));
                pos = 4;
            }
            for _ in &f.frames {
                visit(Field::String(at(pos + 20)));
                pos += FRAME_SIZE;
            }
        }
        kind::DEBUG_S_CROSSSCOPEIMPORTS => {
            let mut pos = 0;
            for i in cross_scope_imports(data)? {
                visit(Field::String(at(pos)));
                pos += 8 + 4 * i.ids.len();
            }
        }
        kind::DEBUG_S_CROSSSCOPEEXPORTS => {
            for i in 0..pairs(kind, data)?.len() {
                visit(Field::Id(at(8 * i)));
            }
        }
        _ => {
            // (Checked as they are read, for the same errors.)
            Contents::parse(kind, data)?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests;
