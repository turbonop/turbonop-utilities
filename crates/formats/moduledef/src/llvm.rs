//! `llvm-lib /def:`: LLVM's grammar of module-definition files
//! (`COFFModuleDefinition.cpp`) and its import-library writer
//! (`COFFImportFile.cpp`'s `writeImportLibrary`), followed as LLVM 24 (at
//! `c74b358f44c3`) has them.
//!
//! **The grammar.** The file is tokens: spaces, tabs, line ends, `\v`, and
//! `\f` separate them; `;` starts a comment to the end of the line; `=`,
//! `==`, and `,` are tokens of their own; `"..."` is a word (without its
//! quotes; an unclosed one runs to the end of the file); any other word
//! runs to `=`, `,`, `;`, a space, a tab, `\v`, or a line end. A NUL ends
//! the file. `BASE`, `CONSTANT`, `DATA`, `EXPORTS`, `EXPORTAS`, `HEAPSIZE`,
//! `LIBRARY`, `NAME`, `NONAME`, `PRIVATE`, `STACKSIZE`, and `VERSION` are
//! keywords, in capitals only. Statements:
//!
//! - `LIBRARY [name] [BASE = number]` and `NAME ...`: the first one with
//!   a name (or the first, if it has none) names the output: the name,
//!   with `.dll` (`LIBRARY`) or `.exe` (`NAME`) if it has no extension.
//! - `EXPORTS`, then exports until a token that isn't a word:
//!   `name [= internal] {@ordinal [NONAME] | DATA | CONSTANT | PRIVATE |
//!   == import} [EXPORTAS name]`, the middle ones in any order and number.
//!   `@n` or `@ n` is an ordinal (decimal); a word `@x` that isn't one ends
//!   the export (it is the next one's name). On x86, `_` is put before a
//!   name that isn't decorated (starts with `@` or `?`, or holds `@`) and
//!   (for `a=b`) isn't a forwarder, and before an undecorated internal
//!   name.
//! - `HEAPSIZE` and `STACKSIZE` `number [, number]`; `VERSION major[.minor]`.
//! - Anything else is an error ("unknown directive").
//!
//! Numbers are LLVM's `getAsInteger` with radix 0: `0x` (or `0X`) hex,
//! `0b` binary, `0o` or `0` and a digit octal, else decimal, all of the
//! word.
//!
//! **The writer.** Members in order: the three descriptor objects, then an
//! import for each export but the `PRIVATE` ones, in order (and an ARM64X
//! library's native exports' after), with these rules:
//!
//! - The symbol is the internal name if there is one (`a=b` imports `b` as
//!   `a`... as LLVM words it, the export `a`), else the name.
//! - `NONAME`: by ordinal. `EXPORTAS`: that name. `== import`: on x86 as
//!   `UNDECORATE` or `NOPREFIX` if either gives `import`; on ARM64EC as
//!   `EXPORTAS import`; by name if it is the symbol; otherwise, once every
//!   other export is made, a weak external (two, for code: `__imp_` and
//!   plain) aliasing the symbol to the import of `import` if there is one,
//!   else an import `EXPORTAS import`. Otherwise by name, but on x86
//!   `NOPREFIX` for a name with `_` and no `@`.
//! - On ARM64EC, code is mangled (`#f`, `$$h`), and imported `EXPORTAS` its
//!   plain name; a name that can't be demangled is an error.
//! - The hint is the ordinal, or 0.

use alloc::borrow::Cow;
use alloc::string::String;
use alloc::vec::Vec;

use turbonop_archive::symbols::ec_demangle;
use turbonop_archive::write::Tool;
use turbonop_pe::consts::machine;
use turbonop_pe::import_object::{ImportNameType, ImportObject, ImportType};

use crate::descriptors::{Descriptors, Stamp};
use crate::library::Members;

/// An export, as LLVM's `COFFShortExport`.
#[derive(Clone, Debug, Default, PartialEq, Eq, Hash)]
pub struct ShortExport {
    /// `Name`: the internal name, if given (`a=b`'s `b`), else the name.
    pub name: Vec<u8>,
    /// `ExtName`: for `a=b`, `a`.
    pub ext_name: Vec<u8>,
    /// `ImportName`: `== import`.
    pub import_name: Vec<u8>,
    /// `ExportAs`.
    pub export_as: Vec<u8>,
    /// The ordinal (0 for none).
    pub ordinal: u16,
    /// `NONAME`.
    pub noname: bool,
    /// `DATA`.
    pub data: bool,
    /// `PRIVATE`.
    pub private: bool,
    /// `CONSTANT`.
    pub constant: bool,
}

/// A module definition, as LLVM's `COFFModuleDefinition`.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct LlvmDefinition {
    /// The exports, in order (each as given: duplicates too).
    pub exports: Vec<ShortExport>,
    /// The output's name (`OutputFile`), empty if no `LIBRARY` or `NAME`.
    pub output_file: Vec<u8>,
    /// The last `LIBRARY` or `NAME`'s name (`ImportName`).
    pub import_name: Vec<u8>,
    /// `BASE=`.
    pub image_base: u64,
    /// `HEAPSIZE`.
    pub heap: (u64, u64),
    /// `STACKSIZE`.
    pub stack: (u64, u64),
    /// `VERSION`.
    pub version: (u32, u32),
}

/// Why LLVM can't read a file: its message (which `llvm-lib` doesn't print:
/// it prints the error code's, "Invalid data was encountered while parsing
/// the file").
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct LlvmParseError(pub String);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum K {
    Unknown,
    Eof,
    Identifier,
    Comma,
    Equal,
    EqualEqual,
    Base,
    Constant,
    Data,
    Exports,
    ExportAs,
    Heapsize,
    Library,
    Name,
    Noname,
    Private,
    Stacksize,
    Version,
}

#[derive(Clone, Copy, Debug)]
struct Token<'a> {
    k: K,
    value: &'a [u8],
}

struct Lexer<'a> {
    buf: &'a [u8],
}

/// LLVM's `StringRef::trim()`: " \t\n\v\f\r" at both ends.
fn trim(s: &[u8]) -> &[u8] {
    let ws = |b: &u8| matches!(b, b' ' | b'\t' | b'\n' | 0x0B | 0x0C | b'\r');
    let start = s.iter().position(|b| !ws(b)).unwrap_or(s.len());
    let end = s.iter().rposition(|b| !ws(b)).map_or(start, |e| e + 1);
    &s[start..end.max(start)]
}

impl<'a> Lexer<'a> {
    fn lex(&mut self) -> Token<'a> {
        loop {
            self.buf = trim(self.buf);
            let Some(&c) = self.buf.first() else {
                return Token {
                    k: K::Eof,
                    value: b"",
                };
            };
            match c {
                0 => {
                    return Token {
                        k: K::Eof,
                        value: b"",
                    };
                }
                b';' => {
                    let end = self.buf.iter().position(|&b| b == b'\n');
                    self.buf = match end {
                        Some(e) => &self.buf[e..],
                        None => b"",
                    };
                }
                b'=' => {
                    self.buf = &self.buf[1..];
                    if let Some(rest) = self.buf.strip_prefix(b"=") {
                        self.buf = rest;
                        return Token {
                            k: K::EqualEqual,
                            value: b"==",
                        };
                    }
                    return Token {
                        k: K::Equal,
                        value: b"=",
                    };
                }
                b',' => {
                    self.buf = &self.buf[1..];
                    return Token {
                        k: K::Comma,
                        value: b",",
                    };
                }
                b'"' => {
                    let s = &self.buf[1..];
                    let (word, rest) = match s.iter().position(|&b| b == b'"') {
                        Some(q) => (&s[..q], &s[q + 1..]),
                        None => (s, &s[s.len()..]),
                    };
                    self.buf = rest;
                    return Token {
                        k: K::Identifier,
                        value: word,
                    };
                }
                _ => {
                    let end = self
                        .buf
                        .iter()
                        .position(|b| b"=,;\r\n \t\x0B".contains(b))
                        .unwrap_or(self.buf.len());
                    let word = &self.buf[..end];
                    self.buf = &self.buf[end..];
                    let k = match word {
                        b"BASE" => K::Base,
                        b"CONSTANT" => K::Constant,
                        b"DATA" => K::Data,
                        b"EXPORTS" => K::Exports,
                        b"EXPORTAS" => K::ExportAs,
                        b"HEAPSIZE" => K::Heapsize,
                        b"LIBRARY" => K::Library,
                        b"NAME" => K::Name,
                        b"NONAME" => K::Noname,
                        b"PRIVATE" => K::Private,
                        b"STACKSIZE" => K::Stacksize,
                        b"VERSION" => K::Version,
                        _ => K::Identifier,
                    };
                    return Token { k, value: word };
                }
            }
        }
    }
}

/// LLVM's `getAsInteger` for unsigned numbers: radix 0 senses `0x`, `0b`,
/// `0o`, and a leading `0`; the whole string must be digits, and the value
/// must fit `max`.
pub fn llvm_integer(s: &[u8], radix: u32, max: u64) -> Option<u64> {
    let mut s = s;
    let mut radix = radix;
    if radix == 0 {
        let lower = |i: usize| s.get(i).map(u8::to_ascii_lowercase);
        if s.len() >= 2 && s[0] == b'0' && lower(1) == Some(b'x') {
            s = &s[2..];
            radix = 16;
        } else if s.len() >= 2 && s[0] == b'0' && lower(1) == Some(b'b') {
            s = &s[2..];
            radix = 2;
        } else if s.starts_with(b"0o") {
            s = &s[2..];
            radix = 8;
        } else if s.len() > 1 && s[0] == b'0' && s[1].is_ascii_digit() {
            s = &s[1..];
            radix = 8;
        } else {
            radix = 10;
        }
    }
    if s.is_empty() {
        return None;
    }
    let mut v: u64 = 0;
    for &b in s {
        let d = match b {
            b'0'..=b'9' => u32::from(b - b'0'),
            b'a'..=b'z' => u32::from(b - b'a') + 10,
            b'A'..=b'Z' => u32::from(b - b'A') + 10,
            _ => return None,
        };
        if d >= radix {
            return None;
        }
        v = v.checked_mul(u64::from(radix))?.checked_add(u64::from(d))?;
    }
    (v <= max).then_some(v)
}

/// LLVM's `isDecorated` (not for MinGW).
fn is_decorated(s: &[u8]) -> bool {
    s.first() == Some(&b'@') || s.first() == Some(&b'?') || s.contains(&b'@')
}

fn underscored(s: &[u8]) -> Vec<u8> {
    let mut v = Vec::with_capacity(s.len() + 1);
    v.push(b'_');
    v.extend_from_slice(s);
    v
}

/// `llvm::sys::path::filename` on Windows: the last component (after a
/// drive's `:` and any `\` or `/`); `.` if the path ends with a separator.
pub fn path_filename(p: &[u8]) -> &[u8] {
    let p = match p {
        [d, b':', rest @ ..] if d.is_ascii_alphabetic() => rest,
        _ => p,
    };
    if p.is_empty() {
        return p;
    }
    if matches!(p.last(), Some(b'\\' | b'/')) {
        return if p.iter().all(|&b| b == b'\\' || b == b'/') {
            &p[..1]
        } else {
            b"."
        };
    }
    let at = p
        .iter()
        .rposition(|&b| b == b'\\' || b == b'/')
        .map_or(0, |i| i + 1);
    &p[at..]
}

/// `llvm::sys::path::stem`: the file name without its last extension (but
/// `.` and `..` as they are).
pub fn path_stem(p: &[u8]) -> &[u8] {
    let f = path_filename(p);
    if f == b"." || f == b".." {
        return f;
    }
    match f.iter().rposition(|&b| b == b'.') {
        Some(at) => &f[..at],
        None => f,
    }
}

/// Whether `llvm::sys::path::has_extension` holds.
fn has_extension(p: &[u8]) -> bool {
    let f = path_filename(p);
    f != b"." && f != b".." && f.contains(&b'.')
}

struct Parser<'a> {
    lex: Lexer<'a>,
    tok: Token<'a>,
    stack: Vec<Token<'a>>,
    add_underscores: bool,
    info: LlvmDefinition,
}

fn err<T>(msg: impl Into<String>) -> Result<T, LlvmParseError> {
    Err(LlvmParseError(msg.into()))
}

fn text(b: &[u8]) -> String {
    String::from_utf8_lossy(b).into_owned()
}

impl<'a> Parser<'a> {
    fn read(&mut self) {
        self.tok = match self.stack.pop() {
            Some(t) => t,
            None => self.lex.lex(),
        };
    }

    fn unget(&mut self) {
        self.stack.push(self.tok);
    }

    fn read_int(&mut self) -> Result<u64, LlvmParseError> {
        self.read();
        if self.tok.k != K::Identifier {
            return err("integer expected");
        }
        llvm_integer(self.tok.value, 0, u64::MAX)
            .ok_or_else(|| LlvmParseError("integer expected".into()))
    }

    fn parse_one(&mut self) -> Result<(), LlvmParseError> {
        self.read();
        match self.tok.k {
            K::Eof => Ok(()),
            K::Exports => loop {
                self.read();
                if self.tok.k != K::Identifier {
                    self.unget();
                    return Ok(());
                }
                self.parse_export()?;
            },
            K::Heapsize => {
                self.info.heap = self.parse_numbers()?;
                Ok(())
            }
            K::Stacksize => {
                self.info.stack = self.parse_numbers()?;
                Ok(())
            }
            K::Library | K::Name => {
                let dll = self.tok.k == K::Library;
                let name = self.parse_name()?;
                self.info.import_name.clone_from(&name);
                if self.info.output_file.is_empty() {
                    let mut out = name.clone();
                    if !has_extension(&name) {
                        out.extend_from_slice(if dll { b".dll" } else { b".exe" });
                    }
                    self.info.output_file = out;
                }
                Ok(())
            }
            K::Version => self.parse_version(),
            _ => err(alloc::format!(
                "unknown directive: {}",
                text(self.tok.value)
            )),
        }
    }

    fn parse_export(&mut self) -> Result<(), LlvmParseError> {
        let mut e = ShortExport {
            name: self.tok.value.to_vec(),
            ..Default::default()
        };
        self.read();
        if self.tok.k == K::Equal {
            self.read();
            if self.tok.k != K::Identifier {
                return err(alloc::format!(
                    "identifier expected, but got {}",
                    text(self.tok.value)
                ));
            }
            e.ext_name = core::mem::take(&mut e.name);
            e.name = self.tok.value.to_vec();
        } else {
            self.unget();
        }
        if self.add_underscores {
            if !is_decorated(&e.name) && (e.ext_name.is_empty() || !e.name.contains(&b'.')) {
                e.name = underscored(&e.name);
            }
            if !e.ext_name.is_empty() && !is_decorated(&e.ext_name) {
                e.ext_name = underscored(&e.ext_name);
            }
        }
        loop {
            self.read();
            if self.tok.k == K::Identifier && self.tok.value.first() == Some(&b'@') {
                if self.tok.value == b"@" {
                    // "foo @ 10".
                    self.read();
                    if let Some(v) = llvm_integer(self.tok.value, 10, 0xFFFF) {
                        e.ordinal = u16::try_from(v).unwrap_or(0);
                    }
                } else if let Some(v) = llvm_integer(&self.tok.value[1..], 10, 0xFFFF) {
                    e.ordinal = u16::try_from(v).unwrap_or(0);
                } else {
                    // "foo \n @bar": the next export.
                    self.unget();
                    self.info.exports.push(e);
                    return Ok(());
                }
                self.read();
                if self.tok.k == K::Noname {
                    e.noname = true;
                } else {
                    self.unget();
                }
                continue;
            }
            match self.tok.k {
                K::Data => e.data = true,
                K::Constant => e.constant = true,
                K::Private => e.private = true,
                K::EqualEqual => {
                    self.read();
                    e.import_name = self.tok.value.to_vec();
                }
                _ => {
                    if self.tok.k == K::ExportAs {
                        self.read();
                        if self.tok.k == K::Eof {
                            return err("unexpected end of file, EXPORTAS identifier expected");
                        }
                        e.export_as = self.tok.value.to_vec();
                    } else {
                        self.unget();
                    }
                    self.info.exports.push(e);
                    return Ok(());
                }
            }
        }
    }

    fn parse_numbers(&mut self) -> Result<(u64, u64), LlvmParseError> {
        let reserve = self.read_int()?;
        self.read();
        if self.tok.k != K::Comma {
            self.unget();
            return Ok((reserve, 0));
        }
        Ok((reserve, self.read_int()?))
    }

    fn parse_name(&mut self) -> Result<Vec<u8>, LlvmParseError> {
        self.read();
        if self.tok.k != K::Identifier {
            self.unget();
            return Ok(Vec::new());
        }
        let name = self.tok.value.to_vec();
        self.read();
        if self.tok.k == K::Base {
            self.read();
            if self.tok.k != K::Equal {
                return err("'=' expected");
            }
            self.info.image_base = self.read_int()?;
        } else {
            self.unget();
            self.info.image_base = 0;
        }
        Ok(name)
    }

    fn parse_version(&mut self) -> Result<(), LlvmParseError> {
        self.read();
        if self.tok.k != K::Identifier {
            return err(alloc::format!(
                "identifier expected, but got {}",
                text(self.tok.value)
            ));
        }
        let v = self.tok.value;
        let (a, b) = match v.iter().position(|&c| c == b'.') {
            Some(d) => (&v[..d], &v[d + 1..]),
            None => (v, &v[v.len()..]),
        };
        let bad = || LlvmParseError(alloc::format!("integer expected, but got {}", text(v)));
        let major = llvm_integer(a, 10, u64::from(u32::MAX)).ok_or_else(bad)?;
        let minor = if b.is_empty() {
            0
        } else {
            llvm_integer(b, 10, u64::from(u32::MAX)).ok_or_else(bad)?
        };
        self.info.version = (
            u32::try_from(major).unwrap_or(0),
            u32::try_from(minor).unwrap_or(0),
        );
        Ok(())
    }
}

/// Reads `text` as `parseCOFFModuleDefinition(text, machine)` does (not
/// MinGW's), with underscores added on x86.
pub fn parse(text: &[u8], machine: u16) -> Result<LlvmDefinition, LlvmParseError> {
    let mut p = Parser {
        lex: Lexer { buf: text },
        tok: Token {
            k: K::Unknown,
            value: b"",
        },
        stack: Vec::new(),
        add_underscores: machine == machine::I386,
        info: LlvmDefinition::default(),
    };
    loop {
        p.parse_one()?;
        if p.tok.k == K::Eof {
            return Ok(p.info);
        }
    }
}

/// LLVM's `applyNameType`: what the loader looks up for a name type.
fn apply_name_type(t: ImportNameType, name: &[u8]) -> &[u8] {
    fn ltrim1(s: &[u8]) -> &[u8] {
        match s.first() {
            Some(b'?' | b'@' | b'_') => &s[1..],
            _ => s,
        }
    }
    match t {
        ImportNameType::NoPrefix => ltrim1(name),
        ImportNameType::Undecorate => {
            let s = ltrim1(name);
            &s[..s.iter().position(|&b| b == b'@').unwrap_or(s.len())]
        }
        _ => name,
    }
}

/// Why `writeImportLibrary` fails: its message.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct LlvmWriteError(pub String);

/// The weak external object LLVM makes to alias `weak` to `symbol`
/// (`__imp_` both, if `imp`), for `machine`: one empty `.drectve`, and
/// `@comp.id`, `@feat.00`, the symbol, and the weak external (searching
/// for an alias), every name of both in the string table.
pub fn weak_external(symbol: &[u8], weak: &[u8], imp: bool, machine: u16) -> Vec<u8> {
    let prefix: &[u8] = if imp { b"__imp_" } else { b"" };
    let a = [prefix, symbol].concat();
    let b = [prefix, weak].concat();
    let mut o = Vec::new();
    let p16 = |o: &mut Vec<u8>, v: u16| o.extend_from_slice(&v.to_le_bytes());
    let p32 = |o: &mut Vec<u8>, v: u32| o.extend_from_slice(&v.to_le_bytes());
    // The header: one section, the symbols after its header.
    p16(&mut o, machine);
    p16(&mut o, 1);
    p32(&mut o, 0);
    p32(&mut o, 20 + 40);
    p32(&mut o, 5);
    p16(&mut o, 0);
    p16(&mut o, 0);
    // .drectve: no data; IMAGE_SCN_LNK_INFO | IMAGE_SCN_LNK_REMOVE.
    o.extend_from_slice(b".drectve");
    o.extend_from_slice(&[0; 28]);
    p32(&mut o, 0xA00);
    let symbol_record = |o: &mut Vec<u8>, name: [u8; 8], section: u16, class: u8, aux: u8| {
        o.extend_from_slice(&name);
        o.extend_from_slice(&0u32.to_le_bytes());
        o.extend_from_slice(&section.to_le_bytes());
        o.extend_from_slice(&0u16.to_le_bytes());
        o.push(class);
        o.push(aux);
    };
    let offset = |off: u32| {
        let mut n = [0u8; 8];
        n[4..].copy_from_slice(&off.to_le_bytes());
        n
    };
    symbol_record(&mut o, *b"@comp.id", 0xFFFF, 3, 0);
    symbol_record(&mut o, *b"@feat.00", 0xFFFF, 3, 0);
    symbol_record(&mut o, offset(4), 0, 2, 0);
    let second = 4 + u32::try_from(a.len()).unwrap_or(0) + 1;
    symbol_record(&mut o, offset(second), 0, 105, 1);
    // The weak external's record: tag 2, IMAGE_WEAK_EXTERN_SEARCH_ALIAS.
    o.extend_from_slice(&2u32.to_le_bytes());
    o.extend_from_slice(&3u32.to_le_bytes());
    o.extend_from_slice(&[0; 10]);
    let len = 4 + a.len() + 1 + b.len() + 1;
    p32(&mut o, u32::try_from(len).unwrap_or(0));
    o.extend_from_slice(&a);
    o.push(0);
    o.extend_from_slice(&b);
    o.push(0);
    o
}

/// LLVM's `getArm64ECMangledFunctionName`, with `insertion` finding where
/// `$$h` goes in a C++ name (LLVM asks its demangler: after the qualified
/// name).
fn llvm_ec_mangle(name: &[u8], insertion: &dyn Fn(&[u8]) -> Option<usize>) -> Option<Vec<u8>> {
    if name.first() != Some(&b'?') {
        if name.first() == Some(&b'#') {
            return None;
        }
        return Some([b"#", name].concat());
    }
    if name.windows(3).any(|w| w == b"$$h") {
        return None;
    }
    let at = insertion(name)?;
    Some([&name[..at], b"$$h", &name[at..]].concat())
}

/// LLVM's `getArm64ECDemangledFunctionName`.
fn llvm_ec_demangle(name: &[u8]) -> Option<Vec<u8>> {
    if let Some(rest) = name.strip_prefix(b"#") {
        return Some(rest.to_vec());
    }
    if name.first() != Some(&b'?') {
        return None;
    }
    let at = name.windows(3).position(|w| w == b"$$h")?;
    let rest = &name[at + 3..];
    if rest.is_empty() {
        return None;
    }
    Some([&name[..at], rest].concat())
}

/// The import library `llvm-lib /def` writes: `writeImportLibrary(dll,
/// exports, machine)`, with an ARM64X library's `native` exports.
/// `insertion` finds where `$$h` goes in a C++ name (see
/// [`llvm_ec_mangle`]'s caller; [`ec_insertion`] is a stand-in).
pub fn members(
    output_file: &[u8],
    exports: &[ShortExport],
    lib_machine: u16,
    native: &[ShortExport],
    insertion: &dyn Fn(&[u8]) -> Option<usize>,
) -> Result<Members, LlvmWriteError> {
    let ec = matches!(lib_machine, machine::ARM64EC | machine::ARM64X);
    let (machine_ec, native_machine) = if ec {
        (machine::ARM64EC, machine::ARM64)
    } else {
        (lib_machine, lib_machine)
    };
    let dll = path_filename(output_file).to_vec();
    let stem = path_stem(output_file);
    let d = Descriptors {
        machine: native_machine,
        dll: &dll,
        stem,
        tool: Tool::LlvmLib,
    };
    let mut data: Vec<Vec<u8>> = d.all(Stamp::Time(0)).into();
    for (list, m) in [(exports, machine_ec), (native, native_machine)] {
        add_exports(&mut data, list, m, machine_ec, &dll, insertion)?;
    }
    let listed = (0..crate::descriptors::count(data.len())).collect();
    Ok(Members {
        dll,
        data,
        listed,
        machine: if ec { lib_machine } else { native_machine },
    })
}

fn short_import(
    sym: &[u8],
    ordinal: u16,
    kind: ImportType,
    t: ImportNameType,
    export_name: &[u8],
    m: u16,
    dll: &[u8],
) -> Vec<u8> {
    let o = ImportObject::new(
        m,
        kind,
        t,
        ordinal,
        sym,
        dll,
        (!export_name.is_empty()).then_some(export_name),
    );
    o.to_vec().unwrap_or_default()
}

fn add_exports(
    data: &mut Vec<Vec<u8>>,
    exports: &[ShortExport],
    m: u16,
    machine: u16,
    dll: &[u8],
    insertion: &dyn Fn(&[u8]) -> Option<usize>,
) -> Result<(), LlvmWriteError> {
    let ec_m = matches!(m, machine::ARM64EC | machine::ARM64X);
    let mut regular: alloc::collections::BTreeMap<Vec<u8>, Vec<u8>> =
        alloc::collections::BTreeMap::new();
    let mut renames: Vec<(Vec<u8>, ImportType, &ShortExport)> = Vec::new();
    for e in exports {
        if e.private {
            continue;
        }
        let mut kind = ImportType::Code;
        if e.data {
            kind = ImportType::Data;
        }
        if e.constant {
            kind = ImportType::Const;
        }
        let symbol_name = &e.name;
        let mut name: Vec<u8> = if e.ext_name.is_empty() {
            symbol_name.clone()
        } else {
            replace(symbol_name, &e.name, &e.ext_name)?
        };
        let mut export_name: Vec<u8> = Vec::new();
        let mut t;
        if e.noname {
            t = ImportNameType::Ordinal;
        } else if !e.export_as.is_empty() {
            t = ImportNameType::ExportAs;
            export_name.clone_from(&e.export_as);
        } else if !e.import_name.is_empty() {
            if machine == machine::I386
                && apply_name_type(ImportNameType::Undecorate, &name) == &e.import_name[..]
            {
                t = ImportNameType::Undecorate;
            } else if machine == machine::I386
                && apply_name_type(ImportNameType::NoPrefix, &name) == &e.import_name[..]
            {
                t = ImportNameType::NoPrefix;
            } else if ec_m {
                t = ImportNameType::ExportAs;
                export_name.clone_from(&e.import_name);
            } else if name == e.import_name {
                t = ImportNameType::Name;
            } else {
                renames.push((name, kind, e));
                continue;
            }
        } else {
            // getNameType(SymbolName, E.Name): the same name.
            t = if symbol_name.first() == Some(&b'_') && symbol_name.contains(&b'@') {
                ImportNameType::Name
            } else if m == machine::I386 && symbol_name.first() == Some(&b'_') {
                ImportNameType::NoPrefix
            } else {
                ImportNameType::Name
            };
        }
        if kind == ImportType::Code && ec_m {
            if let Some(mangled) = llvm_ec_mangle(&name, insertion) {
                if !e.noname && export_name.is_empty() {
                    t = ImportNameType::ExportAs;
                    export_name = core::mem::take(&mut name);
                }
                name = mangled;
            } else if !e.noname && export_name.is_empty() {
                let Some(plain) = llvm_ec_demangle(&name) else {
                    return Err(LlvmWriteError(alloc::format!(
                        "Invalid ARM64EC function name '{}'",
                        text(&name)
                    )));
                };
                t = ImportNameType::ExportAs;
                export_name = plain;
            }
        }
        regular.insert(apply_name_type(t, &name).to_vec(), name.clone());
        data.push(short_import(
            &name,
            e.ordinal,
            kind,
            t,
            &export_name,
            m,
            dll,
        ));
    }
    for (name, kind, e) in renames {
        match regular.get(&e.import_name) {
            Some(symbol) => {
                if kind == ImportType::Code {
                    data.push(weak_external(symbol, &name, false, m));
                }
                data.push(weak_external(symbol, &name, true, m));
            }
            None => data.push(short_import(
                &name,
                e.ordinal,
                kind,
                ImportNameType::ExportAs,
                &e.import_name,
                m,
                dll,
            )),
        }
    }
    Ok(())
}

/// LLVM's `replace(S, From, To)`: `From`'s first place in `S` replaced by
/// `To` (retrying without a leading `_` on both).
fn replace(s: &[u8], from: &[u8], to: &[u8]) -> Result<Vec<u8>, LlvmWriteError> {
    let find = |h: &[u8], n: &[u8]| {
        if n.is_empty() {
            Some(0)
        } else {
            h.windows(n.len()).position(|w| w == n)
        }
    };
    let (mut from, mut to) = (from, to);
    let mut pos = find(s, from);
    if pos.is_none() && from.first() == Some(&b'_') && to.first() == Some(&b'_') {
        from = &from[1..];
        to = &to[1..];
        pos = find(s, from);
    }
    let Some(p) = pos else {
        return Err(LlvmWriteError(alloc::format!(
            "{}: replacing '{}' with '{}' failed",
            text(s),
            text(from),
            text(to)
        )));
    };
    Ok([&s[..p], to, &s[p + from.len()..]].concat())
}

/// Where `$$h` goes in a C++ name, as this crate guesses it: after the
/// first `@@` that an upper-case letter follows (what
/// `turbonop_archive::symbols::ec_mangle` does). LLVM asks its demangler;
/// a caller that has one should use it.
pub fn ec_insertion(name: &[u8]) -> Option<usize> {
    let at = name.windows(2).position(|w| w == b"@@")? + 2;
    name.get(at)?.is_ascii_uppercase().then_some(at)
}

/// LLVM's demangled name of an ARM64EC symbol, as the index lists it
/// (re-exported for the app).
pub fn plain_name(name: &[u8]) -> Cow<'_, [u8]> {
    ec_demangle(name)
}

#[cfg(test)]
mod tests;
