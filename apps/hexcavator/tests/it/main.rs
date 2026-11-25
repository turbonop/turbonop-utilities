//! The integration tests, as one binary (one link, not one per file):
//! each module was a file of its own.

mod common;

mod batch;
mod checksec;
mod data;
mod debugmap;
mod deps;
mod dwarf;
mod info;
mod init;
mod layers;
mod licenses;
mod lines;
mod linkmap;
mod ltcg;
mod mcp;
mod mingw;
mod pdb;
mod query;
mod readobj;
mod relocations;
mod resources;
mod self_contained;
mod serve;
mod signature;
mod size;
mod snapshot;
mod symbolize;
mod targets;
mod types;
mod xref;

// (Tests allocate with mimalloc, as the programs do.)
#[cfg(not(target_family = "wasm"))]
#[global_allocator]
static TEST_ALLOCATOR: mimalloc::MiMalloc = mimalloc::MiMalloc;
