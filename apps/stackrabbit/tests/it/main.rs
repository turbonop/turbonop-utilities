//! The integration tests, as one binary (one link, not one per file).

mod cli;
mod common;
mod functions;
mod profile;
mod protocol;
mod saved_symbols;
mod serve;

// (Tests allocate with mimalloc, as the programs do.)
#[cfg(not(target_family = "wasm"))]
#[global_allocator]
static TEST_ALLOCATOR: mimalloc::MiMalloc = mimalloc::MiMalloc;
