//! turbolink's engine: a PE/COFF linker that accepts what `link.exe`
//! accepts and does the same, on every core (see `docs/turbolink.md`).
//!
//! So far: the command line ([`args`]), as `link.exe` reads it from its
//! arguments, response files, the `LINK` and `_LINK_` environment
//! variables, and objects' `.drectve` sections.

#[cfg(test)]
use turbonop_global_alloc as _;

pub mod args;
pub mod config;
