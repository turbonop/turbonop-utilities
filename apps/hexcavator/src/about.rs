//! `hexcavator about`: everything known about one function or datum of an
//! image. Where it is (section, COFF group, compiland, library, source
//! line), its size, the names identical code folding merged with it, the
//! functions inlined into it, what its class says of it (virtual, pure,
//! static), and what PGO's training counted of it; or, if it isn't in the
//! image, why: inlined wherever it is called, or never kept.

use std::fmt::Write as _;
use std::path::PathBuf;
use std::process::ExitCode;

use serde_json::{Value, json};
use turbonop_debuginfo::{DebugInfo, MethodKind};
use turbonop_size::Model;
use turbonop_size::symbols::{Symbol, SymbolKind};

pub const USAGE: &str = "\
usage: hexcavator about [options] <image> <name>

Everything known about one function or datum (by its name as the symbols
give it, or without its parameters if just one has that), from the image
and its debug information (a PDB, or DWARF):

- where it is: its section and COFF group (and what each holds), its
  compiland and library, and the source line it is defined at;
- its size, and its code's blocks (a function PGO split);
- the names identical code folding merged with it, or the one it was
  merged into;
- the functions inlined into it, by the bytes their copies take;
- what its class says of it: virtual (introduced or overriding), pure,
  static;
- what PGO's training counted of it (a PDB's: whether PGO optimized it,
  for speed or size, and how often it ran).

A function or datum the image doesn't have is explained: inlined
wherever it is called (its copies, and where), or declared but never
kept (nothing referred to it, and the linker removed it: /OPT:REF,
--gc-sections, -dead_strip; or no code of it was ever compiled in).

options:
  -debug <path>  its debug information: a PDB, or a file with its DWARF
                 (or -pdb; default: its own, or the one it names)
  -json          as JSON";

struct Options {
    image: PathBuf,
    pdb: Option<PathBuf>,
    name: String,
    json: bool,
}

fn parse(args: &[String]) -> Result<Options, String> {
    let mut pdb = None;
    let mut json = false;
    let mut rest = Vec::new();
    let mut it = args.iter();
    while let Some(a) = it.next() {
        match a.as_str() {
            "-pdb" | "/pdb" | "-debug" | "/debug" => {
                pdb = Some(PathBuf::from(it.next().ok_or("-debug needs a path")?));
            }
            "-json" | "/json" => json = true,
            "--" => rest.extend(it.by_ref().cloned()),
            _ => rest.push(a.clone()),
        }
    }
    let [image, name] =
        <[String; 2]>::try_from(rest).map_err(|_| "give an image and a name".to_string())?;
    Ok(Options {
        image: image.into(),
        pdb,
        name,
        json,
    })
}

pub fn run(args: &[String]) -> ExitCode {
    let o = match parse(args) {
        Ok(o) => o,
        Err(e) => {
            ereport!("hexcavator about: {e}\n\n{USAGE}");
            return ExitCode::from(2);
        }
    };
    match about(&o) {
        Ok(text) => crate::out::print(text.as_bytes()),
        Err(e) => {
            ereport!("hexcavator about: {e}");
            ExitCode::FAILURE
        }
    }
}

/// Whether symbols of `kind` are code.
fn is_code(kind: SymbolKind) -> bool {
    matches!(
        kind,
        SymbolKind::Function
            | SymbolKind::PrimaryBlock
            | SymbolKind::SeparatedBlock
            | SymbolKind::Thunk
    )
}

/// `name` without its parameters.
fn bare(name: &str) -> &str {
    turbonop_size::findings::strip_parameters(name).trim()
}

/// The symbols named `name` (or, if none is, the one whose name without
/// its parameters is, if just one): its main symbol first.
fn find<'s>(symbols: &'s [Symbol], name: &str) -> Vec<&'s Symbol> {
    let mut found: Vec<&Symbol> = symbols.iter().filter(|s| s.name == name).collect();
    if found.is_empty() {
        let mut names: Vec<&str> = symbols
            .iter()
            .filter(|s| bare(&s.name) == name)
            .map(|s| s.name.as_str())
            .collect();
        names.sort_unstable();
        names.dedup();
        if let [one] = names[..] {
            found = symbols.iter().filter(|s| s.name == one).collect();
        }
    }
    // The main symbol (not a separated block, not folded) first.
    found.sort_by_key(|s| (s.kind == SymbolKind::SeparatedBlock, s.folded, s.addr));
    found
}

/// Where `addr` is: its section, COFF group, compiland, and library.
fn placement(model: &Model, addr: u64) -> Value {
    let i = model.pieces.partition_point(|p| p.range.addr <= addr);
    let piece = i
        .checked_sub(1)
        .map(|j| &model.pieces[j])
        .filter(|p| addr < p.range.addr.saturating_add(p.range.len.max(1)));
    let section = model
        .sections
        .iter()
        .rfind(|s| s.addr <= addr && addr < s.addr.saturating_add(s.size.vm.max(s.size.file)));
    let group = piece
        .and_then(|p| model.groups.get(p.group as usize))
        .or_else(|| {
            model.groups.iter().rfind(|g| {
                g.addr <= addr && addr < g.addr.saturating_add(g.size.vm.max(g.size.file))
            })
        });
    let compiland = piece.and_then(|p| model.compilands.get(p.compiland as usize));
    // (An object linked by itself is its own library: not said twice.)
    let library = compiland
        .and_then(|c| model.libraries.get(c.library as usize))
        .filter(|l| compiland.is_none_or(|c| c.name != l.name));
    json!({
        "section": section.map(|s| s.name.as_str()),
        "group": group.map(|g| g.name.as_str()),
        "group_description": group.and_then(|g| turbonop_size::describe::group(&g.name)),
        "compiland": compiland.map(|c| c.name.as_str()).filter(|n| !n.is_empty()),
        "library": library.map(|l| l.name.as_str()).filter(|n| !n.is_empty()),
    })
}

/// The source line the code at `addr` starts at, by the line tables.
fn source_line(
    debug: &DebugInfo<'_>,
    files: &[&std::path::Path],
    addr: u64,
) -> Option<(String, u32)> {
    let lines =
        turbonop_program::cache::remember(files, "source lines", || debug.source_lines()).ok()?;
    let i = lines.spans.partition_point(|s| s.addr <= addr);
    let span = i
        .checked_sub(1)
        .map(|j| &lines.spans[j])
        .filter(|s| addr < s.addr.saturating_add(s.size.max(1)))?;
    Some((lines.files.get(span.file as usize)?.clone(), span.line))
}

/// The functions inlined into the function at `addr` (its inline sites,
/// at any depth), by inlinee: copies, bytes of their own (not those of
/// functions inlined into them), and the depth of the shallowest.
fn inlined_into(debug: &DebugInfo<'_>, addr: u64) -> Vec<(String, usize, u64, u32)> {
    let Ok(all) = debug.inline_sites() else {
        return Vec::new();
    };
    let sites: Vec<turbonop_debuginfo::InlineSite> = all
        .sites
        .iter()
        .filter(|s| s.function == addr)
        .cloned()
        .collect();
    let own = turbonop_debuginfo::exclusive_sizes_by_index(&sites);
    let mut by: std::collections::BTreeMap<u32, (usize, u64, u32)> =
        std::collections::BTreeMap::new();
    for (s, own) in sites.iter().zip(own) {
        let e = by.entry(s.inlinee).or_insert((0, 0, u32::MAX));
        e.0 += 1;
        e.1 += own.unwrap_or(0);
        e.2 = e.2.min(s.depth);
    }
    let mut out: Vec<(String, usize, u64, u32)> = by
        .into_iter()
        .map(|(i, (n, size, depth))| {
            let name = all.inlinees.get(i as usize).cloned().unwrap_or_default();
            (name, n, size, depth)
        })
        .collect();
    out.sort_by(|a, b| b.2.cmp(&a.2).then_with(|| a.0.cmp(&b.0)));
    out
}

/// The copies of `name` inlined into other functions: how many, the
/// functions they are in (by address), and their bytes (their own).
fn inlined_copies(debug: &DebugInfo<'_>, name: &str) -> Option<(usize, Vec<u64>, u64)> {
    let all = debug.inline_sites().ok()?;
    let wanted: Vec<u32> = all
        .inlinees
        .iter()
        .enumerate()
        .filter(|(_, n)| n.as_str() == name || bare(n) == name)
        .filter_map(|(i, _)| u32::try_from(i).ok())
        .collect();
    if wanted.is_empty() {
        return None;
    }
    let own = turbonop_debuginfo::exclusive_sizes_by_index(&all.sites);
    let mut copies = 0;
    let mut size = 0;
    let mut functions = std::collections::BTreeSet::new();
    for (s, own) in all.sites.iter().zip(own) {
        if wanted.contains(&s.inlinee) {
            copies += 1;
            size += own.unwrap_or(0);
            functions.insert(s.function);
        }
    }
    (copies > 0).then_some((copies, functions.into_iter().collect(), size))
}

/// What the class of `name` (a member function) says of it: the class,
/// and how it is declared, if one class declares it.
fn method_of(debug: &DebugInfo<'_>, name: &str) -> Option<(String, Vec<MethodKind>)> {
    let classes = debug.classes().ok()?;
    let by_name: std::collections::HashMap<String, &turbonop_debuginfo::Class> = classes
        .iter()
        .map(|c| (turbonop_size::members::normalize(&c.name), c))
        .collect();
    let scopes = turbonop_size::members::scopes(name);
    let class = scopes
        .iter()
        .find_map(|s| by_name.get(&turbonop_size::members::normalize(s)))?;
    let unqualified = bare(name);
    let short = scopes
        .first()
        .and_then(|s| unqualified.strip_prefix(*s))
        .and_then(|r| r.strip_prefix("::"))
        .unwrap_or(unqualified);
    let mut kinds: Vec<MethodKind> = class
        .methods
        .iter()
        .filter(|m| m.name == short)
        .map(|m| m.kind)
        .collect();
    kinds.dedup();
    Some((class.name.clone(), kinds))
}

fn kind_words(kind: MethodKind) -> &'static str {
    match kind {
        MethodKind::Plain => "not virtual",
        MethodKind::Virtual => "virtual, overriding a base's",
        MethodKind::IntroVirtual => "virtual, introduced here",
        MethodKind::PureVirtual => "pure virtual, overriding a base's",
        MethodKind::PureIntroVirtual => "pure virtual, introduced here",
        MethodKind::Static => "static",
        MethodKind::Friend => "a friend",
        _ => "declared",
    }
}

/// Up to `n` names that have `name` in them (any case), for a name not
/// found.
fn similar<'s>(symbols: &'s [Symbol], name: &str, n: usize) -> Vec<&'s str> {
    let matching = |needle: &str| -> Vec<&'s str> {
        let needle = needle.to_lowercase();
        symbols
            .iter()
            .filter(|s| !s.folded && (is_code(s.kind) || s.kind == SymbolKind::Data))
            .filter(|s| s.name.to_lowercase().contains(&needle))
            .map(|s| s.name.as_str())
            .collect()
    };
    // The whole name, else its last part (`_Emplace_reallocate` of a
    // template's member, its arguments spelled otherwise).
    let mut out = matching(bare(name));
    if out.is_empty() {
        let unqualified = bare(name);
        let last = turbonop_size::members::scopes(name)
            .first()
            .and_then(|s| unqualified.strip_prefix(*s))
            .map_or(unqualified, |r| r.trim_start_matches(':'));
        let last = last.split('<').next().unwrap_or(last);
        if last.len() >= 3 {
            out = matching(last);
        }
    }
    out.sort_unstable();
    out.dedup();
    out.truncate(n);
    out
}

fn about(o: &Options) -> Result<String, String> {
    let (text, v) = report(&o.image, o.pdb.as_deref(), &o.name)?;
    Ok(if o.json {
        let mut s = serde_json::to_string_pretty(&v).unwrap_or_default();
        s.push('\n');
        s
    } else {
        text
    })
}

/// What is known of `name` in the image at `image`: as text, and as JSON
/// (the MCP server's `about` too).
pub(crate) fn report(
    image: &std::path::Path,
    pdb: Option<&std::path::Path>,
    name: &str,
) -> Result<(String, Value), String> {
    let opened = turbonop_program::Opened::open(image, pdb)?;
    let program = opened.program()?;
    let symbols = program.symbols()?;
    let model = program.model()?;
    let debug = &program.debug;
    let mut files = vec![opened.path(), opened.debug_path()];
    files.extend(opened.split_paths());
    let found = find(symbols, name);
    let v = match found.first() {
        Some(&main) => present(&found, main, symbols, model, debug, &files),
        None => missing(name, symbols, debug),
    };
    Ok((text(&v), v))
}

/// What is known of a symbol the image has.
fn present(
    found: &[&Symbol],
    main: &Symbol,
    symbols: &[Symbol],
    model: &Model,
    debug: &DebugInfo<'_>,
    files: &[&std::path::Path],
) -> Value {
    let code = is_code(main.kind);
    // Identical code folding: the symbol whose bytes these are, and the
    // other names at its address (of its kind: code with code, data with
    // data).
    let at: Vec<&Symbol> = symbols
        .iter()
        .filter(|s| s.addr == main.addr && is_code(s.kind) == code && s.name != main.name)
        .filter(|s| !matches!(s.kind, SymbolKind::SeparatedBlock))
        .collect();
    let folded_into = main
        .folded
        .then(|| at.iter().find(|s| !s.folded).map(|s| s.name.clone()))
        .flatten();
    let folded: Vec<&str> = if main.folded {
        Vec::new()
    } else {
        at.iter()
            .filter(|s| s.folded)
            .map(|s| s.name.as_str())
            .collect()
    };
    // Its bytes (a folded name has none of its own: its canonical's).
    let size = found
        .iter()
        .filter(|s| !s.folded)
        .fold(turbonop_size::Size::default(), |a, s| a + s.size);
    let blocks: Vec<Value> = found
        .iter()
        .filter(|s| {
            matches!(
                s.kind,
                SymbolKind::PrimaryBlock | SymbolKind::SeparatedBlock
            )
        })
        .map(|s| {
            json!({
                "block": if s.kind == SymbolKind::PrimaryBlock { "primary" } else { "separated" },
                "rva": format!("0x{:x}", s.addr),
                "file": s.size.file,
                "vm": s.size.vm,
                "placement": placement(model, s.addr),
            })
        })
        .collect();
    let source = source_line(debug, files, main.addr);
    let inlined: Vec<Value> = if code {
        inlined_into(debug, main.addr)
            .into_iter()
            .map(|(name, copies, size, depth)| json!({ "name": name, "copies": copies, "size": size, "depth": depth }))
            .collect()
    } else {
        Vec::new()
    };
    let method = code.then(|| method_of(debug, &main.name)).flatten();
    let frame = code
        .then(|| debug.variables(main.compiland, main.addr).ok().flatten())
        .flatten();
    let pgo = frame
        .as_ref()
        .filter(|_| matches!(debug, DebugInfo::Pdb(_)))
        .map(|f| {
            json!({
                "optimized": f.pgo_optimized(),
                "counts_valid": f.pgo_counts_valid(),
                "optimized_for_speed": f.optimized_for_speed(),
                "invocations": f.pgo.map(|p| p.invocations),
                "dynamic_instructions": f.pgo.map(|p| p.dynamic_instructions),
                "instructions": f.pgo.map(|p| p.instructions),
                "live_instructions": f.pgo.map(|p| p.live_instructions),
            })
        });
    let mut v = json!({
        "name": main.name,
        "found": true,
        "kind": main.kind.label(),
        "rva": format!("0x{:x}", main.addr),
        "file": size.file,
        "vm": size.vm,
        "placement": placement(model, main.addr),
        "source": source.map(|(file, line)| json!({ "file": file, "line": line })),
        "folded_into": folded_into,
        "folded": folded,
        "blocks": blocks,
        "inlined": inlined,
    });
    if let Some((class, kinds)) = method {
        v["member_of"] = json!(class);
        v["declared"] = json!(kinds.iter().map(|k| kind_words(*k)).collect::<Vec<_>>());
    }
    if let Some(p) = pgo {
        v["pgo"] = p;
    }
    v
}

/// Why a name the image doesn't have isn't there.
fn missing(name: &str, symbols: &[Symbol], debug: &DebugInfo<'_>) -> Value {
    let inlined = inlined_copies(debug, name);
    let method = method_of(debug, name).filter(|(_, kinds)| !kinds.is_empty());
    let plural =
        |n: usize, one: &str, many: &str| format!("{n} {}", if n == 1 { one } else { many });
    let reason = if let Some((copies, functions, size)) = &inlined {
        format!(
            "inlined wherever it is called: {}, in {}, {size} bytes in all; no copy of its own",
            plural(*copies, "inlined copy", "inlined copies"),
            plural(functions.len(), "function", "functions")
        )
    } else if method.is_some() {
        "declared, with no code in the image: nothing called it, so the linker removed it (/OPT:REF, --gc-sections, -dead_strip), or no code of it was ever compiled in".to_string()
    } else {
        "neither it nor a declaration of it is in the debug information: never compiled in, or removed by the linker as nothing referred to it (/OPT:REF and /Gw, --gc-sections, -dead_strip)".to_string()
    };
    // (Names like it only where nothing explains it.)
    let explained = inlined.is_some() || method.is_some();
    let mut v = json!({
        "name": name,
        "found": false,
        "reason": reason,
        "similar": if explained { Vec::new() } else { similar(symbols, name, 8) },
    });
    if let Some((copies, functions, size)) = inlined {
        // The functions its copies are in, by name.
        let mut into: Vec<&str> = functions
            .iter()
            .filter_map(|&a| {
                symbols
                    .iter()
                    .find(|s| s.addr == a && !s.folded && is_code(s.kind))
                    .map(|s| s.name.as_str())
            })
            .collect();
        into.sort_unstable();
        into.dedup();
        v["inlined_copies"] =
            json!({ "copies": copies, "functions": functions.len(), "size": size, "into": into });
    }
    if let Some((class, kinds)) = method {
        v["member_of"] = json!(class);
        v["declared"] = json!(kinds.iter().map(|k| kind_words(*k)).collect::<Vec<_>>());
    }
    v
}

/// The JSON as text.
fn text(v: &Value) -> String {
    let mut out = String::new();
    let s = |k: &str| v.get(k).and_then(Value::as_str);
    let n = |k: &str| v.get(k).and_then(Value::as_u64).unwrap_or(0);
    let _ = writeln!(out, "{}", s("name").unwrap_or(""));
    if v["found"] == false {
        let _ = writeln!(out, "Not in the image: {}.", s("reason").unwrap_or(""));
        let into: Vec<&str> = v["inlined_copies"]["into"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(Value::as_str)
            .collect();
        if !into.is_empty() {
            let _ = writeln!(out, "Inlined into: {}", into.join("; "));
        }
        if let Some(class) = s("member_of") {
            let _ = writeln!(out, "Declared by {class}: {}.", words(&v["declared"]));
        }
        let similar: Vec<&str> = v["similar"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(Value::as_str)
            .collect();
        if !similar.is_empty() {
            let _ = writeln!(out, "Names like it: {}", similar.join("; "));
        }
        return out;
    }
    let p = &v["placement"];
    let ps = |k: &str| p.get(k).and_then(Value::as_str);
    let _ = writeln!(
        out,
        "{} at {}: {} bytes in the file, {} in memory",
        s("kind").unwrap_or(""),
        s("rva").unwrap_or(""),
        n("file"),
        n("vm")
    );
    if let Some(into) = s("folded_into") {
        let _ = writeln!(
            out,
            "Folded into {into} (identical code folding): its bytes are that one's."
        );
    }
    let _ = writeln!(
        out,
        "In {}{}{}",
        ps("section").unwrap_or("?"),
        ps("group")
            .map(|g| format!(", COFF group {g}"))
            .unwrap_or_default(),
        ps("group_description")
            .map(|d| format!(" ({d})"))
            .unwrap_or_default()
    );
    if let Some(c) = ps("compiland") {
        let _ = writeln!(
            out,
            "From {c}{}",
            ps("library")
                .map(|l| format!(", of {l}"))
                .unwrap_or_default()
        );
    }
    if let Some(src) = v.get("source").filter(|x| !x.is_null()) {
        let _ = writeln!(
            out,
            "Defined at {}:{}",
            src["file"].as_str().unwrap_or(""),
            src["line"].as_u64().unwrap_or(0)
        );
    }
    if let Some(class) = s("member_of") {
        let _ = writeln!(out, "A member of {class}: {}.", words(&v["declared"]));
    }
    if let Some(pgo) = v.get("pgo") {
        let speed = if pgo["optimized_for_speed"] == true {
            "speed"
        } else {
            "size"
        };
        let counts = match (
            pgo["invocations"].as_u64(),
            pgo["dynamic_instructions"].as_u64(),
        ) {
            (Some(i), Some(d)) => format!("called {i} times in training, {d} instructions run"),
            _ => "never run in training".into(),
        };
        if pgo["optimized"] == true {
            let _ = writeln!(out, "Optimized by PGO, for {speed}: {counts}.");
        } else if pgo["counts_valid"] == true {
            // (No S_POGODATA: what training counted isn't recorded.)
            let counts = if pgo["invocations"].is_null() {
                "its counts not recorded".into()
            } else {
                counts
            };
            let _ = writeln!(
                out,
                "Seen by PGO's training ({counts}), but not optimized with its profile; compiled for {speed}."
            );
        } else {
            let _ = writeln!(out, "Compiled for {speed}, without PGO.");
        }
    }
    let folded: Vec<&str> = v["folded"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .collect();
    if !folded.is_empty() {
        let _ = writeln!(
            out,
            "\nFolded into it (identical code folding), {}:",
            folded.len()
        );
        for f in folded {
            let _ = writeln!(out, "  {f}");
        }
    }
    let blocks = v["blocks"].as_array().cloned().unwrap_or_default();
    if blocks.len() > 1 {
        let _ = writeln!(out, "\nIts code's blocks (split by PGO):");
        for b in &blocks {
            let _ = writeln!(
                out,
                "  {:<9} {} {:>8} bytes  {}",
                b["block"].as_str().unwrap_or(""),
                b["rva"].as_str().unwrap_or(""),
                b["file"].as_u64().unwrap_or(0),
                b["placement"]["group"].as_str().unwrap_or("")
            );
        }
    }
    let inlined = v["inlined"].as_array().cloned().unwrap_or_default();
    if !inlined.is_empty() {
        let total: u64 = inlined.iter().filter_map(|i| i["size"].as_u64()).sum();
        let _ = writeln!(
            out,
            "\nInlined into it, {} functions, {total} bytes of its code:",
            inlined.len()
        );
        let _ = writeln!(out, "{:>8} {:>6}  Function", "Bytes", "Copies");
        for i in &inlined {
            let _ = writeln!(
                out,
                "{:>8} {:>6}  {}{}",
                i["size"].as_u64().unwrap_or(0),
                i["copies"].as_u64().unwrap_or(0),
                i["name"].as_str().unwrap_or(""),
                if i["depth"].as_u64().unwrap_or(0) > 0 {
                    "  (inside another inlined function)"
                } else {
                    ""
                }
            );
        }
    }
    out
}

fn words(v: &Value) -> String {
    let w: Vec<&str> = v
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .collect();
    if w.is_empty() {
        "declared".into()
    } else {
        w.join("; ")
    }
}
