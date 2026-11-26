//! Module-definition (`.def`) files, read as `lib.exe` and `link.exe`
//! (MSVC 14.52) read them, and the import libraries `lib /DEF` makes of
//! them ([`imports`]).
//!
//! The grammar here is what `lib.exe` was measured to accept and reject,
//! one case at a time (a `.def` file each, as an error ends the file):
//!
//! **Lines and comments.** A file is lines (ending with `\n`, and a `\r`
//! before it is dropped). `;` starts a comment to the end of the line,
//! even inside quotes (`"a;b"` is `"a`), but in `VERSION` (below). Words
//! are separated by spaces and tabs. A line's first word is a statement
//! if it is one of `NAME`, `LIBRARY`, `EXPORTS`, `SECTIONS`, `HEAPSIZE`,
//! `STACKSIZE`, and `VERSION`, spelled in capitals; any other first word
//! is an entry of the `EXPORTS` or `SECTIONS` statement before it, or
//! else a statement `lib.exe` ignores with a warning (LNK4017, "statement
//! not supported for the target platform"): `DESCRIPTION`, `STUB`,
//! `SEGMENTS`, `CODE`, `DATA`, `IMPORTS`, `APPLOADER`, `EXETYPE`, and
//! anything else, `exports` in lower case too. An `EXPORTS` or `SECTIONS`
//! statement may have its first entry on its own line.
//!
//! **`NAME` and `LIBRARY`** `[name] [BASE=address] [WINDOWAPI]
//! [WINDOWCOMPAT]`: the module's name, its preferred base address, and
//! (for programs) its subsystem's flags. The name may be in double
//! quotes, which allows spaces; what follows the closing quote is part of
//! it (`"foo.dll"x` is `foo.dllx`), and an unclosed quote runs to the end
//! of the line. Single quotes are part of the name. A drive or directory
//! is dropped, with a warning (LNK4093). Without an extension, `NAME`'s
//! module is `name.exe` and `LIBRARY`'s `name.dll`; without a name, the
//! `.def` file's (`x.def` gives `x.dll`, for `NAME` too). `BASE` (in any
//! case) takes a number at once after `=` (`BASE = 1` is an error); any
//! other word is an error. A second `NAME` or `LIBRARY` is ignored, with
//! a warning (LNK4275).
//!
//! **Numbers** (`BASE`, `HEAPSIZE`, `STACKSIZE`) are read as C's
//! `strtoul` with base 0 reads them: an optional sign, then `0x` and hex
//! digits, `0` and octal digits, or decimal digits; at least one digit (`0x`
//! alone is an error), and what follows the digits is ignored (`1k` is 1,
//! `09` is 0). `BASE=010` was measured to be 8.
//!
//! **`EXPORTS`**: each entry is
//! `name[=internal] [@ordinal [NONAME]] [DATA | CONSTANT] [PRIVATE | EXPORTAS name]`,
//! in that order (any other order is an error):
//!
//! - The name runs to a space, tab, or `=`; quotes are part of it, as are
//!   `@`, `,`, and the rest (`A@5` is a name). It may be empty (`=A`).
//! - `=internal`: spaces may be around the `=`. The internal name runs to
//!   the next space or tab, and may itself hold `=` (`A==B`'s is `=B`) or
//!   be empty (`A=`). One with a `.` is a forwarder (`other.Func`,
//!   `other.#5`, `other.dll.Func`).
//! - `@ordinal`: a decimal number from 1 to 65535 (else LNK1119, "invalid
//!   ordinal number"), after `@` or after `@` and a space. What follows
//!   its digits in the word is ignored: `@5NONAME` is ordinal 5, and not
//!   `NONAME` (it is imported by ordinal, and counts among the named
//!   exports for the other imports' hints). `NONAME` only follows an
//!   ordinal.
//! - `DATA` and `CONSTANT` (obsolete, with a warning LNK4087), `PRIVATE`,
//!   `NONAME`, and `EXPORTAS` are keywords in any case (`data` too). As an
//!   entry's first word they are names (an export named `DATA`), as are
//!   `DESCRIPTION` and the other ignored statements; the seven statement
//!   words end the `EXPORTS` list.
//! - An export named again is ignored, with a warning (LNK4197) if its
//!   specification differs; two names with one ordinal are an error
//!   (LNK1121).
//!
//! **`SECTIONS`**: each entry is a section's name and any of `READ`,
//! `WRITE`, `EXECUTE`, `SHARED`, `DISCARDABLE`, `READONLY`, `READWRITE`,
//! `EXECUTEONLY`, `EXECUTEREAD`, and `NONSHARED`, in any case, or none.
//! Anything else is an error: `CLASS`, `PRELOAD`, `NONDISCARDABLE`,
//! `SINGLE`, `IOPL`, `MOVEABLE`, a quoted name.
//!
//! **`HEAPSIZE` and `STACKSIZE`** `[reserve][,commit]`: the reserve, a
//! comma and the commit, or both, in one word (`4096 , 512` is an error;
//! other words after it are ignored: `12 34` is 12, and `100 EXPORTS`
//! starts no exports). A commit above the reserve is an error (LNK1229).
//!
//! **`VERSION`** `major[.minor]`, each from 0 to 65535, decimal, and
//! nothing else to the end of the line: neither spaces nor a comment
//! after it.
//!
//! `llvm-lib` (LLVM 24) reads a different grammar: it rejects the ignored
//! statements (`DESCRIPTION`, `STUB`, ...), `SECTIONS`, and keywords not
//! in capitals, takes quotes as quoting in `EXPORTS`, and reads `A==B` as
//! `A EXPORTAS B`. That grammar is not implemented here.

#![no_std]
#![warn(missing_docs)]

#[cfg(test)]
use turbonop_global_alloc as _;

extern crate alloc;

pub mod descriptors;
pub mod imports;
pub mod library;

use alloc::borrow::Cow;
use alloc::vec::Vec;
use core::fmt;

/// `NAME` or `LIBRARY`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum ModuleKind {
    /// `NAME`: a program (`.exe`).
    Name,
    /// `LIBRARY`: a DLL.
    Library,
}

/// The module a `NAME` or `LIBRARY` statement names.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Module<'a> {
    /// Which statement it was.
    pub kind: ModuleKind,
    /// The name, without quotes, drive, or directory; `None` if none was
    /// given.
    pub name: Option<Cow<'a, [u8]>>,
    /// `BASE=`.
    pub base: Option<u64>,
    /// `WINDOWAPI`.
    pub windowapi: bool,
    /// `WINDOWCOMPAT`.
    pub windowcompat: bool,
    /// Its line (from 1).
    pub line: usize,
}

impl Module<'_> {
    /// The module's file name: its name, with `.exe` (`NAME`) or `.dll`
    /// (`LIBRARY`) added if it has no `.`; or, without a name, the `.def`
    /// file's name without its extension (`def_stem`) and `.dll`.
    pub fn file_name(&self, def_stem: &[u8]) -> Vec<u8> {
        match &self.name {
            Some(name) => {
                let mut v = name.to_vec();
                if !name.contains(&b'.') {
                    v.extend_from_slice(match self.kind {
                        ModuleKind::Name => b".exe",
                        ModuleKind::Library => b".dll",
                    });
                }
                v
            }
            None => default_file_name(def_stem),
        }
    }
}

fn default_file_name(def_stem: &[u8]) -> Vec<u8> {
    let mut v = def_stem.to_vec();
    v.extend_from_slice(b".dll");
    v
}

/// What an export is.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum ExportKind {
    /// Code (no keyword).
    Code,
    /// `DATA`.
    Data,
    /// `CONSTANT`.
    Constant,
}

/// One entry of `EXPORTS`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Export<'a> {
    /// The name the module exports it by (and the import library's).
    pub name: &'a [u8],
    /// `=internal`: the module's own symbol, or a forwarder.
    pub internal: Option<&'a [u8]>,
    /// `@ordinal`.
    pub ordinal: Option<u16>,
    /// `NONAME`.
    pub noname: bool,
    /// Code, `DATA`, or `CONSTANT`.
    pub kind: ExportKind,
    /// `PRIVATE`: not in the import library.
    pub private: bool,
    /// `EXPORTAS name`: the name to export it as.
    pub export_as: Option<&'a [u8]>,
    /// Its line (from 1).
    pub line: usize,
}

impl<'a> Export<'a> {
    /// For a forwarder (an internal name holding `.`), the module and the
    /// export or `#ordinal` it forwards to, split at the last `.`.
    pub fn forwarder(&self) -> Option<(&'a [u8], &'a [u8])> {
        let internal = self.internal?;
        let dot = internal.iter().rposition(|&b| b == b'.')?;
        Some((&internal[..dot], &internal[dot + 1..]))
    }

    /// Whether `other` exports the same way (all but the line).
    fn same_as(&self, other: &Export<'_>) -> bool {
        (
            self.name,
            self.internal,
            self.ordinal,
            self.noname,
            self.kind,
            self.private,
            self.export_as,
        ) == (
            other.name,
            other.internal,
            other.ordinal,
            other.noname,
            other.kind,
            other.private,
            other.export_as,
        )
    }
}

/// A section attribute of `SECTIONS`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum SectionAttribute {
    /// `READ`.
    Read,
    /// `WRITE`.
    Write,
    /// `EXECUTE`.
    Execute,
    /// `SHARED`.
    Shared,
    /// `DISCARDABLE`.
    Discardable,
    /// `READONLY`.
    ReadOnly,
    /// `READWRITE`.
    ReadWrite,
    /// `EXECUTEONLY`.
    ExecuteOnly,
    /// `EXECUTEREAD`.
    ExecuteRead,
    /// `NONSHARED`.
    NonShared,
}

impl SectionAttribute {
    const ALL: [(&'static [u8], SectionAttribute); 10] = [
        (b"READ", SectionAttribute::Read),
        (b"WRITE", SectionAttribute::Write),
        (b"EXECUTE", SectionAttribute::Execute),
        (b"SHARED", SectionAttribute::Shared),
        (b"DISCARDABLE", SectionAttribute::Discardable),
        (b"READONLY", SectionAttribute::ReadOnly),
        (b"READWRITE", SectionAttribute::ReadWrite),
        (b"EXECUTEONLY", SectionAttribute::ExecuteOnly),
        (b"EXECUTEREAD", SectionAttribute::ExecuteRead),
        (b"NONSHARED", SectionAttribute::NonShared),
    ];

    /// The attribute `word` names, in any case.
    pub fn parse(word: &[u8]) -> Option<Self> {
        Self::ALL
            .iter()
            .find(|(w, _)| w.eq_ignore_ascii_case(word))
            .map(|&(_, a)| a)
    }
}

/// One entry of `SECTIONS`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Section<'a> {
    /// The section's name.
    pub name: &'a [u8],
    /// Its attributes, in order.
    pub attributes: Vec<SectionAttribute>,
    /// Its line (from 1).
    pub line: usize,
}

/// `HEAPSIZE` or `STACKSIZE`: a reserve, a commit, or both.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct Sizes {
    /// The reserve.
    pub reserve: Option<u64>,
    /// The commit.
    pub commit: Option<u64>,
}

/// `VERSION major[.minor]`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct Version {
    /// The major version.
    pub major: u16,
    /// The minor version (0 if not given).
    pub minor: u16,
}

/// What `lib.exe` warns of, and ignores.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Warning<'a> {
    /// A statement it doesn't support (LNK4017), by its first word.
    Ignored {
        /// The line.
        line: usize,
        /// Its first word.
        statement: &'a [u8],
    },
    /// A second `NAME` or `LIBRARY` (LNK4275).
    SecondModule {
        /// The line.
        line: usize,
    },
    /// A module name's drive or directory, dropped (LNK4093).
    DirectoryIgnored {
        /// The line.
        line: usize,
    },
    /// `CONSTANT` (LNK4087: obsolete).
    Constant {
        /// The line.
        line: usize,
    },
    /// An export named again with a different specification (LNK4197);
    /// the first is used.
    DuplicateExport {
        /// The later entry's line.
        line: usize,
        /// The name.
        name: &'a [u8],
    },
}

/// Why a file can't be read (each one of `lib.exe`'s fatal errors).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum ParseError<'a> {
    /// A word that doesn't belong (LNK1118, "syntax error in '...'
    /// statement").
    Syntax {
        /// The line.
        line: usize,
        /// The statement (or the export's word) `lib.exe` names.
        word: &'a [u8],
    },
    /// An ordinal that isn't a number from 1 to 65535 (LNK1119).
    InvalidOrdinal {
        /// The line.
        line: usize,
        /// What follows the `@`.
        text: &'a [u8],
    },
    /// Two exports with one ordinal (LNK1121).
    DuplicateOrdinal {
        /// The ordinal.
        ordinal: u16,
    },
    /// A commit above the reserve (LNK1229).
    CommitAboveReserve {
        /// The line.
        line: usize,
    },
}

impl fmt::Display for ParseError<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let s = |b: &[u8]| alloc::string::String::from_utf8_lossy(b).into_owned();
        match self {
            ParseError::Syntax { line, word } => {
                write!(f, "line {line}: syntax error in '{}'", s(word))
            }
            ParseError::InvalidOrdinal { line, text } => {
                write!(f, "line {line}: invalid ordinal number '{}'", s(text))
            }
            ParseError::DuplicateOrdinal { ordinal } => {
                write!(f, "duplicate ordinal number '{ordinal}'")
            }
            ParseError::CommitAboveReserve { line } => {
                write!(f, "line {line}: commit size greater than reserve size")
            }
        }
    }
}

impl core::error::Error for ParseError<'_> {}

/// A module-definition file, as read.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ModuleDefinition<'a> {
    /// The first `NAME` or `LIBRARY`.
    pub module: Option<Module<'a>>,
    /// Every `EXPORTS` entry, in order (duplicates too: see
    /// [`exports`](Self::exports)).
    pub entries: Vec<Export<'a>>,
    /// Every `SECTIONS` entry, in order.
    pub sections: Vec<Section<'a>>,
    /// The last `HEAPSIZE`.
    pub heap: Option<Sizes>,
    /// The last `STACKSIZE`.
    pub stack: Option<Sizes>,
    /// The last `VERSION`.
    pub version: Option<Version>,
    /// What `lib.exe` warns of.
    pub warnings: Vec<Warning<'a>>,
}

impl<'a> ModuleDefinition<'a> {
    /// The exports, each name once (its first entry), in order.
    pub fn exports(&self) -> Vec<Export<'a>> {
        let mut seen: alloc::collections::BTreeSet<&[u8]> = alloc::collections::BTreeSet::new();
        self.entries
            .iter()
            .filter(|e| seen.insert(e.name))
            .copied()
            .collect()
    }

    /// The module's file name (see [`Module::file_name`]).
    pub fn file_name(&self, def_stem: &[u8]) -> Vec<u8> {
        self.module
            .as_ref()
            .map_or_else(|| default_file_name(def_stem), |m| m.file_name(def_stem))
    }
}

fn is_space(b: u8) -> bool {
    b == b' ' || b == b'\t'
}

/// The words of `s`, split at spaces and tabs.
fn words(s: &[u8]) -> impl Iterator<Item = &[u8]> {
    s.split(|&b| is_space(b)).filter(|w| !w.is_empty())
}

fn skip_space(s: &[u8]) -> &[u8] {
    let at = s.iter().position(|&b| !is_space(b)).unwrap_or(s.len());
    &s[at..]
}

/// A number as `strtoul` with base 0 reads it: `None` without a digit.
/// What follows the digits is ignored.
fn number(s: &[u8]) -> Option<u64> {
    number_prefix(s).map(|(v, _)| v)
}

/// A number at the start of `s`, and the bytes it takes.
fn number_prefix(s: &[u8]) -> Option<(u64, usize)> {
    let whole = s.len();
    let (negative, s) = match s.first() {
        Some(b'-') => (true, &s[1..]),
        Some(b'+') => (false, &s[1..]),
        _ => (false, s),
    };
    let (radix, digits) = if s.len() >= 2 && s[0] == b'0' && (s[1] | 0x20) == b'x' {
        (16, &s[2..])
    } else if s.first() == Some(&b'0') {
        (8, s)
    } else {
        (10, s)
    };
    let mut value = 0u64;
    let mut count = 0;
    for &b in digits {
        let Some(d) = char::from(b).to_digit(radix) else {
            break;
        };
        value = value
            .saturating_mul(u64::from(radix))
            .saturating_add(u64::from(d));
        count += 1;
    }
    let used = whole - digits.len() + count;
    (count > 0).then_some((
        if negative {
            value.wrapping_neg()
        } else {
            value
        },
        used,
    ))
}

/// The statements' words.
const STATEMENTS: [&[u8]; 7] = [
    b"NAME",
    b"LIBRARY",
    b"EXPORTS",
    b"SECTIONS",
    b"HEAPSIZE",
    b"STACKSIZE",
    b"VERSION",
];

#[derive(Clone, Copy, PartialEq, Eq)]
enum Mode {
    Top,
    Exports,
    Sections,
}

/// Reads a module-definition file.
pub fn parse(text: &[u8]) -> Result<ModuleDefinition<'_>, ParseError<'_>> {
    let mut def = ModuleDefinition::default();
    let mut mode = Mode::Top;
    for (i, raw) in text.split(|&b| b == b'\n').enumerate() {
        let line = i + 1;
        let raw = raw.strip_suffix(b"\r").unwrap_or(raw);
        let content = &raw[..raw.iter().position(|&b| b == b';').unwrap_or(raw.len())];
        let Some(first) = words(content).next() else {
            continue;
        };
        if !STATEMENTS.contains(&first) {
            match mode {
                Mode::Exports => def.entries.push(export(content, line, &mut def.warnings)?),
                Mode::Sections => def.sections.push(section(content, line)?),
                Mode::Top => def.warnings.push(Warning::Ignored {
                    line,
                    statement: first,
                }),
            }
            continue;
        }
        // The statement's arguments: what follows its word.
        let start = content.len() - skip_space(content).len() + first.len();
        let rest = &content[start..];
        mode = Mode::Top;
        match first {
            b"NAME" | b"LIBRARY" => {
                if def.module.is_some() {
                    def.warnings.push(Warning::SecondModule { line });
                } else {
                    let kind = if first == b"NAME" {
                        ModuleKind::Name
                    } else {
                        ModuleKind::Library
                    };
                    def.module = Some(module(kind, first, rest, line, &mut def.warnings)?);
                }
            }
            b"EXPORTS" => {
                mode = Mode::Exports;
                if words(rest).next().is_some() {
                    def.entries.push(export(rest, line, &mut def.warnings)?);
                }
            }
            b"SECTIONS" => {
                mode = Mode::Sections;
                if words(rest).next().is_some() {
                    def.sections.push(section(rest, line)?);
                }
            }
            b"HEAPSIZE" => def.heap = Some(sizes(first, rest, line)?),
            b"STACKSIZE" => def.stack = Some(sizes(first, rest, line)?),
            _ => {
                // VERSION: the raw line, its comment and spaces included.
                let raw_rest = skip_space(&raw[start..]);
                def.version = Some(version(raw_rest).ok_or(ParseError::Syntax {
                    line,
                    word: b"VERSION",
                })?);
            }
        }
    }
    // Each name's first entry counts; two of them with one ordinal are an
    // error.
    let mut first: alloc::collections::BTreeMap<&[u8], Export<'_>> =
        alloc::collections::BTreeMap::new();
    let mut ordinals: alloc::collections::BTreeMap<u16, &[u8]> =
        alloc::collections::BTreeMap::new();
    for e in &def.entries {
        match first.get(e.name) {
            Some(f) => {
                if !f.same_as(e) {
                    def.warnings.push(Warning::DuplicateExport {
                        line: e.line,
                        name: e.name,
                    });
                }
            }
            None => {
                first.insert(e.name, *e);
                if let Some(o) = e.ordinal
                    && ordinals.insert(o, e.name).is_some()
                {
                    return Err(ParseError::DuplicateOrdinal { ordinal: o });
                }
            }
        }
    }
    Ok(def)
}

fn module<'a>(
    kind: ModuleKind,
    word: &'a [u8],
    rest: &'a [u8],
    line: usize,
    warnings: &mut Vec<Warning<'a>>,
) -> Result<Module<'a>, ParseError<'a>> {
    let s = skip_space(rest);
    // The name: a quoted part, then up to a space or tab.
    let (name, after): (Option<Cow<'a, [u8]>>, &'a [u8]) = if let Some(q) = s.strip_prefix(b"\"") {
        let close = q.iter().position(|&b| b == b'"');
        let quoted = &q[..close.unwrap_or(q.len())];
        let tail = close.map_or(&q[q.len()..], |c| &q[c + 1..]);
        let end = tail.iter().position(|&b| is_space(b)).unwrap_or(tail.len());
        let name = if end == 0 {
            Cow::Borrowed(quoted)
        } else {
            let mut v = quoted.to_vec();
            v.extend_from_slice(&tail[..end]);
            Cow::Owned(v)
        };
        (Some(name), &tail[end..])
    } else {
        let end = s.iter().position(|&b| is_space(b)).unwrap_or(s.len());
        ((end > 0).then_some(Cow::Borrowed(&s[..end])), &s[end..])
    };
    let name = name.map(
        |n| match n.iter().rposition(|&b| matches!(b, b'\\' | b'/' | b':')) {
            Some(at) => {
                warnings.push(Warning::DirectoryIgnored { line });
                match n {
                    Cow::Borrowed(b) => Cow::Borrowed(&b[at + 1..]),
                    Cow::Owned(v) => Cow::Owned(v[at + 1..].to_vec()),
                }
            }
            None => n,
        },
    );
    let mut m = Module {
        kind,
        name,
        base: None,
        windowapi: false,
        windowcompat: false,
        line,
    };
    let error = ParseError::Syntax { line, word };
    for w in words(after) {
        if w.len() >= 5 && w[..5].eq_ignore_ascii_case(b"BASE=") {
            m.base = Some(number(&w[5..]).ok_or(error)?);
        } else if w.eq_ignore_ascii_case(b"WINDOWAPI") {
            m.windowapi = true;
        } else if w.eq_ignore_ascii_case(b"WINDOWCOMPAT") {
            m.windowcompat = true;
        } else {
            return Err(error);
        }
    }
    Ok(m)
}

fn export<'a>(
    s: &'a [u8],
    line: usize,
    warnings: &mut Vec<Warning<'a>>,
) -> Result<Export<'a>, ParseError<'a>> {
    let s = skip_space(s);
    let end = s
        .iter()
        .position(|&b| is_space(b) || b == b'=')
        .unwrap_or(s.len());
    let mut e = Export {
        name: &s[..end],
        internal: None,
        ordinal: None,
        noname: false,
        kind: ExportKind::Code,
        private: false,
        export_as: None,
        line,
    };
    let mut rest = skip_space(&s[end..]);
    if let Some(r) = rest.strip_prefix(b"=") {
        let r = skip_space(r);
        let end = r.iter().position(|&b| is_space(b)).unwrap_or(r.len());
        e.internal = Some(&r[..end]);
        rest = &r[end..];
    }
    // What may come next: 0 anything, 1 after an ordinal, 2 after NONAME,
    // 3 after DATA or CONSTANT, 4 nothing.
    let mut stage = 0;
    let mut words = words(rest);
    while let Some(w) = words.next() {
        let error = ParseError::Syntax { line, word: w };
        let is = |k: &[u8]| w.eq_ignore_ascii_case(k);
        if let Some(text) = w.strip_prefix(b"@") {
            if stage != 0 {
                return Err(error);
            }
            let text = if text.is_empty() {
                words.next().unwrap_or(&[])
            } else {
                text
            };
            // The leading digits; what follows them is ignored.
            let digits = &text[..text
                .iter()
                .position(|b| !b.is_ascii_digit())
                .unwrap_or(text.len())];
            let ordinal = (!digits.is_empty())
                .then(|| {
                    digits.iter().try_fold(0u32, |a, &d| {
                        a.checked_mul(10)?.checked_add(u32::from(d - b'0'))
                    })
                })
                .flatten()
                .and_then(|o| u16::try_from(o).ok())
                .filter(|&o| o != 0)
                .ok_or(ParseError::InvalidOrdinal { line, text })?;
            e.ordinal = Some(ordinal);
            stage = 1;
        } else if is(b"NONAME") {
            if stage != 1 {
                return Err(error);
            }
            e.noname = true;
            stage = 2;
        } else if is(b"DATA") || is(b"CONSTANT") {
            if stage > 2 {
                return Err(error);
            }
            if is(b"CONSTANT") {
                e.kind = ExportKind::Constant;
                warnings.push(Warning::Constant { line });
            } else {
                e.kind = ExportKind::Data;
            }
            stage = 3;
        } else if is(b"PRIVATE") {
            if stage > 3 {
                return Err(error);
            }
            e.private = true;
            stage = 4;
        } else if is(b"EXPORTAS") {
            if stage > 3 {
                return Err(error);
            }
            e.export_as = Some(words.next().ok_or(error)?);
            stage = 4;
        } else {
            return Err(error);
        }
    }
    Ok(e)
}

fn section(s: &[u8], line: usize) -> Result<Section<'_>, ParseError<'_>> {
    let mut words = words(s);
    let name = words.next().unwrap_or(&[]);
    let attributes = words
        .map(|w| {
            SectionAttribute::parse(w).ok_or(ParseError::Syntax {
                line,
                word: b"SECTIONS",
            })
        })
        .collect::<Result<Vec<_>, _>>()?;
    if name.first() == Some(&b'"') {
        return Err(ParseError::Syntax {
            line,
            word: b"SECTIONS",
        });
    }
    Ok(Section {
        name,
        attributes,
        line,
    })
}

fn sizes<'a>(word: &'a [u8], rest: &'a [u8], line: usize) -> Result<Sizes, ParseError<'a>> {
    let error = ParseError::Syntax { line, word };
    let mut ws = words(rest);
    let w = ws.next().ok_or(error)?;
    // (Words after it are ignored, but a comma apart from it is an error.)
    if ws.next().is_some_and(|next| next.starts_with(b",")) {
        return Err(error);
    }
    let (reserve, after) = if w.first() == Some(&b',') {
        (None, w)
    } else {
        let (value, used) = number_prefix(w).ok_or(error)?;
        (Some(value), &w[used..])
    };
    let commit = match after.strip_prefix(b",") {
        Some(c) => Some(number(c).ok_or(error)?),
        None => None,
    };
    if let (Some(r), Some(c)) = (reserve, commit)
        && c > r
    {
        return Err(ParseError::CommitAboveReserve { line });
    }
    Ok(Sizes { reserve, commit })
}

fn version(s: &[u8]) -> Option<Version> {
    let part = |p: &[u8]| -> Option<u16> {
        if p.is_empty() || !p.iter().all(u8::is_ascii_digit) {
            return None;
        }
        p.iter()
            .try_fold(0u32, |a, &d| {
                a.checked_mul(10)?.checked_add(u32::from(d - b'0'))
            })
            .and_then(|v| u16::try_from(v).ok())
    };
    let mut parts = s.splitn(2, |&b| b == b'.');
    let major = part(parts.next()?)?;
    let minor = match parts.next() {
        Some(p) => part(p)?,
        None => 0,
    };
    Some(Version { major, minor })
}

#[cfg(test)]
mod tests;
