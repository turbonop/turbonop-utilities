//! `hexc ltcg`: dumps Visual C++ link-time code generation (`/GL`)
//! objects, and the ones in libraries: what their intermediate language
//! records (see `hexcavator_pe::ltcg`).

use std::io::{self, Write};
use std::process::ExitCode;

use hexcavator_codeview::TypeIndex;
use hexcavator_codeview::types::{TypeFormatter, TypeRecord, TypeSource, leaf};
use hexcavator_mmap::Mmap;
use hexcavator_pe::ltcg::{self, DirectiveKind, LtcgObject, Storage, SymbolKind};

pub const USAGE: &str = "\
usage: hexc ltcg [options] <file>...

Dumps Visual C++ LTCG objects (compiled with /GL), and those in libraries:
the intermediate language the linker hands to the compiler's back end.

options:
  -headers      the object's header, back end, and IL sections (default)
  -options      the back end's command line (default)
  -sources      the source files, with their checksums (default)
  -directives   the linker directives (default)
  -symbols      the functions and variables (default)
  -publics      of those, only the public definitions
  -types        the CodeView type and id records
  -all          everything above

Without options, everything but -types and -publics.";

#[derive(Default, Clone, Copy)]
struct Options {
    headers: bool,
    options: bool,
    sources: bool,
    directives: bool,
    symbols: bool,
    publics: bool,
    types: bool,
}

pub fn run(args: &[String]) -> ExitCode {
    let mut o = Options::default();
    let mut paths = Vec::new();
    for a in args {
        match a.trim_start_matches('-') {
            _ if !a.starts_with('-') => paths.push(a.as_str()),
            "headers" => o.headers = true,
            "options" => o.options = true,
            "sources" => o.sources = true,
            "directives" => o.directives = true,
            "symbols" => o.symbols = true,
            "publics" => o.publics = true,
            "types" => o.types = true,
            "all" => {
                o = Options {
                    headers: true,
                    options: true,
                    sources: true,
                    directives: true,
                    symbols: true,
                    publics: false,
                    types: true,
                }
            }
            "h" | "help" => return crate::out::print(format!("{USAGE}\n").as_bytes()),
            _ => {
                ereport!("hexc ltcg: unknown option '{a}'\n\n{USAGE}");
                return ExitCode::from(2);
            }
        }
    }
    if paths.is_empty() {
        ereport!("{USAGE}");
        return ExitCode::from(2);
    }
    if !(o.headers || o.options || o.sources || o.directives || o.symbols || o.publics || o.types) {
        o = Options {
            headers: true,
            options: true,
            sources: true,
            directives: true,
            symbols: true,
            publics: false,
            types: false,
        };
    }
    let mut out = io::BufWriter::new(crate::out::stdout());
    let mut ok = true;
    for path in paths {
        let result = match Mmap::open(path) {
            Ok(map) => dump_file(&mut out, &o, path, &map),
            Err(e) => Ok(Err(e.to_string())),
        };
        match result {
            Ok(Ok(())) => {}
            Ok(Err(message)) => {
                let _ = out.flush();
                ereport!("hexc: {path}: {message}");
                ok = false;
            }
            Err(e) if e.kind() == io::ErrorKind::BrokenPipe => return ExitCode::SUCCESS,
            Err(e) => {
                ereport!("hexc: {e}");
                return ExitCode::FAILURE;
            }
        }
    }
    if out.flush().is_err() {
        return ExitCode::FAILURE;
    }
    if ok {
        ExitCode::SUCCESS
    } else {
        ExitCode::FAILURE
    }
}

/// Dumps a file: an LTCG object, or a library's LTCG objects. The outer
/// result is an output error; the inner one is a problem with the file.
fn dump_file(
    out: &mut dyn Write,
    o: &Options,
    path: &str,
    data: &[u8],
) -> io::Result<Result<(), String>> {
    if LtcgObject::is_ltcg(data) {
        return dump_object(out, o, path, data);
    }
    if hexcavator_archive::Archive::is_archive(data) {
        let ar = match hexcavator_archive::Archive::parse(data) {
            Ok(a) => a,
            Err(e) => return Ok(Err(e.to_string())),
        };
        let mut any = false;
        for m in ar.members() {
            if m.kind != hexcavator_archive::MemberKind::File || !LtcgObject::is_ltcg(m.data) {
                continue;
            }
            any = true;
            let name = format!("{path}({})", String::from_utf8_lossy(&m.name));
            if let Err(e) = dump_object(out, o, &name, m.data)? {
                return Ok(Err(format!("{name}: {e}")));
            }
        }
        if !any {
            return Ok(Err("the library has no LTCG (/GL) objects".into()));
        }
        return Ok(Ok(()));
    }
    Ok(Err(
        "not an LTCG object (one compiled with /GL) or a library of them".into(),
    ))
}

fn s(b: &[u8]) -> std::borrow::Cow<'_, str> {
    String::from_utf8_lossy(b)
}

fn guid(b: &[u8; 16]) -> String {
    format!(
        "{{{:08X}-{:04X}-{:04X}-{:02X}{:02X}-{:02X}{:02X}{:02X}{:02X}{:02X}{:02X}}}",
        u32::from_le_bytes([b[0], b[1], b[2], b[3]]),
        u16::from_le_bytes([b[4], b[5]]),
        u16::from_le_bytes([b[6], b[7]]),
        b[8],
        b[9],
        b[10],
        b[11],
        b[12],
        b[13],
        b[14],
        b[15]
    )
}

/// What each IL section holds.
fn stream_role(name: &[u8]) -> &'static str {
    match name {
        b".drectve" => "the back end to load",
        b".cil$fg" => "the back end's command line",
        b".cil$gl" => "global symbols, source files, linker directives",
        b".cil$ex" => "function bodies",
        b".cil$sy" => "functions' local symbols",
        b".cil$in" => "initializers",
        b".cil$db" => "CodeView types and ids",
        _ => "",
    }
}

fn dump_object(
    out: &mut dyn Write,
    o: &Options,
    name: &str,
    data: &[u8],
) -> io::Result<Result<(), String>> {
    let obj = match LtcgObject::parse(data) {
        Ok(x) => x,
        Err(e) => return Ok(Err(e.to_string())),
    };
    writeln!(out, "LTCG object {name}\n")?;
    if o.headers {
        let machine = hexcavator_pe::consts::machine::name(obj.machine).unwrap_or("unknown");
        writeln!(out, "  machine          {:X} ({machine})", obj.machine)?;
        writeln!(out, "  time date stamp  {:08X}", obj.time_date_stamp)?;
        writeln!(out, "  class ID         {}", guid(&obj.class_id))?;
        writeln!(out, "  size             {:X}", obj.size_of_data)?;
        writeln!(out, "  IL version       {:04X}", obj.il_version)?;
        if let Some(id) = obj.comp_id() {
            writeln!(
                out,
                "  compiler         {:08X} (@comp.id: product {:#x}, build {})",
                id,
                id >> 16,
                id & 0xFFFF
            )?;
        }
        if let Some(f) = obj.feat_00() {
            writeln!(out, "  @feat.00         {f:08X}")?;
        }
        if let Some(c) = obj.compiler() {
            writeln!(out, "  back end         {}", s(c))?;
        }
        writeln!(out, "\n  IL sections")?;
        for st in obj.streams() {
            writeln!(
                out,
                "    {:<10} {:>8X} bytes at {:<8X} {}",
                s(st.name),
                st.data.len(),
                st.offset,
                stream_role(st.name)
            )?;
        }
        writeln!(out)?;
    }
    if o.options {
        let args = obj.backend_args();
        writeln!(out, "  Back end's command line ({} arguments)", args.len())?;
        for a in args {
            writeln!(out, "    {}", s(a))?;
        }
        writeln!(out)?;
    }
    let need_globals = o.sources || o.directives || o.symbols || o.publics;
    let globals = if need_globals {
        match obj.globals() {
            Ok(g) => Some(g),
            Err(e) => return Ok(Err(e.to_string())),
        }
    } else {
        None
    };
    if let Some(g) = &globals {
        if o.sources {
            writeln!(out, "  Source files ({})", g.source_files.len())?;
            for f in &g.source_files {
                let kind = match f.checksum_kind {
                    1 => "MD5",
                    2 => "SHA-1",
                    3 => "SHA-256",
                    _ => "checksum",
                };
                let hex: String = f.checksum.iter().map(|b| format!("{b:02x}")).collect();
                writeln!(out, "    {}", s(f.path))?;
                writeln!(out, "      {kind} {hex}")?;
            }
            writeln!(out)?;
        }
        if o.directives {
            writeln!(out, "  Linker directives ({})", g.directives.len())?;
            for d in &g.directives {
                match d.kind {
                    DirectiveKind::Library => writeln!(out, "    /DEFAULTLIB:{}", s(d.text))?,
                    DirectiveKind::Linker => writeln!(out, "    {}", s(d.text))?,
                    DirectiveKind::Other(k) => writeln!(out, "    ({k:#x}) {}", s(d.text))?,
                }
            }
            writeln!(out)?;
        }
        if o.symbols || o.publics {
            let list: Vec<_> = g
                .symbols
                .iter()
                .filter(|x| !o.publics || o.symbols || x.storage().is_public_definition())
                .collect();
            let what = if o.publics && !o.symbols {
                "Public definitions"
            } else {
                "Symbols"
            };
            writeln!(out, "  {what} ({})", list.len())?;
            writeln!(
                out,
                "    {:>6}  {:<8}  {:<9}  {:>6}  flags  {:<20}  name",
                "id", "kind", "storage", "size", "attributes"
            )?;
            for x in list {
                symbol_line(out, x)?;
            }
            writeln!(out)?;
        }
    }
    if o.types {
        match obj.types() {
            Ok(t) => types(out, &t)?,
            Err(e) => return Ok(Err(e.to_string())),
        }
    }
    Ok(Ok(()))
}

fn symbol_line(out: &mut dyn Write, x: &ltcg::IlSymbol<'_>) -> io::Result<()> {
    let mut attrs = Vec::new();
    let (kind, storage, size) = match x.kind {
        SymbolKind::Function {
            storage,
            attributes,
            body,
        } => {
            use ltcg::function::*;
            for (bit, n) in [
                (INLINE, "inline"),
                (VARARGS, "varargs"),
                (NORETURN, "noreturn"),
                (CALLEE_POPS, "callee-pops"),
            ] {
                if attributes & bit != 0 {
                    attrs.push(n);
                }
            }
            if body && storage == Storage::Declared {
                attrs.push("body");
            }
            ("function", storage, String::new())
        }
        SymbolKind::Data {
            storage,
            writable,
            size,
        } => {
            if !writable {
                attrs.push("read-only");
            }
            ("data", storage, format!("{size:X}"))
        }
    };
    let name = s(x.name);
    let undecorated = crate::dump::undecorate(&name)
        .map(|u| format!(" ({u})"))
        .unwrap_or_default();
    writeln!(
        out,
        "    {:>6X}  {kind:<8}  {:<9}  {size:>6}  {:>5X}  {:<20}  {name}{undecorated}",
        x.id,
        storage.name(),
        x.flags,
        attrs.join(",")
    )
}

/// The records as a type source (one index space: types, then ids).
struct Records<'a>(&'a [(u16, &'a [u8])]);

impl TypeSource for Records<'_> {
    fn record(&self, ti: TypeIndex) -> Option<TypeRecord<'_>> {
        let i = ti.0.checked_sub(ltcg::Types::FIRST_INDEX)? as usize;
        self.0.get(i).map(|&(kind, data)| TypeRecord { kind, data })
    }

    fn records(&self) -> Box<dyn Iterator<Item = (TypeIndex, TypeRecord<'_>)> + '_> {
        Box::new(self.0.iter().enumerate().map(|(i, &(kind, data))| {
            (
                TypeIndex(
                    ltcg::Types::FIRST_INDEX.saturating_add(u32::try_from(i).unwrap_or(u32::MAX)),
                ),
                TypeRecord { kind, data },
            )
        }))
    }
}

fn types(out: &mut dyn Write, t: &ltcg::Types<'_>) -> io::Result<()> {
    let source = Records(&t.records);
    let tf = TypeFormatter { types: &source };
    writeln!(out, "  Types ({})", t.type_count)?;
    for (i, &(kind, data)) in t.records.iter().enumerate() {
        if i == t.type_count {
            writeln!(out, "\n  Ids ({})", t.records.len() - t.type_count)?;
        }
        let ti = TypeIndex(
            ltcg::Types::FIRST_INDEX.saturating_add(u32::try_from(i).unwrap_or(u32::MAX)),
        );
        let rec = TypeRecord { kind, data };
        let mut what = if i < t.type_count {
            crate::pdbdump::describe_type(ti, &rec, &tf)
        } else {
            String::new()
        };
        if what.is_empty() {
            what = rec.name().map(|n| s(n).into_owned()).unwrap_or_default();
        }
        writeln!(out, "    {:#x} {} {what}", ti.0, leaf::name(kind))?;
    }
    writeln!(out)
}
