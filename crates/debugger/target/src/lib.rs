//! Debugging targets: what a debugger examines, behind one model, whether
//! it's a minidump, a core file, or (one day) a live process.
//!
//! - [`Registers`]: a thread's register values by DWARF number, the
//!   numbering every TurboNOP debugging crate uses ([`turbonop_abi::regs`]).
//!   A register a target didn't record is absent, never zero.
//! - [`context`]: minidump register contexts as [`Registers`].
//! - [`os`]: the operating system a dump came from.
//!
//! The rest of the model (snapshots, capabilities, the address space with
//! each byte's provenance) is described in `docs/dump-analyzer.md`.

#![no_std]

extern crate alloc;

pub mod context;
pub mod os;
mod registers;

pub use registers::Registers;
