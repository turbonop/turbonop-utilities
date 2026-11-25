//! The `stackrabbit` command-line driver (see the library for the commands).

use std::process::ExitCode;

// mimalloc gives each thread its own heap: the import, the tables' readers
// and the trees allocate on every core at once.
#[global_allocator]
static ALLOCATOR: mimalloc::MiMalloc = mimalloc::MiMalloc;

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    stackrabbit::run(&args)
}
