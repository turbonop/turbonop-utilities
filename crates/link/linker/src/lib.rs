//! turbolink's engine: a PE/COFF linker that accepts what `link.exe`
//! accepts and does the same, on every core (see `docs/turbolink.md`).
//!
//! The phases: the command line ([`args`], [`config`]); the inputs read
//! and their symbols resolved, libraries searched as `link.exe` searches
//! them ([`resolve`], [`object`]); what stays and where ([`layout`]); the
//! image written ([`write`]); all of it in order ([`link`]).

#[cfg(test)]
use turbonop_global_alloc as _;

pub mod args;
pub mod config;
pub mod exports;
pub mod implib;
pub mod layout;
pub mod link;
pub mod object;
pub mod resolve;
pub mod write;
