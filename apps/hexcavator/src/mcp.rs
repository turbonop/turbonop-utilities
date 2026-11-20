//! `hexc mcp`: a Model Context Protocol server over stdio, so that LLM
//! agents can ask what the bytes of a binary are.
//!
//! Messages are JSON-RPC 2.0, one per line. The tools (see [`tools`]) read
//! PE, ELF, Mach-O, and WebAssembly files alike, with their debug
//! information (a PDB, or DWARF), opened as `hexc`'s commands open them.
//!
//! Loaded binaries are cached for the session: the first question about a
//! large image reads its debug information; later ones are fast.

use std::cell::{OnceCell, RefCell};
use std::collections::HashMap;
use std::io::{self, BufRead, Write};
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::rc::Rc;
use std::sync::Arc;

use hexcavator_image::Image;
use serde_json::Value;

/// An object from members, in order.
fn object<K: Into<String>>(members: impl IntoIterator<Item = (K, Value)>) -> Value {
    Value::Object(members.into_iter().map(|(k, v)| (k.into(), v)).collect())
}
use hexcavator_mmap::Mmap;
use hexcavator_size::Model;
use hexcavator_size::rollup::{self, Domain, Report, Rollup, SortBy};
use hexcavator_size::sources::{Maps, Source};
use hexcavator_size::symbols::Symbol;

pub const USAGE: &str = "\
usage: hexc mcp

Serves the Model Context Protocol on stdin and stdout (JSON-RPC 2.0, one
message per line), with tools that report what the bytes of an executable,
library, or object (PE, ELF, Mach-O, WebAssembly) are, and why: size_report,
symbols, findings, linker_log, query, why, recommend, dependencies,
symbolize, initialization, resources, and lookup.";

/// The protocol versions this server speaks, newest first.
const PROTOCOL_VERSIONS: &[&str] = &["2025-06-18", "2025-03-26", "2024-11-05"];

/// A binary, read once per session: opened as every `hexc` command opens
/// one (see [`hexcavator_program::Opened`]), whatever its format.
struct Loaded {
    path: PathBuf,
    map: Mmap,
    /// An object or library (no address space: no program to analyze).
    object: bool,
    /// A PE image (whose COFF groups are a source).
    pe: bool,
    /// The debug information given (`pdb`, or `debug`), if any.
    debug_arg: Option<PathBuf>,
    /// The image and its debug information, opened when first needed.
    opened: OnceCell<Result<hexcavator_program::Opened, String>>,
    /// The size model and symbols, built when first needed.
    built: OnceCell<Result<(Arc<Model>, Arc<Vec<Symbol>>), String>>,
    /// The cross-reference graph, built once.
    xref: OnceCell<Result<hexcavator_size::xref::Xref, String>>,
    /// Maps built so far, by their sources.
    maps: RefCell<HashMap<Vec<Source>, Arc<Maps>>>,
}

impl Loaded {
    fn open(path: &Path, debug: Option<&Path>) -> Result<Self, String> {
        let map = Mmap::open(path).map_err(|e| format!("{}: {e}", path.display()))?;
        let image = Image::parse(&map).ok();
        let object = image
            .as_ref()
            .is_none_or(|i| i.kind() != hexcavator_image::Kind::Linked);
        let pe = image.as_ref().is_some_and(|i| i.pe().is_some());
        Ok(Self {
            path: path.to_path_buf(),
            map,
            object,
            pe,
            debug_arg: debug.map(Path::to_path_buf),
            opened: OnceCell::new(),
            built: OnceCell::new(),
            xref: OnceCell::new(),
            maps: RefCell::default(),
        })
    }

    /// The image and its debug information, of any format, opened once.
    fn opened(&self) -> Result<&hexcavator_program::Opened, String> {
        self.opened
            .get_or_init(|| {
                if self.object {
                    return Err(format!(
                        "{}: an object or library: only size_report reads it",
                        self.path.display()
                    ));
                }
                hexcavator_program::Opened::open(&self.path, self.debug_arg.as_deref())
                    .map_err(|e| match e {
                        hexcavator_program::Error::NoDebug { .. } => format!(
                            "{e}; give the debug information (a PDB, a dSYM, or a DWARF file) with the `debug` argument"
                        ),
                        e => e.to_string(),
                    })
            })
            .as_ref()
            .map_err(Clone::clone)
    }

    /// Whether its debug information was found.
    fn has_debug(&self) -> bool {
        self.opened().is_ok()
    }

    /// The program: the image and its debug information, parsed.
    fn program(&self) -> Result<hexcavator_program::Program<'_>, String> {
        self.opened()?.program().map_err(|e| e.to_string())
    }

    fn built(&self) -> Result<&(Arc<Model>, Arc<Vec<Symbol>>), String> {
        self.built
            .get_or_init(|| self.program()?.into_built().map_err(|e| e.to_string()))
            .as_ref()
            .map_err(Clone::clone)
    }

    fn model(&self) -> Result<&Model, String> {
        Ok(&self.built()?.0)
    }

    fn symbols(&self) -> Result<&[Symbol], String> {
        Ok(&self.built()?.1)
    }

    /// The cross-reference graph of the image, built once.
    fn xref(&self) -> Result<&hexcavator_size::xref::Xref, String> {
        self.xref
            .get_or_init(|| {
                let program = self.program()?;
                hexcavator_size::xref::build(
                    &program.image,
                    &program.debug,
                    self.model()?,
                    self.symbols()?,
                )
            })
            .as_ref()
            .map_err(Clone::clone)
    }

    /// The maps for `list` (as `hexc size` builds them), built once.
    fn maps(&self, list: &[Source]) -> Result<Arc<Maps>, String> {
        if let Some(m) = self.maps.borrow().get(list) {
            return Ok(Arc::clone(m));
        }
        let m = crate::size::maps_of(
            &self.path,
            self.debug_arg.as_deref(),
            None,
            None,
            list,
            false,
        )?;
        self.maps.borrow_mut().insert(list.to_vec(), Arc::clone(&m));
        Ok(m)
    }
}

#[derive(Default)]
struct Server {
    cache: HashMap<(PathBuf, Option<PathBuf>), Rc<Loaded>>,
}

/// A tool's failure, reported to the client as a tool result.
type ToolResult = Result<(String, Value), String>;

fn arg_str<'a>(args: &'a Value, name: &str) -> Option<&'a str> {
    args.get(name).and_then(Value::as_str)
}

fn arg_path(args: &Value, name: &str) -> Option<PathBuf> {
    arg_str(args, name).map(PathBuf::from)
}

/// Sources from an array of names or a comma-separated string.
fn arg_sources(args: &Value, default: &[Source]) -> Result<Vec<Source>, String> {
    let names: Vec<String> = match args.get("sources") {
        None | Some(Value::Null) => return Ok(default.to_vec()),
        Some(Value::String(s)) => s.split(',').map(|s| s.trim().to_string()).collect(),
        Some(Value::Array(a)) => a
            .iter()
            .filter_map(|v| v.as_str().map(str::to_string))
            .collect(),
        Some(_) => return Err("`sources` must be an array of names".into()),
    };
    names
        .iter()
        .filter(|n| !n.is_empty())
        .map(|n| Source::parse(n).ok_or(format!("unknown data source '{n}'")))
        .collect()
}

fn parse_address(v: &Value) -> Option<u64> {
    match v {
        Value::String(s) => {
            let s = s.trim();
            match s.strip_prefix("0x").or_else(|| s.strip_prefix("0X")) {
                Some(h) => u64::from_str_radix(h, 16).ok(),
                None => s.parse().ok().or_else(|| u64::from_str_radix(s, 16).ok()),
            }
        }
        other => other.as_u64(),
    }
}

impl Server {
    fn load(&mut self, path: &Path, pdb: Option<&Path>) -> Result<Rc<Loaded>, String> {
        let key = (path.to_path_buf(), pdb.map(Path::to_path_buf));
        if let Some(l) = self.cache.get(&key) {
            return Ok(Rc::clone(l));
        }
        let l = Rc::new(Loaded::open(path, pdb)?);
        self.cache.insert(key, Rc::clone(&l));
        Ok(l)
    }

    fn loaded_from(&mut self, args: &Value) -> Result<Rc<Loaded>, String> {
        let path = arg_path(args, "path").ok_or("`path` is required")?;
        let debug = arg_path(args, "debug").or_else(|| arg_path(args, "pdb"));
        self.load(&path, debug.as_deref())
    }

    fn size_report(&mut self, args: &Value) -> ToolResult {
        let loaded = self.loaded_from(args)?;
        let list = arg_sources(args, &[Source::Sections])?;
        let rows = args.get("rows").and_then(Value::as_u64).unwrap_or(20);
        let options = rollup::Options {
            max_rows: if rows == 0 {
                usize::MAX
            } else {
                usize::try_from(rows).unwrap_or(usize::MAX)
            },
            min: rollup::Threshold::None,
            sort_by: match arg_str(args, "sort") {
                Some("vm") => SortBy::Vm,
                Some("file") => SortBy::File,
                _ => SortBy::Both,
            },
            domain: match arg_str(args, "domain") {
                Some("vm") => Domain::Vm,
                Some("file") => Domain::File,
                _ => Domain::Both,
            },
        };
        let filter = arg_str(args, "source_filter")
            .map(|f| regex::Regex::new(f).map_err(|e| format!("source_filter: {e}")))
            .transpose()?;
        let rollup = Rollup::filtered(&*loaded.maps(&list)?, filter.as_ref());
        let base = match arg_path(args, "base") {
            Some(b) => {
                let base_pdb = arg_path(args, "base_debug").or_else(|| arg_path(args, "base_pdb"));
                let l = self.load(&b, base_pdb.as_deref())?;
                Some(Rollup::filtered(&*l.maps(&list)?, filter.as_ref()))
            }
            None => None,
        };
        let names: Vec<&str> = list.iter().map(|s| s.name()).collect();
        let report = Report::new(&names, &rollup, base.as_ref(), &options);
        let structured =
            serde_json::from_str::<Value>(&report.json()).map_err(|e| e.to_string())?;
        Ok((report.pretty(&options), structured))
    }

    fn symbols(&mut self, args: &Value) -> ToolResult {
        let loaded = self.loaded_from(args)?;
        let limit = args
            .get("limit")
            .and_then(Value::as_u64)
            .map_or(50, |l| usize::try_from(l).unwrap_or(usize::MAX));
        let query = arg_str(args, "query").map(str::to_lowercase);
        let matches = |name: &str| {
            query
                .as_ref()
                .is_none_or(|q| name.to_lowercase().contains(q))
        };
        if !loaded.has_debug() {
            // Objects, and images without debug information: the symbols
            // source's labels.
            let maps = loaded.maps(&[Source::Symbols])?;
            let rollup = Rollup::new(&maps);
            let mut rows: Vec<(&str, i64, i64)> = rollup
                .children
                .iter()
                .filter(|(k, _)| matches(k))
                .map(|(k, r)| (&**k, r.file, r.vm))
                .collect();
            rows.sort_by(|a, b| b.1.max(b.2).cmp(&a.1.max(a.2)).then_with(|| a.0.cmp(b.0)));
            let total = rows.len();
            let list: Vec<Value> = rows
                .iter()
                .take(limit)
                .map(|(n, file, vm)| {
                    object([
                        ("name", Value::from(*n)),
                        ("file_size", Value::from(*file)),
                        ("vm_size", Value::from(*vm)),
                    ])
                })
                .collect();
            let text = rows
                .iter()
                .take(limit)
                .map(|(n, f, v)| format!("{f:>10} {v:>10}  {n}"))
                .collect::<Vec<_>>()
                .join("\n");
            return Ok((
                format!("{total} symbols match; file and memory sizes:\n{text}"),
                object([
                    ("matched", Value::from(total)),
                    ("symbols", Value::Array(list)),
                ]),
            ));
        }
        let all = loaded.symbols()?;
        let model = loaded.model()?;
        let kind = arg_str(args, "kind").map(str::to_lowercase);
        let section = arg_str(args, "section");
        let folded = args
            .get("include_folded")
            .and_then(Value::as_bool)
            .unwrap_or(false);
        let section_of = |addr: u64| {
            model
                .sections
                .iter()
                .find(|s| addr >= s.addr && addr < s.addr + s.size.file.max(s.size.vm))
                .map_or("", |s| s.name.as_str())
        };
        let mut found: Vec<&Symbol> = all
            .iter()
            .filter(|s| folded || !s.folded)
            .filter(|s| matches(&s.name))
            .filter(|s| {
                kind.as_ref().is_none_or(|k| {
                    let name = s.kind.sizebench_name().to_lowercase();
                    name == *k
                        || name.starts_with(k.as_str())
                        || format!("{:?}", s.kind).to_lowercase() == *k
                })
            })
            .filter(|s| section.is_none_or(|n| section_of(s.addr) == n))
            .collect();
        match arg_str(args, "sort") {
            Some("address") => found.sort_by_key(|s| s.addr),
            Some("name") => found.sort_by(|a, b| a.name.cmp(&b.name)),
            _ => found.sort_by(|a, b| {
                b.size
                    .vm
                    .max(b.size.file)
                    .cmp(&a.size.vm.max(a.size.file))
                    .then_with(|| a.addr.cmp(&b.addr))
            }),
        }
        let total = found.len();
        let total_size: u64 = found.iter().map(|s| s.size.file).sum();
        let rows: Vec<Value> = found
            .iter()
            .take(limit)
            .map(|s| {
                object([
                    ("name", Value::from(s.name.as_str())),
                    ("kind", Value::from(s.kind.sizebench_name())),
                    ("address", Value::from(format!("0x{:x}", s.addr))),
                    ("section", Value::from(section_of(s.addr))),
                    ("file_size", Value::from(s.size.file)),
                    ("vm_size", Value::from(s.size.vm)),
                    ("folded", Value::from(s.folded)),
                ])
            })
            .collect();
        let text = found
            .iter()
            .take(limit)
            .map(|s| {
                format!(
                    "{:>10} {:>10}  {:08x}  {}",
                    s.size.file, s.size.vm, s.addr, s.name
                )
            })
            .collect::<Vec<_>>()
            .join("\n");
        Ok((
            format!(
                "{total} symbols match ({total_size} file bytes); file size, memory size, address, name:\n{text}"
            ),
            object([
                ("matched", Value::from(total)),
                ("matched_file_size", Value::from(total_size)),
                ("symbols", Value::Array(rows)),
            ]),
        ))
    }

    fn why(&mut self, args: &Value) -> ToolResult {
        let name = arg_str(args, "name")
            .ok_or("`name` is required")?
            .to_string();
        let loaded = self.loaded_from(args)?;
        let x = loaded.xref().map_err(|e| format!("why: {e}"))?;
        let n = crate::xref::lookup(x, &name)?;
        Ok((crate::xref::why_text(x, n), crate::xref::why_json(x, n)))
    }

    fn recommend(&mut self, args: &Value) -> ToolResult {
        let loaded = self.loaded_from(args)?;
        let importers: Vec<PathBuf> = match args.get("importers") {
            None | Some(Value::Null) => Vec::new(),
            Some(Value::Array(a)) => a
                .iter()
                .filter_map(|v| v.as_str().map(PathBuf::from))
                .collect(),
            Some(Value::String(s)) => vec![PathBuf::from(s)],
            Some(_) => return Err("`importers` must be an array of paths".into()),
        };
        let mut r = if loaded.has_debug() || loaded.debug_arg.is_some() {
            let program = loaded.program().map_err(|e| format!("recommend: {e}"))?;
            let x = loaded.xref()?;
            crate::recommend::recommendations(
                &program.image,
                &program.debug,
                loaded.model()?,
                x,
                loaded.symbols()?,
            )
        } else {
            // Without debug information: what the image's structure shows,
            // as `hexc recommend` gives it.
            let image = Image::parse(&loaded.map).map_err(|e| format!("recommend: {e}"))?;
            crate::recommend::structural_only(&image)
        };
        let image = Image::parse(&loaded.map).map_err(|e| e.to_string())?;
        crate::recommend::with_exports(&mut r, &image, &loaded.path, &importers);
        let structured =
            serde_json::from_str(&crate::recommend::as_json(&r)).map_err(|e| e.to_string())?;
        Ok((crate::recommend::as_text(&r), structured))
    }

    fn query(&mut self, args: &Value) -> ToolResult {
        use hexcavator_size::table::{COLUMNS, SymbolTable};
        let text = arg_str(args, "query").unwrap_or("");
        let mut query = hexcavator_query::parse(text, &COLUMNS).map_err(|e| e.to_string())?;
        if query.limit.is_none() {
            query.limit = Some(50);
        }
        let loaded = self.loaded_from(args)?;
        let program = loaded.program().map_err(|e| format!("query: {e}"))?;
        let syms = loaded.symbols()?;
        let model = loaded.model()?;
        let modules = program.debug.compilands().map_err(|e| e.to_string())?;
        let lines = program.debug.source_lines().map_err(|e| e.to_string())?;
        let out = hexcavator_query::run(
            &query,
            &SymbolTable::new(model, syms, &modules).with_lines(&lines),
        )
        .map_err(|e| e.message)?;
        let rows: Vec<Value> = out
            .rows
            .iter()
            .map(|r| {
                Value::Object(
                    out.columns
                        .iter()
                        .zip(r)
                        .map(|(c, v)| (c.clone(), crate::query::value_json(v)))
                        .collect(),
                )
            })
            .collect();
        Ok((
            crate::query::table_text(&out),
            object([
                ("matched", Value::from(out.matched)),
                ("rows", Value::Array(rows)),
            ]),
        ))
    }

    fn lookup(&mut self, args: &Value) -> ToolResult {
        let loaded = self.loaded_from(args)?;
        let addr = args
            .get("address")
            .and_then(parse_address)
            .ok_or("`address` is required (a number, or a hex string like \"0x1000\")")?;
        let file = arg_str(args, "domain") == Some("file");
        // (Every format's sources; COFF groups are PE's, and segments the
        // others'.)
        let mut list = vec![Source::Sections];
        if loaded.pe {
            if loaded.has_debug() {
                list.push(Source::Groups);
            }
        } else {
            list.push(Source::Segments);
        }
        list.push(Source::Symbols);
        if loaded.object {
            list.push(Source::ArMembers);
        } else if loaded.has_debug() {
            list.extend([Source::CompileUnits, Source::Libraries]);
        }
        let maps = loaded.maps(&list)?;
        let mut by_source = Vec::new();
        let mut text = Vec::new();
        for (source, map) in &maps.sources {
            let m = if file { &map.file } else { &map.vm };
            if let Some(e) = m.get(addr) {
                let label = maps.labels.name(e.label);
                text.push(format!(
                    "{:<13} {label}  [0x{:x}, 0x{:x})",
                    source.name(),
                    e.start,
                    e.end
                ));
                by_source.push((
                    source.name(),
                    object([
                        ("label", Value::from(label)),
                        ("start", Value::from(format!("0x{:x}", e.start))),
                        ("end", Value::from(format!("0x{:x}", e.end))),
                    ]),
                ));
            }
        }
        // Every name at the symbol's address (folded ones too).
        let mut names = Vec::new();
        if !file && loaded.has_debug() {
            let all = loaded.symbols()?;
            let i = all.partition_point(|s| s.addr <= addr);
            if let Some(start) = all[..i]
                .iter()
                .rev()
                .find(|s| !s.folded && addr < s.addr + s.size.vm.max(s.size.file).max(1))
                .map(|s| s.addr)
            {
                for s in all.iter().filter(|s| s.addr == start) {
                    names.push(object([
                        ("name", Value::from(s.name.as_str())),
                        ("kind", Value::from(s.kind.sizebench_name())),
                        ("folded", Value::from(s.folded)),
                    ]));
                    text.push(format!(
                        "  {}{}",
                        if s.folded { "(folded) " } else { "" },
                        s.name
                    ));
                }
            }
        }
        if by_source.is_empty() {
            return Err(format!(
                "nothing is at 0x{addr:x} in the {}",
                if file { "file" } else { "image" }
            ));
        }
        Ok((
            format!("0x{addr:x}:\n{}", text.join("\n")),
            object([
                ("address", Value::from(format!("0x{addr:x}"))),
                ("domain", Value::from(if file { "file" } else { "vm" })),
                ("sources", object(by_source)),
                ("symbols", Value::Array(names)),
            ]),
        ))
    }

    fn findings(&mut self, args: &Value) -> ToolResult {
        let loaded = self.loaded_from(args)?;
        let program = loaded.program().map_err(|e| format!("findings: {e}"))?;
        let syms = loaded.symbols()?;
        let (image, debug) = (&program.image, &program.debug);
        let modules = debug.compilands().map_err(|e| e.to_string())?;
        let name = |c: u32| {
            modules
                .get(c as usize)
                .map_or_else(String::new, |m| m.name.clone())
        };
        let limit = args
            .get("limit")
            .and_then(Value::as_u64)
            .map_or(20, |l| usize::try_from(l).unwrap_or(usize::MAX));
        let (text, json) = hexcavator_size::findings::report(
            image,
            debug,
            syms,
            &name,
            hexcavator_size::findings::ReportOptions { limit },
        );
        let structured = serde_json::from_str::<Value>(&json).map_err(|e| e.to_string())?;
        Ok((text, structured))
    }

    fn linker_log(&mut self, args: &Value) -> ToolResult {
        let log = arg_path(args, "log").ok_or("`log` is required")?;
        let limit = args
            .get("limit")
            .and_then(Value::as_u64)
            .map_or(20, |l| usize::try_from(l).unwrap_or(usize::MAX));
        let options = |json| crate::linkerlog::Options {
            log: log.clone(),
            base: arg_path(args, "base"),
            why: arg_str(args, "why").map(str::to_string),
            image: arg_path(args, "path"),
            pdb: arg_path(args, "debug").or_else(|| arg_path(args, "pdb")),
            top: if limit == 0 { usize::MAX } else { limit },
            json,
        };
        let text = crate::linkerlog::report(&options(false))?;
        let json = crate::linkerlog::report(&options(true))?;
        let structured = serde_json::from_str::<Value>(&json).map_err(|e| e.to_string())?;
        Ok((text, structured))
    }

    fn call(&mut self, name: &str, args: &Value) -> Option<ToolResult> {
        Some(match name {
            "linker_log" => self.linker_log(args),
            "findings" => self.findings(args),
            "size_report" => self.size_report(args),
            "symbols" => self.symbols(args),
            "lookup" => self.lookup(args),
            "dependencies" => dependencies(args),
            "initialization" => initialization(args),
            "symbolize" => symbolize(args),
            "resources" => resources(args),
            "query" => self.query(args),
            "why" => self.why(args),
            "recommend" => self.recommend(args),
            _ => return None,
        })
    }

    /// The response to one message, if it needs one.
    fn handle(&mut self, message: &Value) -> Option<Value> {
        let id = message.get("id").cloned();
        let method = message.get("method").and_then(Value::as_str);
        let params = message.get("params").cloned().unwrap_or(Value::Null);
        let Some(method) = method else {
            // A response or garbage: nothing to answer.
            return id.map(|id| error(id, -32600, "invalid request"));
        };
        // Notifications get no response.
        let id = id?;
        let result = match method {
            "initialize" => {
                let asked = params.get("protocolVersion").and_then(Value::as_str);
                let version = asked
                    .filter(|v| PROTOCOL_VERSIONS.contains(v))
                    .unwrap_or(PROTOCOL_VERSIONS[0]);
                object([
                    ("protocolVersion", Value::from(version)),
                    (
                        "capabilities",
                        object([("tools", object([("listChanged", Value::from(false))]))]),
                    ),
                    (
                        "serverInfo",
                        object([
                            ("name", Value::from("hexc")),
                            ("version", Value::from(env!("CARGO_PKG_VERSION"))),
                        ]),
                    ),
                    ("instructions", Value::from(INSTRUCTIONS)),
                ])
            }
            "ping" => object::<&str>([]),
            "tools/list" => tools(),
            "tools/call" => {
                let name = params.get("name").and_then(Value::as_str).unwrap_or("");
                let args = params
                    .get("arguments")
                    .cloned()
                    .unwrap_or(object::<&str>([]));
                match self.call(name, &args) {
                    None => return Some(error(id, -32602, &format!("unknown tool '{name}'"))),
                    Some(Ok((text, structured))) => object([
                        (
                            "content",
                            Value::Array(vec![object([
                                ("type", Value::from("text")),
                                ("text", Value::from(text)),
                            ])]),
                        ),
                        ("structuredContent", structured),
                        ("isError", Value::from(false)),
                    ]),
                    Some(Err(e)) => object([
                        (
                            "content",
                            Value::Array(vec![object([
                                ("type", Value::from("text")),
                                ("text", Value::from(e)),
                            ])]),
                        ),
                        ("isError", Value::from(true)),
                    ]),
                }
            }
            other => return Some(error(id, -32601, &format!("method not found: {other}"))),
        };
        Some(object([
            ("jsonrpc", Value::from("2.0")),
            ("id", id),
            ("result", result),
        ]))
    }
}

const INSTRUCTIONS: &str = "hexc reports what the bytes of an executable, library, or object are, and why they're there: PE (Windows), ELF (Linux), Mach-O (macOS, iOS), and WebAssembly alike, with their debug information (a PDB, or DWARF: in the image, a separate debug file, or a dSYM), which is found from the image if it can be (else give it with `debug`). Start with size_report on the file (sources: sections; then symbols, compileunits, libraries, or several for a breakdown; groups are a PE image's COFF groups). Use symbols or query to find the largest or a named symbol, findings for what wastes space and how to fix it, recommend for build changes that would make it smaller, why for why a function is in it, and lookup to ask what is at an address. Give `base` to size_report to diff two builds. Sizes are in bytes; `file` is bytes on disk, `vm` bytes in memory. For crash reports and stack traces, symbolize gives each address's function, source line, and inlined functions, or annotates a log's text.";

fn error(id: Value, code: i64, message: &str) -> Value {
    object([
        ("jsonrpc", Value::from("2.0")),
        ("id", id),
        (
            "error",
            object([
                ("code", Value::from(code)),
                ("message", Value::from(message)),
            ]),
        ),
    ])
}

fn schema(properties: Vec<(&str, Value)>, required: &[&str]) -> Value {
    object([
        ("type", Value::from("object")),
        ("properties", object(properties)),
        (
            "required",
            Value::Array(required.iter().map(|r| Value::from(*r)).collect()),
        ),
    ])
}

fn prop(kind: &str, description: &str) -> Value {
    object([
        ("type", Value::from(kind)),
        ("description", Value::from(description)),
    ])
}

/// The `dependencies` tool: what an image loads (`hexc deps -json`).
fn dependencies(args: &Value) -> ToolResult {
    let path = arg_str(args, "path").ok_or("`path` is required")?;
    let mut command = vec!["deps".to_string(), "-json".to_string()];
    if let Some(s) = arg_str(args, "sysroot") {
        command.extend(["-sysroot".into(), s.to_string()]);
    }
    if args.get("system").and_then(Value::as_bool) == Some(true) {
        command.push("-system".into());
    }
    command.push(path.to_string());
    let (_, out) = crate::out::capture(|| crate::command(&command));
    let json = String::from_utf8_lossy(&out.stdout).into_owned();
    if json.is_empty() {
        return Err(String::from_utf8_lossy(&out.stderr).trim().to_string());
    }
    let structured: Value = serde_json::from_str(&json).map_err(|e| e.to_string())?;
    let mut text = command.clone();
    text[1] = "-problems".into();
    let (_, summary) = crate::out::capture(|| crate::command(&text));
    Ok((
        String::from_utf8_lossy(&summary.stdout).into_owned(),
        structured,
    ))
}

/// The `symbolize` tool: the frames at addresses (as `hexc symbolize`
/// gives them), or text with where each address in it is added.
fn symbolize(args: &Value) -> ToolResult {
    let mut images: Vec<String> = match args.get("images") {
        Some(Value::Array(a)) => a
            .iter()
            .filter_map(Value::as_str)
            .map(str::to_string)
            .collect(),
        _ => Vec::new(),
    };
    if let Some(p) = arg_str(args, "path") {
        images.insert(0, p.to_string());
    }
    if images.is_empty() {
        return Err("`path` (or `images`) is required".into());
    }
    if let Some(text) = arg_str(args, "text") {
        let out = crate::symbolize::annotate_text(&images, text)?;
        let structured = object([("text", Value::from(out.clone()))]);
        return Ok((out, structured));
    }
    let addresses: Vec<String> = match args.get("addresses") {
        Some(Value::Array(a)) => a
            .iter()
            .map(|v| v.as_str().map_or_else(|| v.to_string(), str::to_string))
            .collect(),
        Some(Value::String(s)) => s
            .split([',', ' '])
            .filter(|s| !s.is_empty())
            .map(str::to_string)
            .collect(),
        _ => return Err("`addresses` or `text` is required".into()),
    };
    let mut command = vec!["symbolize".to_string()];
    if args.get("offset").and_then(Value::as_bool) == Some(true) {
        command.push("-offset".into());
    }
    if args.get("return").and_then(Value::as_bool) == Some(true) {
        command.push("-return".into());
    }
    if let Some(p) = arg_str(args, "debug").or_else(|| arg_str(args, "pdb")) {
        command.extend(["-debug".into(), p.to_string()]);
    }
    command.extend(images);
    command.extend(addresses);
    let run = |json: bool| {
        let mut c = command.clone();
        if json {
            c.insert(1, "-json".into());
        }
        let (code, out) = crate::out::capture(|| crate::command(&c));
        let stdout = String::from_utf8_lossy(&out.stdout).into_owned();
        if code != std::process::ExitCode::SUCCESS && stdout.is_empty() {
            return Err(String::from_utf8_lossy(&out.stderr).trim().to_string());
        }
        Ok(stdout)
    };
    let text = run(false)?;
    let structured = serde_json::from_str(&run(true)?).map_err(|e| e.to_string())?;
    Ok((text, object([("addresses", structured)])))
}

/// The `initialization` tool: what runs when an image loads (`hexc init`).
fn initialization(args: &Value) -> ToolResult {
    let path = arg_str(args, "path").ok_or("`path` is required")?;
    let mut command = vec!["init".to_string()];
    if let Some(p) = arg_str(args, "debug").or_else(|| arg_str(args, "pdb")) {
        command.extend(["-debug".into(), p.to_string()]);
    }
    let depth = args
        .get("depth")
        .and_then(Value::as_u64)
        .unwrap_or(2)
        .min(16);
    command.extend(["-depth".into(), depth.to_string(), path.to_string()]);
    let (code, out) = crate::out::capture(|| crate::command(&command));
    if code != std::process::ExitCode::SUCCESS {
        return Err(String::from_utf8_lossy(&out.stderr).trim().to_string());
    }
    let text = String::from_utf8_lossy(&out.stdout).into_owned();
    command.insert(1, "-json".into());
    let (_, json) = crate::out::capture(|| crate::command(&command));
    let structured =
        serde_json::from_str(&String::from_utf8_lossy(&json.stdout)).map_err(|e| e.to_string())?;
    Ok((text, structured))
}

/// The `resources` tool: a binary's resources and metadata, or one of
/// them decoded (`hexc resources`).
fn resources(args: &Value) -> ToolResult {
    let path = arg_str(args, "path").ok_or("`path` is required")?;
    let mut command = vec!["resources".to_string()];
    if let Some(id) = arg_str(args, "id") {
        command.extend(["-show".into(), id.to_string()]);
    }
    command.push(path.to_string());
    let (code, out) = crate::out::capture(|| crate::command(&command));
    if code != std::process::ExitCode::SUCCESS {
        return Err(String::from_utf8_lossy(&out.stderr).trim().to_string());
    }
    let text = String::from_utf8_lossy(&out.stdout).into_owned();
    command.insert(1, "-json".into());
    let (_, json) = crate::out::capture(|| crate::command(&command));
    let mut structured: Value =
        serde_json::from_str(&String::from_utf8_lossy(&json.stdout)).map_err(|e| e.to_string())?;
    // (An image's bytes are no use to a model: what they are is.)
    if let Some(o) = structured.as_object_mut() {
        o.remove("file");
        o.remove("hex");
    }
    Ok((text, structured))
}

fn tools() -> Value {
    let path = prop(
        "string",
        "Path to an executable or shared library (PE, ELF, Mach-O, WebAssembly), an object, or a static library.",
    );
    let pdb = prop(
        "string",
        "Path to the image's debug information (a PDB, a dSYM, or a DWARF file), if it isn't found from the image. (`pdb` is another name for it.)",
    );
    let sources: Vec<Value> = Source::ALL.iter().map(|s| Value::from(s.name())).collect();
    let tool = |name: &str, description: &str, input: Value| {
        object([
            ("name", Value::from(name)),
            ("description", Value::from(description)),
            ("inputSchema", input),
        ])
    };
    object([(
        "tools",
        Value::Array(vec![
            tool(
                "size_report",
                "Where the bytes of a binary go, rolled up by data sources (one level per source), as bloaty reports them: every byte of the file and of memory is counted once. With `base`, the difference from another build.",
                schema(
                    vec![
                        ("path", path.clone()),
                        ("debug", pdb.clone()),
                        (
                            "sources",
                            object([
                                ("type", Value::from("array")),
                                (
                                    "items",
                                    object([
                                        ("type", Value::from("string")),
                                        ("enum", Value::Array(sources)),
                                    ]),
                                ),
                                (
                                    "description",
                                    Value::from(
                                        "Data sources, outermost first (default: [\"sections\"]).",
                                    ),
                                ),
                            ]),
                        ),
                        (
                            "rows",
                            prop(
                                "integer",
                                "Rows per level before the rest are folded into [N Others] (default 20; 0 for all).",
                            ),
                        ),
                        (
                            "sort",
                            prop(
                                "string",
                                "Sort by \"vm\", \"file\", or \"both\" (the larger; default).",
                            ),
                        ),
                        (
                            "domain",
                            prop(
                                "string",
                                "Show \"vm\", \"file\", or \"both\" sizes (default both).",
                            ),
                        ),
                        (
                            "base",
                            prop(
                                "string",
                                "A base build to diff against: sizes become differences.",
                            ),
                        ),
                        (
                            "base_debug",
                            prop(
                                "string",
                                "The base build's debug information, if it isn't found.",
                            ),
                        ),
                        (
                            "source_filter",
                            prop(
                                "string",
                                "A regular expression: only regions some label of which matches are counted (the rest are reported as filtered).",
                            ),
                        ),
                    ],
                    &["path"],
                ),
            ),
            tool(
                "symbols",
                "The largest symbols of a binary (functions, data, strings, and the image's own structures such as imports, resources, and exception data), filtered by name, kind, or section.",
                schema(
                    vec![
                        ("path", path.clone()),
                        ("debug", pdb.clone()),
                        (
                            "query",
                            prop("string", "A case-insensitive substring of the name."),
                        ),
                        (
                            "kind",
                            prop(
                                "string",
                                "A kind, such as function, data, public, string, or import (prefixes match).",
                            ),
                        ),
                        (
                            "section",
                            prop("string", "Only symbols in this section (such as .text)."),
                        ),
                        (
                            "sort",
                            prop("string", "\"size\" (default), \"address\", or \"name\"."),
                        ),
                        ("limit", prop("integer", "How many to list (default 50).")),
                        (
                            "include_folded",
                            prop(
                                "boolean",
                                "Include names folded into others by identical COMDAT folding (they have size 0).",
                            ),
                        ),
                    ],
                    &["path"],
                ),
            ),
            tool(
                "findings",
                "What wastes space in an image, with advice: variables duplicated across compilands (file-scope statics and non-extern consts in headers), and template families (instantiations of one template) by total size.",
                schema(
                    vec![
                        ("path", path.clone()),
                        ("debug", pdb.clone()),
                        (
                            "limit",
                            prop("integer", "Items per finding (default 20; 0 for all)."),
                        ),
                    ],
                    &["path"],
                ),
            ),
            tool(
                "linker_log",
                "What a linker's log says: MSVC's /VERBOSE output, lld's and GNU ld's --trace and --why-extract, or ld64's -why_load and -t. Which library members were linked, what was discarded and folded; with `why`, why one member was linked (the symbol it was loaded for, and who referenced it, back to the command line); with `path` (the image, with its debug information), its largest library members, each with why it was linked.",
                schema(
                    vec![
                        ("log", prop("string", "Path to the linker's log.")),
                        (
                            "why",
                            prop(
                                "string",
                                "A member to explain: lib(member), or just member.obj.",
                            ),
                        ),
                        (
                            "path",
                            prop(
                                "string",
                                "The image the log is of, to rank its library members by size.",
                            ),
                        ),
                        (
                            "base",
                            prop(
                                "string",
                                "A base build's log: what changed (members linked now and not then, with why; members no longer linked).",
                            ),
                        ),
                        ("debug", pdb.clone()),
                        (
                            "limit",
                            prop(
                                "integer",
                                "How many members to list (default 20; 0 for all).",
                            ),
                        ),
                    ],
                    &["log"],
                ),
            ),
            tool(
                "query",
                "Query an image's symbols (from its debug information) as a table: filter, group, sort, and limit. Columns: name, kind (function, data, public, string, thunk, pdata, unwind, import-thunk, ...), file and vm (sizes), rva (the address from the image's start), section, group (a PE image's COFF group), compiland, library, folded, static, source and line (where its code starts). The query is a filter (column op value joined by and/or/not and parentheses; ops = != < <= > >= ~ !~, ~ a regular expression; numbers take k, M, G), then any of: group by <columns> (count and summed sizes), sort by <column> [desc], select <columns>, limit <n> (default 50). Examples: \"kind=function file>4k sort by file desc limit 20\", \"name~'^std::' group by library\", \"static group by name sort by count desc\".",
                schema(
                    vec![
                        ("path", path.clone()),
                        ("debug", pdb.clone()),
                        ("query", prop("string", "The query.")),
                    ],
                    &["path", "query"],
                ),
            ),
            tool(
                "why",
                "Why a function or datum is in an executable or library (PE, ELF, Mach-O, WebAssembly, with its debug information): the chain of references from a root (the entry point, an export, a loader directory, a static initializer) to it (calls, branches, addresses its code uses, pointers in data such as vtable slots, exception data). If nothing reaches it, why the linker kept it anyway (its section isn't a COMDAT or its own, or a kept one refers to it) and what refers to it.",
                schema(
                    vec![
                        ("path", path.clone()),
                        ("debug", pdb.clone()),
                        (
                            "name",
                            prop(
                                "string",
                                "The name as the debug information gives it (\"Square::area() const\"), or without the parameters if just one function has it.",
                            ),
                        ),
                    ],
                    &["path", "name"],
                ),
            ),
            tool(
                "recommend",
                "Build changes that would make an executable or library smaller (PE, ELF, Mach-O, WebAssembly), each with the bytes it would save, measured from the image and its debug information, in its toolchain's terms (MSVC's, GCC's and Clang's, or Apple's): compile an object with /Gy or /Gw (-ffunction-sections, -fdata-sections), link with /OPT:REF or /OPT:ICF (--gc-sections or -dead_strip, --icf) when the link didn't (its options from the PDB, or judged from what it left), define a duplicated constant once; hide exports (template instantiations and inline functions, -fvisibility=hidden; a library's calls to its own exports through the PLT, -Bsymbolic-functions; and, given its importers, what none imports, -fvisibility=hidden); and what the image's structure shows (RELR relocations, .hash, WebAssembly custom sections), which, with its exports, is all there is without debug information.",
                schema(
                    vec![
                        ("path", path.clone()),
                        ("debug", pdb.clone()),
                        (
                            "importers",
                            object([
                                ("type", Value::from("array")),
                                ("items", object([("type", Value::from("string"))])),
                                (
                                    "description",
                                    Value::from(
                                        "Executables and libraries that load it, or directories of them (searched, with their subdirectories, for those that do): what they import of its exports is kept, the rest is recommended hidden.",
                                    ),
                                ),
                            ]),
                        ),
                    ],
                    &["path"],
                ),
            ),
            tool(
                "dependencies",
                "What an executable or library (PE, ELF, Mach-O) loads, found as its platform's loader would find each library (known DLLs, the application's directory, System32, API sets, side-by-side assemblies; rpath, runpath, ld.so.conf; @rpath, @loader_path, the dyld shared cache), and what's missing: libraries not found or for another architecture, and imported symbols a library doesn't export. Text: the problems; structured: the whole graph (modules, each with its dependencies and imports).",
                schema(
                    vec![
                        ("path", prop("string", "The executable or library.")),
                        (
                            "sysroot",
                            prop(
                                "string",
                                "The system the libraries are looked for in: a Windows directory, or the root of a Linux or macOS file system (default: this machine's, for its own format).",
                            ),
                        ),
                        (
                            "system",
                            prop(
                                "boolean",
                                "Read what system libraries load too (default: they're leaves).",
                            ),
                        ),
                    ],
                    &["path"],
                ),
            ),
            tool(
                "symbolize",
                "The function, source file and line, and inlined functions (innermost first) at addresses of executables or libraries (PE with its PDB; ELF, Mach-O, WebAssembly with DWARF; or, without debug information, their own symbol tables), as a symbolizer for crash reports and stack traces. Or, given `text` (a log, a stack trace), that text with where each address in it is added after it.",
                schema(
                    vec![
                        (
                            "path",
                            prop(
                                "string",
                                "The image the addresses are in; `path@base` if it was loaded at `base` (then addresses are as loaded).",
                            ),
                        ),
                        (
                            "images",
                            object([
                                ("type", Value::from("array")),
                                ("items", object([("type", Value::from("string"))])),
                                (
                                    "description",
                                    Value::from(
                                        "More images (each `path` or `path@base`), for text or addresses of several modules.",
                                    ),
                                ),
                            ]),
                        ),
                        (
                            "addresses",
                            object([
                                ("type", Value::from("array")),
                                ("items", object([("type", Value::from("string"))])),
                                (
                                    "description",
                                    Value::from(
                                        "Addresses: hex (\"0x140001043\"), as the image's tools show them (a PE image's with its preferred base) unless a base is given; or `name+0xoffset` (an offset from the start of the image named name).",
                                    ),
                                ),
                            ]),
                        ),
                        (
                            "text",
                            prop(
                                "string",
                                "Text to annotate instead (a crash log, a stack trace).",
                            ),
                        ),
                        (
                            "offset",
                            prop(
                                "boolean",
                                "The addresses are offsets from the image's start.",
                            ),
                        ),
                        (
                            "return",
                            prop(
                                "boolean",
                                "The addresses are return addresses (stack frames' but the first): the call before each is looked up.",
                            ),
                        ),
                        (
                            "pdb",
                            prop(
                                "string",
                                "The first image's debug information, if it isn't found from the image.",
                            ),
                        ),
                    ],
                    &[],
                ),
            ),
            tool(
                "initialization",
                "What runs when an executable or library (PE, ELF, Mach-O, with its debug information) is loaded, before its main: TLS callbacks, the entry point, the CRT's initializer lists (C initializers, global objects' constructors), .preinit_array, DT_INIT, .ctors, .init_array, __mod_init_func; each phase's functions in the order they run, with what each calls (`depth` levels down), and then main (or DllMain).",
                schema(
                    vec![
                        ("path", prop("string", "The executable or library.")),
                        (
                            "pdb",
                            prop(
                                "string",
                                "Its debug information, if it isn't found from the image.",
                            ),
                        ),
                        (
                            "depth",
                            prop("integer", "How many levels of callees to show (default 2)."),
                        ),
                    ],
                    &["path"],
                ),
            ),
            tool(
                "resources",
                "A binary's resources and what it says of itself: a PE image's resources (version information, manifest, string and message tables, dialogs, menus, accelerators, icons, cursors, bitmaps, raw data) by type, name, and language; a Mach-O image's Info.plist, entitlements, and versions; an ELF image's notes (build ID, ABI tag, package metadata, properties) and .comment; a WebAssembly module's producers and custom sections. Without `id`, the list, each in a line; with `id` (as listed, like \"VERSION/#1/1033\" or \"Info.plist\"), that one decoded in full.",
                schema(
                    vec![
                        ("path", prop("string", "The binary.")),
                        (
                            "id",
                            prop("string", "One of them, as listed, to decode in full."),
                        ),
                    ],
                    &["path"],
                ),
            ),
            tool(
                "lookup",
                "What is at an address of a binary, by every data source: section, segment or COFF group, symbol (with every name folded there), compile unit, and library.",
                schema(
                    vec![
                        ("path", path),
                        ("debug", pdb),
                        (
                            "address",
                            prop(
                                "string",
                                "An address, as the image's tools show it (a PE image's from its start, an RVA), or with domain \"file\" a file offset; a number or a hex string like \"0x1a2b0\".",
                            ),
                        ),
                        (
                            "domain",
                            prop(
                                "string",
                                "\"vm\" (default: the address is in memory) or \"file\".",
                            ),
                        ),
                    ],
                    &["path", "address"],
                ),
            ),
        ]),
    )])
}

/// Serves `input` until it ends, writing responses to `output`.
pub fn serve(input: impl BufRead, mut output: impl Write) -> io::Result<()> {
    let mut server = Server::default();
    for line in input.lines() {
        let line = line?;
        if line.trim().is_empty() {
            continue;
        }
        let response = match serde_json::from_str::<Value>(&line) {
            Ok(message) => server.handle(&message),
            Err(e) => Some(error(Value::Null, -32700, &format!("parse error: {e}"))),
        };
        if let Some(r) = response {
            writeln!(output, "{r}")?;
            output.flush()?;
        }
    }
    Ok(())
}

pub fn run(args: &[String]) -> ExitCode {
    if !args.is_empty() {
        ereport!("{USAGE}");
        return ExitCode::from(2);
    }
    let stdin = io::stdin().lock();
    let stdout = io::stdout().lock();
    match serve(stdin, stdout) {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            ereport!("hexc mcp: {e}");
            ExitCode::FAILURE
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn exchange(messages: &[&str]) -> Vec<Value> {
        let input = messages.join("\n");
        let mut out = Vec::new();
        serve(input.as_bytes(), &mut out).unwrap();
        String::from_utf8(out)
            .unwrap()
            .lines()
            .map(|l| serde_json::from_str::<Value>(l).unwrap())
            .collect()
    }

    #[test]
    fn protocol() {
        let r = exchange(&[
            r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-03-26","capabilities":{},"clientInfo":{"name":"t","version":"1"}}}"#,
            r#"{"jsonrpc":"2.0","method":"notifications/initialized"}"#,
            r#"{"jsonrpc":"2.0","id":"two","method":"tools/list"}"#,
            r#"{"jsonrpc":"2.0","id":3,"method":"ping"}"#,
            r#"{"jsonrpc":"2.0","id":4,"method":"bogus"}"#,
            r"not json",
            r#"{"jsonrpc":"2.0","id":5,"method":"tools/call","params":{"name":"nope"}}"#,
            r#"{"jsonrpc":"2.0","id":6,"method":"tools/call","params":{"name":"size_report","arguments":{}}}"#,
        ]);
        assert_eq!(r.len(), 7);
        let init = r[0].get("result").unwrap();
        assert_eq!(
            init.get("protocolVersion").unwrap().as_str(),
            Some("2025-03-26")
        );
        assert_eq!(
            init.get("serverInfo")
                .unwrap()
                .get("name")
                .unwrap()
                .as_str(),
            Some("hexc")
        );
        assert_eq!(r[1].get("id").unwrap().as_str(), Some("two"));
        let tools = r[1]
            .get("result")
            .unwrap()
            .get("tools")
            .unwrap()
            .as_array()
            .unwrap();
        let names: Vec<&str> = tools
            .iter()
            .map(|t| t.get("name").unwrap().as_str().unwrap())
            .collect();
        assert_eq!(
            names,
            [
                "size_report",
                "symbols",
                "findings",
                "linker_log",
                "query",
                "why",
                "recommend",
                "dependencies",
                "symbolize",
                "initialization",
                "resources",
                "lookup"
            ]
        );
        assert!(r[2].get("result").is_some());
        assert_eq!(
            r[3].get("error").unwrap().get("code").unwrap().as_i64(),
            Some(-32601)
        );
        assert_eq!(
            r[4].get("error").unwrap().get("code").unwrap().as_i64(),
            Some(-32700)
        );
        assert_eq!(
            r[5].get("error").unwrap().get("code").unwrap().as_i64(),
            Some(-32602)
        );
        let failed = r[6].get("result").unwrap();
        assert_eq!(failed.get("isError").unwrap().as_bool(), Some(true));
    }

    #[test]
    fn addresses() {
        assert_eq!(parse_address(&Value::from("0x1A2b")), Some(0x1A2B));
        assert_eq!(parse_address(&Value::from("4096")), Some(4096));
        assert_eq!(parse_address(&Value::from("1a2b")), Some(0x1A2B));
        assert_eq!(parse_address(&Value::from(16)), Some(16));
        assert_eq!(parse_address(&Value::from("zz")), None);
        assert_eq!(parse_address(&Value::from(-1)), None);
    }

    #[test]
    fn source_arguments() {
        let v = serde_json::from_str::<Value>(r#"{"sources":["sections","symbols"]}"#).unwrap();
        assert_eq!(
            arg_sources(&v, &[]).unwrap(),
            [Source::Sections, Source::Symbols]
        );
        let v = serde_json::from_str::<Value>(r#"{"sources":"compileunits, libraries"}"#).unwrap();
        assert_eq!(
            arg_sources(&v, &[]).unwrap(),
            [Source::CompileUnits, Source::Libraries]
        );
        let v = serde_json::from_str::<Value>(r"{}").unwrap();
        assert_eq!(
            arg_sources(&v, &[Source::Groups]).unwrap(),
            [Source::Groups]
        );
        let v = serde_json::from_str::<Value>(r#"{"sources":["bogus"]}"#).unwrap();
        assert!(arg_sources(&v, &[]).is_err());
        let v = serde_json::from_str::<Value>(r#"{"sources":5}"#).unwrap();
        assert!(arg_sources(&v, &[]).is_err());
    }
}
