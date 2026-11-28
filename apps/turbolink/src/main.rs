//! turbolink: a PE/COFF linker that takes `link.exe`'s command line (see
//! `docs/turbolink.md`).

use turbonop_global_alloc as _;

use std::process::ExitCode;

use turbonop_linker::{args, config};

/// What `/HELP` (or `/?`, or no arguments) prints.
const USAGE: &str = "turbolink: a linker that takes link.exe's command line

usage: turbolink [options] [files] [@commandfile]
       turbolink /LIB [lib.exe's options and files]   (turbolib.exe, beside turbolink)

Options are link.exe's (/OUT, /DLL, /DEBUG, /ENTRY, /SUBSYSTEM, /DEF,
/EXPORT, /MERGE, /SECTION, /OPT, /GUARD, ...); see docs/turbolink.md.
";

/// `/LIB` first, as link.exe takes it: the library manager, which is
/// turbolib (beside turbolink) with the rest of the command line.
fn library_manager(rest: &[String]) -> ExitCode {
    let exe = std::env::current_exe()
        .ok()
        .and_then(|p| p.parent().map(|d| d.join("turbolib.exe")))
        .filter(|p| p.is_file());
    let Some(exe) = exe else {
        eprintln!("turbolink : fatal error: /LIB needs turbolib.exe in turbolink's directory");
        return ExitCode::FAILURE;
    };
    match std::process::Command::new(&exe).args(rest).status() {
        Ok(s) if s.success() => ExitCode::SUCCESS,
        Ok(s) => ExitCode::from(u8::try_from(s.code().unwrap_or(1)).unwrap_or(1)),
        Err(e) => {
            eprintln!("turbolink : fatal error: can't run {}: {e}", exe.display());
            ExitCode::FAILURE
        }
    }
}

fn main() -> ExitCode {
    let argv: Vec<String> = std::env::args().skip(1).collect();
    let option = |a: &str, name: &str| {
        a.strip_prefix(['/', '-'])
            .is_some_and(|o| o.eq_ignore_ascii_case(name))
    };
    if argv.first().is_some_and(|a| option(a, "lib")) {
        return library_manager(&argv[1..]);
    }
    if argv.is_empty() || argv.iter().any(|a| option(a, "help") || option(a, "?")) {
        print!("{USAGE}");
        return ExitCode::SUCCESS;
    }
    let link = std::env::var("LINK").ok();
    let link_after = std::env::var("_LINK_").ok();
    let words = match args::command_line(argv, link.as_deref(), link_after.as_deref(), &|p| {
        std::fs::read(p)
    }) {
        Ok(w) => w,
        Err(e) => {
            eprintln!("turbolink : fatal error: {e}");
            return ExitCode::FAILURE;
        }
    };
    let mut diags = Vec::new();
    let config = config::parse(&words, &mut diags);
    let mut failed = false;
    for d in &diags {
        match d {
            config::Diagnostic::Unknown(o) => {
                eprintln!("turbolink : warning LNK4044: unrecognized option '{o}'; ignored");
            }
            config::Diagnostic::NotYet(o) => {
                eprintln!("turbolink : error: '{o}' is not supported yet");
                failed = true;
            }
            config::Diagnostic::Ignored(_) => {}
            config::Diagnostic::Bad(m) => {
                eprintln!("turbolink : fatal error: {m}");
                failed = true;
            }
        }
    }
    if failed {
        return ExitCode::FAILURE;
    }
    if config.inputs.is_empty() {
        eprintln!("turbolink : fatal error: no input files");
        return ExitCode::FAILURE;
    }
    let time = config.time;
    let outcome = turbonop_linker::link::link(config);
    for d in &outcome.diagnostics {
        let kind = if d.warning { "warning" } else { "error" };
        if d.code == 0 {
            eprintln!("turbolink : {kind}: {}", d.message);
        } else {
            eprintln!("turbolink : {kind} LNK{}: {}", d.code, d.message);
        }
    }
    if time {
        for (name, t) in &outcome.times {
            eprintln!("turbolink: {name}: {:.3} ms", t.as_secs_f64() * 1000.0);
        }
    }
    if outcome.written {
        ExitCode::SUCCESS
    } else {
        ExitCode::FAILURE
    }
}
