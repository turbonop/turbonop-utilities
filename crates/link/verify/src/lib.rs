//! Proving that turbolib's output is equivalent to its input.
//!
//! - [`library`]: `turbolib verify`, the static check: a rewritten
//!   library against the original, from the two files alone.
//! - [`image`]: `turbolib compare-images`, the link check: two programs
//!   (an image and its PDB each), linked from the original and the
//!   rewritten libraries, compared structurally, since folding changes
//!   addresses.
//! - [`pdb`]: the structural comparison of two PDBs that `compare-images`
//!   makes, by content rather than by index.
//! - [`report`]: what each found, by category, in plain words.
//!
//! See `docs/turbolib.md` ("Verification") for what each can and can't
//! prove.

// COFF, PE, and PDB sizes and offsets are 32-bit.
#![allow(clippy::cast_possible_truncation)]

#[cfg(test)]
use turbonop_global_alloc as _;

mod hash;
pub mod image;
pub mod library;
pub mod pdb;
pub mod report;
mod shard;
