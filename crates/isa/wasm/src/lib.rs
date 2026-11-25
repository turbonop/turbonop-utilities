//! The WebAssembly instruction set: [`code`] decodes a function body's
//! instructions and their immediates, and [`ops`] lists every instruction
//! (through GC, threads and relaxed SIMD).
//!
//! Finding the function bodies in a module is `turbonop-wasm`'s job.
//! Malformed code gives errors, never a panic.

#![no_std]
#![warn(missing_docs)]

extern crate alloc;

pub mod code;
pub mod ops;

// (Tests allocate with mimalloc, as the programs do.)
#[cfg(all(test, not(target_family = "wasm")))]
#[global_allocator]
static TEST_ALLOCATOR: mimalloc::MiMalloc = mimalloc::MiMalloc;
