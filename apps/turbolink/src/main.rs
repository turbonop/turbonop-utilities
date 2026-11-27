//! turbolink: a PE/COFF linker that takes `link.exe`'s command line (see
//! `docs/turbolink.md`).

use turbonop_global_alloc as _;

use std::process::ExitCode;

use turbonop_linker::{args, config};

fn main() -> ExitCode {
    let argv: Vec<String> = std::env::args().skip(1).collect();
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
    eprintln!("turbolink : fatal error: linking is not implemented yet");
    ExitCode::FAILURE
}
