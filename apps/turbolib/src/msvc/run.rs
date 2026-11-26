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
use super::output;
use crate::settings::{Fold, Pull, Settings};
use std::borrow::Cow;
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use turbonop_archive::write::{
    Fields, Layout, MemberHeader, NewMember, SizeLimit, Style, WriteError,
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
fn resolve(cwd: &Path, p: &str) -> PathBuf {
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
struct Read {
    name: String,
    path: PathBuf,
    data: turbonop_mmap::Mmap,
}

fn read_input(opts: &Options, env: &Env, name: &str, flavor: Flavor) -> Result<Read, Diag> {
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
        return r.report(Diag::fatal(
            1104,
            "turbolib can't write import libraries from /DEF yet",
        ));
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

fn write_file(path: &Path, data: &[u8]) -> io::Result<()> {
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
        for (i, kind) in read.iter().zip(&kinds) {
            if *kind == InputKind::Library
                && let Ok(archive) = turbonop_archive::Archive::parse(&i.data)
            {
                for m in archive.files() {
                    if let Ok(k) = inputs::classify(flavor, "", m.data) {
                        check.lib_member(k);
                    }
                }
            }
        }
        if check.machine.is_none() {
            r.report(Diag::warning(
                4068,
                "/MACHINE not specified; defaulting to X64",
            ))?;
        }
    }
    clock.lap("classify");
    let ordered = match flavor {
        Flavor::Lib => match order::lib_order(&inputs, &opts.remove, opts.verbose) {
            Ok(o) => o,
            Err(d) => return r.report(d),
        },
        Flavor::LlvmLib => order::llvm_order(&inputs),
    };
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
                    place: input.name.clone(),
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
                    place: format!("{}({})", read[i].name, text(&member.name)),
                }
            }
        });
    }

    // Folding: the members rewritten, then the synthesized ones.
    clock.lap("order");
    let folded = (settings.fold != Fold::None).then(|| {
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
            },
        )
    });
    let finals: Vec<Final<'_>> = match &folded {
        None => members
            .iter()
            .map(|m| Final {
                name: &m.name,
                fields: m.fields,
                data: m.data,
                holes: &[],
                place: Cow::Borrowed(&m.place),
            })
            .collect(),
        Some(folded) => {
            if settings.stats {
                r.lines.extend(stats_lines(&folded.report));
            }
            let synthesized = style.object_fields(now(), false);
            // The synthesized members come first; then the members, in
            // their order.
            let first = folded.members.iter().take_while(|m| m.synthesized).count();
            folded
                .members
                .iter()
                .enumerate()
                .map(|(i, m)| Final {
                    name: &m.name,
                    fields: if m.synthesized {
                        synthesized
                    } else {
                        members[i - first].fields
                    },
                    data: &m.data,
                    holes: &m.holes,
                    place: if m.synthesized {
                        Cow::Owned(text(&m.name))
                    } else {
                        Cow::Borrowed(&members[i - first].place)
                    },
                })
                .collect()
        }
    };

    // The index, and lib.exe's warnings about names defined twice.
    clock.lap("fold");
    let ec = matches!(check.machine, Some(inputs::ARM64EC | inputs::ARM64X));
    let symbols = turbonop_par::map(&finals, 16, |m| {
        library::member_symbols(flavor, &m.place, m.data, ec)
    });
    let mut member_symbols = Vec::with_capacity(symbols.len());
    for s in symbols {
        match s {
            Ok(s) => member_symbols.push(s),
            Err(d) => return r.report(d),
        }
    }
    if flavor == Flavor::Lib {
        let strong = turbonop_par::map(&finals, 16, |m| library::strong_definitions(m.data));
        let places: Vec<String> = finals.iter().map(|m| m.place.to_string()).collect();
        for w in library::duplicate_warnings(&strong, &places) {
            r.report(w)?;
        }
    }
    clock.lap("symbols");
    let index = library::index(flavor, member_symbols, ec);
    let new: Vec<NewMember<'_>> = finals
        .iter()
        .map(|m| NewMember {
            name: m.name,
            fields: m.fields,
            data: m.data,
        })
        .collect();
    clock.lap("index");
    let headers: Vec<MemberHeader<'_>> = new
        .iter()
        .map(|m| MemberHeader {
            name: m.name,
            fields: m.fields,
            size: m.data.len() as u64,
        })
        .collect();
    let layout = match Layout::new(&headers, &index, style, SizeLimit::FourGiB) {
        Ok(l) => l,
        Err(e) => return r.report(write_error(flavor, &e)),
    };
    if !r.write_output {
        return Ok(());
    }
    let out = output_path(opts, &read);
    let path = resolve(&env.cwd, &out);
    let cannot = |out: String| match flavor {
        Flavor::Lib => Diag::fatal(1104, format!("cannot open file '{out}'")),
        Flavor::LlvmLib => Diag::llvm(Kind::Fatal, "cannot write the output").on(out),
    };
    let datas: Vec<output::Bytes<'_>> = finals
        .iter()
        .map(|m| output::Bytes {
            data: m.data,
            holes: m.holes,
        })
        .collect();
    let Ok(temp) = output::write_beside(&path, &layout, &datas, style.conventions().padding) else {
        return r.report(cannot(out));
    };
    clock.lap("archive");
    // The inputs are mapped, and the output may be one of them (`lib
    // x.lib a.obj` updates x.lib): let them go before the new file takes
    // its place.
    drop(datas);
    drop(layout);
    drop(headers);
    drop(index);
    drop(new);
    drop(finals);
    drop(folded);
    drop(members);
    drop(members_of);
    drop(archives);
    drop(read);
    if output::replace(&temp, &path).is_err() {
        return r.report(cannot(out));
    }
    clock.lap("write");
    if settings.stats {
        r.lines.push(clock.report());
    }
    Ok(())
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
    place: Cow<'d, str>,
}

/// What `--stats` prints about folding.
fn stats_lines(r: &rewrite::Report) -> Vec<String> {
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
fn now() -> u64 {
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
