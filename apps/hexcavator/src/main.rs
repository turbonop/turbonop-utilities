//! The `hexcavator` command-line driver (see the library for the commands).

use std::process::ExitCode;

// musl's allocator takes one lock for every thread's allocations, which
// leaves most cores waiting on it in hexcavator's parallel work; mimalloc
// gives each thread its own heap. (Other platforms' allocators already do.)
#[cfg(target_env = "musl")]
#[global_allocator]
static ALLOCATOR: mimalloc::MiMalloc = mimalloc::MiMalloc;

// WebAssembly's threads (the web version's): wasi-libc's allocator, too,
// takes one lock for every thread's allocations; hexcavator-alloc caches
// small blocks in shards over it.
#[cfg(target_family = "wasm")]
#[global_allocator]
static ALLOCATOR: hexcavator_alloc::Cached<std::alloc::System> =
    hexcavator_alloc::Cached::new(std::alloc::System);

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    hexcavator::run(&args)
}
