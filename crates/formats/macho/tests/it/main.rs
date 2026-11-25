//! The integration tests, as one binary (one link, not one per file):
//! each module was a file of its own.

mod build_version;
mod debugmap;
mod dylibs;
mod fixups;
mod objc;
mod readobj;
mod unwind_info;

// (Tests allocate with mimalloc, as the programs do.)
#[cfg(not(target_family = "wasm"))]
#[global_allocator]
static TEST_ALLOCATOR: mimalloc::MiMalloc = mimalloc::MiMalloc;
