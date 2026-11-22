//! The `hexcavator` command-line driver (see the library for the commands).

use std::process::ExitCode;

// musl's allocator takes one lock for every thread's allocations, which
// leaves most cores waiting on it in hexcavator's parallel work; mimalloc
// gives each thread its own heap. (Other platforms' allocators already do.)
#[cfg(target_env = "musl")]
#[global_allocator]
static ALLOCATOR: mimalloc::MiMalloc = mimalloc::MiMalloc;

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    hexcavator::run(&args)
}
