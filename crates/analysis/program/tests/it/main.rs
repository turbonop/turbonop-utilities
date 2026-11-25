//! The integration tests, as one binary (one link, not one per file):
//! each module was a file of its own.

mod cache;
mod debug_map_paths;
mod split_dwarf;

// (Tests allocate with mimalloc, as the programs do.)
#[cfg(not(target_family = "wasm"))]
#[global_allocator]
static TEST_ALLOCATOR: mimalloc::MiMalloc = mimalloc::MiMalloc;
