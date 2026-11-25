//! Mach-O core files (`MH_CORE`) as targets: macOS's (and lldb's
//! `process save-core`), x86-64 and ARM64.
//!
//! A core file doesn't record threads' IDs: they're numbered from 1 in the
//! order written. The thread that faulted is the one with an exception
//! state. Modules come from lldb's `all image infos` note, or else from the
//! Mach-O headers at the starts of the core's segments.

use alloc::string::String;
use alloc::vec::Vec;

use turbonop_abi::{Os, regs};
use turbonop_core::{Arch, Error, ErrorKind, Result};
use turbonop_macho::MachO;
use turbonop_macho::consts::{CPU_TYPE_ARM64, CPU_TYPE_X86_64, MH_MAGIC_64};
use turbonop_macho::coredump::{CoreFile, ThreadState};

use crate::Registers;
use crate::model::{
    Capabilities, DebugId, Exception, ImageId, Memory, Module, ModuleSource, Provenance, Snapshot,
    Target, Thread,
};

/// A Mach-O core's memory: its segments' file bytes.
pub struct MachCoreMemory<'d>(Vec<(u64, &'d [u8])>);

impl Memory for MachCoreMemory<'_> {
    fn read_at(&self, address: u64, buf: &mut [u8]) -> usize {
        let i = self.0.partition_point(|r| r.0 <= address);
        let Some(&(start, data)) = i.checked_sub(1).map(|i| &self.0[i]) else {
            return 0;
        };
        let Some(d) = usize::try_from(address - start)
            .ok()
            .and_then(|o| data.get(o..))
        else {
            return 0;
        };
        let n = d.len().min(buf.len());
        buf[..n].copy_from_slice(&d[..n]);
        n
    }

    fn next_available(&self, address: u64) -> Option<u64> {
        let i = self.0.partition_point(|r| r.0 <= address);
        self.0.get(i).map(|r| r.0)
    }

    fn provenance(&self) -> Provenance {
        Provenance::Dump
    }
}

/// A Mach-O core file, as a target.
pub struct MachCoreTarget<'d> {
    arch: Arch,
    threads: Vec<Thread>,
    modules: Vec<Module>,
    exception: Option<Exception>,
    memory: MachCoreMemory<'d>,
    addressable_bits: Option<u8>,
}

impl<'d> MachCoreTarget<'d> {
    /// Reads a Mach-O core file.
    ///
    /// # Errors
    ///
    /// If it isn't a Mach-O core file of x86-64 or ARM64.
    pub fn new(data: &'d [u8]) -> Result<MachCoreTarget<'d>> {
        let macho = MachO::parse(data)?;
        let arch = match macho.header().cputype {
            CPU_TYPE_X86_64 => Arch::X64,
            CPU_TYPE_ARM64 => Arch::Arm64,
            _ => {
                return Err(Error::new(
                    ErrorKind::Unsupported,
                    4,
                    "core file architecture",
                ));
            }
        };
        let core = CoreFile::new(macho).ok_or(Error::new(
            ErrorKind::Unsupported,
            12,
            "not a core file",
        ))?;
        let memory = MachCoreMemory(core.memory());
        let mut exception = None;
        let mut threads = Vec::new();
        for (n, t) in (1u64..).zip(core.threads()) {
            let mut r = Registers::new();
            match t.state {
                Some(ThreadState::X64(g)) => {
                    use regs::x64::*;
                    let order = [
                        RAX, RBX, RCX, RDX, RDI, RSI, RBP, RSP, R8, R9, R10, R11, R12, R13, R14,
                        R15, RIP, RFLAGS, CS, FS, GS,
                    ];
                    for (reg, v) in order.into_iter().zip(g) {
                        r.set(reg, Some(u128::from(v)));
                    }
                    for (i, &v) in (0u16..).zip(&t.vectors) {
                        r.set(xmm(i), Some(v));
                    }
                }
                Some(ThreadState::Arm64 { regs: g, .. }) => {
                    for (reg, &v) in (0u16..).zip(&g) {
                        r.set(reg, Some(u128::from(v)));
                    }
                    for (i, &v) in (0u16..).zip(&t.vectors) {
                        r.set(regs::arm64::v(i), Some(v));
                    }
                }
                None => {}
            }
            if let Some(e) = t.exception
                && exception.is_none()
                && (e.address != 0 || e.code != 0)
            {
                exception = Some(Exception {
                    thread: n,
                    code: e.code,
                    flags: e.detail,
                    address: e.address,
                    parameters: Vec::new(),
                    registers: None,
                });
            }
            let sp = r.get64(if arch == Arch::X64 {
                regs::x64::RSP
            } else {
                regs::arm64::SP
            });
            let stack = sp.and_then(|sp| {
                let i = memory.0.partition_point(|m| m.0 <= sp);
                let (start, d) = *memory.0.get(i.checked_sub(1)?)?;
                let end = start + d.len() as u64;
                (sp < end).then_some(start..end)
            });
            threads.push(Thread {
                id: n,
                name: None,
                registers: r,
                stack,
                thread_pointer: None,
            });
        }
        let mut modules: Vec<Module> = core
            .images()
            .into_iter()
            .map(|i| module(i.path, i.load_address, Some(i.uuid), &memory))
            .collect();
        if modules.is_empty() {
            // The Mach-O headers at the segments' starts.
            for &(start, d) in &memory.0 {
                if d.get(..4)
                    .map(|m| u32::from_le_bytes([m[0], m[1], m[2], m[3]]))
                    != Some(MH_MAGIC_64)
                {
                    continue;
                }
                let Ok(m) = MachO::parse(d) else { continue };
                let name = m
                    .install_name()
                    .map(|n| String::from_utf8_lossy(n).into_owned());
                modules.push(module(name.unwrap_or_default(), start, m.uuid(), &memory));
            }
        }
        modules.sort_by_key(|m| m.base);
        Ok(MachCoreTarget {
            arch,
            threads,
            modules,
            exception,
            addressable_bits: core.addressable_bits(),
            memory,
        })
    }

    /// How many bits of a code address are the address (the rest a pointer
    /// authentication code), if the core says.
    pub fn addressable_bits(&self) -> Option<u8> {
        self.addressable_bits
    }
}

/// A module at `base`, its extent read from its header in memory (its
/// segments' span), if that's there.
fn module(path: String, base: u64, uuid: Option<[u8; 16]>, memory: &MachCoreMemory<'_>) -> Module {
    let mut size = 0;
    let mut header = alloc::vec![0u8; 64 * 1024];
    let n = memory.read_at(base, &mut header);
    if let Ok(m) = MachO::parse(&header[..n]) {
        let segments = m.segments();
        let text = segments
            .iter()
            .find(|s| s.fileoff == 0 && s.filesize > 0)
            .map(|s| s.vmaddr);
        if let Some(link) = text {
            size = segments
                .iter()
                .filter(|s| s.vmaddr >= link)
                .map(|s| s.vmaddr + s.vmsize - link)
                .max()
                .unwrap_or(0);
        }
    }
    Module {
        path,
        base,
        size: size.max(1),
        image_id: uuid.map(ImageId::Uuid),
        debug_id: uuid.map(DebugId::Uuid),
        source: ModuleSource::ModuleList,
    }
}

impl Target for MachCoreTarget<'_> {
    fn arch(&self) -> Arch {
        self.arch
    }

    fn os(&self) -> Option<Os> {
        Some(Os::MacOs)
    }

    fn snapshot(&self) -> Snapshot {
        Snapshot { generation: 0 }
    }

    fn capabilities(&self) -> Capabilities {
        Capabilities::DUMP
    }

    fn threads(&self) -> &[Thread] {
        &self.threads
    }

    fn modules(&self) -> &[Module] {
        &self.modules
    }

    fn exception(&self) -> Option<&Exception> {
        self.exception.as_ref()
    }

    fn memory(&self) -> &dyn Memory {
        &self.memory
    }

    fn code_address_bits(&self) -> Option<u8> {
        self.addressable_bits
    }
}
