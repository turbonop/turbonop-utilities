//! Debugging targets: what a debugger examines, behind one model, whether
//! it's a minidump, a core file, or (one day) a live process.
//!
//! - [`Target`] ([`model`]): a target's architecture, operating system,
//!   threads, modules, the exception it stopped for, and its
//!   [`Memory`], as of a [`Snapshot`], with its [`Capabilities`] (a dump
//!   can only be read).
//! - [`Registers`]: a thread's register values by DWARF number, the
//!   numbering every TurboNOP debugging crate uses ([`turbonop_abi::regs`]).
//!   A register a target didn't record is absent, never zero.
//! - [`AddressSpace`]: a target's memory with its modules' image files
//!   ([`LoadedImage`]) standing in for what it didn't capture, each byte's
//!   [`Provenance`] known.
//! - [`MinidumpTarget`]: minidumps (Windows', Breakpad's, Crashpad's).
//! - [`context`]: minidump register contexts as [`Registers`]; [`os`]: the
//!   operating system a dump came from; [`linux_maps`]: Linux's process
//!   maps.

extern crate alloc;

pub mod build_id;
pub mod context;
pub mod core_file;
mod image;
pub mod linux_maps;
pub mod minidump;
pub mod model;
pub mod os;
mod registers;
mod space;

pub use core_file::CoreTarget;
pub use image::LoadedImage;
pub use minidump::MinidumpTarget;
pub use model::{
    Capabilities, DebugId, Exception, ImageId, Memory, Module, ModuleSource, Provenance, Snapshot,
    Target, Thread, read_exact,
};
pub use registers::Registers;
pub use space::{AddressSpace, Run};
