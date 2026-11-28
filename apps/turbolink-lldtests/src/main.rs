//! turbolink-lldtests: runs lld's COFF tests (llvm-project's
//! `lld/test/COFF`, lit tests) with turbolink in lld-link's place, on every
//! core, and reports each test's class (see `classify`): a Markdown summary
//! (counts per class, tests per cause) and a TSV of every test.
//!
//! ```text
//! turbolink-lldtests --tests <llvm-project>/lld/test/COFF \
//!     --tools <llvm-build>/bin --turbolink <turbolink.exe> --work <dir> \
//!     [--report report.md] [--tsv results.tsv] [--filter text]
//!     [--bash <bash.exe>] [--timeout seconds] [--threads n]
//! ```

use turbonop_global_alloc as _;

mod classify;
mod lit;
mod run;

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::process::ExitCode;
use std::time::Duration;

use classify::Class;

/// The command line.
struct Args {
    tests: PathBuf,
    tools: PathBuf,
    turbolink: PathBuf,
    work: PathBuf,
    report: Option<PathBuf>,
    tsv: Option<PathBuf>,
    filter: Option<String>,
    bash: PathBuf,
    timeout: u64,
    threads: Option<usize>,
}

fn parse_args() -> Result<Args, String> {
    let mut a = std::env::args().skip(1);
    let get = |what: &str, a: &mut dyn Iterator<Item = String>| {
        a.next().ok_or(format!("{what} needs a value"))
    };
    let (mut tests, mut tools, mut turbolink, mut work) = (None, None, None, None);
    let (mut report, mut tsv, mut filter, mut threads) = (None, None, None, None);
    let mut bash = None;
    let mut timeout = 120;
    while let Some(arg) = a.next() {
        match arg.as_str() {
            "--tests" => tests = Some(PathBuf::from(get(&arg, &mut a)?)),
            "--tools" => tools = Some(PathBuf::from(get(&arg, &mut a)?)),
            "--turbolink" => turbolink = Some(PathBuf::from(get(&arg, &mut a)?)),
            "--work" => work = Some(PathBuf::from(get(&arg, &mut a)?)),
            "--report" => report = Some(PathBuf::from(get(&arg, &mut a)?)),
            "--tsv" => tsv = Some(PathBuf::from(get(&arg, &mut a)?)),
            "--filter" => filter = Some(get(&arg, &mut a)?),
            "--bash" => bash = Some(PathBuf::from(get(&arg, &mut a)?)),
            "--timeout" => {
                timeout = get(&arg, &mut a)?
                    .parse()
                    .map_err(|e| format!("--timeout: {e}"))?;
            }
            "--threads" => {
                threads = Some(
                    get(&arg, &mut a)?
                        .parse()
                        .map_err(|e| format!("--threads: {e}"))?,
                );
            }
            _ => return Err(format!("unknown argument {arg}")),
        }
    }
    // Git's bash (not Windows' bash.exe, which is WSL's).
    let bash = bash.unwrap_or_else(|| {
        let git = PathBuf::from(r"C:\Program Files\Git\bin\bash.exe");
        if git.is_file() {
            git
        } else {
            PathBuf::from("bash")
        }
    });
    Ok(Args {
        tests: tests.ok_or("--tests is needed")?,
        tools: tools.ok_or("--tools is needed")?,
        turbolink: turbolink.ok_or("--turbolink is needed")?,
        work: work.ok_or("--work is needed")?,
        report,
        tsv,
        filter,
        bash,
        timeout,
        threads,
    })
}

fn main() -> ExitCode {
    let args = match parse_args() {
        Ok(a) => a,
        Err(e) => {
            eprintln!("turbolink-lldtests: {e}");
            return ExitCode::FAILURE;
        }
    };
    if let Some(n) = args.threads {
        turbonop_par::set_threads(n);
    }
    // The tests: the directory's files (not Inputs/ or its other
    // subdirectories, which hold the tests' inputs).
    let mut names: Vec<String> = match std::fs::read_dir(&args.tests) {
        Ok(d) => d
            .filter_map(Result::ok)
            .filter(|e| e.file_type().is_ok_and(|t| t.is_file()))
            .filter_map(|e| e.file_name().into_string().ok())
            .filter(|n| {
                [".s", ".test", ".yaml", ".ll", ".c", ".cpp"]
                    .iter()
                    .any(|x| n.ends_with(x))
            })
            .filter(|n| args.filter.as_ref().is_none_or(|f| n.contains(f.as_str())))
            .collect(),
        Err(e) => {
            eprintln!("turbolink-lldtests: {}: {e}", args.tests.display());
            return ExitCode::FAILURE;
        }
    };
    names.sort();
    // lld-link, on PATH: turbolink.
    let shim = args.work.join("bin");
    if let Err(e) = std::fs::create_dir_all(&shim)
        .and_then(|()| std::fs::copy(&args.turbolink, shim.join("lld-link.exe")).map(|_| ()))
    {
        eprintln!(
            "turbolink-lldtests: can't put turbolink at {}: {e}",
            shim.display()
        );
        return ExitCode::FAILURE;
    }
    // (And turbolib beside it, if it's beside turbolink: `lld-link /lib`.)
    if let Some(lib) = args
        .turbolink
        .parent()
        .map(|d| d.join("turbolib.exe"))
        .filter(|p| p.is_file())
        && let Err(e) = std::fs::copy(&lib, shim.join("turbolib.exe"))
    {
        eprintln!("turbolink-lldtests: can't copy {}: {e}", lib.display());
        return ExitCode::FAILURE;
    }
    let setup = run::Setup {
        tests: args.tests.clone(),
        tools: args.tools.clone(),
        shim,
        work: args.work.join("tests"),
        bash: args.bash.clone(),
        timeout: Duration::from_secs(args.timeout),
    };
    let results: Vec<(String, Class, String)> = turbonop_par::map_balanced(&names, 1, |name| {
        let text = std::fs::read_to_string(setup.tests.join(name)).unwrap_or_default();
        let t = lit::parse(name, &text);
        if t.runs.is_empty() {
            return (
                name.clone(),
                Class::Skipped("no RUN lines".into()),
                String::new(),
            );
        }
        // (With the machines of the inputs it names: `%p/Inputs/x.yaml`.)
        let text = format!("{text}\n{}", classify::inputs_text(&t, &setup.tests));
        if let Some(why) = classify::skip(&t, &text) {
            return (name.clone(), Class::Skipped(why), String::new());
        }
        let o = run::run(&t, &setup);
        let detail = match &o {
            run::Outcome::Passed => String::new(),
            run::Outcome::Failed { index, line, .. } => format!("RUN {}: {line}", index + 1),
        };
        (name.clone(), classify::classify(&t, &o), detail)
    });
    let report = report(&results);
    print!("{}", summary(&results));
    if let Some(p) = &args.report
        && let Err(e) = std::fs::write(p, &report)
    {
        eprintln!("turbolink-lldtests: {}: {e}", p.display());
    }
    if let Some(p) = &args.tsv {
        let mut t = String::from("test\tclass\tcause\tfailing line\n");
        for (name, class, detail) in &results {
            let clean = |s: &str| s.replace(['\t', '\n'], " ");
            t.push_str(&format!(
                "{name}\t{}\t{}\t{}\n",
                class.name(),
                clean(class.cause()),
                clean(detail)
            ));
        }
        if let Err(e) = std::fs::write(p, t) {
            eprintln!("turbolink-lldtests: {}: {e}", p.display());
        }
    }
    ExitCode::SUCCESS
}

/// The counts per class.
fn summary(results: &[(String, Class, String)]) -> String {
    let mut counts: BTreeMap<&str, usize> = BTreeMap::new();
    for (_, c, _) in results {
        *counts.entry(c.name()).or_default() += 1;
    }
    let mut s = format!("{} tests\n", results.len());
    for (name, n) in counts {
        s.push_str(&format!("  {n:4}  {name}\n"));
    }
    s
}

/// The Markdown report: the counts, then each class's causes (most tests
/// first) and their tests.
fn report(results: &[(String, Class, String)]) -> String {
    let mut by_class: BTreeMap<&str, BTreeMap<&str, Vec<&str>>> = BTreeMap::new();
    for (name, c, _) in results {
        by_class
            .entry(c.name())
            .or_default()
            .entry(c.cause())
            .or_default()
            .push(name);
    }
    let mut s = String::from("| Class | Tests |\n|---|---:|\n");
    for (class, causes) in &by_class {
        s.push_str(&format!(
            "| {class} | {} |\n",
            causes.values().map(Vec::len).sum::<usize>()
        ));
    }
    s.push_str(&format!("| **all** | {} |\n", results.len()));
    for (class, causes) in &by_class {
        if *class == "passes" {
            continue;
        }
        s.push_str(&format!("\n### {class}\n\n"));
        let mut causes: Vec<_> = causes.iter().collect();
        causes.sort_by(|a, b| b.1.len().cmp(&a.1.len()).then(a.0.cmp(b.0)));
        for (cause, tests) in causes {
            let shown: Vec<&str> = tests.iter().take(6).copied().collect();
            let more = if tests.len() > 6 {
                format!(", and {} more", tests.len() - 6)
            } else {
                String::new()
            };
            let cause = if cause.is_empty() { "—" } else { cause };
            s.push_str(&format!(
                "- {} ({}): {}{more}\n",
                cause.replace('|', "\\|"),
                tests.len(),
                shown.join(", ")
            ));
        }
    }
    s
}
