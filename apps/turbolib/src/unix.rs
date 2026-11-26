//! The `ar` personality: GNU `ar`'s and `llvm-ar`'s command lines (and
//! their `ranlib`s'), followed as each tool behaves (read in binutils
//! 2.46's `ar.c` and `arsup.c` and LLVM 24's `llvm-ar.cpp`, and measured),
//! so turbolib can stand in for either.

mod args;
mod localtime;
mod mri;
mod run;

pub use args::{Flavor, flavor};
pub use run::{Env, run};
