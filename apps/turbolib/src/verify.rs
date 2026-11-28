//! `turbolib verify` and `turbolib compare-images`: the commands that
//! prove a rewritten library, and programs linked from it, equivalent to
//! the original (`turbonop-verify`).

use std::process::ExitCode;
use std::time::Instant;

use turbonop_verify::library::{self, Depth, Fold, Pull};

const VERIFY_USAGE: &str = "\
usage: turbolib verify [options] <original library> <rewritten library>

(COFF libraries, or GNU and BSD archives of ELF objects; for ELF, only
--fold=identical and --locals=keep, the engine's modes, are checked.)

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
  --fold=identical moved COMDATs must be held identically (the default)
  --fold=select-any
                   or as another copy of their name the selection allows
  --locals=keep    units compared with the static symbols they reach by
                   name and place (the default)
  --locals=copy, --locals=merge
                   by content, and local COMDATs may be copied into any
                   number of synthesized members
  --check=full     compare COMDATs and sections by their whole contents
                   (bytes and relocations; the default)
  --check=links    compare them by their shape alone (each section's name,
                   characteristics, size, and recorded checksum): the
                   names defined, referred to, indexed, and loaded are
                   checked as in full, but not that a moved COMDAT is
                   identical to the copy kept, nor that what each member
                   keeps is unchanged byte for byte (COFF libraries only;
                   with --locals=copy or merge, the check is full)
  -j N             use N threads (1: no threads but this one)
  --examples=N     list N examples of each failure (0: all; default 5)
  --stats          say how long it took
";

const COMPARE_USAGE: &str = "\
usage: turbolib compare-images [options] <a.exe> <a.pdb> <b.exe> <b.pdb>
       turbolib compare-images [options] <a> <b>        (ELF programs, Mach-O images)

Checks that two programs, linked from the same inputs except for
turbolib's rewriting of libraries, are equivalent: the same functions and
data, with the same bytes once every address in them is mapped through
each program's symbols; the same imports, exports, initializers, TLS
callbacks, load configuration, CFG tables, and unwind data; and PDBs with
the same publics, globals, procedures, lines, inlinees, locals, and types,
compared by content. ELF programs are compared through their symbols,
dynamic relocations, and DWARF: functions and data by their bytes with
addresses mapped through each program's labels; the headers, dynamic
section, dynamic symbols and relocations, initializer arrays, notes, and
.eh_frame; and every live subprogram's DWARF, line rows, variables, and
type definitions, by content. Mach-O images are compared the same way,
through their symbols, fixups, and stubs: functions and data, symbols,
initializers, compact unwind information and FDEs, imports, and the DWARF
of each image's dSYM (<image>.dSYM, as dsymutil makes it), if both have
one. Prints what differs, by category, with
counts and examples, and exits with 1 if anything fails.

options:
  -j N             use N threads (1: no threads but this one)
  --examples=N     list N examples of each failure (0: all; default 5)
  --stats          say how long it took
";

/// The options both commands share, and the paths given.
struct Common {
    stats: bool,
    options: library::Options,
    paths: Vec<String>,
}

fn parse(args: &[String], pull: bool) -> Result<Common, String> {
    let mut c = Common {
        stats: false,
        options: library::Options::default(),
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
            s if s.starts_with("--examples=") => {
                let v = &s["--examples=".len()..];
                let n: usize = v
                    .parse()
                    .map_err(|_| format!("--examples={v}: not a number"))?;
                turbonop_verify::report::set_examples(if n == 0 { usize::MAX } else { n });
            }
            "--pull=preserve" if pull => c.options.pull = Pull::Preserve,
            "--pull=relaxed" if pull => c.options.pull = Pull::Relaxed,
            "--fold=identical" if pull => c.options.fold = Fold::Identical,
            "--fold=select-any" if pull => c.options.fold = Fold::SelectAny,
            "--locals=keep" if pull => c.options.locals = false,
            "--locals=copy" | "--locals=merge" if pull => c.options.locals = true,
            "--check=full" if pull => c.options.depth = Depth::Full,
            "--check=links" if pull => c.options.depth = Depth::Links,
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
        let coff_only = || {
            (c.options.depth == Depth::Links)
                .then_some(Err("--check=links: COFF libraries only".to_string()))
        };
        if turbonop_verify::elf::library::is_elf_archive(&a) {
            if let Some(e) = coff_only() {
                return e;
            }
            return turbonop_verify::elf::library::verify(&a, &b, &c.options);
        }
        if turbonop_verify::macho::is_macho_archive(&a) {
            if let Some(e) = coff_only() {
                return e;
            }
            return turbonop_verify::macho::verify(&a, &b, &c.options);
        }
        library::verify(&a, &b, &c.options)
    };
    finish(run(), c.stats, start)
}

/// `turbolib compare-images`.
pub fn compare_images(args: &[String]) -> ExitCode {
    let c = match parse(args, false) {
        Ok(c) if c.paths.len() == 4 || c.paths.len() == 2 => c,
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
        if files.len() == 2
            && files
                .iter()
                .all(|f| turbonop_verify::macho_image::is_macho_image(f))
        {
            // Each image's dSYM, where dsymutil puts it.
            let dsym = |p: &str| -> Option<turbonop_mmap::Mmap> {
                let name = std::path::Path::new(p).file_name()?;
                let d = std::path::PathBuf::from(format!("{p}.dSYM"))
                    .join("Contents/Resources/DWARF")
                    .join(name);
                map(d.to_str()?).ok()
            };
            let (da, db) = (dsym(&c.paths[0]), dsym(&c.paths[1]));
            let dsyms = match (&da, &db) {
                (Some(x), Some(y)) => Some((&x[..], &y[..])),
                _ => None,
            };
            return turbonop_verify::macho_image::compare(&files[0], &files[1], dsyms);
        }
        if files.len() == 2 {
            if !files.iter().all(|f| turbonop_verify::elf::is_elf(f)) {
                return Err("two files are compared as ELF programs: these aren't".into());
            }
            return turbonop_verify::elf::compare(&files[0], &files[1]);
        }
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

#[cfg(test)]
mod tests {
    use super::*;

    fn words(w: &[&str]) -> Vec<String> {
        w.iter().map(ToString::to_string).collect()
    }

    #[test]
    fn check_depths() {
        let c = parse(&words(&["a.lib", "b.lib"]), true).unwrap();
        assert_eq!(c.options.depth, Depth::Full);
        let c = parse(&words(&["--check=links", "a.lib", "b.lib"]), true).unwrap();
        assert_eq!(c.options.depth, Depth::Links);
        let c = parse(&words(&["--check=links", "--check=full", "a", "b"]), true).unwrap();
        assert_eq!(c.options.depth, Depth::Full);
        assert!(parse(&words(&["--check=names", "a", "b"]), true).is_err());
        // compare-images takes no --check.
        assert!(parse(&words(&["--check=links", "a", "b"]), false).is_err());
    }
}
