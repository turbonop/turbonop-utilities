//! The integration tests, as one binary (one link, not one per file):
//! each module was a file of its own.

mod chained;
mod exports;
mod findings;
mod map;
mod relocations;
mod sizebench;
mod sources;
mod symbols;

// (Tests allocate with mimalloc, as the programs do.)
#[cfg(not(target_family = "wasm"))]
#[global_allocator]
static TEST_ALLOCATOR: mimalloc::MiMalloc = mimalloc::MiMalloc;
