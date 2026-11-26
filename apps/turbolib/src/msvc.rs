//! The `lib` personality: `lib.exe`'s and `llvm-lib`'s command lines,
//! followed as each tool behaves (measured with `lib.exe` 14.52 and read in
//! LLVM's `LibDriver.cpp`), so turbolib can stand in for either.

mod args;
mod def;
mod diag;
mod inputs;
mod library;
mod options;
mod order;
mod run;

pub use args::Flavor;
pub use run::{Env, run};
