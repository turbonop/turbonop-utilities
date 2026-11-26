//! `turbolib verify` and `turbolib compare-images`: the commands that
//! prove a rewritten library, and programs linked from it, equivalent to
//! the original (`turbonop-verify`).

use std::process::ExitCode;
use std::time::Instant;

use turbonop_verify::library::{self, Pull};

const VERIFY_USAGE: &str = "\
usage: turbolib verify [options] <original library> <rewritten library>

Checks that the rewritten library is equivalent to the original: every
symbol still defined, with its kind; every original member there, in order,
its sections and references changed only as folding changes them; every
moved COMDAT held, identically, by exactly one synthesized member that the
member anchors; passed-through members byte-identical; and both archive
indexes consistent with their members. Prints what fails, with counts, and
exits with 1 if anything does.

options:
  --pull=preserve  every symbol of the original's index must still send to
                   the same member (the default)
  --pull=relaxed   report the symbols the index sends elsewhere, but don't
                   fail them
  -j N             use N threads (1: no threads but this one)
  --stats          say how long it took
";

const COMPARE_USAGE: &str = "\
usage: turbolib compare-images [options] <a.exe> <a.pdb> <b.exe> <b.pdb>

Checks that two programs, linked from the same inputs except for
turbolib's rewriting of libraries, are equivalent: the same functions and
data, with the same bytes once every address in them is mapped through
each program's symbols; the same imports, exports, initializers, TLS
callbacks, load configuration, CFG tables, and unwind data; and PDBs with
the same publics, globals, procedures, lines, inlinees, locals, and types,
compared by content. Prints what differs, by category, with counts and
examples, and exits with 1 if anything fails.

options:
  -j N             use N threads (1: no threads but this one)
  --stats          say how long it took
";

/// The options both commands share, and the paths given.
struct Common {
    stats: bool,
    pull: Pull,
    paths: Vec<String>,
}

fn parse(args: &[String], pull: bool) -> Result<Common, String> {
    let mut c = Common {
        stats: false,
        pull: Pull::Preserve,
        paths: Vec::new(),
    };
    let mut it = args.iter();
    while let Some(a) = it.next() {
        let threads = |v: &str| -> Result<(), String> {
            let n: usize = v.parse().map_err(|_| format!("-j {v}: not a number"))?;
            if n == 0 {
                return Err("-j 0: at least one thread".into());
            }
            turbonop_par::set_threads(n);
            Ok(())
        };
        match a.as_str() {
            "--stats" => c.stats = true,
            "--pull=preserve" if pull => c.pull = Pull::Preserve,
            "--pull=relaxed" if pull => c.pull = Pull::Relaxed,
            "-j" => threads(it.next().ok_or("-j needs a number")?)?,
            s if s.starts_with("-j") && s.len() > 2 => threads(&s[2..])?,
            s if s.starts_with('-') => return Err(format!("{s}: unknown option")),
            s => c.paths.push(s.to_string()),
        }
    }
    Ok(c)
}

fn map(path: &str) -> Result<turbonop_mmap::Mmap, String> {
    turbonop_mmap::Mmap::open(path).map_err(|e| format!("{path}: {e}"))
}

/// `turbolib verify`.
pub fn verify(args: &[String]) -> ExitCode {
    let c = match parse(args, true) {
        Ok(c) if c.paths.len() == 2 => c,
        Ok(_) => {
            eprint!("{VERIFY_USAGE}");
            return ExitCode::from(2);
        }
        Err(e) => {
            eprintln!("turbolib: {e}");
            eprint!("{VERIFY_USAGE}");
            return ExitCode::from(2);
        }
    };
    let start = Instant::now();
    let run = || -> Result<turbonop_verify::report::Report, String> {
        let a = map(&c.paths[0])?;
        let b = map(&c.paths[1])?;
        library::verify(&a, &b, &library::Options { pull: c.pull })
    };
    finish(run(), c.stats, start)
}

/// `turbolib compare-images`.
pub fn compare_images(args: &[String]) -> ExitCode {
    let c = match parse(args, false) {
        Ok(c) if c.paths.len() == 4 => c,
        Ok(_) => {
            eprint!("{COMPARE_USAGE}");
            return ExitCode::from(2);
        }
        Err(e) => {
            eprintln!("turbolib: {e}");
            eprint!("{COMPARE_USAGE}");
            return ExitCode::from(2);
        }
    };
    let start = Instant::now();
    let run = || -> Result<turbonop_verify::report::Report, String> {
        let files: Vec<turbonop_mmap::Mmap> =
            c.paths.iter().map(|p| map(p)).collect::<Result<_, _>>()?;
        turbonop_verify::image::compare(
            turbonop_verify::image::Program {
                image: &files[0],
                pdb: &files[1],
            },
            turbonop_verify::image::Program {
                image: &files[2],
                pdb: &files[3],
            },
        )
    };
    finish(run(), c.stats, start)
}

fn finish(
    result: Result<turbonop_verify::report::Report, String>,
    stats: bool,
    start: Instant,
) -> ExitCode {
    match result {
        Ok(report) => {
            print!("{}", report.text());
            if stats {
                eprintln!(
                    "turbolib: {:.2?} on {} threads",
                    start.elapsed(),
                    turbonop_par::threads()
                );
            }
            if report.passed() {
                ExitCode::SUCCESS
            } else {
                ExitCode::FAILURE
            }
        }
        Err(e) => {
            eprintln!("turbolib: {e}");
            ExitCode::from(2)
        }
    }
}
