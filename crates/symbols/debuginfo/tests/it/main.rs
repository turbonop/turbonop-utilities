//! The integration tests, as one binary (one link, not one per file):
//! each module was a file of its own.

mod dwarf;
mod hostile_dwarf;
mod hostile_pdb;
mod locate;
mod options;
mod pdb;
mod split_dwarf;
mod values;
mod values_golden;
mod xz;

// (Tests allocate with mimalloc, as the programs do.)
#[cfg(not(target_family = "wasm"))]
#[global_allocator]
static TEST_ALLOCATOR: mimalloc::MiMalloc = mimalloc::MiMalloc;
