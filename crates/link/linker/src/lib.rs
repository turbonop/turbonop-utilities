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
pub mod icf;
pub mod implib;
pub mod layout;
pub mod link;
pub mod msqsort;
pub mod object;
pub mod pdb;
pub mod resolve;
pub mod resources;
pub mod rich;
pub mod write;

/// `n` (a count or size the link computed) as a `u32`: an image and what
/// it holds are smaller than 4 GB.
#[track_caller]
pub(crate) fn n32(n: usize) -> u32 {
    u32::try_from(n).expect("an image and its parts are under 4 GB")
}

/// `n` (a count the link computed) as a `u16`.
#[track_caller]
pub(crate) fn n16(n: usize) -> u16 {
    u16::try_from(n).expect("fewer than 65536")
}

/// The low 32 bits of `v` (a relocation's addition wraps, as the
/// processor's does).
pub(crate) fn low32(v: u64) -> u32 {
    u32::try_from(v & 0xFFFF_FFFF).expect("masked to 32 bits")
}
