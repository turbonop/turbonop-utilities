//! turbolib's engine: finding what a static library's members duplicate
//! (COMDATs and, under `/Z7`, CodeView types), and folding it.
//!
//! - [`member`]: what one member holds: its fold units (each COMDAT with
//!   everything that moves with it), its environment, its types hashed
//!   structurally, and the bytes of each by category.
//! - [`analyze`]: what folding would save in a library, under each of
//!   turbolib's modes, without writing anything (`turbolib analyze`).
//! - [`rewrite`]: folding: a library's members rewritten, and the
//!   synthesized members that hold what they shared.
//!
//! See `docs/turbolib.md` for the plan.

// COFF and CodeView sizes and offsets are 32-bit.
#![allow(clippy::cast_possible_truncation)]

#[cfg(test)]
use turbonop_global_alloc as _;

pub mod analyze;
pub mod elf;
mod hash;
pub mod member;
pub mod rewrite;
