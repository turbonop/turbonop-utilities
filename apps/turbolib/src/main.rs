//! turbolib: a static-library archiver that keeps one copy of what C++
//! objects duplicate (see `docs/turbolib.md`).
//!
//! So far: `turbolib analyze`, which says what folding would save in
//! libraries, without writing anything; and the `lib` personality (run as
//! `lib` or `llvm-lib`, or as `turbolib lib ...`), which takes `lib.exe`'s
//! and `llvm-lib`'s command lines.

use turbonop_global_alloc as _;

mod msvc;

use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::time::{Duration, Instant};
use turbonop_fold::analyze::{self, Library, MODES, Summary};
use turbonop_fold::member::Category;

const USAGE: &str = "\
usage: turbolib analyze [options] <library or directory>...
       turbolib lib|llvm-lib <lib.exe's or llvm-lib's arguments>

Says what folding would save in each static library (.lib, .a; a directory
is searched for them), under each of turbolib's modes. Writes nothing.

options:
  -j N         use N threads (1: no threads but this one); default: one per core
  --json       print the figures as JSON
  --diagnose   say what each library holds that turbolib would pass through
  --stats      say how long each stage took
  --top N      list the N libraries that would save the most (default 15)
";

struct Options {
    json: bool,
    diagnose: bool,
    stats: bool,
    top: usize,
    paths: Vec<PathBuf>,
}

fn parse(args: &[String]) -> Result<Options, String> {
    let mut o = Options {
        json: false,
        diagnose: false,
        stats: false,
        top: 15,
        paths: Vec::new(),
    };
    let mut it = args.iter();
    while let Some(a) = it.next() {
        match a.as_str() {
            "--json" => o.json = true,
            "--diagnose" => o.diagnose = true,
            "--stats" => o.stats = true,
            "-j" | "--top" => {
                let v = it.next().ok_or_else(|| format!("{a} needs a number"))?;
                let n: usize = v.parse().map_err(|_| format!("{a} {v}: not a number"))?;
                if a == "-j" {
                    if n == 0 {
                        return Err("-j 0: at least one thread".into());
                    }
                    turbonop_par::set_threads(n);
                } else {
                    o.top = n;
                }
            }
            s if s.starts_with("-j") && s.len() > 2 => {
                let n: usize = s[2..].parse().map_err(|_| format!("{s}: not a number"))?;
                if n == 0 {
                    return Err("-j0: at least one thread".into());
                }
                turbonop_par::set_threads(n);
            }
            s if s.starts_with('-') => return Err(format!("{s}: unknown option")),
            s => o.paths.push(PathBuf::from(s)),
        }
    }
    if o.paths.is_empty() {
        return Err("no libraries given".into());
    }
    Ok(o)
}

/// The libraries `paths` name: files as given, and directories' `.lib`
/// and `.a` files (recursively, sorted).
fn libraries(paths: &[PathBuf]) -> Vec<PathBuf> {
    fn walk(dir: &Path, out: &mut Vec<PathBuf>) {
        let Ok(entries) = std::fs::read_dir(dir) else {
            return;
        };
        let mut entries: Vec<PathBuf> = entries.filter_map(|e| e.ok().map(|e| e.path())).collect();
        entries.sort();
        for p in entries {
            if p.is_dir() {
                walk(&p, out);
            } else if matches!(p.extension().and_then(|e| e.to_str()), Some("lib" | "a")) {
                out.push(p);
            }
        }
    }
    let mut out = Vec::new();
    for p in paths {
        if p.is_dir() {
            walk(p, &mut out);
        } else {
            out.push(p.clone());
        }
    }
    out
}

#[derive(Default)]
struct Times {
    open: Duration,
    analyze: Duration,
}

fn analyze_one(path: &Path, times: &mut Times) -> Result<Library, String> {
    let start = Instant::now();
    let map = turbonop_mmap::Mmap::open(path).map_err(|e| format!("{}: {e}", path.display()))?;
    let data: &[u8] = &map;
    let name = path.display().to_string();
    // Each member's name and data.
    let (names, datas): (Vec<Vec<u8>>, Vec<&[u8]>) = if turbonop_archive::Archive::is_archive(data)
    {
        let archive = turbonop_archive::Archive::parse(data).map_err(|e| format!("{name}: {e}"))?;
        if archive.is_thin() {
            return Err(format!("{name}: a thin archive (not yet read)"));
        }
        archive.files().map(|m| (m.name.to_vec(), m.data)).unzip()
    } else {
        // An object on its own: a library of one.
        let file = path
            .file_name()
            .map_or_else(Vec::new, |n| n.to_string_lossy().as_bytes().to_vec());
        (vec![file], vec![data])
    };
    let members: Vec<(&[u8], &[u8])> = names.iter().map(Vec::as_slice).zip(datas).collect();
    times.open += start.elapsed();
    let start = Instant::now();
    let lib = analyze::library(&name, data.len() as u64, &members);
    times.analyze += start.elapsed();
    Ok(lib)
}

fn size(n: u64) -> String {
    const UNITS: [&str; 5] = ["B", "KiB", "MiB", "GiB", "TiB"];
    let mut v = n as f64;
    let mut u = 0;
    while v >= 1024.0 && u + 1 < UNITS.len() {
        v /= 1024.0;
        u += 1;
    }
    if u == 0 {
        format!("{n} B")
    } else {
        format!("{v:.1} {}", UNITS[u])
    }
}

fn percent(part: u64, whole: u64) -> String {
    if whole == 0 {
        "-".into()
    } else {
        format!("{:.1}%", part as f64 * 100.0 / whole as f64)
    }
}

fn print_summary(s: &Summary) {
    println!(
        "{} {}, {} ({} objects, {} import members, {} LTCG objects, {} unrecognized)",
        s.libraries,
        if s.libraries == 1 {
            "library"
        } else {
            "libraries"
        },
        size(s.size),
        s.members.coff,
        s.members.import,
        s.members.ltcg,
        s.members.unrecognized
    );
    println!();
    println!("  {:<16} {:>12} {:>7}", "what", "bytes", "share");
    for c in Category::ALL {
        println!(
            "  {:<16} {:>12} {:>7}",
            c.name(),
            size(s.bytes.get(c)),
            percent(s.bytes.get(c), s.size)
        );
    }
    let c = &s.comdats;
    println!();
    println!(
        "COMDATs: {} units, {} names; {} names in more than one member ({} of them strong \
         definitions, never folded); {} names whose copies differ in one environment; \
         {} units reach internal-linkage symbols; {} names' archive-index definer has side effects",
        c.units,
        c.names,
        c.duplicated_names,
        c.strong_duplicates,
        c.differing_names,
        c.reaching_locals,
        c.pull_hazards
    );
    println!(
        "  COMDAT bytes: {} ({} of the libraries)",
        size(c.bytes.total()),
        percent(c.bytes.total(), s.size)
    );
    println!();
    println!(
        "  {:<26} {:>11} {:>7}   {:>10} {:>10} {:>10} {:>10}",
        "fold mode", "saves", "of all", "code", "unwind", "data", ".debug$S"
    );
    for (i, mode) in MODES.iter().enumerate() {
        let b = &c.saved[i];
        println!(
            "  {:<26} {:>11} {:>7}   {:>10} {:>10} {:>10} {:>10}",
            mode,
            size(b.total()),
            percent(b.total(), s.size),
            size(b.get(Category::Code)),
            size(b.get(Category::Unwind)),
            size(b.get(Category::Data)),
            size(b.get(Category::DebugSymbols))
        );
    }
    let t = &s.types;
    println!();
    println!(
        "Types (.debug$T): {} members hashed ({} /Zi type-server references, {} using a PCH, \
         {} PCH objects, {} unreadable); {} records naming a later one",
        t.members, t.type_server, t.precomp, t.pch, t.unreadable, t.forward_references
    );
    println!(
        "  {} records, {}; {} distinct ({}); {} in two or more members ({})",
        t.records,
        size(t.bytes),
        t.distinct_records,
        size(t.distinct_bytes),
        t.shared_records,
        size(t.shared_bytes)
    );
    println!(
        "  precomp=library: {} after ({} saved, {} of all); its precompiled-types member holds {}, \
         merged into a PDB whenever a link loads any member; {} of function ids stay duplicated (link.exe can't take them from a PCH)",
        size(t.after),
        size(t.saved),
        percent(t.saved, s.size),
        size(t.shared_bytes),
        size(t.unshareable_duplicate_bytes)
    );
    println!();
    println!(
        "Conservative default (identical, pull=preserve, locals=keep, precomp=library): saves {} of {} ({})",
        size(s.saved),
        size(s.size),
        percent(s.saved, s.size)
    );
}

fn run(o: &Options) -> Result<(), String> {
    let start = Instant::now();
    let paths = libraries(&o.paths);
    if paths.is_empty() {
        return Err("no libraries found".into());
    }
    let mut times = Times::default();
    let mut libs = Vec::new();
    let mut total = Summary::default();
    for p in &paths {
        match analyze_one(p, &mut times) {
            Ok(l) => {
                total.add(&l.summary);
                libs.push(l);
            }
            Err(e) => eprintln!("turbolib: {e}"),
        }
    }
    if o.json {
        #[derive(serde::Serialize)]
        struct Out<'a> {
            modes: &'a [&'a str],
            total: &'a Summary,
            libraries: &'a [Library],
        }
        let out = Out {
            modes: &MODES,
            total: &total,
            libraries: &libs,
        };
        println!(
            "{}",
            serde_json::to_string_pretty(&out).map_err(|e| e.to_string())?
        );
    } else {
        print_summary(&total);
        let mut by_saving: Vec<&Library> = libs.iter().collect();
        by_saving.sort_by_key(|l| std::cmp::Reverse(l.summary.saved));
        if o.top > 0 && libs.len() > 1 {
            println!();
            println!(
                "  {:>10} {:>10} {:>7}  library (by the conservative default's saving)",
                "size", "saves", ""
            );
            for l in by_saving.iter().take(o.top) {
                let s = &l.summary;
                println!(
                    "  {:>10} {:>10} {:>7}  {}",
                    size(s.size),
                    size(s.saved),
                    percent(s.saved, s.size),
                    l.path
                );
            }
        }
        if o.diagnose {
            println!();
            for l in &libs {
                for d in &l.diagnostics {
                    println!("{}: {d}", l.path);
                }
            }
        }
    }
    if o.stats {
        let all = start.elapsed();
        eprintln!(
            "turbolib: {} libraries, {} in {:.2?} ({:.0} MiB/s) on {} threads: opening {:.2?}, analyzing {:.2?}",
            libs.len(),
            size(total.size),
            all,
            total.size as f64 / 1_048_576.0 / all.as_secs_f64().max(1e-9),
            turbonop_par::threads(),
            times.open,
            times.analyze
        );
    }
    Ok(())
}

/// The personality the program's name selects: `lib` and `llvm-lib` (any
/// case, with or without `.exe`), or turbolib's own commands otherwise.
fn personality(argv0: &str) -> Option<msvc::Flavor> {
    // Either separator, whatever the platform: a Windows path can reach a
    // Linux build through Wine or a build system.
    let file = argv0.rsplit(['\\', '/']).next()?.to_ascii_lowercase();
    let stem = file.strip_suffix(".exe").unwrap_or(&file);
    match stem {
        "lib" => Some(msvc::Flavor::Lib),
        "llvm-lib" => Some(msvc::Flavor::LlvmLib),
        _ => None,
    }
}

/// Runs a `lib` personality and exits with its code, which for `lib.exe`
/// is the number of the error that stopped it (up to 4 digits), so it
/// can't be an `ExitCode`.
fn run_msvc(flavor: msvc::Flavor, args: &[String]) -> ! {
    use std::io::Write;
    let outcome = msvc::run(flavor, args, &msvc::Env::from_process());
    let _ = std::io::stdout().write_all(outcome.stdout.as_bytes());
    let _ = std::io::stdout().flush();
    let _ = std::io::stderr().write_all(outcome.stderr.as_bytes());
    std::process::exit(outcome.exit)
}

fn main() -> ExitCode {
    let mut argv = std::env::args();
    let argv0 = argv.next().unwrap_or_default();
    let args: Vec<String> = argv.collect();
    if let Some(flavor) = personality(&argv0) {
        run_msvc(flavor, &args);
    }
    let Some((command, rest)) = args.split_first() else {
        eprint!("{USAGE}");
        return ExitCode::from(2);
    };
    match command.as_str() {
        "lib" => run_msvc(msvc::Flavor::Lib, rest),
        "llvm-lib" => run_msvc(msvc::Flavor::LlvmLib, rest),
        "analyze" => {}
        _ => {
            eprintln!("turbolib: {command}: unknown command (so far: analyze, lib, llvm-lib)");
            eprint!("{USAGE}");
            return ExitCode::from(2);
        }
    }
    let o = match parse(rest) {
        Ok(o) => o,
        Err(e) => {
            eprintln!("turbolib: {e}");
            eprint!("{USAGE}");
            return ExitCode::from(2);
        }
    };
    match run(&o) {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("turbolib: {e}");
            ExitCode::FAILURE
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn personalities() {
        assert_eq!(personality("lib"), Some(msvc::Flavor::Lib));
        assert_eq!(personality(r"C:\tools\LIB.EXE"), Some(msvc::Flavor::Lib));
        assert_eq!(
            personality("/usr/bin/llvm-lib"),
            Some(msvc::Flavor::LlvmLib)
        );
        assert_eq!(personality("turbolib.exe"), None);
        assert_eq!(personality("libtool"), None);
    }
}
