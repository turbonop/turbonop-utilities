use turbonop_global_alloc as _;

use std::process::ExitCode;

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    dump_analyzer::main(&args)
}
