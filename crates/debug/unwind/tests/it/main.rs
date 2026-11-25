//! The integration tests, as one binary (one link, not one per file):
//! each module was a file of its own.

mod support;

mod any;
mod apple_compact;
mod arm_ehabi;
mod arm_ehabi_readobj;
mod dwarf_cfi;
mod windows_arm32;
mod windows_arm64;
mod windows_x64;
mod windows_x86;

// (Tests allocate with mimalloc, as the programs do.)
#[cfg(not(target_family = "wasm"))]
#[global_allocator]
static TEST_ALLOCATOR: mimalloc::MiMalloc = mimalloc::MiMalloc;
