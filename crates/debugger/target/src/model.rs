//! The model every target shares: what it can do, its threads and
//! modules, the exception it stopped for, and its memory.
//!
//! A target is examined one [`Snapshot`] at a time: the state at one
//! instant. A dump has one snapshot, forever. A live process would have a
//! new one each time it stops, and anything computed from one snapshot
//! (frames, values) is keyed by its generation so it is never used for
//! another.

use alloc::string::String;
use alloc::vec::Vec;
use core::ops::Range;

use turbonop_abi::{Abi, Os};
use turbonop_core::Arch;

use crate::Registers;

/// Where bytes read from a target came from.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Provenance {
    /// The target's own memory, as a dump captured it.
    Dump,
    /// A live process's memory, read now.
    Live,
    /// A module's file (on disk, or from a symbol server), standing in for
    /// memory the target didn't capture. It may differ from what ran:
    /// code patched at run time, data written since the module loaded.
    Image,
}

impl Provenance {
    /// The less certain of two: what bytes from both are.
    pub fn worst(self, other: Provenance) -> Provenance {
        self.max(other)
    }
}

/// An instant of a target's state.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct Snapshot {
    /// Which: 0 for a dump's only one; a live target's count up.
    pub generation: u64,
}

/// What a target can do. Every target can read memory.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct Capabilities(pub u32);

impl Capabilities {
    pub const READ_MEMORY: u32 = 1 << 0;
    pub const WRITE_MEMORY: u32 = 1 << 1;
    /// Running, stepping, and stopping.
    pub const RUN: u32 = 1 << 2;
    pub const BREAKPOINTS: u32 = 1 << 3;
    /// Events as they happen (module loads, exceptions, thread starts).
    pub const EVENTS: u32 = 1 << 4;

    /// A dump's: reading only.
    pub const DUMP: Capabilities = Capabilities(Self::READ_MEMORY);

    pub fn has(self, capability: u32) -> bool {
        self.0 & capability == capability
    }
}

/// A thread, as of a snapshot.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Thread {
    /// Its ID (the system's).
    pub id: u64,
    /// Its name, if it has one.
    pub name: Option<String>,
    /// Its registers (by DWARF number), as the target recorded them.
    pub registers: Registers,
    /// Its stack's memory, as far as the target says (a dump's captured
    /// range).
    pub stack: Option<Range<u64>>,
    /// Its thread block's address: Windows' TEB, or the thread pointer.
    pub thread_pointer: Option<u64>,
}

/// What identifies a module's image file: what a symbol server indexes
/// it by.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum ImageId {
    /// A PE image's `TimeDateStamp` and `SizeOfImage`.
    Pe {
        time_date_stamp: u32,
        size_of_image: u32,
    },
    /// An ELF image's build ID (or, from Breakpad for a module without
    /// one, a hash of its text).
    BuildId(Vec<u8>),
    /// A Mach-O image's `LC_UUID`.
    Uuid([u8; 16]),
}

/// What identifies a module's debug information.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum DebugId {
    /// A PDB 7.0 file: its GUID, age, and the path the linker gave it.
    Pdb {
        guid: [u8; 16],
        age: u32,
        path: String,
    },
    /// A PDB 2.0 file: its signature (a time stamp), age, and path.
    Pdb20 {
        signature: u32,
        age: u32,
        path: String,
    },
    /// An ELF build ID: the separate debug file has it too.
    BuildId(Vec<u8>),
    /// A Mach-O UUID: the dSYM has it too.
    Uuid([u8; 16]),
}

/// A module loaded in the target.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Module {
    /// Its path, as the target knew it.
    pub path: String,
    /// Where it's loaded: the address its image's first byte (its headers)
    /// is at.
    pub base: u64,
    /// How many bytes it spans from there.
    pub size: u64,
    /// Its image file's identity, if the target recorded one.
    pub image_id: Option<ImageId>,
    /// Its debug information's, if the target recorded one.
    pub debug_id: Option<DebugId>,
    /// Where the target learned of it.
    pub source: ModuleSource,
}

impl Module {
    /// Whether `address` is in it.
    pub fn contains(&self, address: u64) -> bool {
        address.wrapping_sub(self.base) < self.size
    }

    /// Its file name (the last part of its path, by either separator).
    pub fn file_name(&self) -> &str {
        self.path.rsplit(['/', '\\']).next().unwrap_or(&self.path)
    }
}

/// Where a target learned of a module.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum ModuleSource {
    /// Its list of modules (a minidump's module list, a core file's
    /// `NT_FILE` note).
    ModuleList,
    /// The process's memory map (Linux's `/proc/<pid>/maps`), where the
    /// module list missed it.
    MemoryMap,
}

/// What a target stopped for: an exception (a signal, on POSIX systems).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Exception {
    /// The thread it happened on.
    pub thread: u64,
    /// Its code (Windows' `ExceptionCode`; a signal number).
    pub code: u32,
    /// Its flags (Windows'), or the signal's code (`si_code`).
    pub flags: u32,
    /// Where it happened (Windows' `ExceptionAddress`; a signal's fault
    /// address).
    pub address: u64,
    /// Its parameters (an access violation's: read, write, or execute,
    /// and the address).
    pub parameters: Vec<u64>,
    /// The thread's registers when it happened, if recorded apart from
    /// the thread's own (which may be where a handler stopped it).
    pub registers: Option<Registers>,
}

/// A target's memory: what it captured (a dump) or can read (a live
/// process).
pub trait Memory: Sync {
    /// Copies the bytes at `address` and after into `buf`, as many as are
    /// contiguous and available; how many (0 if none at `address`).
    fn read_at(&self, address: u64, buf: &mut [u8]) -> usize;

    /// Where the next available bytes after `address` begin, if any are.
    fn next_available(&self, address: u64) -> Option<u64>;

    /// Where its bytes come from.
    fn provenance(&self) -> Provenance;
}

/// A debugging target.
pub trait Target: Sync {
    fn arch(&self) -> Arch;

    /// Its operating system, if known.
    fn os(&self) -> Option<Os>;

    /// Its calling convention, if one is described.
    fn abi(&self) -> Option<Abi> {
        Abi::new(self.arch(), self.os()?)
    }

    /// The snapshot its state is as of.
    fn snapshot(&self) -> Snapshot;

    fn capabilities(&self) -> Capabilities;

    fn threads(&self) -> &[Thread];

    fn modules(&self) -> &[Module];

    /// The exception it stopped for, if any.
    fn exception(&self) -> Option<&Exception>;

    /// Its memory.
    fn memory(&self) -> &dyn Memory;

    /// The module `address` is in.
    fn module_at(&self, address: u64) -> Option<&Module> {
        self.modules().iter().find(|m| m.contains(address))
    }
}
