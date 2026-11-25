//! Linux core files (ELF `ET_CORE`) as targets: x86-64, i386, AArch64, and
//! ARM.

use alloc::string::String;
use alloc::vec::Vec;

use turbonop_abi::{Abi, Os, regs};
use turbonop_core::{Arch, Error, ErrorKind, Result};
use turbonop_elf::ElfFile;
use turbonop_elf::consts::{EM_386, EM_AARCH64, EM_ARM, EM_X86_64};
use turbonop_elf::coredump::{
    self, CoreFile, CoreThread, GeneralRegisters, NT_ARM_TLS, NT_ARM_VFP, NT_PRFPREG,
};

use crate::Registers;
use crate::build_id::{ElfInMemory, elf_in_memory};
use crate::model::{
    Capabilities, DebugId, Exception, ImageId, Memory, Module, ModuleSource, Provenance, Snapshot,
    Target, Thread,
};

/// A core file's memory: its loaded segments' file bytes.
pub struct CoreMemory<'d>(Vec<(u64, &'d [u8])>);

impl Memory for CoreMemory<'_> {
    fn read_at(&self, address: u64, buf: &mut [u8]) -> usize {
        let i = self.0.partition_point(|r| r.0 <= address);
        let Some(&(start, data)) = i.checked_sub(1).map(|i| &self.0[i]) else {
            return 0;
        };
        let Ok(off) = usize::try_from(address - start) else {
            return 0;
        };
        let Some(d) = data.get(off..) else { return 0 };
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

/// A Linux core file, as a target.
pub struct CoreTarget<'d> {
    core: CoreFile<'d>,
    arch: Arch,
    os: Os,
    threads: Vec<Thread>,
    modules: Vec<Module>,
    exception: Option<Exception>,
    memory: CoreMemory<'d>,
}

impl<'d> CoreTarget<'d> {
    /// Reads a core file's threads, modules, signal, and memory.
    ///
    /// # Errors
    ///
    /// If it isn't an ELF core file of an architecture read here.
    pub fn new(data: &'d [u8]) -> Result<CoreTarget<'d>> {
        let elf = ElfFile::parse(data)?;
        let arch = match elf.header().machine {
            EM_X86_64 => Arch::X64,
            EM_386 => Arch::X86,
            EM_AARCH64 => Arch::Arm64,
            EM_ARM => Arch::Arm,
            _ => {
                return Err(Error::new(
                    ErrorKind::Unsupported,
                    18,
                    "core file architecture",
                ));
            }
        };
        let endian = elf.endian();
        let core =
            CoreFile::new(elf).ok_or(Error::new(ErrorKind::Unsupported, 16, "not a core file"))?;
        let memory = CoreMemory(core.memory());
        let threads: Vec<Thread> = core
            .threads()
            .iter()
            .map(|t| thread(t, endian, &memory))
            .collect();
        let files = core.files();
        // Android's system paths; anything else, Linux.
        let android = files
            .iter()
            .any(|f| f.path.starts_with(b"/system/") || f.path.starts_with(b"/apex/"));
        let os = if android { Os::Android } else { Os::Linux };
        let exception = threads.first().map(|t| {
            let status = core.threads()[0].status;
            let si = core.siginfo();
            let pc = Abi::new(arch, os)
                .and_then(|a| t.registers.get64(a.program_counter()))
                .unwrap_or(0);
            Exception {
                thread: t.id,
                code: si.map_or(u32::from(status.cursig.unsigned_abs()), |s| {
                    s.signo.unsigned_abs()
                }),
                flags: si.map_or(status.code, |s| s.code).cast_unsigned(),
                address: si.and_then(|s| s.addr).unwrap_or(pc),
                parameters: Vec::new(),
                registers: None,
            }
        });
        let modules = modules(&files, &memory);
        Ok(CoreTarget {
            core,
            arch,
            os,
            threads,
            modules,
            exception,
            memory,
        })
    }

    /// The core file itself, for what the model doesn't hold.
    pub fn core(&self) -> &CoreFile<'d> {
        &self.core
    }
}

impl Target for CoreTarget<'_> {
    fn arch(&self) -> Arch {
        self.arch
    }

    fn os(&self) -> Option<Os> {
        Some(self.os)
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
}

fn thread(t: &CoreThread<'_>, endian: turbonop_core::Endian, memory: &CoreMemory<'_>) -> Thread {
    let mut r = Registers::new();
    let mut set = |n: u16, v: u128| r.set(n, Some(v));
    let (arch, thread_pointer) = match t.status.registers {
        GeneralRegisters::X64(g) => {
            use coredump::x64 as c;
            use regs::x64::*;
            for (n, i) in [
                (RAX, c::RAX),
                (RDX, c::RDX),
                (RCX, c::RCX),
                (RBX, c::RBX),
                (RSI, c::RSI),
                (RDI, c::RDI),
                (RBP, c::RBP),
                (RSP, c::RSP),
                (R8, c::R8),
                (R9, c::R9),
                (R10, c::R10),
                (R11, c::R11),
                (R12, c::R12),
                (R13, c::R13),
                (R14, c::R14),
                (R15, c::R15),
                (RIP, c::RIP),
                (RFLAGS, c::EFLAGS),
                (CS, c::CS),
                (SS, c::SS),
                (DS, c::DS),
                (ES, c::ES),
                (FS, c::FS),
                (GS, c::GS),
                (FS_BASE, c::FS_BASE),
                (GS_BASE, c::GS_BASE),
            ] {
                set(n, u128::from(g[i]));
            }
            if let Some((xmm, mxcsr)) = t.note(NT_PRFPREG).and_then(coredump::x64_fp_registers) {
                for (i, &v) in (0u16..).zip(&xmm) {
                    set(regs::x64::xmm(i), v);
                }
                set(MXCSR, u128::from(mxcsr));
            }
            (Arch::X64, Some(g[c::FS_BASE]))
        }
        GeneralRegisters::X86(g) => {
            use coredump::x86 as c;
            use regs::x86::*;
            for (n, i) in [
                (EAX, c::EAX),
                (ECX, c::ECX),
                (EDX, c::EDX),
                (EBX, c::EBX),
                (ESP, c::ESP),
                (EBP, c::EBP),
                (ESI, c::ESI),
                (EDI, c::EDI),
                (EIP, c::EIP),
                (EFLAGS, c::EFLAGS),
                (CS, c::CS),
                (SS, c::SS),
                (DS, c::DS),
                (ES, c::ES),
                (FS, c::FS),
                (GS, c::GS),
            ] {
                set(n, u128::from(g[i]));
            }
            (Arch::X86, None)
        }
        GeneralRegisters::Arm64(g) => {
            // X0..X30, SP, PC (and PSTATE, which DWARF doesn't number).
            for (n, &v) in (0u16..).zip(&g[..33]) {
                set(n, u128::from(v));
            }
            if let Some((v, _, _)) = t
                .note(NT_PRFPREG)
                .and_then(|d| coredump::arm64_fp_registers(d, endian))
            {
                for (i, &x) in (0u16..).zip(&v) {
                    set(regs::arm64::v(i), x);
                }
            }
            let tls = t.note(NT_ARM_TLS).and_then(|d| {
                let b: [u8; 8] = d.get(..8)?.try_into().ok()?;
                Some(match endian {
                    turbonop_core::Endian::Little => u64::from_le_bytes(b),
                    turbonop_core::Endian::Big => u64::from_be_bytes(b),
                })
            });
            (Arch::Arm64, tls)
        }
        GeneralRegisters::Arm(g) => {
            for (n, &v) in (0u16..).zip(&g[..16]) {
                set(n, u128::from(v));
            }
            if let Some((d, _)) = t
                .note(NT_ARM_VFP)
                .and_then(|d| coredump::arm_vfp_registers(d, endian))
            {
                for (i, &x) in (0u16..).zip(&d) {
                    set(regs::arm::d(i), u128::from(x));
                }
            }
            (Arch::Arm, None)
        }
    };
    // The stack: the loaded segment the stack pointer is in.
    let sp = Abi::new(arch, Os::Linux).map(Abi::stack_pointer);
    let stack = sp.and_then(|sp| r.get64(sp)).and_then(|sp| {
        let i = memory.0.partition_point(|m| m.0 <= sp);
        let (start, data) = *memory.0.get(i.checked_sub(1)?)?;
        let end = start + data.len() as u64;
        (sp < end).then_some(start..end)
    });
    Thread {
        id: u64::from(t.status.pid.unsigned_abs()),
        name: None,
        registers: r,
        stack,
        thread_pointer,
    }
}

/// The modules among the mapped files: each file mapped from offset 0 that
/// is an ELF file (or whose headers weren't dumped), spanning to the end
/// of its last mapping, its build ID read from its notes in memory.
fn modules(files: &[coredump::MappedFile<'_>], memory: &CoreMemory<'_>) -> Vec<Module> {
    let mut out: Vec<Module> = Vec::new();
    for (i, f) in files.iter().enumerate() {
        if f.offset != 0 || out.iter().any(|m| m.base == f.start) {
            continue;
        }
        let end = files[i..]
            .iter()
            .take_while(|g| g.path == f.path || g.offset != 0)
            .filter(|g| g.path == f.path)
            .map(|g| g.end)
            .max()
            .unwrap_or(f.end);
        let build_id = match elf_in_memory(memory, f.start) {
            ElfInMemory::NotElf => continue,
            ElfInMemory::Unknown => None,
            ElfInMemory::Elf { build_id } => build_id,
        };
        out.push(Module {
            path: String::from_utf8_lossy(f.path).into_owned(),
            base: f.start,
            size: end - f.start,
            image_id: build_id.clone().map(ImageId::BuildId),
            debug_id: build_id.map(DebugId::BuildId),
            source: ModuleSource::ModuleList,
        });
    }
    out.sort_by_key(|m| m.base);
    out
}
