//! Running the `lib` personality: from the command line to what is printed,
//! written, and returned.
//!
//! `lib.exe` prints everything, diagnostics included, on standard output
//! (measured); `llvm-lib` prints diagnostics on standard error. Under
//! `lib.exe`'s flavor, turbolib prints its own one-line banner where
//! `lib.exe` prints Microsoft's, unless `/NOLOGO` is given.

use super::args::{self, Flavor};
use super::diag::{Diag, Kind, Reporter};
use super::inputs::{self, InputKind, MachineCheck};
use super::library;
use super::options::{self, Options};
use super::order::{self, Input};
use crate::output;
use crate::settings::{Fold, Inject, Locals, Pick, Precomp, Pull, Settings};
use std::borrow::Cow;
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use turbonop_archive::write::{
    Fields, Layout, MemberHeader, MemberSymbol, SizeLimit, Style, WriteError,
};
use turbonop_fold::rewrite;

/// The environment a run sees.
#[derive(Clone, Debug, Default)]
pub struct Env {
    /// `_LIB_`: more options, read after the command line (`lib.exe` only).
    pub lib_options: Option<String>,
    /// `LIB`: directories searched for inputs, separated by `;`.
    pub lib_path: Option<String>,
    /// `TURBOLIB_OPTIONS`: turbolib's own options, read before
    /// `/TURBOLIB:`'s.
    pub turbolib_options: Option<String>,
    /// The current directory, against which relative paths are read.
    pub cwd: PathBuf,
}

impl Env {
    /// The process's environment.
    pub fn from_process() -> Env {
        Env {
            lib_options: std::env::var("_LIB_").ok(),
            lib_path: std::env::var("LIB").ok(),
            turbolib_options: std::env::var("TURBOLIB_OPTIONS").ok(),
            cwd: std::env::current_dir().unwrap_or_default(),
        }
    }
}

/// turbolib's options: `TURBOLIB_OPTIONS`, then each `/TURBOLIB:`. A bad one
/// is fatal, worded as `lib.exe` words a bad option (LNK1117).
fn settings(flavor: Flavor, opts: &Options, env: &Env) -> Result<Settings, Diag> {
    let fail = |text: String| match flavor {
        Flavor::Lib => Diag::fatal(1117, text),
        Flavor::LlvmLib => Diag::llvm(Kind::Fatal, text),
    };
    let mut s = Settings::default();
    if let Some(text) = &env.turbolib_options {
        s.apply_env(text)
            .map_err(|e| fail(format!("TURBOLIB_OPTIONS: {e}")))?;
    }
    for o in &opts.turbolib {
        s.apply(o)
            .map_err(|e| fail(format!("syntax error in option 'TURBOLIB:{o}' ({e})")))?;
    }
    s.check_supported()
        .map_err(|e| fail(format!("turbolib: {e}")))?;
    Ok(s)
}

/// What a run printed and returned.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Outcome {
    pub stdout: String,
    pub stderr: String,
    pub exit: i32,
}

/// The banner printed without `/NOLOGO`, followed by a blank line as
/// `lib.exe`'s is.
pub fn banner() -> String {
    format!(
        "turbolib {} (TurboNOP), a library manager compatible with lib.exe\n\n",
        env!("CARGO_PKG_VERSION")
    )
}

struct Out {
    flavor: Flavor,
    stdout: String,
    stderr: String,
}

impl Out {
    /// A diagnostic, on the stream the tool uses for them.
    fn diag(&mut self, s: &str) {
        let to = match self.flavor {
            Flavor::Lib => &mut self.stdout,
            Flavor::LlvmLib => &mut self.stderr,
        };
        to.push_str(s);
        to.push('\n');
    }
}

/// Runs the personality on `args` (the words after the program's name).
pub fn run(flavor: Flavor, args: &[String], env: &Env) -> Outcome {
    let mut out = Out {
        flavor,
        stdout: String::new(),
        stderr: String::new(),
    };
    let exit = run_inner(flavor, args, env, &mut out);
    Outcome {
        stdout: out.stdout,
        stderr: out.stderr,
        exit,
    }
}

fn run_inner(flavor: Flavor, args: &[String], env: &Env, out: &mut Out) -> i32 {
    let mut read = |p: &str| std::fs::read(resolve(&env.cwd, p));
    let lib_env = match flavor {
        Flavor::Lib => env.lib_options.as_deref(),
        Flavor::LlvmLib => None,
    };
    let words = match args::expand(flavor, args, lib_env, &mut read) {
        Ok(w) => w,
        Err(e) => {
            // lib.exe reads response files before /NOLOGO takes effect, so
            // its banner comes first (measured).
            let d = match flavor {
                Flavor::Lib => {
                    out.stdout.push_str(&banner());
                    Diag::fatal(1104, format!("cannot open file '{}'", e.path))
                }
                Flavor::LlvmLib => Diag::llvm(Kind::Fatal, e.error.to_string()),
            };
            out.diag(&d.render(flavor));
            return exit_of(flavor, &d);
        }
    };
    let (opts, parse_diags) = match options::parse(flavor, &words) {
        Ok(p) => p,
        Err(d) => {
            // So are /WX: and /IGNORE: numbers.
            if flavor == Flavor::Lib && d.code == 1147 {
                out.stdout.push_str(&banner());
            }
            out.diag(&d.render(flavor));
            return exit_of(flavor, &d);
        }
    };
    if flavor == Flavor::Lib && !opts.nologo {
        out.stdout.push_str(&banner());
    }
    if opts.help {
        out.stdout.push_str(&help(flavor));
        out.stdout.push('\n');
        return 0;
    }
    let mut r = Reporter::new(flavor, opts.warnings.clone());
    for d in parse_diags {
        if r.report(d).is_err() {
            return finish(out, r);
        }
    }
    let settings = match settings(flavor, &opts, env) {
        Ok(s) => s,
        Err(d) => {
            let _ = r.report(d);
            return finish(out, r);
        }
    };
    if let Some(n) = settings.threads {
        turbonop_par::set_threads(n);
    }
    let _ = run_mode(flavor, &opts, &settings, env, out, &mut r);
    finish(out, r)
}

fn exit_of(flavor: Flavor, d: &Diag) -> i32 {
    match flavor {
        Flavor::Lib => d.code as i32,
        Flavor::LlvmLib => 1,
    }
}

fn finish(out: &mut Out, r: Reporter) -> i32 {
    for l in &r.lines {
        out.diag(l);
    }
    r.exit
}

/// `p` against the current directory.
pub(super) fn resolve(cwd: &Path, p: &str) -> PathBuf {
    let path = Path::new(p);
    if path.is_absolute() {
        path.to_path_buf()
    } else {
        cwd.join(path)
    }
}

/// Where an input is found: as given, then in each `/LIBPATH` directory,
/// then in each of `LIB`'s.
fn find_input(opts: &Options, env: &Env, name: &str) -> Option<PathBuf> {
    let direct = resolve(&env.cwd, name);
    if direct.is_file() {
        return Some(direct);
    }
    if Path::new(name).is_absolute() {
        return None;
    }
    let lib_dirs = env
        .lib_path
        .as_deref()
        .unwrap_or("")
        .split(';')
        .filter(|d| !d.is_empty())
        .map(str::to_string);
    opts.libpath
        .iter()
        .cloned()
        .chain(lib_dirs)
        .map(|d| resolve(&env.cwd, &d).join(name))
        .find(|p| p.is_file())
}

/// An input read into memory.
pub(super) struct Read {
    pub(super) name: String,
    pub(super) path: PathBuf,
    pub(super) data: turbonop_mmap::Mmap,
}

pub(super) fn read_input(
    opts: &Options,
    env: &Env,
    name: &str,
    flavor: Flavor,
) -> Result<Read, Diag> {
    let missing = || match flavor {
        Flavor::Lib => Diag::fatal(1181, format!("cannot open input file '{name}'")),
        Flavor::LlvmLib => Diag::llvm(Kind::Fatal, "no such file or directory").on(name),
    };
    let path = find_input(opts, env, name).ok_or_else(missing)?;
    let data = turbonop_mmap::Mmap::open(&path).map_err(|_| missing())?;
    Ok(Read {
        name: name.to_string(),
        path,
        data,
    })
}

/// A member's name as text (names are bytes; lib.exe writes them in the
/// code page it runs under, and llvm-lib in UTF-8).
fn text(name: &[u8]) -> String {
    String::from_utf8_lossy(name).into_owned()
}

fn run_mode(
    flavor: Flavor,
    opts: &Options,
    settings: &Settings,
    env: &Env,
    out: &mut Out,
    r: &mut Reporter,
) -> Result<(), ()> {
    if opts.def.is_some() {
        return super::def::run(flavor, opts, env, r);
    }
    if let Some(list_to) = &opts.list {
        return list(flavor, opts, env, out, r, list_to.as_deref());
    }
    if !opts.extract.is_empty() {
        return extract(flavor, opts, env, r);
    }
    build(flavor, opts, settings, env, r)
}

/// `/LIST`: the first input that is a library, its members in reverse
/// archive order, on standard output or in the named file. Other inputs
/// are ignored, and nothing is written (measured, as `llvm-lib` also does).
fn list(
    flavor: Flavor,
    opts: &Options,
    env: &Env,
    out: &mut Out,
    r: &mut Reporter,
    to: Option<&str>,
) -> Result<(), ()> {
    for name in &opts.inputs {
        let input = match read_input(opts, env, name, flavor) {
            Ok(i) => i,
            Err(d) => return r.report(d),
        };
        let Ok(archive) = turbonop_archive::Archive::parse(&input.data) else {
            continue;
        };
        let mut names: Vec<String> = archive.files().map(|m| text(&m.name)).collect();
        names.reverse();
        let mut listing = String::new();
        for n in names {
            listing.push_str(&n);
            listing.push('\n');
        }
        match to {
            // lib.exe's diagnostics share standard output, so the listing
            // goes in order with them.
            None if flavor == Flavor::Lib => r.lines.extend(listing.lines().map(str::to_string)),
            None => out.stdout.push_str(&listing),
            Some(file) => {
                if std::fs::write(resolve(&env.cwd, file), listing).is_err() {
                    return r.report(Diag::fatal(1104, format!("cannot open file '{file}'")));
                }
            }
        }
        return Ok(());
    }
    Ok(())
}

/// `/EXTRACT:member`: the member, from the first input library, to `/OUT`
/// or (measured) to the member's file name in the current directory.
/// Members are matched as `lib.exe` matches names, without case; a member
/// not found is LNK4014.
fn extract(flavor: Flavor, opts: &Options, env: &Env, r: &mut Reporter) -> Result<(), ()> {
    let Some(name) = opts.inputs.first() else {
        return Ok(());
    };
    let input = match read_input(opts, env, name, flavor) {
        Ok(i) => i,
        Err(d) => return r.report(d),
    };
    let Ok(archive) = turbonop_archive::Archive::parse(&input.data) else {
        return r.report(Diag::fatal(1107, "invalid or corrupt file").on(input.name.clone()));
    };
    for want in &opts.extract {
        let key = order::key(want);
        let found = archive.files().find(|m| order::key(&text(&m.name)) == key);
        let Some(member) = found else {
            r.report(Diag::warning(
                4014,
                format!("cannot find member object {want}"),
            ))?;
            continue;
        };
        let to = match &opts.out {
            Some(o) => o.clone(),
            None => {
                let n = text(&member.name);
                n.rsplit(['\\', '/']).next().unwrap_or(&n).to_string()
            }
        };
        if write_file(&resolve(&env.cwd, &to), member.data).is_err() {
            return r.report(Diag::fatal(1104, format!("cannot open file '{to}'")));
        }
    }
    Ok(())
}

pub(super) fn write_file(path: &Path, data: &[u8]) -> io::Result<()> {
    let mut f = std::fs::File::create(path)?;
    f.write_all(data)
}

/// Making or updating a library.
fn build(
    flavor: Flavor,
    opts: &Options,
    settings: &Settings,
    env: &Env,
    r: &mut Reporter,
) -> Result<(), ()> {
    if opts.inputs.is_empty() && !opts.empty {
        // Measured: lib.exe writes nothing and says nothing. llvm-lib warns.
        if flavor == Flavor::LlvmLib && !opts.warnings.ignored.contains(&options::EMPTY_OUTPUT) {
            r.report(Diag::llvm(
                Kind::Warning,
                "warning: no input files, not writing output file\n         pass \
                 /llvmlibempty to write empty .lib file,\n         pass \
                 /ignore:emptyoutput to suppress warning",
            ))?;
        }
        return Ok(());
    }
    let mut clock = Clock::new();
    let mut read = Vec::new();
    for name in &opts.inputs {
        match read_input(opts, env, name, flavor) {
            Ok(i) => read.push(i),
            Err(d) => return r.report(d),
        }
    }
    clock.lap("open");
    // Measured: lib.exe given one library and nothing else (no objects, no
    // /REMOVE) copies it, byte for byte, dates and tables as they were
    // (with /Brepro, /MACHINE, /NAME, /WX too), and says nothing. turbolib
    // does the same unless it is to fold. (Its own tables would differ
    // where the input's weren't made from its members, as ARM64X import
    // libraries' /<ECSYMBOLS>/ aren't.)
    // When folding, too, the library unfolded (nothing folded, or folding
    // failed) is that copy, and lib.exe's warnings (LNK4068, LNK4006),
    // which it doesn't give for a copy, aren't given.
    let folding =
        settings.fold != Fold::None || settings.precomp != Precomp::None || settings.report_odr;
    let lone = flavor == Flavor::Lib
        && read.len() == 1
        && opts.remove.is_empty()
        && turbonop_archive::Archive::parse(&read[0].data).is_ok_and(|a| !a.is_thin());
    if lone && !folding {
        return copy_lone(opts, env, read, r);
    }
    // The threads to run: with others' turbolib processes in mind (see
    // `slots`), unless a number was given.
    let mut slots = crate::slots::Slots::default();
    let total: u64 = read.iter().map(|i| i.data.len() as u64).sum();
    let sharing = settings.threads.is_none() && std::env::var_os("TURBONOP_THREADS").is_none();
    if sharing {
        crate::slots::share(&mut slots, total);
    }
    let mut check = MachineCheck::new(opts.machine.map(options::Machine::number), flavor);
    let mut inputs = Vec::new();
    let mut kinds = Vec::new();
    for i in &read {
        let kind = match inputs::classify(flavor, &i.name, &i.data) {
            Ok(k) => k,
            Err(d) => return r.report(d),
        };
        kinds.push(kind);
        if kind != InputKind::Library {
            if flavor == Flavor::LlvmLib
                && let Err(d) = check.llvm(&i.name, kind)
            {
                return r.report(d);
            }
            inputs.push(Input::Object(i.name.clone()));
            continue;
        }
        let Ok(archive) = turbonop_archive::Archive::parse(&i.data) else {
            unreachable!("classified as a library")
        };
        let mut names = Vec::new();
        for m in archive.files() {
            let name = text(&m.name);
            if flavor == Flavor::LlvmLib {
                let checked =
                    inputs::classify(flavor, &name, m.data).and_then(|k| check.llvm(&name, k));
                if let Err(d) = checked {
                    return r.report(d);
                }
            }
            names.push(name);
        }
        inputs.push(Input::Library(i.name.clone(), names));
    }
    if flavor == Flavor::Lib {
        for (i, kind) in read.iter().zip(&kinds) {
            if *kind != InputKind::Library
                && let Err(d) = check.lib_object(&i.name, *kind)
            {
                return r.report(d);
            }
        }
    }
    clock.lap("classify");
    let mut ordered = match flavor {
        Flavor::Lib => match order::lib_order(&inputs, &opts.remove, opts.verbose) {
            Ok(o) => o,
            Err(d) => return r.report(d),
        },
        Flavor::LlvmLib => order::llvm_order(&inputs),
    };
    if flavor == Flavor::Lib {
        // Library members' machines: see `members_for_machine`.
        let archives: Vec<Option<turbonop_archive::Archive<'_>>> = read
            .iter()
            .zip(&kinds)
            .map(|(i, k)| {
                (*k == InputKind::Library)
                    .then(|| turbonop_archive::Archive::parse(&i.data).ok())
                    .flatten()
            })
            .collect();
        let files: Vec<Vec<turbonop_archive::Member<'_>>> = archives
            .iter()
            .map(|a| a.map(|a| a.files().collect()).unwrap_or_default())
            .collect();
        let machine_of = |source: order::Source| match source {
            order::Source::Member(i, m) => inputs::classify(flavor, "", files[i][m].data)
                .ok()
                .and_then(InputKind::machine),
            order::Source::Object(_) => None,
        };
        let from_objects = check.machine.is_some();
        if !from_objects {
            check.machine = ordered.members.iter().find_map(|&s| machine_of(s));
        }
        if check.machine.is_none() && !lone {
            r.report(Diag::warning(
                4068,
                "/MACHINE not specified; defaulting to X64",
            ))?;
        }
        // (A lone library is lib.exe's copy, every member kept: folding it
        // must keep them too.)
        if let Some(target) = check.machine.filter(|_| !lone) {
            let explicit = opts.machine.is_some() || from_objects;
            let mut kept = Vec::with_capacity(ordered.members.len());
            for &s in &ordered.members {
                match members_for_machine(target, explicit, machine_of(s)) {
                    Ok(true) => kept.push(s),
                    Ok(false) => {}
                    Err(d) => {
                        let order::Source::Member(i, m) = s else {
                            unreachable!("objects are checked apart")
                        };
                        let place = format!(
                            "{}({})",
                            shown_file(flavor, &read[i].name),
                            text(&files[i][m].name)
                        );
                        return r.report(d.on(place));
                    }
                }
            }
            ordered.members = kept;
        }
    }
    r.lines.extend(ordered.messages.iter().cloned());
    for w in ordered.warnings {
        r.report(w)?;
    }
    if opts.thin {
        return r.report(Diag::llvm(
            Kind::Fatal,
            "turbolib can't write thin libraries (/llvmlibthin) yet",
        ));
    }

    // Each member's name, header fields, bytes, and name in messages.
    let style = match flavor {
        Flavor::Lib if opts.brepro => Style::LibExeBrepro,
        Flavor::Lib => Style::LibExe { time: now() },
        Flavor::LlvmLib => Style::LlvmLib,
    };
    let archives: Vec<Option<turbonop_archive::Archive<'_>>> = read
        .iter()
        .zip(&kinds)
        .map(|(i, k)| {
            (*k == InputKind::Library)
                .then(|| turbonop_archive::Archive::parse(&i.data).ok())
                .flatten()
        })
        .collect();
    let members_of: Vec<Vec<turbonop_archive::Member<'_>>> = archives
        .iter()
        .map(|a| a.map(|a| a.files().collect()).unwrap_or_default())
        .collect();
    let mut members: Vec<Placed<'_>> = Vec::with_capacity(ordered.members.len());
    for &source in &ordered.members {
        members.push(match source {
            order::Source::Object(i) => {
                let input = &read[i];
                let name = match flavor {
                    Flavor::Lib => order::lib_member_name(&input.name),
                    Flavor::LlvmLib => input.name.clone(),
                };
                let (mtime, read_only) = file_time(&input.path);
                Placed {
                    name: name.into_bytes(),
                    fields: style.object_fields(mtime, read_only),
                    data: &input.data,
                    place: shown_file(flavor, &input.name),
                }
            }
            order::Source::Member(i, m) => {
                let member = &members_of[i][m];
                let fields = match style {
                    Style::LibExe { .. } => Fields::of(member),
                    Style::LibExeBrepro => Fields::new(b"-1", b"", b"", trim(member.mode)),
                    Style::LlvmLib => None,
                }
                .unwrap_or_else(|| style.object_fields(0, false));
                Placed {
                    name: member.name.to_vec(),
                    fields,
                    data: member.data,
                    place: format!(
                        "{}({})",
                        shown_file(flavor, &read[i].name),
                        text(&member.name)
                    ),
                }
            }
        });
    }

    // lib.exe's warnings about names defined twice: folding moves no
    // strong definition, so they are the unfolded library's.
    let duplicates = if flavor == Flavor::Lib && !lone {
        // (Import objects' names too, outside ARM64EC and ARM64X archives;
        // in those, lib.exe warns about some of an import's names and not
        // others, which isn't modeled.)
        let ec_archive = matches!(
            opts.machine.map(options::Machine::number),
            Some(inputs::ARM64EC | inputs::ARM64X)
        );
        let imports: Vec<Vec<Vec<u8>>> = members
            .iter()
            .map(|m| {
                if ec_archive {
                    Vec::new()
                } else {
                    library::import_definitions(m.data)
                }
            })
            .collect();
        let strong: Vec<Vec<&[u8]>> =
            turbonop_par::map(&members, 16, |m| library::strong_definitions(m.data, None))
                .into_iter()
                .zip(&imports)
                .map(|(mut s, i)| {
                    s.extend(i.iter().map(Vec::as_slice));
                    s
                })
                .collect();
        let places: Vec<String> = members.iter().map(|m| m.place.clone()).collect();
        library::duplicate_warnings(&strong, &places)
    } else {
        Vec::new()
    };

    // Folding: the members rewritten, then the synthesized ones. Anything
    // that goes wrong in it is caught, and the library is then written
    // unfolded (see `guard`).
    clock.lap("order");
    let mut fell_back: Option<String> = None;
    let folded = if folding {
        match crate::guard::catch(|| fold(&members, settings)) {
            Ok(f) => Some(f),
            Err(why) => {
                fell_back = Some(why);
                None
            }
        }
    } else {
        None
    };
    // Folding that changed nothing (a library of import objects, or of
    // objects sharing nothing): the unfolded library is the same bytes,
    // and is written without a check.
    let unchanged = folded.as_ref().is_some_and(|f| {
        f.members.len() == members.len()
            && f.members.iter().zip(&members).all(|(o, m)| {
                !o.synthesized && o.holes.is_empty() && *o.name == *m.name && *o.data == *m.data
            })
    });
    if let Some(folded) = &folded {
        if settings.stats {
            r.lines.extend(stats_lines(&folded.report, &members));
            // What folding's members hold in memory (their bytes but
            // the holes, which are the inputs').
            let held = |synthesized: bool| {
                let (mut bytes, mut holes) = (0, 0);
                for m in folded
                    .members
                    .iter()
                    .filter(|m| m.synthesized == synthesized)
                {
                    if let Cow::Owned(d) = &m.data {
                        bytes += d.len();
                        holes += m.holes.len();
                    }
                }
                format!("{:.1} MiB ({holes} holes)", bytes as f64 / 1_048_576.0)
            };
            r.lines.push(format!(
                "turbolib: held: rewritten members {}, synthesized {}",
                held(false),
                held(true)
            ));
        }
        if settings.report_odr {
            r.lines.extend(odr_lines(&folded.report));
        }
    }
    for w in duplicates {
        r.report(w)?;
    }
    clock.lap("fold");
    // (lib.exe writes `/<ECSYMBOLS>/` only for `/MACHINE:ARM64EC` or
    // `ARM64X`: a machine it infers from a library's ARM64EC member gives
    // none, measured.)
    let ec = match flavor {
        Flavor::Lib => matches!(
            opts.machine.map(options::Machine::number),
            Some(inputs::ARM64EC | inputs::ARM64X)
        ),
        Flavor::LlvmLib => matches!(check.machine, Some(inputs::ARM64EC | inputs::ARM64X)),
    };
    let unfolded: Vec<Final<'_>> = members
        .iter()
        .map(|m| Final {
            name: &m.name,
            fields: m.fields,
            data: m.data,
            holes: &[],
            symbol_table: None,
            place: Cow::Borrowed(&m.place),
            index_only: None,
        })
        .collect();
    let out = output_path(opts, &read);
    let path = resolve(&env.cwd, &out);
    // lib.exe's words, then the system's reason on a line of its own.
    let cannot = |r: &mut Reporter, out: &str, e: &io::Error| {
        let d = match flavor {
            Flavor::Lib => Diag::fatal(1104, format!("cannot open file '{out}'")),
            Flavor::LlvmLib => Diag::llvm(Kind::Fatal, "cannot write the output").on(out),
        };
        let result = r.report(d);
        r.lines.push(format!("turbolib: {out}: {e}"));
        result
    };

    if sharing {
        // Slots others let go of since.
        crate::slots::share(&mut slots, total);
    }

    // The folded library, written beside the output and checked; or why
    // it can't be used.
    let mut temp: Option<output::Temp> = None;
    if let Some(folded) = folded.as_ref().filter(|_| !unchanged) {
        let finals = folded_finals(folded, &members, style);
        let write_output = r.write_output;
        let checked = std::cell::Cell::new(None);
        let attempt = crate::guard::catch(|| -> Result<Option<output::Temp>, Attempt> {
            if settings.inject == Some(Inject::Layout) {
                return Err(Attempt::Unusable("injected layout failure".into()));
            }
            let layout = lay_out(flavor, style, ec, &finals).map_err(|d| {
                Attempt::Unusable(format!("the folded library can't be laid out: {}", d.text))
            })?;
            if !write_output {
                return Ok(None);
            }
            let datas: Vec<output::Bytes<'_>> = finals.iter().map(Final::bytes).collect();
            // The self-check's original (the unfolded library, laid out and
            // read) on the other cores while the folded library is written.
            let start = std::time::Instant::now();
            let original = settings
                .verify
                .then(|| lay_out(flavor, style, ec, &unfolded))
                .transpose()
                .map_err(|d| {
                    Attempt::Unusable(format!(
                        "the unfolded library can't be laid out: {}",
                        d.text
                    ))
                })?;
            let options = verify_options(settings);
            let (written, read) = turbonop_par::join(
                || output::write_beside(&path, &layout, &datas, style.conventions().padding),
                || {
                    original.as_ref().map(|l| {
                        let members: Vec<turbonop_verify::library::LaidOut<'_>> = unfolded
                            .iter()
                            .enumerate()
                            .map(|(i, m)| (Cow::Borrowed(m.name), l.offset(i), m.data))
                            .collect();
                        let read =
                            turbonop_verify::library::read_laid_out(l.head(), members, &options);
                        (read, start.elapsed())
                    })
                },
            );
            let written = written.map_err(Attempt::Io)?;
            if let Some((read, originals)) = read {
                let read = read.map_err(Attempt::Unusable)?;
                let times =
                    self_check(&read, &written, &options, settings).map_err(Attempt::Unusable)?;
                checked.set(Some((start.elapsed(), originals, times)));
            }
            Ok(Some(written))
        });
        match attempt {
            Ok(Ok(t)) => temp = t,
            // The disk, not folding: the unfolded library would fail the
            // same way.
            Ok(Err(Attempt::Io(e))) => return cannot(r, &out, &e),
            Ok(Err(Attempt::Unusable(why))) | Err(why) => fell_back = Some(why),
        }
        clock.lap("archive");
        if let (true, Some((all, originals, (rewritten, compared)))) =
            (settings.stats, checked.get())
        {
            r.lines.push(format!(
                "turbolib: self-check {:.3} s (in archive): original laid out and read {:.3} (as the folded library was written), written library read {:.3}, compared {:.3}",
                all.as_secs_f64(),
                originals.as_secs_f64(),
                rewritten.as_secs_f64(),
                compared.as_secs_f64(),
            ));
        }
    }
    if let Some(why) = &fell_back {
        if settings.strict {
            let text = format!("turbolib: folding failed: {why}");
            return r.report(match flavor {
                Flavor::Lib => Diag::fatal(1000, text),
                Flavor::LlvmLib => Diag::llvm(Kind::Fatal, text),
            });
        }
        r.lines.push(format!(
            "turbolib: warning: folding {out} failed, so it was written unfolded: {why}"
        ));
    }
    if lone && (folded.is_none() || fell_back.is_some() || unchanged) {
        drop(unfolded);
        drop(folded);
        drop(members);
        drop(members_of);
        drop(archives);
        return copy_lone(opts, env, read, r);
    }
    if folded.is_none() || fell_back.is_some() || unchanged {
        let layout = match lay_out(flavor, style, ec, &unfolded) {
            Ok(l) => l,
            Err(d) => return r.report(d),
        };
        if !r.write_output {
            return Ok(());
        }
        let datas: Vec<output::Bytes<'_>> = unfolded.iter().map(Final::bytes).collect();
        match output::write_beside(&path, &layout, &datas, style.conventions().padding) {
            Ok(t) => temp = Some(t),
            Err(e) => return cannot(r, &out, &e),
        }
        clock.lap("archive");
    }
    let Some(temp) = temp else {
        return Ok(());
    };
    // The inputs are mapped, and the output may be one of them (`lib
    // x.lib a.obj` updates x.lib): let them go before the new file takes
    // its place.
    drop(unfolded);
    drop(folded);
    drop(members);
    drop(members_of);
    drop(archives);
    drop(read);
    if let Err(e) = temp.replace(&path) {
        return cannot(r, &out, &e);
    }
    clock.lap("write");
    if settings.stats {
        r.lines.push(clock.report());
    }
    Ok(())
}

/// Whether `lib.exe` keeps a library's member of `machine` in a library for
/// `target` (`explicit`: given by `/MACHINE` or an object, rather than
/// inferred from the first member), or the error it stops with. lib.exe
/// doesn't check library members against the machine as it checks
/// objects, but measured with 14.52 on import libraries `lib /DEF` made:
///
/// - for ARM64, it leaves out ARM64EC and x64 members, without a word (so
///   an ARM64X library archived again without `/MACHINE` loses its EC
///   half, its first member being ARM64);
/// - for an ARM64EC machine inferred from the first member, x64 members;
/// - for x64, an ARM64 or ARM64EC member is LNK1392;
/// - x86 and x64 members together are kept, and so is everything for
///   ARM64X, and for ARM64EC given by `/MACHINE`.
fn members_for_machine(target: u16, explicit: bool, machine: Option<u16>) -> Result<bool, Diag> {
    use inputs::{ARM64, ARM64EC, X64};
    let Some(m) = machine else {
        return Ok(true);
    };
    Ok(match target {
        ARM64 => !matches!(m, ARM64EC | X64),
        ARM64EC if !explicit => m != X64,
        X64 if matches!(m, ARM64 | ARM64EC) => {
            return Err(Diag::fatal(
                1392,
                "Incompatible object type found. Did you mean /machine:arm64x or /machine:arm64ec?",
            ));
        }
        _ => true,
    })
}

/// lib.exe's library for one library given alone (no objects, no
/// `/REMOVE`): the input, byte for byte.
fn copy_lone(opts: &Options, env: &Env, read: Vec<Read>, r: &mut Reporter) -> Result<(), ()> {
    if !r.write_output {
        return Ok(());
    }
    let out = output_path(opts, &read);
    let path = resolve(&env.cwd, &out);
    let written = output::write_pieces_beside(&path, &[&read[0].data]);
    // (The output may be the input: let it go first.)
    drop(read);
    if let Err(e) = written.and_then(|t| t.replace(&path)) {
        let failed = r.report(Diag::fatal(1104, format!("cannot open file '{out}'")));
        r.lines.push(format!("turbolib: {out}: {e}"));
        return failed;
    }
    Ok(())
}

/// Why a folded library wasn't used.
enum Attempt {
    /// Writing it failed (the disk, the output's place).
    Io(io::Error),
    /// It can't be used: the unfolded library is written instead.
    Unusable(String),
}

/// Folds `members` as `settings` say.
fn fold<'d>(members: &'d [Placed<'_>], settings: &Settings) -> rewrite::Output<'d> {
    if settings.inject == Some(Inject::Panic) {
        // On worker threads, as a panic in folding would be.
        let items = [0u8; 64];
        turbonop_par::map(&items, 1, |_| -> u8 { panic!("injected panic") });
    }
    let inputs: Vec<rewrite::Input<'_>> = members
        .iter()
        .map(|m| rewrite::Input {
            name: &m.name,
            data: m.data,
        })
        .collect();
    let pull = match settings.pull {
        Pull::Preserve => rewrite::Pull::Preserve,
        Pull::Relaxed => rewrite::Pull::Relaxed,
    };
    rewrite::rewrite(
        &inputs,
        rewrite::Options {
            pull,
            weak_externals: false,
            fold: match (settings.fold, settings.pick) {
                (Fold::None, _) => rewrite::Fold::None,
                (Fold::Identical, _) => rewrite::Fold::Identical,
                (Fold::SelectAny, Pick::First) => rewrite::Fold::SelectAny(rewrite::Pick::First),
                (Fold::SelectAny, Pick::Largest) => {
                    rewrite::Fold::SelectAny(rewrite::Pick::Largest)
                }
            },
            locals: match settings.locals {
                Locals::Keep => rewrite::Locals::Keep,
                Locals::Copy => rewrite::Locals::Copy,
                Locals::Merge => rewrite::Locals::Merge,
            },
            odr: settings.report_odr,
            precomp: match settings.precomp {
                Precomp::Library => rewrite::Precomp::Library { min: 2 },
                Precomp::Clustered(max) => rewrite::Precomp::Clustered {
                    min: 2,
                    max,
                    weight: turbonop_fold::rewrite::precomp::cluster::WEIGHT,
                },
                Precomp::None => rewrite::Precomp::None,
            },
        },
    )
}

/// The members as written after folding: the rewritten ones in their
/// order, with the synthesized ones among them.
fn folded_finals<'d>(
    folded: &'d rewrite::Output<'_>,
    members: &'d [Placed<'_>],
    style: Style,
) -> Vec<Final<'d>> {
    let synthesized = style.object_fields(now(), false);
    let mut original = 0;
    folded
        .members
        .iter()
        .map(|m| {
            let from = (!m.synthesized).then(|| {
                original += 1;
                &members[original - 1]
            });
            Final {
                name: &m.name,
                fields: from.map_or(synthesized, |f| f.fields),
                data: &m.data,
                holes: &m.holes,
                symbol_table: m.symbol_table,
                index_only: m.index_only.as_deref(),
                place: from.map_or_else(
                    || Cow::Owned(text(&m.name)),
                    |f| Cow::Borrowed(f.place.as_ref()),
                ),
            }
        })
        .collect()
}

/// The archive `finals` make: its index (the members' symbols) and
/// layout.
fn lay_out<'d>(
    flavor: Flavor,
    style: Style,
    ec: bool,
    finals: &[Final<'d>],
) -> Result<Layout<'d>, Diag> {
    let symbols = turbonop_par::map(finals, 16, |m| {
        library::member_symbols(flavor, &m.place, m.data, m.symbol_table, ec)
    });
    let mut member_symbols = Vec::with_capacity(symbols.len());
    for (s, m) in symbols.into_iter().zip(finals) {
        let mut s = s?;
        if let Some(names) = m.index_only {
            s.symbols = names
                .iter()
                .map(|n| MemberSymbol::defined(n.as_slice()))
                .collect();
        } else {
            // Never indexed, so it loads no member of its own (see
            // ALIAS_DEFAULT): a synthesized member's, and one an input
            // library's synthesized member defines (a library folded
            // before, archived again).
            s.symbols
                .retain(|x| *x.name != *turbonop_fold::rewrite::ALIAS_DEFAULT);
        }
        member_symbols.push(s);
    }
    let index = library::index(flavor, member_symbols, ec);
    let headers: Vec<MemberHeader<'d>> = finals
        .iter()
        .map(|m| MemberHeader {
            name: m.name,
            fields: m.fields,
            size: m.len() as u64,
        })
        .collect();
    Layout::new(&headers, &index, style, SizeLimit::FourGiB).map_err(|e| write_error(flavor, &e))
}

/// The self-check's options: the modes folding used.
fn verify_options(settings: &Settings) -> turbonop_verify::library::Options {
    use turbonop_verify::library as v;
    v::Options {
        pull: match settings.pull {
            Pull::Preserve => v::Pull::Preserve,
            Pull::Relaxed => v::Pull::Relaxed,
        },
        fold: match settings.fold {
            Fold::SelectAny => v::Fold::SelectAny,
            Fold::None | Fold::Identical => v::Fold::Identical,
        },
        locals: settings.locals != Locals::Keep,
    }
}

/// Checks the folded library written at `written` against `original`, the
/// library turbolib would have written without folding, read
/// (`turbonop_verify`, in the modes folding used), and says why it fails if
/// it does; or how long reading the written library and comparing took.
fn self_check(
    original: &turbonop_verify::library::Read<'_>,
    written: &output::Temp,
    options: &turbonop_verify::library::Options,
    settings: &Settings,
) -> Result<(std::time::Duration, std::time::Duration), String> {
    use turbonop_verify::library as v;
    let start = std::time::Instant::now();
    let map = turbonop_mmap::Mmap::open(written.path())
        .map_err(|e| format!("the folded library can't be read back: {e}"))?;
    let rewritten = v::read_rewritten(&map, options)?;
    let read = start.elapsed();
    let report = v::compare(original, &rewritten, options);
    let compared = start.elapsed() - read;
    if settings.inject == Some(Inject::Verify) {
        return Err("the self-check failed: injected failure".into());
    }
    // The index checks of the unfolded library itself (lib.exe's rules for
    // its index, which `turbonop_verify` models but doesn't follow
    // everywhere: ARM64 import libraries' EC symbols, members that win a
    // name) aren't about folding; nor is the same check failing on the
    // folded library.
    let index_check = |c: &str| {
        [
            "index entries no member defines",
            "index entries naming a member other than the first definer",
            "definitions missing from the index",
            "second linker members not sorted",
            "linker members that disagree",
        ]
        .iter()
        .any(|k| c.ends_with(k))
    };
    let original_failed = |c: &str| {
        report.failed(&format!(
            "original {}",
            c.strip_prefix("rewritten ").unwrap_or(c)
        ))
    };
    match report.findings().find(|(s, c, _)| {
        *s == turbonop_verify::report::Severity::Failure
            && !(index_check(c) && (c.starts_with("original ") || original_failed(c)))
    }) {
        None => Ok((read, compared)),
        Some((_, category, f)) => Err(format!(
            "the self-check failed: {category} ({}{})",
            f.count,
            f.examples
                .first()
                .map(|e| format!(", e.g. {e}"))
                .unwrap_or_default()
        )),
    }
}

/// How long each stage took, for `--stats`.
struct Clock {
    start: std::time::Instant,
    last: std::time::Instant,
    laps: Vec<(&'static str, std::time::Duration)>,
}

impl Clock {
    fn new() -> Clock {
        let now = std::time::Instant::now();
        Clock {
            start: now,
            last: now,
            laps: Vec::new(),
        }
    }

    fn lap(&mut self, stage: &'static str) {
        let now = std::time::Instant::now();
        self.laps.push((stage, now - self.last));
        self.last = now;
    }

    fn report(&self) -> String {
        let mut s = format!("turbolib: {:.3} s:", (self.last - self.start).as_secs_f64());
        for (stage, d) in &self.laps {
            s.push_str(&format!(" {stage} {:.3}", d.as_secs_f64()));
        }
        s
    }
}

/// One member as written: after folding, its data may be new.
struct Final<'d> {
    name: &'d [u8],
    fields: Fields,
    data: &'d [u8],
    holes: &'d [(usize, &'d [u8])],
    /// Where `data` has the symbol table, if it has holes.
    symbol_table: Option<u32>,
    place: Cow<'d, str>,
    /// The index's names for it, if not those it defines (folding's).
    index_only: Option<&'d [Vec<u8>]>,
}

impl<'d> Final<'d> {
    fn bytes(&self) -> output::Bytes<'d> {
        output::Bytes {
            data: self.data,
            holes: self.holes,
        }
    }
}

impl Final<'_> {
    /// The member's length: its bytes and its holes'.
    fn len(&self) -> usize {
        self.data.len() + self.holes.iter().map(|h| h.1.len()).sum::<usize>()
    }
}

/// `--report odr`'s findings: each name whose definitions differ, with
/// the members holding each definition. Information, not warnings.
fn odr_lines(r: &rewrite::Report) -> Vec<String> {
    let mut out = vec![format!(
        "turbolib: odr: {} names with differing definitions ({} of them in one environment)",
        r.odr.len(),
        r.odr.iter().filter(|f| f.same_environment).count()
    )];
    for f in &r.odr {
        let more = match f.names.len() {
            0 | 1 => String::new(),
            n => format!(" (and {} more names)", n - 1),
        };
        out.push(format!(
            "turbolib: odr: {}{more}: {} definitions{}",
            f.names.first().map_or_else(String::new, |n| text(n)),
            f.variants.len(),
            if f.same_environment {
                ""
            } else {
                ", each in other environments (compiler options) only"
            }
        ));
        for (i, v) in f.variants.iter().enumerate() {
            let members: Vec<String> = v.members.iter().map(|m| text(m)).collect();
            out.push(format!(
                "turbolib: odr:   {}: {} bytes, selection {}: {}",
                i + 1,
                v.size,
                v.selection,
                members.join(", ")
            ));
        }
    }
    out
}

fn stats_lines(r: &rewrite::Report, members: &[Placed<'_>]) -> Vec<String> {
    let mb = |b: u64| b as f64 / 1_048_576.0;
    let mut out = vec![
        format!(
            "turbolib: {} members, {} fold units; {} groups folded, {} copies removed",
            r.members, r.units, r.groups, r.copies_removed
        ),
        format!(
            "turbolib: {} members rewritten, {} synthesized; {:.1} MiB to {:.1} MiB ({:.1}% saved)",
            r.rewritten,
            r.synthesized,
            mb(r.bytes_in),
            mb(r.bytes_out),
            100.0 * (1.0 - r.bytes_out as f64 / (r.bytes_in.max(1)) as f64)
        ),
    ];
    let t = &r.times;
    out.push(format!(
        "turbolib: fold: read {:.3} definers {:.3} group {:.3} rewrite {:.3} synthesize {:.3}",
        t[0].as_secs_f64(),
        t[1].as_secs_f64(),
        t[2].as_secs_f64(),
        t[3].as_secs_f64(),
        t[4].as_secs_f64()
    ));
    let ty = &r.types;
    if ty.pchs > 0 || ty.candidates > 0 {
        out.push(format!(
            "turbolib: types: {} shared records ({:.1} MiB) in {} precompiled-types member(s); \
             {} of {} members share them ({} PCH users absorbed), {} share too little, {} unreadable; \
             their .debug$T {:.1} MiB to {:.1} MiB; {:.3} s",
            ty.shared_records,
            mb(ty.shared_bytes),
            ty.pchs,
            ty.sharing,
            ty.candidates,
            ty.absorbed,
            ty.nothing_shared,
            ty.unreadable,
            mb(ty.types_before),
            mb(ty.types_after),
            ty.time.as_secs_f64()
        ));
        if let Some(w) = &ty.first_unreadable {
            out.push(format!("turbolib: types: the first unreadable: {w}"));
        }
        for c in &ty.clusters {
            let model = |x: &turbonop_fold::rewrite::precomp::cluster::Cost| {
                format!(
                    "{:.1} MiB, {:.1} MiB per link",
                    mb(x.library),
                    mb(x.per_link())
                )
            };
            let tried: Vec<String> = c
                .tried
                .iter()
                .map(|(t, x)| format!("{t}: {}", model(x)))
                .collect();
            out.push(format!(
                "turbolib: types: clustered (machine {:#06x}): {} group(s) of {} members, {} out; \
                 the model's shared-type bytes and reads: {} (one group: {}; none: {}); tried {}",
                c.machine,
                c.groups,
                c.members,
                c.out,
                model(&c.cost),
                model(&c.library),
                model(&c.none),
                tried.join("; ")
            ));
        }
    }
    if r.differing_groups > 0 || !r.mismatches.is_empty() {
        out.push(format!(
            "turbolib: select-any: {} groups folded whose copies differ; {} names whose SAME_SIZE or EXACT_MATCH copies disagree, left as they are",
            r.differing_groups,
            r.mismatches.len()
        ));
        for x in &r.mismatches {
            let copies: Vec<String> = x
                .copies
                .iter()
                .map(|&(m, size, checksum)| {
                    format!(
                        "{} ({size} bytes, checksum {checksum:#010x})",
                        members.get(m).map_or_else(String::new, |p| text(&p.name))
                    )
                })
                .collect();
            out.push(format!(
                "turbolib: select-any: {} (selection {}): {}",
                x.names.first().map_or_else(String::new, |n| text(n)),
                x.selection,
                copies.join(", ")
            ));
        }
    }
    let l = &r.locals;
    if l.units_copied + l.pieces_copied + l.absorbed > 0 {
        out.push(format!(
            "turbolib: locals: {} local units and {} pieces copied ({:.1} MiB); {} local units moved with what reaches them ({:.1} MiB)",
            l.units_copied,
            l.pieces_copied,
            mb(l.bytes_copied),
            l.absorbed,
            mb(l.absorbed_bytes)
        ));
    }
    for (why, n) in &r.passed_through {
        out.push(format!(
            "turbolib: {n} members passed through: {}",
            why.describe()
        ));
    }
    for (why, n) in &r.not_folded {
        out.push(format!(
            "turbolib: {n} copies not folded: {}",
            why.describe()
        ));
    }
    out
}

/// How messages name an input file: `lib.exe` by its file name alone
/// (measured: `sub\a.obj` and `d/x.lib(b.obj)` are `a.obj` and
/// `x.lib(b.obj)` in LNK4006), `llvm-lib` as given.
fn shown_file(flavor: Flavor, name: &str) -> String {
    match flavor {
        Flavor::Lib => name.rsplit(['\\', '/']).next().unwrap_or(name).to_string(),
        Flavor::LlvmLib => name.to_string(),
    }
}

/// One output member, before writing.
struct Placed<'d> {
    name: Vec<u8>,
    fields: Fields,
    data: &'d [u8],
    /// How messages name it: `a.obj`, or `x.lib(a.obj)`.
    place: String,
}

fn trim(field: &[u8]) -> &[u8] {
    let end = field.iter().rposition(|&b| b != b' ').map_or(0, |p| p + 1);
    &field[..end]
}

/// Seconds since 1970, now.
pub(super) fn now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs())
}

/// A file's last-written time (seconds since 1970) and whether it is
/// read-only.
fn file_time(path: &Path) -> (u64, bool) {
    let Ok(meta) = std::fs::metadata(path) else {
        return (0, false);
    };
    let mtime = meta
        .modified()
        .ok()
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .map_or(0, |d| d.as_secs());
    (mtime, meta.permissions().readonly())
}

/// `/OUT`, or the first input's name with `.lib` for its extension (as
/// both tools name it).
fn output_path(opts: &Options, read: &[Read]) -> String {
    if let Some(o) = &opts.out {
        return o.clone();
    }
    let first = &read[0].name;
    let base = first.rfind(['\\', '/']).map_or(0, |i| i + 1);
    match first[base..].rfind('.') {
        Some(dot) => format!("{}.lib", &first[..base + dot]),
        None => format!("{first}.lib"),
    }
}

fn write_error(flavor: Flavor, e: &WriteError) -> Diag {
    match (flavor, e) {
        (Flavor::Lib, WriteError::TooLarge { size }) => Diag::fatal(
            1248,
            format!("image size ({size:X}) exceeds maximum allowable size (FFFFFFFF)"),
        ),
        (Flavor::Lib, WriteError::TooManyMembers { .. }) => {
            Diag::fatal(1189, "library limit of 65535 objects exceeded")
        }
        (Flavor::Lib, e) => Diag::fatal(1104, format!("cannot write the library: {e}")),
        (Flavor::LlvmLib, e) => Diag::llvm(Kind::Fatal, e.to_string()),
    }
}

fn help(flavor: Flavor) -> String {
    let tool = match flavor {
        Flavor::Lib => "lib.exe",
        Flavor::LlvmLib => "llvm-lib",
    };
    format!(
        "usage: lib [options] [files]\n\n\
         turbolib, run as {tool}, takes {tool}'s options. turbolib's own are\n\
         /TURBOLIB:<option>; see docs/turbolib.md."
    )
}

#[cfg(test)]
mod tests;
