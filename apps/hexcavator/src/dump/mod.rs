//! `hexcavator dump`: a dumpbin-compatible dumper.
//!
//! For PE images and COFF objects the output follows `dumpbin` closely, so
//! that the two can be compared line by line. Formats dumpbin does not handle
//! (DOS, NE, LE/LX) get output in the same style.

mod arm64_unwind;
mod coff_symbols;
mod debug;
mod disasm;
mod dos;
mod imports;
mod load_config;
mod pe;
mod readobj;
mod unwind;
mod x64_eh;

use std::collections::BTreeMap;
use std::io::{self, Write};
use std::process::ExitCode;

use turbonop_mmap::Mmap;
use turbonop_mz::{MzFile, NewHeaderKind};
use turbonop_pe::PeFile;

/// `name` undecorated as dumpbin shows it (without `__ptr64`), if it is a
/// decorated C++ name (or the import thunk of one).
pub fn undecorate(name: &str) -> Option<String> {
    use std::cell::RefCell;
    thread_local! {
        static DEMANGLER: RefCell<turbonop_demangle::msvc::Demangler> = RefCell::default();
    }
    if let Some(imported) = name.strip_prefix("__imp_") {
        return undecorate(imported).map(|u| format!("__declspec(dllimport) {u}"));
    }
    if !name.starts_with('?') {
        return None;
    }
    DEMANGLER.with(|d| {
        d.borrow_mut()
            .demangle(name, turbonop_demangle::msvc::flags::NO_PTR64)
            .filter(|s| s != name)
    })
}

pub const USAGE: &str = "\
usage: hexcavator dump [options] <file>...

options (dumpbin-style; write as /name, -name, or --name):
  /all          everything below
  /coffgroups   the COFF groups the linker laid out (the POGO debug entry's;
                hexcavator's own: dumpbin doesn't decode them)
  /disasm       disassembly of code sections
  /exports      exported functions
  /headers      file, optional, and section headers
  /imports      imported functions, including delay-loaded ones
  /loadconfig   load configuration directory
  /nopdb        do not load symbols from the image's PDB
  /pdata        exception directory (function table)
  /relocations  base relocations
  /summary      section size totals (the default)
  /symbols      COFF symbol table (an object's, or an image's that has one)
  /tls          thread-local storage directory
  /unwindinfo   function table with unwind information

ELF, Mach-O, and WebAssembly files (and archives of them) are dumped as
llvm-readobj shows them, with its options' names: --file-headers,
--sections, --program-headers, --dynamic-table, --relocations, --symbols,
--dyn-symbols, --notes, --needed-libs, --macho-segment, --macho-dysymtab,
--macho-version-min. /headers gives the file header, sections, and
segments; /relocations, the relocations; /symbols, the symbols; /imports or /exports, the
dynamic symbols and needed libraries; /all, everything; and nothing, the
file header and sections. A Mach-O file's fixups (rebases and binds, and
arm64e's signed pointers' keys and discriminators) are listed as
llvm-objdump's --macho --dyld-info lists them, with --dyld-info.";

/// Options that may be written with a leading `/` as well as `-`/`--`.
const OPTIONS: &[&str] = &[
    "all",
    "coffgroups",
    "disasm",
    "exports",
    "headers",
    "imports",
    "loadconfig",
    "nopdb",
    "pdata",
    "relocations",
    "summary",
    "symbols",
    "tls",
    "unwindinfo",
];

#[derive(Default)]
struct Options {
    /// For ELF, Mach-O, and WebAssembly files: llvm-readobj's parts.
    readobj: readobj::Parts,
    headers: bool,
    disasm: bool,
    loadconfig: bool,
    imports: bool,
    exports: bool,
    relocations: bool,
    pdata: bool,
    unwindinfo: bool,
    tls: bool,
    /// The POGO debug entry's COFF groups.
    coffgroups: bool,
    /// The COFF symbol table (and, of other formats, their symbols).
    symbols: bool,
    /// Do not look for the image's PDB.
    nopdb: bool,
    /// `-all`: every part of each file (none of them out of place).
    all: bool,
}

impl Options {
    /// The dumpbin options given that a file of another format (ELF,
    /// Mach-O, WebAssembly) has no part for, each with what shows it.
    fn pe_only(&self) -> Vec<&'static str> {
        if self.all {
            return Vec::new();
        }
        [
            (
                self.disasm,
                "/disasm (hexcavator size -disassemble <function> disassembles any format's)",
            ),
            (
                self.pdata,
                "/pdata (hexcavator dwarf -frames shows DWARF's call frames)",
            ),
            (
                self.unwindinfo,
                "/unwindinfo (hexcavator dwarf -frames shows DWARF's call frames)",
            ),
            (self.tls, "/tls"),
            (self.coffgroups, "/coffgroups"),
            (self.loadconfig, "/loadconfig"),
            (self.nopdb, "/nopdb"),
        ]
        .into_iter()
        .filter_map(|(given, what)| given.then_some(what))
        .collect()
    }
}

/// Section sizes accumulated across all files, by name.
/// Keyed by name and characteristics: like dumpbin, same-named sections with
/// different attributes are totalled separately.
type Summary = BTreeMap<(Vec<u8>, u32), u64>;

pub fn run(args: &[String]) -> ExitCode {
    let mut options = Options::default();
    let mut paths = Vec::new();
    for arg in args {
        match option_name(arg) {
            Some(name) => match name.to_ascii_lowercase().as_str() {
                "headers" => options.headers = true,
                "disasm" => options.disasm = true,
                "loadconfig" => options.loadconfig = true,
                "imports" => options.imports = true,
                "exports" => options.exports = true,
                "relocations" => options.relocations = true,
                "pdata" => options.pdata = true,
                "unwindinfo" => options.unwindinfo = true,
                "tls" => options.tls = true,
                "coffgroups" => options.coffgroups = true,
                "symbols" => options.symbols = true,
                "nopdb" => options.nopdb = true,
                "all" => {
                    options.all = true;
                    options.headers = true;
                    options.loadconfig = true;
                    options.imports = true;
                    options.exports = true;
                    options.relocations = true;
                    options.unwindinfo = true;
                    options.tls = true;
                    options.coffgroups = true;
                    options.symbols = true;
                    options.disasm = true;
                }
                "summary" => {}
                other if options.readobj.set(other) => {}
                _ => ereport!("hexcavator: warning: unrecognized option '{arg}'; ignored"),
            },
            None => paths.push(arg.as_str()),
        }
    }
    if paths.is_empty() {
        ereport!("{USAGE}");
        return ExitCode::from(2);
    }

    let mut out = io::BufWriter::new(crate::out::stdout());
    match dump_files(&mut out, &options, &paths).and_then(|ok| out.flush().map(|_| ok)) {
        Ok(true) => ExitCode::SUCCESS,
        Ok(false) => ExitCode::FAILURE,
        // A closed pipe (e.g. `| head`) is not an error.
        Err(e) if e.kind() == io::ErrorKind::BrokenPipe => ExitCode::SUCCESS,
        Err(e) => {
            ereport!("hexcavator: {e}");
            ExitCode::FAILURE
        }
    }
}

/// The option name in `arg`, if it is an option.
///
/// `/name` is only treated as an option when `name` is a known option, so
/// that absolute Unix paths still work.
fn option_name(arg: &str) -> Option<&str> {
    if let Some(rest) = arg.strip_prefix("--").or_else(|| arg.strip_prefix('-')) {
        return (!rest.is_empty()).then_some(rest);
    }
    let rest = arg.strip_prefix('/')?;
    let name = rest.split(':').next().unwrap_or(rest);
    OPTIONS
        .iter()
        .any(|o| o.eq_ignore_ascii_case(name))
        .then_some(rest)
}

/// Dumps each file; returns whether all of them could be dumped.
fn dump_files(out: &mut dyn Write, options: &Options, paths: &[&str]) -> io::Result<bool> {
    let mut summary = Summary::new();
    let mut ok = true;
    // (Each option out of place said once.)
    let mut warned_pe_only = false;
    let mut warned_readobj = false;
    for (i, path) in paths.iter().enumerate() {
        // ELF, Mach-O, and WebAssembly files: as llvm-readobj shows them.
        if let Ok(map) = Mmap::open(path)
            && let Some(result) = readobj_file(options, path, &map)
        {
            let pe_only = options.pe_only();
            if !pe_only.is_empty() && !warned_pe_only {
                warned_pe_only = true;
                ereport!(
                    "hexcavator: warning: {path}: not a PE or COFF file, which these are of: {}",
                    pe_only.join("; ")
                );
            }
            match result {
                Ok(text) => out.write_all(text.as_bytes())?,
                Err(message) => {
                    out.flush()?;
                    ereport!("hexcavator: {path}: {message}");
                    ok = false;
                }
            }
            continue;
        }
        // (llvm-readobj's parts are ELF's, Mach-O's, and WebAssembly's.)
        if options.readobj.any() && !options.all && !warned_readobj {
            warned_readobj = true;
            ereport!(
                "hexcavator: warning: {path}: llvm-readobj's options are of ELF, Mach-O, and WebAssembly files; dumpbin's (/headers, /exports, /relocations, ...) are of this one"
            );
        }
        // Like `dumpbin /nologo`: a blank line before the first file, and a
        // form feed between files.
        writeln!(out, "{}", if i == 0 { "" } else { "\x0c" })?;
        writeln!(out, "Dump of file {path}\n")?;
        let result = match Mmap::open(path) {
            Ok(map) => dump_file(out, options, path, &map, &mut summary),
            Err(e) => Ok(Err(e.to_string())),
        };
        if let Err(message) = result? {
            out.flush()?;
            ereport!("hexcavator: {path}: {message}");
            ok = false;
        }
    }
    // (dumpbin's summary: of the PE and COFF files dumped, if any were.)
    if !summary.is_empty()
        || paths.iter().all(|p| {
            Mmap::open(p).is_ok_and(|m| {
                readobj_file(options, p, &m).is_none()
                    && !turbonop_pe::ltcg::LtcgObject::is_ltcg(&m)
            })
        })
    {
        pe::summary(out, &summary)?;
    }
    Ok(ok)
}

/// An ELF, Mach-O, or WebAssembly file's dump, as llvm-readobj shows it;
/// `None` for other files.
fn readobj_file(options: &Options, path: &str, data: &[u8]) -> Option<Result<String, String>> {
    let mut parts = options.readobj;
    if options.headers {
        parts.file_headers = true;
        parts.sections = true;
        parts.program_headers = true;
        parts.macho_segment = true;
    }
    if options.relocations {
        parts.relocations = true;
    }
    if options.symbols {
        parts.symbols = true;
    }
    if options.imports || options.exports {
        parts.dyn_symbols = true;
        parts.needed_libs = true;
    }
    if options.headers && options.imports && options.loadconfig {
        // (-all.)
        parts = readobj::Parts::all();
    }
    if !parts.any() {
        parts.file_headers = true;
        parts.sections = true;
    }
    // (llvm-readobj names the file as it was given.)
    let mut out = String::new();
    readobj_bytes(&mut out, path, data, &parts, 0).map(|r| r.map(|()| out))
}

/// Dumps `data` (named `name`) as llvm-readobj does into `out`: an ELF or
/// Mach-O image or object, a WebAssembly module, each image of a universal
/// file, each object of an archive. `None` if it is none of these.
fn readobj_bytes(
    out: &mut String,
    name: &str,
    data: &[u8],
    parts: &readobj::Parts,
    depth: u32,
) -> Option<Result<(), String>> {
    if turbonop_elf::ElfFile::is_elf(data) {
        return Some(match turbonop_elf::ElfFile::parse(data) {
            Ok(elf) => {
                readobj::elf::dump(out, name, &elf, parts);
                Ok(())
            }
            Err(e) => Err(e.to_string()),
        });
    }
    if turbonop_macho::is_macho(data) {
        return Some(readobj::macho::dump(out, name, data, parts));
    }
    if turbonop_wasm::is_wasm(data) {
        return Some(readobj::wasm::dump(out, name, data, parts));
    }
    // (A universal file of archives holds archives; nothing nests deeper.)
    if depth > 1 {
        return None;
    }
    if let Some(archs) = turbonop_macho::fat_archs(data) {
        for a in archs.iter().take(64) {
            let (Ok(s), Ok(n)) = (usize::try_from(a.offset), usize::try_from(a.size)) else {
                continue;
            };
            let Some(slice) = data.get(s..s.saturating_add(n)) else {
                continue;
            };
            if let Some(Err(e)) = readobj_bytes(out, name, slice, parts, depth + 1) {
                return Some(Err(e));
            }
        }
        return Some(Ok(()));
    }
    if turbonop_archive::Archive::is_archive(data) {
        let ar = match turbonop_archive::Archive::parse(data) {
            Ok(a) => a,
            Err(e) => return Some(Err(e.to_string())),
        };
        let mut any = false;
        for m in ar.members() {
            if m.kind != turbonop_archive::MemberKind::File {
                continue;
            }
            let member = format!("{name}({})", String::from_utf8_lossy(&m.name));
            if let Some(r) = readobj_bytes(out, &member, m.data, parts, depth + 1) {
                any = true;
                if let Err(e) = r {
                    return Some(Err(format!("{member}: {e}")));
                }
            }
        }
        // (An archive of COFF objects is dumpbin's.)
        return any.then_some(Ok(()));
    }
    None
}

/// Dumps one file. The outer result is an output error; the inner one is a
/// problem with the file.
fn dump_file(
    out: &mut dyn Write,
    options: &Options,
    path: &str,
    data: &[u8],
    summary: &mut Summary,
) -> io::Result<Result<(), String>> {
    if let Ok(mz) = MzFile::parse(data) {
        return match mz.new_header().map(|h| h.kind) {
            Some(NewHeaderKind::Pe) => match PeFile::parse_image(data) {
                Ok(file) => pe::dump(out, options, path, &file, summary).map(Ok),
                Err(e) => Ok(Err(e.to_string())),
            },
            _ => dos::dump(out, options, &mz).map(Ok),
        };
    }
    if turbonop_pe::ltcg::LtcgObject::is_ltcg(data) {
        return ltcg_header(out, options, data).map(Ok);
    }
    match PeFile::parse_object(data) {
        Ok(file) if turbonop_pe::consts::machine::name(file.machine()).is_some() => {
            pe::dump(out, options, path, &file, summary).map(Ok)
        }
        Ok(_) | Err(_) => Ok(Err("unrecognized file format".to_string())),
    }
}

/// An LTCG (`/GL`) object, as dumpbin shows one: its anonymous object
/// header (with /headers), and nothing more (its contents are the
/// compiler's: `hexcavator ltcg` dumps them).
fn ltcg_header(out: &mut dyn Write, options: &Options, data: &[u8]) -> io::Result<()> {
    writeln!(out, "File Type: ANONYMOUS OBJECT")?;
    if !options.headers {
        return Ok(());
    }
    let u16_at = |o: usize| u16::from_le_bytes([data[o], data[o + 1]]);
    let u32_at = |o: usize| u32::from_le_bytes([data[o], data[o + 1], data[o + 2], data[o + 3]]);
    let machine = u16_at(6);
    let stamp = u32_at(8);
    let date = if stamp == 0 || stamp > 0x7FFF_FFFF {
        String::new()
    } else {
        format!(" {} UTC", crate::time::asctime_utc(stamp.into()))
    };
    let c = &data[12..28];
    writeln!(
        out,
        "
ANON OBJECT HEADER VALUES"
    )?;
    writeln!(out, "{:16X} version", u16_at(4))?;
    writeln!(
        out,
        "{machine:16X} machine ({})",
        turbonop_pe::consts::machine::name(machine).unwrap_or("Unknown")
    )?;
    writeln!(out, "{stamp:16X} time date stamp{date}")?;
    writeln!(
        out,
        "                 ClassID: {{{:08X}-{:04X}-{:04X}-{:02X}{:02X}-{:02X}{:02X}{:02X}{:02X}{:02X}{:02X}}}",
        u32::from_le_bytes([c[0], c[1], c[2], c[3]]),
        u16::from_le_bytes([c[4], c[5]]),
        u16::from_le_bytes([c[6], c[7]]),
        c[8],
        c[9],
        c[10],
        c[11],
        c[12],
        c[13],
        c[14],
        c[15]
    )?;
    writeln!(out, "{:16X} size", u32_at(28))?;
    // (Version 1 has no flags; dumpbin shows 0.)
    writeln!(out, "{:16X} flags", 0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn option_syntax() {
        assert_eq!(option_name("/headers"), Some("headers"));
        assert_eq!(option_name("/HEADERS"), Some("HEADERS"));
        assert_eq!(option_name("-headers"), Some("headers"));
        assert_eq!(option_name("--headers"), Some("headers"));
        assert_eq!(option_name("--bogus"), Some("bogus"));
        assert_eq!(option_name("/usr/lib/foo.so"), None);
        assert_eq!(option_name("foo.dll"), None);
        assert_eq!(option_name("-"), None);
    }
}
