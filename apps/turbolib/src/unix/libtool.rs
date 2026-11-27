//! The `libtool -static` personality: `llvm-libtool-darwin`'s command line
//! and what it writes (read in LLVM 24's `llvm-libtool-darwin.cpp`,
//! `ArchiveWriter.cpp`, and `MachOUniversalWriter.cpp`, and measured).
//!
//! It makes a Darwin archive of Mach-O objects: the files named (archives
//! flattened into their members, universal files into their slices), each
//! a member named by its file's base name, grouped by architecture. One
//! architecture gives one archive; several give a universal file holding
//! one archive per architecture. Archives are written as `llvm-ar` writes
//! the Darwin dialect (see [`turbonop_archive::unix`]), with a symbol table
//! even when no member has symbols, and in deterministic mode (the default;
//! `-U` turns it off) each name's copies dated 1, 2, ... so that a debug
//! map can tell them apart.
//!
//! Apple's own `libtool` (cctools) writes `__.SYMDEF SORTED` and pads
//! differently; that tool isn't available here to measure, so this
//! personality is `llvm-libtool-darwin`'s, under either name.
//!
//! Options: `-static` (required), `-o <file>`, `-D`, `-U`,
//! `-filelist <file>[,<dir>]`, `-arch_only <arch>`, `-l<x>` and `-L<dir>`
//! (joined or separate), `-syslibroot <dir>` (ignored, as by
//! `llvm-libtool-darwin`), `-dependency_info <file>`,
//! `-no_warning_for_no_symbols`, `-warnings_as_errors`, `-V`/`-version`,
//! `-h`/`-help`; each with `-` or `--`. turbolib's own options are taken as
//! for `ar` (`--fold=`, `-j N`, ...).

use super::run::{self, Bytes, Env, Fatal, Item, Outcome, Plan, Run};
use std::path::Path;
use std::sync::Arc;
use turbonop_archive::unix::Dialect;
use turbonop_archive::write::Fields;
use turbonop_archive::{Archive, MemberKind};
use turbonop_macho::consts::{
    CPU_SUBTYPE_MASK, CPU_TYPE_ARM, CPU_TYPE_ARM64, CPU_TYPE_ARM64_32, CPU_TYPE_X86,
    CPU_TYPE_X86_64,
};
use turbonop_mmap::Mmap;

/// The version `-dependency_info` records (`llvm-libtool-darwin`'s, at the
/// LLVM commit the personality follows).
const TOOL_VERSION: &str = "llvm-libtool-darwin 24.0.0git";

/// The names `-arch_only` accepts (`MachOObjectFile::getValidArchs`), as
/// its error lists them.
const VALID_ARCHS: &[&str] = &[
    "i386",
    "x86_64",
    "x86_64h",
    "armv4t",
    "arm",
    "armv5e",
    "armv6",
    "armv6m",
    "armv7",
    "armv7em",
    "armv7k",
    "armv7m",
    "armv7s",
    "armv8m.base",
    "armv8m.main",
    "armv8.1m.main",
    "arm64",
    "arm64e",
    "arm64e.x1",
    "arm64_32",
    "ppc",
    "ppc64",
];

/// An architecture name's CPU type and subtype (TextAPI's table; names it
/// doesn't know are `(0, 0)`, which no file matches).
fn arch_cpu(name: &str) -> (u32, u32) {
    match name {
        "i386" => (CPU_TYPE_X86, 3),
        "x86_64" => (CPU_TYPE_X86_64, 3),
        "x86_64h" => (CPU_TYPE_X86_64, 8),
        "armv4t" => (CPU_TYPE_ARM, 5),
        "armv6" => (CPU_TYPE_ARM, 6),
        "armv7" => (CPU_TYPE_ARM, 9),
        "armv7s" => (CPU_TYPE_ARM, 11),
        "armv7k" => (CPU_TYPE_ARM, 12),
        "armv6m" => (CPU_TYPE_ARM, 14),
        "armv7m" => (CPU_TYPE_ARM, 15),
        "armv7em" => (CPU_TYPE_ARM, 16),
        "armv8m.main" | "armv8m.base" => (CPU_TYPE_ARM, 17),
        "armv8.1m.main" => (CPU_TYPE_ARM, 19),
        "arm64" => (CPU_TYPE_ARM64, 0),
        "arm64e" => (CPU_TYPE_ARM64, 2),
        "arm64e.x1" => (CPU_TYPE_ARM64, 3),
        "arm64_32" => (CPU_TYPE_ARM64_32, 1),
        _ => (0, 0),
    }
}

/// A file's architecture as `llvm-libtool-darwin` sees it: its header's
/// type and subtype through its triple's name and back (so arm64's
/// `CPU_SUBTYPE_ARM64_V8` is arm64, and capability bits are dropped).
fn file_cpu(cputype: u32, cpusubtype: u32) -> (u32, u32) {
    let sub = cpusubtype & !CPU_SUBTYPE_MASK;
    match cputype {
        CPU_TYPE_X86_64 => (cputype, if sub == 8 { 8 } else { 3 }),
        CPU_TYPE_X86 => (cputype, 3),
        CPU_TYPE_ARM64 => (cputype, if sub == 2 { 2 } else { 0 }),
        CPU_TYPE_ARM64_32 => (cputype, 1),
        _ => (cputype, sub),
    }
}

/// The key members are grouped by (`getCPUID`): the type, and the subtype
/// for ARM, ARM64, ARM64_32, and x86-64.
fn cpu_id(cputype: u32, cpusubtype: u32) -> u64 {
    match cputype {
        CPU_TYPE_ARM | CPU_TYPE_ARM64 | CPU_TYPE_ARM64_32 | CPU_TYPE_X86_64 => {
            u64::from(cputype) << 32 | u64::from(cpusubtype)
        }
        _ => u64::from(cputype),
    }
}

/// The command line.
#[derive(Default, Debug)]
struct Command {
    help: bool,
    version: bool,
    static_: bool,
    output: Option<String>,
    inputs: Vec<String>,
    libraries: Vec<String>,
    search_dirs: Vec<String>,
    file_list: Option<String>,
    arch_only: Option<String>,
    dependency_info: Option<String>,
    deterministic: bool,
    nondeterministic: bool,
    no_warning_for_no_symbols: bool,
    warnings_as_errors: bool,
    turbolib: Vec<String>,
}

/// Parses the command line as `llvm-libtool-darwin`'s option table does
/// (an option's last value wins; unknown options are errors).
fn parse(mut words: Vec<String>) -> Result<Command, String> {
    let mut c = Command {
        turbolib: super::args::take_turbolib(&mut words)?,
        ..Command::default()
    };
    let mut unknown = Vec::new();
    let mut i = 0;
    while i < words.len() {
        let w = words[i].clone();
        i += 1;
        let Some(name) = w.strip_prefix("--").or_else(|| w.strip_prefix('-')) else {
            c.inputs.push(w);
            continue;
        };
        if name.is_empty() {
            c.inputs.push(w);
            continue;
        }
        let value = |i: &mut usize| -> Result<String, String> {
            let v = words
                .get(*i)
                .cloned()
                .ok_or_else(|| format!("{w}: missing argument"))?;
            *i += 1;
            Ok(v)
        };
        match name {
            "help" | "h" => c.help = true,
            "D" => c.deterministic = true,
            "U" => c.nondeterministic = true,
            "version" | "V" => c.version = true,
            "no_warning_for_no_symbols" => c.no_warning_for_no_symbols = true,
            "warnings_as_errors" => c.warnings_as_errors = true,
            "static" => c.static_ = true,
            "o" => c.output = Some(value(&mut i)?),
            "filelist" => c.file_list = Some(value(&mut i)?),
            "arch_only" => c.arch_only = Some(value(&mut i)?),
            "syslibroot" => {
                value(&mut i)?;
            }
            "dependency_info" => c.dependency_info = Some(value(&mut i)?),
            "l" => c.libraries.push(value(&mut i)?),
            "L" => c.search_dirs.push(value(&mut i)?),
            _ if name.starts_with('l') => c.libraries.push(name[1..].to_string()),
            _ if name.starts_with('L') => c.search_dirs.push(name[1..].to_string()),
            _ => unknown.push(w.clone()),
        }
    }
    if let Some(u) = unknown.first() {
        return Err(format!("unknown argument '{u}'"));
    }
    Ok(c)
}

/// `-dependency_info`'s record: the tool, the inputs found (sorted), the
/// paths searched and not found (sorted, once each), the output.
fn dependency_info(inputs: &[String], missing: &[String], output: &str) -> Vec<u8> {
    let mut out = Vec::new();
    let mut add = |op: u8, s: &str| {
        out.push(op);
        out.extend_from_slice(s.as_bytes());
        out.push(0);
    };
    add(0x00, TOOL_VERSION);
    let mut found: Vec<&String> = inputs.iter().collect();
    found.sort();
    for f in found {
        add(0x10, f);
    }
    let mut not: Vec<&String> = missing.iter().collect();
    not.sort();
    not.dedup();
    for f in not {
        add(0x11, f);
    }
    add(0x40, output);
    out
}

/// Joins a directory and a name as `sys::path::append` does.
fn append(dir: &str, name: &str) -> String {
    if dir.is_empty() {
        name.to_string()
    } else if dir.ends_with('/') || (cfg!(windows) && dir.ends_with('\\')) {
        format!("{dir}{name}")
    } else {
        format!("{dir}/{name}")
    }
}

/// One member, with the file it came from (for the duplicates warning).
struct Member {
    item: Item,
    file: String,
}

/// The members of each architecture, by key, in the order named.
#[derive(Default)]
struct Groups {
    by_arch: std::collections::BTreeMap<u64, Vec<Member>>,
}

/// What reading the inputs needs.
struct Reader<'r, 'e> {
    run: &'r mut Run<'e>,
    deterministic: bool,
    arch: Option<(u32, u32)>,
    no_warning_for_no_symbols: bool,
    warnings_as_errors: bool,
    groups: Groups,
}

impl Reader<'_, '_> {
    fn error(&self, text: impl Into<String>) -> Fatal {
        Fatal(format!("error: {}", text.into()))
    }

    fn accept(&self, cputype: u32, cpusubtype: u32) -> bool {
        let Some((t, s)) = self.arch else {
            return true;
        };
        if t != cputype {
            return false;
        }
        match t {
            CPU_TYPE_ARM | CPU_TYPE_ARM64_32 | CPU_TYPE_X86_64 => s == cpusubtype,
            CPU_TYPE_ARM64 if s == 0 => cpusubtype == 0 || cpusubtype == 1,
            CPU_TYPE_ARM64 => s == cpusubtype,
            _ => true,
        }
    }

    /// Adds a member that must be a Mach-O object (`verifyAndAddMachOObject`).
    fn add_object(&mut self, item: Item, file: &str) -> Result<(), Fatal> {
        let data = item.data();
        if data.starts_with(b"BC\xC0\xDE") || data.get(..4) == Some(&[0xDE, 0xC0, 0x17, 0x0B]) {
            return Err(self.error(format!(
                "'{}': LLVM bitcode members aren't read by turbolib",
                item.name
            )));
        }
        let Ok(o) = turbonop_macho::MachO::parse(data) else {
            let other = turbonop_elf::ElfFile::is_elf(data)
                || turbonop_pe::PeFile::parse_object(data).is_ok()
                || data.starts_with(b"\0asm");
            return Err(self.error(if other {
                format!("'{}': format not supported", item.name)
            } else {
                format!(
                    "'{}': The file was not recognized as a valid object file",
                    item.name
                )
            }));
        };
        let h = o.header();
        let (t, s) = file_cpu(h.cputype, h.cpusubtype);
        if self.arch.is_some() && !self.accept(t, s) {
            return Ok(());
        }
        if !self.no_warning_for_no_symbols && o.symtab().is_none_or(|st| st.nsyms == 0) {
            let text = format!(
                "'{}': has no symbols for architecture {}",
                item.name,
                turbonop_macho::arch_name(h.cputype, h.cpusubtype)
            );
            if self.warnings_as_errors {
                return Err(self.error(text));
            }
            self.run.stderr.push_str(&format!("warning: {text}\n"));
        }
        self.groups
            .by_arch
            .entry(cpu_id(t, s))
            .or_default()
            .push(Member {
                item,
                file: file.to_string(),
            });
        Ok(())
    }

    /// An archive's members (`getOldMember`): their own fields unless
    /// deterministic (then `0`, `0`, `0`, `644`).
    fn add_archive(
        &mut self,
        map: &Arc<Mmap>,
        start: usize,
        len: usize,
        file: &str,
    ) -> Result<(), Fatal> {
        let data = &map.as_slice()[start..start + len];
        let archive = Archive::parse(data).map_err(|e| self.error(format!("'{file}': {e}")))?;
        let base = data.as_ptr() as usize;
        for m in archive.members() {
            if m.kind != MemberKind::File {
                continue;
            }
            let fields = if self.deterministic {
                Fields::new(b"0", b"0", b"0", b"644").unwrap_or(Fields::BLANK)
            } else {
                let mode = std::str::from_utf8(m.mode)
                    .ok()
                    .and_then(|t| u32::from_str_radix(t.trim(), 8).ok())
                    .unwrap_or(0o644)
                    & 0o777;
                let mode = format!("{mode:o}");
                Fields::new(m.date, m.uid, m.gid, mode.as_bytes()).unwrap_or(Fields::BLANK)
            };
            let at = m.data.as_ptr() as usize - base + start;
            let item = Item {
                name: String::from_utf8_lossy(&m.name).into_owned(),
                origin: None,
                fields,
                size: m.data.len() as u64,
                bytes: Some(Bytes {
                    map: Arc::clone(map),
                    start: at,
                    len: m.data.len(),
                }),
                old: false,
                folded: None,
            };
            self.add_object(item, file).map_err(|Fatal(t)| {
                Fatal(match t.strip_prefix("error: ") {
                    Some(rest) => format!("error: '{file}': {rest}"),
                    None => t,
                })
            })?;
        }
        Ok(())
    }

    /// A file named on the command line.
    fn add_file(&mut self, file: &str) -> Result<(), Fatal> {
        let path = self.run.env.cwd.join(file);
        std::fs::metadata(&path)
            .map_err(|e| self.error(format!("'{file}': {}", run::io_text(&e))))?;
        let map = Arc::new(
            Mmap::open(&path).map_err(|e| self.error(format!("'{file}': {}", run::io_text(&e))))?,
        );
        let data = map.as_slice();
        let name = Path::new(file)
            .file_name()
            .map_or_else(|| file.to_string(), |n| n.to_string_lossy().into_owned());
        if data.starts_with(turbonop_archive::MAGIC) {
            return self.add_archive(&map, 0, data.len(), file);
        }
        if let Some(archs) = turbonop_macho::fat_archs(data)
            && turbonop_macho::is_fat(data)
        {
            for a in archs {
                let (Ok(start), Ok(len)) = (usize::try_from(a.offset), usize::try_from(a.size))
                else {
                    return Err(self.error(format!("'{file}': truncated universal file")));
                };
                let Some(slice) = data.get(start..start.saturating_add(len)) else {
                    return Err(self.error(format!("'{file}': truncated universal file")));
                };
                if slice.starts_with(turbonop_archive::MAGIC) {
                    self.add_archive(&map, start, len, file)?;
                } else {
                    // NewArchiveMember(MemoryBufferRef): fields 0, 0, 0, 644.
                    let item = Item {
                        name: name.clone(),
                        origin: None,
                        fields: Fields::new(b"0", b"0", b"0", b"644").unwrap_or(Fields::BLANK),
                        size: len as u64,
                        bytes: Some(Bytes {
                            map: Arc::clone(&map),
                            start,
                            len,
                        }),
                        old: false,
                        folded: None,
                    };
                    self.add_object(item, file)?;
                }
            }
            return Ok(());
        }
        let mut item = run::new_item(self.run, "", file, false, false, false, self.deterministic)?;
        item.name = name;
        self.add_object(item, file)
    }
}

/// Runs `llvm-libtool-darwin`'s command line.
pub fn run(argv0: &str, args: &[String], env: &Env) -> Outcome {
    let mut r = Run::new(super::Flavor::LlvmAr, argv0, env);
    let exit = match run_inner(&mut r, args) {
        Ok(()) => 0,
        Err(Fatal(text)) => {
            r.stderr.push_str(&text);
            r.stderr.push('\n');
            1
        }
    };
    Outcome {
        stdout: r.stdout,
        stderr: r.stderr,
        exit,
    }
}

fn run_inner(r: &mut Run<'_>, args: &[String]) -> Result<(), Fatal> {
    let c = parse(args.to_vec()).map_err(Fatal)?;
    let err = |t: String| Fatal(format!("error: {t}"));
    if c.help {
        r.stdout.extend_from_slice(
            b"OVERVIEW: llvm-libtool-darwin\n\nUSAGE: llvm-libtool-darwin [options] <input files>\n\n\
              OPTIONS:\n  -arch_only <arch_type>  -D  -dependency_info <string>  -filelist <listfile[,dirname]>\n  \
              -l<x>  -L<dir>  -no_warning_for_no_symbols  -o <filename>  -static  -U  -V  -warnings_as_errors\n \
              turbolib's options: --fold=, --pull=, --locals=, --stats, -j N (see docs/turbolib.md)\n",
        );
        return Ok(());
    }
    if !c.static_ {
        if c.version {
            print_version(r);
            return Ok(());
        }
        return Err(err("-static option: must be specified".into()));
    }
    let Some(output) = c.output.clone() else {
        return Err(err("-o option: must be specified".into()));
    };
    if c.deterministic && c.nondeterministic {
        return Err(err("cannot specify both -D and -U flags".into()));
    }
    let deterministic = !c.nondeterministic;
    let mut inputs = c.inputs.clone();
    let mut missing = Vec::new();
    for base in &c.libraries {
        let name = if base.ends_with(".o") {
            base.clone()
        } else {
            format!("lib{base}.a")
        };
        let mut found = None;
        for dir in
            c.search_dirs
                .iter()
                .map(String::as_str)
                .chain(["/lib", "/usr/lib", "/usr/local/lib"])
        {
            let p = append(dir, &name);
            if r.env.cwd.join(&p).exists() {
                found = Some(p);
                break;
            }
            missing.push(p);
        }
        match found {
            Some(p) => inputs.push(p),
            None => return Err(err(format!("cannot locate file '{name}'"))),
        }
    }
    if let Some(list) = &c.file_list {
        let (file, dir) = list.rsplit_once(',').unwrap_or((list.as_str(), ""));
        let text = std::fs::read(r.env.cwd.join(file))
            .map_err(|e| err(format!("'{file}': {}", run::io_text(&e))))?;
        if text.is_empty() {
            return Err(err(format!("file list file: '{file}' is empty")));
        }
        let text = String::from_utf8_lossy(&text);
        let body = text.strip_suffix('\n').unwrap_or(&text);
        for line in body.split('\n') {
            let line = line.strip_suffix('\r').unwrap_or(line);
            if line.is_empty() {
                return Err(err(format!(
                    "file list file: '{file}': filename cannot be empty"
                )));
            }
            inputs.push(append(dir, line));
        }
    }
    if inputs.is_empty() {
        return Err(err("no input files specified".into()));
    }
    let arch = match &c.arch_only {
        Some(a) if !VALID_ARCHS.contains(&a.as_str()) => {
            let mut names = String::new();
            for v in VALID_ARCHS {
                names.push_str(v);
                names.push(' ');
            }
            return Err(err(format!(
                "invalid architecture '{a}': valid architecture names are {names}"
            )));
        }
        Some(a) => Some(arch_cpu(a)),
        None => None,
    };
    if let Some(path) = &c.dependency_info {
        let bytes = dependency_info(&inputs, &missing, &output);
        if let Err(e) = std::fs::write(r.env.cwd.join(path), bytes) {
            r.stderr.push_str(&format!(
                "error: failed to write to {path}: {}\n",
                run::io_text(&e)
            ));
        }
    }
    if c.version {
        print_version(r);
    }
    let mut settings = crate::settings::Settings::default();
    if let Some(text) = &r.env.turbolib_options {
        settings
            .apply_env(text)
            .map_err(|e| err(format!("TURBOLIB_OPTIONS: {e}")))?;
    }
    for o in &c.turbolib {
        settings.apply(o).map_err(err)?;
    }
    settings.check_supported().map_err(err)?;
    if let Some(n) = settings.threads {
        turbonop_par::set_threads(n);
    }
    r.settings = settings;

    let mut reader = Reader {
        run: r,
        deterministic,
        arch,
        no_warning_for_no_symbols: c.no_warning_for_no_symbols,
        warnings_as_errors: c.warnings_as_errors,
        groups: Groups::default(),
    };
    for f in &inputs {
        reader.add_file(f)?;
    }
    let groups = reader.groups;
    if let Some(a) = &c.arch_only
        && groups.by_arch.is_empty()
    {
        return Err(err(format!(
            "no library created (no object files in input files matching -arch_only {a})"
        )));
    }
    // Members named twice (by their names), each architecture's.
    let mut text = String::new();
    for members in groups.by_arch.values() {
        let mut names: Vec<(&str, Vec<&str>)> = Vec::new();
        for m in members {
            match names.iter_mut().find(|(n, _)| *n == m.item.name) {
                Some((_, files)) => files.push(&m.file),
                None => names.push((&m.item.name, vec![&m.file])),
            }
        }
        for (name, files) in names.iter().filter(|(_, f)| f.len() > 1) {
            text.push_str(&format!("file '{name}' was specified multiple times.\n"));
            for f in files {
                text.push_str(&format!("in: {f}\n"));
            }
            text.push('\n');
        }
    }
    if !text.is_empty() {
        if c.warnings_as_errors {
            return Err(err(text));
        }
        r.stderr.push_str(&format!("warning: {text}\n"));
    }
    let plans: Vec<Plan> = groups
        .by_arch
        .into_values()
        .map(|members| Plan {
            members: members.into_iter().map(|m| m.item).collect(),
            thin: false,
            dialect: Dialect::Darwin,
            symtab: true,
            deterministic,
        })
        .collect();
    if plans.is_empty() {
        // (No members: an archive of none, as writeArchive writes it.)
        let plan = Plan {
            members: Vec::new(),
            thin: false,
            dialect: Dialect::Darwin,
            symtab: true,
            deterministic,
        };
        return run::write_plan(r, &output, plan).map_err(darwin_error);
    }
    if plans.len() == 1 {
        let plan = plans.into_iter().next().expect("one plan");
        return run::write_plan(r, &output, plan).map_err(darwin_error);
    }
    write_universal(r, &output, plans).map_err(darwin_error)
}

/// The personality's errors are worded `error: ...`, without the program's
/// name.
fn darwin_error(Fatal(text): Fatal) -> Fatal {
    match text.split_once(": error: ") {
        Some((_, rest)) => Fatal(format!("error: {rest}")),
        None => Fatal(text),
    }
}

fn print_version(r: &mut Run<'_>) {
    r.stdout.extend_from_slice(
        format!(
            "turbolib {} (TurboNOP), an archiver compatible with llvm-libtool-darwin\n",
            env!("CARGO_PKG_VERSION")
        )
        .as_bytes(),
    );
}

/// A universal file of one archive per architecture: each slice's CPU type
/// and subtype the first member's header's, aligned to 8 bytes (2^3) for
/// 64-bit members and 4 otherwise, sorted as `Slice`'s `operator<` sorts
/// them (by type and subtype for the same type, arm64-family last, else by
/// alignment), stably, from the architectures' order.
fn write_universal(r: &mut Run<'_>, output: &str, plans: Vec<Plan>) -> Result<(), Fatal> {
    let mut slices = Vec::with_capacity(plans.len());
    for plan in plans {
        let first = plan
            .members
            .first()
            .map(|m| m.data().to_vec())
            .unwrap_or_default();
        let (cputype, cpusubtype, is64) = match turbonop_macho::MachO::parse(&first) {
            Ok(o) => (o.header().cputype, o.header().cpusubtype, o.header().is64),
            Err(_) => (0, 0, false),
        };
        let bytes = run::plan_bytes(r, output, plan)?;
        slices.push((cputype, cpusubtype, if is64 { 3u32 } else { 2 }, bytes));
    }
    slices.sort_by(|a, b| {
        use std::cmp::Ordering;
        if a.0 == b.0 {
            return a.1.cmp(&b.1);
        }
        if a.0 == CPU_TYPE_ARM64 {
            return Ordering::Greater;
        }
        if b.0 == CPU_TYPE_ARM64 {
            return Ordering::Less;
        }
        a.2.cmp(&b.2)
    });
    let shape: Vec<(u32, u32, u64, u32)> = slices
        .iter()
        .map(|s| (s.0, s.1, s.3.len() as u64, s.2))
        .collect();
    let (head, offsets) = turbonop_macho::fat_layout(&shape)
        .ok_or_else(|| Fatal("error: fat file too large to be created".into()))?;
    let zeros = [0u8; 8];
    let mut pieces: Vec<&[u8]> = vec![&head];
    let mut at = head.len() as u64;
    for (s, &o) in slices.iter().zip(&offsets) {
        let mut gap = usize::try_from(o - at).unwrap_or(0);
        while gap > 0 {
            let n = gap.min(zeros.len());
            pieces.push(&zeros[..n]);
            gap -= n;
        }
        pieces.push(&s.3);
        at = o + s.3.len() as u64;
    }
    let path = r.env.cwd.join(output);
    let temp = crate::output::write_pieces_beside(&path, &pieces)
        .map_err(|e| Fatal(format!("error: {output}: {}", run::io_text(&e))))?;
    crate::output::replace(&temp, &path)
        .map_err(|e| Fatal(format!("error: {output}: {}", run::io_text(&e))))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn options() {
        let w = |s: &[&str]| s.iter().map(|x| (*x).to_string()).collect::<Vec<_>>();
        let c = parse(w(&[
            "--static", "-o", "x.a", "-lfoo", "-L", "d", "-Ld2", "a.o", "-D",
        ]))
        .unwrap();
        assert!(c.static_ && c.deterministic);
        assert_eq!(c.output.as_deref(), Some("x.a"));
        assert_eq!(c.libraries, ["foo"]);
        assert_eq!(c.search_dirs, ["d", "d2"]);
        assert_eq!(c.inputs, ["a.o"]);
        assert_eq!(
            parse(w(&["-static", "-bogus"])).unwrap_err(),
            "unknown argument '-bogus'"
        );
        assert_eq!(
            parse(w(&["-static", "-o"])).unwrap_err(),
            "-o: missing argument"
        );
        let c = parse(w(&["-static", "--fold=identical", "-j", "3", "a.o"])).unwrap();
        assert_eq!(c.turbolib, ["fold=identical", "threads=3"]);
    }

    #[test]
    fn universal_header() {
        let (head, offsets) = turbonop_macho::fat_layout(&[
            (7 | 0x0100_0000, 3, 0x310, 3),
            (0x0100_000C, 0, 0x2A0, 3),
        ])
        .unwrap();
        assert_eq!(offsets, [0x30, 0x340]);
        assert_eq!(&head[..8], &[0xCA, 0xFE, 0xBA, 0xBE, 0, 0, 0, 2]);
        assert_eq!(head.len(), 48);
    }

    #[test]
    fn dependency_record() {
        let d = dependency_info(&["x/a.o".into()], &[], "o.a");
        assert_eq!(d, b"\0llvm-libtool-darwin 24.0.0git\0\x10x/a.o\0\x40o.a\0");
    }

    #[test]
    fn groups() {
        assert_eq!(file_cpu(CPU_TYPE_ARM64, 1), (CPU_TYPE_ARM64, 0));
        assert_eq!(file_cpu(CPU_TYPE_ARM64, 0x8000_0002), (CPU_TYPE_ARM64, 2));
        assert_eq!(cpu_id(CPU_TYPE_X86, 3), u64::from(CPU_TYPE_X86));
        assert_eq!(arch_cpu("x86_64"), (CPU_TYPE_X86_64, 3));
    }
}
