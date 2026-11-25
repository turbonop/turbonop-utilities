//! The `hexcavator` command-line driver (see the library for the commands).

use std::process::ExitCode;

// (mimalloc natively, a thread-caching allocator for WebAssembly's threads:
// see turbonop-global-alloc.)
use turbonop_global_alloc as _;

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    hexcavator::run(&args)
}
