//! `hexcavator ltcg`: dumps Visual C++ link-time code generation (`/GL`)
//! objects, and the ones in libraries: what their intermediate language
//! records (see `turbonop_pe::ltcg`).

use std::io::{self, Write};
use std::process::ExitCode;

use turbonop_codeview::TypeIndex;
use turbonop_codeview::types::{TypeFormatter, TypeRecord, TypeSource, leaf};
use turbonop_mmap::Mmap;
use turbonop_pe::ltcg::{self, DirectiveKind, LtcgObject, Storage, SymbolKind};

pub const USAGE: &str = "\
usage: hexcavator ltcg [options] <file>...

Dumps Visual C++ LTCG objects (compiled with /GL), and those in libraries:
the intermediate language the linker hands to the compiler's back end.

options:
  -headers      the object's header, back end, and IL sections (default)
  -options      the back end's command line (default)
  -sources      the source files, with their checksums (default)
  -directives   the linker directives (default)
  -symbols      the functions and variables (default)
  -publics      of those, only the public definitions lib.exe lists in a
                library's index (inline functions only if something
                emitted refers to them)
  -records      the other records: sections, namespaces, named types,
                constants, attributes
  -refs         what each function refers to: what it calls (directly or
                virtually), reads or writes, and takes the address of; and
                what each variable's initializer refers to
  -locals       each function's scopes and local symbols (.cil$sy)
  -init         the variables' initial values (.cil$in)
  -types        the CodeView type and id records
  -all          everything above

Without options, everything but -types, -publics, -records, -locals,
and -init.";

#[derive(Default, Clone, Copy)]
struct Options {
    headers: bool,
    options: bool,
    sources: bool,
    directives: bool,
    symbols: bool,
    publics: bool,
    types: bool,
    refs: bool,
    records: bool,
    locals: bool,
    init: bool,
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
            "refs" => o.refs = true,
            "records" => o.records = true,
            "locals" => o.locals = true,
            "init" => o.init = true,
            "all" => {
                o = Options {
                    headers: true,
                    options: true,
                    sources: true,
                    directives: true,
                    symbols: true,
                    publics: false,
                    types: true,
                    refs: true,
                    records: true,
                    locals: true,
                    init: true,
                }
            }
            "h" | "help" => return crate::out::print(format!("{USAGE}\n").as_bytes()),
            _ => {
                ereport!("hexcavator ltcg: unknown option '{a}'\n\n{USAGE}");
                return ExitCode::from(2);
            }
        }
    }
    if paths.is_empty() {
        ereport!("{USAGE}");
        return ExitCode::from(2);
    }
    if !(o.headers
        || o.options
        || o.sources
        || o.directives
        || o.symbols
        || o.publics
        || o.types
        || o.refs
        || o.records
        || o.locals
        || o.init)
    {
        o = Options {
            headers: true,
            options: true,
            sources: true,
            directives: true,
            symbols: true,
            ..Options::default()
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
                ereport!("hexcavator: {path}: {message}");
                ok = false;
            }
            Err(e) if e.kind() == io::ErrorKind::BrokenPipe => return ExitCode::SUCCESS,
            Err(e) => {
                ereport!("hexcavator: {e}");
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
    if turbonop_archive::Archive::is_archive(data) {
        let ar = match turbonop_archive::Archive::parse(data) {
            Ok(a) => a,
            Err(e) => return Ok(Err(e.to_string())),
        };
        let mut any = false;
        for m in ar.members() {
            if m.kind != turbonop_archive::MemberKind::File || !LtcgObject::is_ltcg(m.data) {
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
        let machine = turbonop_pe::consts::machine::name(obj.machine).unwrap_or("unknown");
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
    let need_globals = o.headers
        || o.sources
        || o.directives
        || o.symbols
        || o.publics
        || o.refs
        || o.records
        || o.locals
        || o.init;
    let globals = if need_globals {
        match obj.globals() {
            Ok(g) => Some(g),
            Err(e) => return Ok(Err(e.to_string())),
        }
    } else {
        None
    };
    let locals = obj.locals().ok();
    let units = obj.il_units();
    if o.headers
        && let Some(g) = &globals
    {
        match g.walked {
            Ok(()) => writeln!(
                out,
                "  Records          {} (every one walked)",
                g.spans.len()
            )?,
            Err(at) => writeln!(
                out,
                "  Records          {} walked; one not understood at {at:#x} (the rest found by their heads)",
                g.spans.len()
            )?,
        }
        let with_record = g.symbols.iter().filter(|x| x.il.is_some()).count();
        writeln!(
            out,
            "  IL units         {} ({with_record} with a record; the rest functions nothing refers to)\n",
            units.len()
        )?;
    }
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
            let listed = g.lib_listed();
            let shown = |i: usize| !o.publics || o.symbols || listed[i];
            let count = (0..g.symbols.len()).filter(|&i| shown(i)).count();
            let what = if o.publics && !o.symbols {
                "Public definitions (as lib.exe lists them)"
            } else {
                "Symbols"
            };
            writeln!(out, "  {what} ({count})")?;
            writeln!(
                out,
                "    {:>6}  {:<8}  {:<9}  {:>6}  flags  {:<20}  name",
                "id", "kind", "storage", "size", "attributes"
            )?;
            let sizes = il_sizes(&obj, g, &units, locals.as_ref());
            for (i, x) in g.symbols.iter().enumerate() {
                if shown(i) {
                    symbol_line(out, x, sizes[i])?;
                }
            }
            writeln!(out)?;
        }
        if o.refs {
            refs(out, g)?;
        }
        if o.records {
            records(out, g)?;
        }
    }
    if o.locals {
        match obj.locals() {
            Ok(l) => locals_dump(out, &l, globals.as_ref(), &units)?,
            Err(e) => return Ok(Err(e.to_string())),
        }
    }
    if o.init {
        match obj.initializers() {
            Ok(i) => init_dump(out, &i, globals.as_ref())?,
            Err(e) => return Ok(Err(e.to_string())),
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

/// Each function's IL size: its unit's, if the units agree with
/// `.cil$sy`; else from its offset to the next record's.
pub(crate) fn il_sizes(
    obj: &LtcgObject<'_>,
    g: &ltcg::Globals<'_>,
    units: &ltcg::IlUnits,
    locals: Option<&ltcg::Locals<'_>>,
) -> Vec<Option<u32>> {
    if locals.is_some_and(|l| units.agrees_with(l)) {
        g.symbols
            .iter()
            .map(|x| units.size_at(x.il?.il_offset))
            .collect()
    } else {
        g.il_sizes(obj.stream(b".cil$ex").map_or(0, <[u8]>::len))
    }
}

/// A symbol's name by its id, if it has a record.
fn name_of(g: Option<&ltcg::Globals<'_>>, id: u32) -> String {
    g.and_then(|g| {
        g.symbols
            .iter()
            .find(|x| x.id == id)
            .map(|x| x.name)
            .or_else(|| g.constants.iter().find(|x| x.id == id).map(|x| x.name))
    })
    .map_or_else(|| format!("symbol {id:X}"), |n| s(n).into_owned())
}

fn hex_bytes(b: &[u8]) -> String {
    b.iter().map(|x| format!("{x:02x}")).collect()
}

/// The other records: sections, namespaces, named types, constants,
/// attributes.
fn records(out: &mut dyn Write, g: &ltcg::Globals<'_>) -> io::Result<()> {
    writeln!(out, "  Sections ({})", g.sections.len())?;
    for x in &g.sections {
        writeln!(
            out,
            "    {:>6X}  {:<12} {:<6} flags {:02X}  characteristics {:08X}",
            x.id,
            s(x.name),
            s(x.class),
            x.flags,
            x.characteristics
        )?;
    }
    writeln!(out, "\n  Namespaces ({})", g.namespaces.len())?;
    for x in &g.namespaces {
        writeln!(out, "    {}", s(x.name))?;
    }
    writeln!(out, "\n  Named types ({})", g.types.len())?;
    for x in &g.types {
        writeln!(
            out,
            "    {:>6X}  {:#06x}  {}",
            x.id,
            x.type_index,
            s(x.name)
        )?;
    }
    writeln!(out, "\n  Constants ({})", g.constants.len())?;
    for x in &g.constants {
        let value = x
            .integer()
            .map_or_else(|| hex_bytes(x.value), |v| v.to_string());
        writeln!(out, "    {:>6X}  {value:<12} {}", x.id, s(x.name))?;
    }
    writeln!(out, "\n  Attributes ({})", g.attributes.len())?;
    for x in &g.attributes {
        let scope = if x.scope.is_empty() {
            String::new()
        } else {
            format!("{}::", s(x.scope))
        };
        let args: Vec<String> = x
            .args
            .iter()
            .map(|(_, a)| format!("\"{}\"", s(a)))
            .collect();
        let args = if args.is_empty() {
            String::new()
        } else {
            format!("({})", args.join(", "))
        };
        let users: Vec<String> = g
            .symbols
            .iter()
            .filter(|y| y.traits.attribute_ids.iter().any(|a| a == x.id))
            .map(|y| s(y.name).into_owned())
            .collect();
        writeln!(
            out,
            "    {:>6X}  [[{scope}{}{args}]]  {}",
            x.id,
            s(x.name),
            users.join(" ")
        )?;
    }
    writeln!(out)
}

/// Each function's scopes and locals (`.cil$sy`).
fn locals_dump(
    out: &mut dyn Write,
    l: &ltcg::Locals<'_>,
    g: Option<&ltcg::Globals<'_>>,
    units: &ltcg::IlUnits,
) -> io::Result<()> {
    use ltcg::{LocalItem, LocalKind};
    let owner = |offset: usize| -> String {
        g.and_then(|g| {
            g.symbols
                .iter()
                .find(|x| x.il.is_some_and(|il| il.symbols_offset as usize == offset))
        })
        .map_or_else(
            || "(no record: a function nothing refers to)".to_string(),
            |x| s(x.name).into_owned(),
        )
    };
    let note = match l.walked {
        Ok(()) => String::new(),
        Err(at) => format!("; a record not understood at {at:#x}"),
    };
    writeln!(
        out,
        "  Local symbols ({} functions{note})",
        l.functions.len()
    )?;
    for (k, f) in l.functions.iter().enumerate() {
        let il = units
            .range(k)
            .map(|(a, b)| format!(", IL at {a:X}, {:X} bytes", b - a))
            .unwrap_or_default();
        writeln!(out, "    {}{il}", owner(f.offset))?;
        for i in &f.items {
            match i {
                LocalItem::Scope(x) if !x.name.is_empty() => {
                    writeln!(out, "      label {}", s(x.name))?;
                }
                LocalItem::Scope(x) => {
                    writeln!(out, "      scope {:X} (kind {:#x})", x.id, x.kind)?;
                }
                LocalItem::Begin(_) => {}
                LocalItem::Local(x) => match x.kind {
                    LocalKind::Variable {
                        class,
                        size,
                        type_index,
                        ..
                    } => writeln!(
                        out,
                        "      {} {} ({size} bytes, type {type_index:#x}, scope {})",
                        if class == 3 { "parameter" } else { "local" },
                        s(x.name),
                        x.scope
                    )?,
                    LocalKind::Unnamed {
                        size, type_index, ..
                    } => writeln!(
                        out,
                        "      temporary ({size} bytes, type {type_index:#x}, scope {})",
                        x.scope
                    )?,
                    LocalKind::Static { size, global, .. } => writeln!(
                        out,
                        "      static {} ({size} bytes, global {global:X})",
                        s(x.name)
                    )?,
                },
                LocalItem::Type(x) => writeln!(out, "      type {}", s(x.name))?,
                LocalItem::Constant(x) => writeln!(out, "      constant {}", s(x.name))?,
            }
        }
    }
    writeln!(out)
}

/// The variables' initial values (`.cil$in`).
fn init_dump(
    out: &mut dyn Write,
    i: &ltcg::Initializers<'_>,
    g: Option<&ltcg::Globals<'_>>,
) -> io::Result<()> {
    use ltcg::InitItem;
    let note = match i.walked {
        Ok(()) => String::new(),
        Err(at) => format!("; an item not understood at {at:#x}"),
    };
    writeln!(out, "  Initializers ({}{note})", i.list.len())?;
    for x in &i.list {
        writeln!(out, "    {} ({} bytes)", name_of(g, x.id), x.size())?;
        for item in &x.items {
            match *item {
                InitItem::Integer { size, .. } => writeln!(
                    out,
                    "      integer {size}: {:#x}",
                    item.integer().unwrap_or(0)
                )?,
                InitItem::Float { size, bytes } => {
                    writeln!(out, "      float {size}: {}", hex_bytes(bytes))?;
                }
                InitItem::Address {
                    target,
                    addend,
                    size,
                } => writeln!(
                    out,
                    "      address {size}: {} + {addend:#x}",
                    name_of(g, target)
                )?,
                InitItem::ImageRelative {
                    target,
                    addend,
                    size,
                } => writeln!(
                    out,
                    "      image-relative {size}: {} + {addend:#x}",
                    name_of(g, target)
                )?,
                InitItem::Bytes(b) => {
                    let text = b.strip_suffix(b"\0").unwrap_or(b);
                    if text.iter().all(|c| (0x20..0x7F).contains(c)) {
                        writeln!(out, "      bytes {}: {:?}", b.len(), s(text))?;
                    } else {
                        writeln!(out, "      bytes {}: {}", b.len(), hex_bytes(b))?;
                    }
                }
                InitItem::Zeros(n) => writeln!(out, "      zeros {n}")?,
            }
        }
    }
    writeln!(out)
}

/// What each function refers to, by name: calls, and uses.
fn refs(out: &mut dyn Write, g: &ltcg::Globals<'_>) -> io::Result<()> {
    let by_id: std::collections::HashMap<u32, &ltcg::IlSymbol<'_>> =
        g.symbols.iter().map(|x| (x.id, x)).collect();
    let with: Vec<_> = g
        .symbols
        .iter()
        .filter(|x| x.il.is_some() || !x.data_refs.is_empty())
        .collect();
    writeln!(out, "  References ({} functions and variables)", with.len())?;
    for x in with {
        writeln!(out, "    {}", s(x.name))?;
        for (id, kind) in x.refs().iter() {
            let target = by_id.get(&id);
            let function = target.is_some_and(|t| matches!(t.kind, SymbolKind::Function { .. }));
            let how = match kind {
                ltcg::RefKind::Direct if function => "calls".to_string(),
                ltcg::RefKind::Direct => "uses".to_string(),
                ltcg::RefKind::Address => "takes the address of".to_string(),
                ltcg::RefKind::Virtual => "calls virtually".to_string(),
                ltcg::RefKind::Other(k) => format!("refers ({k}) to"),
            };
            match target {
                Some(t) => writeln!(out, "      {how} {}", s(t.name))?,
                None => writeln!(out, "      {how} symbol {id:X}")?,
            }
        }
    }
    writeln!(out)
}

fn symbol_line(
    out: &mut dyn Write,
    x: &ltcg::IlSymbol<'_>,
    il_size: Option<u32>,
) -> io::Result<()> {
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
            // (Its IL's size, if it has a body.)
            (
                "function",
                storage,
                il_size.map(|n| format!("{n:X}")).unwrap_or_default(),
            )
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
