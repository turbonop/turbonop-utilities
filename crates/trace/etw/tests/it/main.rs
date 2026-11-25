//! The integration tests, as one binary (one link, not one per file):
//! each module was a file of its own.

mod decode;
mod encode;
mod event_info;
mod manifest;
mod tracelogging;
mod values;

// (Tests allocate with mimalloc, as the programs do.)
#[cfg(not(target_family = "wasm"))]
#[global_allocator]
static TEST_ALLOCATOR: mimalloc::MiMalloc = mimalloc::MiMalloc;
