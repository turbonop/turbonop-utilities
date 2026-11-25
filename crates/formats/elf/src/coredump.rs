//! Core files (`ET_CORE`), as Linux writes them (the kernel, and `gcore`):
//! the process's memory in `PT_LOAD` segments, and its state in `PT_NOTE`
//! segments' notes named `CORE` (and `LINUX`):
//!
//! - `NT_PRSTATUS`, one per thread ([`PrStatus`]): its signal, IDs, and
//!   general registers ([`GeneralRegisters`]), followed by its other
//!   register notes (`NT_PRFPREG`, `NT_ARM_VFP`, `NT_X86_XSTATE`, ...)
//!   until the next thread's ([`CoreFile::threads`]).
//! - `NT_PRPSINFO` ([`ProcessInfo`]): the process's IDs, name, and
//!   arguments.
//! - `NT_SIGINFO` ([`SigInfo`]): the signal that ended it.
//! - `NT_AUXV`: the auxiliary vector.
//! - `NT_FILE` ([`MappedFile`]): the files mapped, and where.
//! - `NT_ARM_PAC_MASK`: which bits of an ARM64 address hold its pointer
//!   authentication code.
//!
//! The structures' layouts are the kernel's for x86-64, i386, AArch64, and
//! ARM (`elf_prstatus`, `elf_prpsinfo`, `siginfo_t`, each architecture's
//! `user_regs_struct` and floating-point state). The first thread is the
//! one that took the signal.

use alloc::vec::Vec;

use turbonop_core::Endian;

use crate::consts::{EM_386, EM_AARCH64, EM_ARM, EM_X86_64, PT_LOAD, PT_NOTE};
use crate::{Class, ElfFile, Note};

pub const NT_PRSTATUS: u32 = 1;
pub const NT_PRFPREG: u32 = 2;
pub const NT_PRPSINFO: u32 = 3;
pub const NT_TASKSTRUCT: u32 = 4;
pub const NT_AUXV: u32 = 6;
pub const NT_SIGINFO: u32 = 0x5349_4749;
pub const NT_FILE: u32 = 0x4649_4C45;
pub const NT_PRXFPREG: u32 = 0x46E6_2B7F;
pub const NT_X86_XSTATE: u32 = 0x202;
pub const NT_ARM_VFP: u32 = 0x400;
pub const NT_ARM_TLS: u32 = 0x401;
pub const NT_ARM_PAC_MASK: u32 = 0x406;

/// A thread's general registers, as its architecture's `elf_gregset_t`
/// holds them.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum GeneralRegisters {
    /// x86-64's `user_regs_struct`: R15, R14, R13, R12, RBP, RBX, R11, R10,
    /// R9, R8, RAX, RCX, RDX, RSI, RDI, `orig_rax`, RIP, CS, EFLAGS, RSP,
    /// SS, FS base, GS base, DS, ES, FS, GS.
    X64([u64; 27]),
    /// i386's: EBX, ECX, EDX, ESI, EDI, EBP, EAX, DS, ES, FS, GS,
    /// `orig_eax`, EIP, CS, EFLAGS, ESP, SS.
    X86([u32; 17]),
    /// AArch64's `user_pt_regs`: X0..X30, SP, PC, PSTATE.
    Arm64([u64; 34]),
    /// ARM's: R0..R15, CPSR, `orig_r0`.
    Arm([u32; 18]),
}

/// Indexes of x86-64's registers in [`GeneralRegisters::X64`].
pub mod x64 {
    pub const R15: usize = 0;
    pub const R14: usize = 1;
    pub const R13: usize = 2;
    pub const R12: usize = 3;
    pub const RBP: usize = 4;
    pub const RBX: usize = 5;
    pub const R11: usize = 6;
    pub const R10: usize = 7;
    pub const R9: usize = 8;
    pub const R8: usize = 9;
    pub const RAX: usize = 10;
    pub const RCX: usize = 11;
    pub const RDX: usize = 12;
    pub const RSI: usize = 13;
    pub const RDI: usize = 14;
    pub const ORIG_RAX: usize = 15;
    pub const RIP: usize = 16;
    pub const CS: usize = 17;
    pub const EFLAGS: usize = 18;
    pub const RSP: usize = 19;
    pub const SS: usize = 20;
    pub const FS_BASE: usize = 21;
    pub const GS_BASE: usize = 22;
    pub const DS: usize = 23;
    pub const ES: usize = 24;
    pub const FS: usize = 25;
    pub const GS: usize = 26;
}

/// Indexes of i386's registers in [`GeneralRegisters::X86`].
pub mod x86 {
    pub const EBX: usize = 0;
    pub const ECX: usize = 1;
    pub const EDX: usize = 2;
    pub const ESI: usize = 3;
    pub const EDI: usize = 4;
    pub const EBP: usize = 5;
    pub const EAX: usize = 6;
    pub const DS: usize = 7;
    pub const ES: usize = 8;
    pub const FS: usize = 9;
    pub const GS: usize = 10;
    pub const ORIG_EAX: usize = 11;
    pub const EIP: usize = 12;
    pub const CS: usize = 13;
    pub const EFLAGS: usize = 14;
    pub const ESP: usize = 15;
    pub const SS: usize = 16;
}

/// A thread's status (`elf_prstatus`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PrStatus {
    /// `pr_info`: the signal's number, code, and error number.
    pub signo: i32,
    pub code: i32,
    pub errno: i32,
    /// `pr_cursig`: the signal current when it stopped.
    pub cursig: i16,
    /// `pr_sigpend` and `pr_sighold`: pending and blocked signals.
    pub sigpend: u64,
    pub sighold: u64,
    /// The thread's ID (`pr_pid`), and its process's parent, group, and
    /// session.
    pub pid: i32,
    pub ppid: i32,
    pub pgrp: i32,
    pub sid: i32,
    pub registers: GeneralRegisters,
    /// `pr_fpvalid`: whether a floating-point register note follows.
    pub fpvalid: bool,
}

/// A thread: its status, and the register notes that follow it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CoreThread<'d> {
    pub status: PrStatus,
    /// Its other notes (`NT_PRFPREG`, `NT_X86_XSTATE`, `NT_ARM_VFP`, ...).
    pub notes: Vec<Note<'d>>,
}

impl<'d> CoreThread<'d> {
    /// Its note of a kind.
    pub fn note(&self, kind: u32) -> Option<&'d [u8]> {
        self.notes.iter().find(|n| n.kind == kind).map(|n| n.desc)
    }
}

/// The process (`elf_prpsinfo`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ProcessInfo<'d> {
    /// `pr_state`, and its letter (`pr_sname`: R, S, D, T, Z, ...).
    pub state: u8,
    pub sname: u8,
    pub zombie: bool,
    pub nice: i8,
    pub flag: u64,
    pub uid: u32,
    pub gid: u32,
    pub pid: i32,
    pub ppid: i32,
    pub pgrp: i32,
    pub sid: i32,
    /// The executable's name (its first 15 bytes).
    pub fname: &'d [u8],
    /// Its arguments (their first 79 bytes, spaces between).
    pub psargs: &'d [u8],
}

/// The signal that ended the process (`siginfo_t`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SigInfo {
    pub signo: i32,
    pub errno: i32,
    pub code: i32,
    /// For SIGSEGV, SIGBUS, SIGILL, SIGFPE, and SIGTRAP: the faulting
    /// address (`si_addr`).
    pub addr: Option<u64>,
}

/// A file mapped into the process (an `NT_FILE` entry).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct MappedFile<'d> {
    pub start: u64,
    pub end: u64,
    /// Its offset in the file, in bytes.
    pub offset: u64,
    pub path: &'d [u8],
}

/// A core file.
#[derive(Clone, Debug)]
pub struct CoreFile<'d> {
    elf: ElfFile<'d>,
    notes: Vec<Note<'d>>,
}

impl<'d> CoreFile<'d> {
    /// The core file `elf` is, if it is one.
    pub fn new(elf: ElfFile<'d>) -> Option<CoreFile<'d>> {
        if elf.header().kind != crate::consts::ET_CORE {
            return None;
        }
        let mut notes = Vec::new();
        for s in elf.segments().filter(|s| s.kind == PT_NOTE) {
            let (Ok(start), Ok(len)) = (usize::try_from(s.offset), usize::try_from(s.filesz))
            else {
                continue;
            };
            let Some(data) = start
                .checked_add(len)
                .and_then(|end| elf.data().get(start..end))
            else {
                continue;
            };
            notes.extend(elf.notes(data));
        }
        Some(CoreFile { elf, notes })
    }

    pub fn elf(&self) -> &ElfFile<'d> {
        &self.elf
    }

    /// Every note, in order.
    pub fn notes(&self) -> &[Note<'d>] {
        &self.notes
    }

    /// The first note of a kind.
    pub fn note(&self, kind: u32) -> Option<&'d [u8]> {
        self.notes.iter().find(|n| n.kind == kind).map(|n| n.desc)
    }

    /// Its memory: each `PT_LOAD` segment's address and the bytes the file
    /// holds for it (fewer than its size in memory, or none, where the
    /// kernel didn't dump them), by address.
    pub fn memory(&self) -> Vec<(u64, &'d [u8])> {
        let mut out: Vec<(u64, &'d [u8])> = self
            .elf
            .segments()
            .filter(|s| s.kind == PT_LOAD && s.filesz > 0)
            .filter_map(|s| {
                let start = usize::try_from(s.offset).ok()?;
                let len = usize::try_from(s.filesz.min(s.memsz)).ok()?;
                let data = self.elf.data().get(start..start.checked_add(len)?)?;
                Some((s.vaddr, data))
            })
            .collect();
        out.sort_by_key(|r| r.0);
        out
    }

    /// The threads, in the order written (the first took the signal).
    pub fn threads(&self) -> Vec<CoreThread<'d>> {
        let mut out: Vec<CoreThread<'d>> = Vec::new();
        for n in &self.notes {
            if n.kind == NT_PRSTATUS && n.name == b"CORE" {
                if let Some(status) = self.prstatus(n.desc) {
                    out.push(CoreThread {
                        status,
                        notes: Vec::new(),
                    });
                }
            } else if let Some(t) = out.last_mut()
                && is_thread_note(n)
            {
                t.notes.push(*n);
            }
        }
        out
    }

    /// The process's information.
    pub fn process_info(&self) -> Option<ProcessInfo<'d>> {
        let d = self.note(NT_PRPSINFO)?;
        let r = self.reader(d);
        let wide = self.elf.class() == Class::Elf64;
        // (32-bit kernels' uid and gid are 16 bits, ARM's and i386's.)
        let (flag, uid, gid, ids, fname) = if wide {
            (r.u64(8)?, r.u32(16)?, r.u32(20)?, 24, 40)
        } else {
            (
                u64::from(r.u32(4)?),
                u32::from(r.u16(8)?),
                u32::from(r.u16(10)?),
                12,
                28,
            )
        };
        let text = |at: usize, len: usize| {
            let b = d.get(at..at + len).unwrap_or(&[]);
            &b[..b.iter().position(|&c| c == 0).unwrap_or(b.len())]
        };
        Some(ProcessInfo {
            state: *d.first()?,
            sname: *d.get(1)?,
            zombie: *d.get(2)? != 0,
            nice: i8::from_ne_bytes([*d.get(3)?]),
            flag,
            uid,
            gid,
            pid: r.i32(ids)?,
            ppid: r.i32(ids + 4)?,
            pgrp: r.i32(ids + 8)?,
            sid: r.i32(ids + 12)?,
            fname: text(fname, 16),
            psargs: text(fname + 16, 80),
        })
    }

    /// The signal that ended the process.
    pub fn siginfo(&self) -> Option<SigInfo> {
        let d = self.note(NT_SIGINFO)?;
        let r = self.reader(d);
        let signo = r.i32(0)?;
        // SIGILL, SIGTRAP, SIGBUS, SIGFPE, SIGSEGV: a fault address, after
        // the three ints (and, 64-bit, padding to 8).
        let faults = matches!(signo, 4 | 5 | 7 | 8 | 11);
        let addr = if !faults {
            None
        } else if self.elf.class() == Class::Elf64 {
            r.u64(16)
        } else {
            r.u32(12).map(u64::from)
        };
        Some(SigInfo {
            signo,
            errno: r.i32(4)?,
            code: r.i32(8)?,
            addr,
        })
    }

    /// The auxiliary vector: its (type, value) pairs, to `AT_NULL`.
    pub fn auxv(&self) -> Vec<(u64, u64)> {
        let Some(d) = self.note(NT_AUXV) else {
            return Vec::new();
        };
        let r = self.reader(d);
        let w = r.word_size();
        let mut out = Vec::new();
        let mut at = 0;
        while let (Some(k), Some(v)) = (r.word(at), r.word(at + w)) {
            if k == 0 {
                break;
            }
            out.push((k, v));
            at += 2 * w;
        }
        out
    }

    /// The files mapped (`NT_FILE`), in its order.
    pub fn files(&self) -> Vec<MappedFile<'d>> {
        let Some(d) = self.note(NT_FILE) else {
            return Vec::new();
        };
        let r = self.reader(d);
        let w = r.word_size();
        let (Some(count), Some(page)) = (r.word(0), r.word(w)) else {
            return Vec::new();
        };
        let Ok(count) = usize::try_from(count) else {
            return Vec::new();
        };
        let entries = 2 * w;
        let Some(names_at) = count
            .checked_mul(3 * w)
            .and_then(|n| n.checked_add(entries))
        else {
            return Vec::new();
        };
        let mut names = d.get(names_at..).unwrap_or(&[]).split(|&b| b == 0);
        let mut out = Vec::new();
        for i in 0..count {
            let at = entries + 3 * w * i;
            let (Some(start), Some(end), Some(pages)) =
                (r.word(at), r.word(at + w), r.word(at + 2 * w))
            else {
                break;
            };
            let Some(path) = names.next() else { break };
            out.push(MappedFile {
                start,
                end,
                offset: pages.saturating_mul(page),
                path,
            });
        }
        out
    }

    /// ARM64's pointer authentication masks (`NT_ARM_PAC_MASK`): the bits
    /// of a data address, and of a code address, that hold its code.
    pub fn pac_masks(&self) -> Option<(u64, u64)> {
        let r = self.reader(self.note(NT_ARM_PAC_MASK)?);
        Some((r.u64(0)?, r.u64(8)?))
    }

    /// A thread's status note (`NT_PRSTATUS`), by the file's architecture.
    pub fn prstatus(&self, d: &[u8]) -> Option<PrStatus> {
        let r = self.reader(d);
        let machine = self.elf.header().machine;
        let wide = self.elf.class() == Class::Elf64;
        // The header: pr_info (3 ints), pr_cursig (a short, padded), the
        // signal sets (longs), four IDs (ints), four timevals (two longs
        // each), then the registers.
        let (sigpend, sighold, ids, regs) = if wide {
            (r.u64(16)?, r.u64(24)?, 32, 112)
        } else {
            (u64::from(r.u32(16)?), u64::from(r.u32(20)?), 24, 72)
        };
        let (registers, size) = match machine {
            EM_X86_64 => {
                let mut g = [0u64; 27];
                for (i, v) in g.iter_mut().enumerate() {
                    *v = r.u64(regs + 8 * i)?;
                }
                (GeneralRegisters::X64(g), 27 * 8)
            }
            EM_AARCH64 => {
                let mut g = [0u64; 34];
                for (i, v) in g.iter_mut().enumerate() {
                    *v = r.u64(regs + 8 * i)?;
                }
                (GeneralRegisters::Arm64(g), 34 * 8)
            }
            EM_386 => {
                let mut g = [0u32; 17];
                for (i, v) in g.iter_mut().enumerate() {
                    *v = r.u32(regs + 4 * i)?;
                }
                (GeneralRegisters::X86(g), 17 * 4)
            }
            EM_ARM => {
                let mut g = [0u32; 18];
                for (i, v) in g.iter_mut().enumerate() {
                    *v = r.u32(regs + 4 * i)?;
                }
                (GeneralRegisters::Arm(g), 18 * 4)
            }
            _ => return None,
        };
        Some(PrStatus {
            signo: r.i32(0)?,
            code: r.i32(4)?,
            errno: r.i32(8)?,
            cursig: r.i16(12)?,
            sigpend,
            sighold,
            pid: r.i32(ids)?,
            ppid: r.i32(ids + 4)?,
            pgrp: r.i32(ids + 8)?,
            sid: r.i32(ids + 12)?,
            registers,
            fpvalid: r.i32(regs + size).is_some_and(|v| v != 0),
        })
    }

    fn reader<'a>(&self, d: &'a [u8]) -> Reader<'a> {
        Reader {
            d,
            endian: self.elf.endian(),
            wide: self.elf.class() == Class::Elf64,
        }
    }
}

/// x86-64's XMM registers and MXCSR, from its `NT_PRFPREG` note (the
/// `FXSAVE` area: MXCSR at 24, XMM0..XMM15 at 160).
pub fn x64_fp_registers(d: &[u8]) -> Option<([u128; 16], u32)> {
    let mut xmm = [0u128; 16];
    for (i, x) in xmm.iter_mut().enumerate() {
        *x = u128::from_le_bytes(d.get(160 + 16 * i..176 + 16 * i)?.try_into().ok()?);
    }
    let mxcsr = u32::from_le_bytes(d.get(24..28)?.try_into().ok()?);
    Some((xmm, mxcsr))
}

/// AArch64's V registers, FPSR, and FPCR, from its `NT_PRFPREG` note
/// (`user_fpsimd_state`).
pub fn arm64_fp_registers(d: &[u8], endian: Endian) -> Option<([u128; 32], u32, u32)> {
    let mut v = [0u128; 32];
    for (i, x) in v.iter_mut().enumerate() {
        let b: [u8; 16] = d.get(16 * i..16 * i + 16)?.try_into().ok()?;
        *x = match endian {
            Endian::Little => u128::from_le_bytes(b),
            Endian::Big => u128::from_be_bytes(b),
        };
    }
    let r = Reader {
        d,
        endian,
        wide: true,
    };
    Some((v, r.u32(512)?, r.u32(516)?))
}

/// ARM's D registers and FPSCR, from its `NT_ARM_VFP` note.
pub fn arm_vfp_registers(d: &[u8], endian: Endian) -> Option<([u64; 32], u32)> {
    let r = Reader {
        d,
        endian,
        wide: false,
    };
    let mut dr = [0u64; 32];
    for (i, x) in dr.iter_mut().enumerate() {
        *x = r.u64(8 * i)?;
    }
    Some((dr, r.u32(256)?))
}

/// Whether a note is one of a thread's (following its `NT_PRSTATUS`).
fn is_thread_note(n: &Note<'_>) -> bool {
    match n.name {
        b"CORE" => matches!(n.kind, NT_PRFPREG | NT_TASKSTRUCT) || n.kind == NT_PRXFPREG,
        b"LINUX" => true,
        _ => false,
    }
}

struct Reader<'a> {
    d: &'a [u8],
    endian: Endian,
    wide: bool,
}

impl Reader<'_> {
    fn bytes<const N: usize>(&self, at: usize) -> Option<[u8; N]> {
        self.d.get(at..at.checked_add(N)?)?.try_into().ok()
    }

    fn u16(&self, at: usize) -> Option<u16> {
        let b = self.bytes::<2>(at)?;
        Some(match self.endian {
            Endian::Little => u16::from_le_bytes(b),
            Endian::Big => u16::from_be_bytes(b),
        })
    }

    fn i16(&self, at: usize) -> Option<i16> {
        self.u16(at).map(|v| i16::from_ne_bytes(v.to_ne_bytes()))
    }

    fn u32(&self, at: usize) -> Option<u32> {
        let b = self.bytes::<4>(at)?;
        Some(match self.endian {
            Endian::Little => u32::from_le_bytes(b),
            Endian::Big => u32::from_be_bytes(b),
        })
    }

    fn i32(&self, at: usize) -> Option<i32> {
        self.u32(at).map(|v| i32::from_ne_bytes(v.to_ne_bytes()))
    }

    fn u64(&self, at: usize) -> Option<u64> {
        let b = self.bytes::<8>(at)?;
        Some(match self.endian {
            Endian::Little => u64::from_le_bytes(b),
            Endian::Big => u64::from_be_bytes(b),
        })
    }

    fn word_size(&self) -> usize {
        if self.wide { 8 } else { 4 }
    }

    fn word(&self, at: usize) -> Option<u64> {
        if self.wide {
            self.u64(at)
        } else {
            self.u32(at).map(u64::from)
        }
    }
}

#[cfg(test)]
#[path = "coredump_tests.rs"]
mod tests;
