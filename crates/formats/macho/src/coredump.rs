//! Mach-O core files (`MH_CORE`), as macOS's kernel and lldb write them: the
//! process's memory in `LC_SEGMENT_64` commands; each thread's state in an
//! `LC_THREAD` command (a sequence of flavored states: general registers,
//! floating-point and vector registers, the exception state); and lldb's
//! `LC_NOTE`s: `addrable bits` (how many bits of an address are the
//! address, the rest a pointer authentication code), `all image infos`
//! (the images loaded, and where), `main bin spec` (the main binary).

use alloc::string::String;
use alloc::vec::Vec;

use turbonop_core::Endian;

use crate::MachO;
use crate::consts::{
    CPU_TYPE_ARM64, CPU_TYPE_X86_64, LC_NOTE, LC_SEGMENT_64, LC_THREAD, LC_UNIXTHREAD, MH_CORE,
};

/// `x86_THREAD_STATE64`: RAX, RBX, RCX, RDX, RDI, RSI, RBP, RSP, R8..R15,
/// RIP, RFLAGS, CS, FS, GS (21 words).
pub const X86_THREAD_STATE64: u32 = 4;
/// `x86_FLOAT_STATE64` (`FXSAVE`-like: XMM0..XMM15 at 168).
pub const X86_FLOAT_STATE64: u32 = 5;
/// `x86_EXCEPTION_STATE64`: trap number, CPU, error code, fault address.
pub const X86_EXCEPTION_STATE64: u32 = 6;
/// `x86_THREAD_STATE`: a flavored header, then a state.
pub const X86_THREAD_STATE: u32 = 7;
/// `ARM_THREAD_STATE64`: X0..X28, FP, LR, SP, PC, CPSR (and flags).
pub const ARM_THREAD_STATE64: u32 = 6;
/// `ARM_EXCEPTION_STATE64`: FAR, ESR, the exception.
pub const ARM_EXCEPTION_STATE64: u32 = 7;
/// `ARM_NEON_STATE64`: V0..V31, FPSR, FPCR.
pub const ARM_NEON_STATE64: u32 = 17;

/// A thread's registers.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ThreadState {
    /// x86-64's, in `x86_THREAD_STATE64`'s order.
    X64([u64; 21]),
    /// ARM64's: X0..X28, FP, LR, SP, PC; and CPSR.
    Arm64 { regs: [u64; 33], cpsr: u32 },
}

/// A thread's exception state: where it faulted, and why.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ExceptionState {
    /// The fault address (x86's `faultvaddr`, ARM's FAR).
    pub address: u64,
    /// The trap number (x86) or the exception syndrome (ARM's ESR).
    pub code: u32,
    /// The error code (x86) or the exception (ARM).
    pub detail: u32,
}

/// A thread of a core file.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CoreThread {
    pub state: Option<ThreadState>,
    pub exception: Option<ExceptionState>,
    /// The vector registers (XMM0..XMM15, or V0..V31), if recorded.
    pub vectors: Vec<u128>,
}

/// An image the core says was loaded (lldb's `all image infos`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ImageInfo {
    pub path: String,
    pub uuid: [u8; 16],
    pub load_address: u64,
}

/// A Mach-O core file.
pub struct CoreFile<'d> {
    macho: MachO<'d>,
}

impl<'d> CoreFile<'d> {
    /// The core file `macho` is, if it is one.
    pub fn new(macho: MachO<'d>) -> Option<CoreFile<'d>> {
        (macho.header().filetype == MH_CORE).then_some(CoreFile { macho })
    }

    pub fn macho(&self) -> &MachO<'d> {
        &self.macho
    }

    fn endian(&self) -> Endian {
        self.macho.endian()
    }

    fn u32_at(&self, d: &[u8], at: usize) -> Option<u32> {
        let b: [u8; 4] = d.get(at..at + 4)?.try_into().ok()?;
        Some(match self.endian() {
            Endian::Little => u32::from_le_bytes(b),
            Endian::Big => u32::from_be_bytes(b),
        })
    }

    fn u64_at(&self, d: &[u8], at: usize) -> Option<u64> {
        let b: [u8; 8] = d.get(at..at + 8)?.try_into().ok()?;
        Some(match self.endian() {
            Endian::Little => u64::from_le_bytes(b),
            Endian::Big => u64::from_be_bytes(b),
        })
    }

    /// Its memory: each segment's address and the bytes the file holds for
    /// it, by address.
    pub fn memory(&self) -> Vec<(u64, &'d [u8])> {
        let data = self.macho.data();
        let mut out: Vec<(u64, &'d [u8])> = self
            .macho
            .load_commands()
            .into_iter()
            .filter(|lc| lc.cmd == LC_SEGMENT_64)
            .filter_map(|lc| {
                let d = lc.data;
                let vmaddr = self.u64_at(d, 24)?;
                let vmsize = self.u64_at(d, 32)?;
                let fileoff = usize::try_from(self.u64_at(d, 40)?).ok()?;
                let filesize = usize::try_from(self.u64_at(d, 48)?.min(vmsize)).ok()?;
                if filesize == 0 {
                    return None;
                }
                Some((vmaddr, data.get(fileoff..fileoff.checked_add(filesize)?)?))
            })
            .collect();
        out.sort_by_key(|r| r.0);
        out
    }

    /// The threads, in the order written.
    pub fn threads(&self) -> Vec<CoreThread> {
        let cpu = self.macho.header().cputype;
        let mut out = Vec::new();
        for lc in self.macho.load_commands() {
            if lc.cmd != LC_THREAD && lc.cmd != LC_UNIXTHREAD {
                continue;
            }
            let d = lc.data;
            let mut t = CoreThread {
                state: None,
                exception: None,
                vectors: Vec::new(),
            };
            let mut at = 8;
            while let (Some(flavor), Some(count)) = (self.u32_at(d, at), self.u32_at(d, at + 4)) {
                let state_at = at + 8;
                let Some(len) = usize::try_from(count).ok().and_then(|c| c.checked_mul(4)) else {
                    break;
                };
                let Some(state) = d.get(state_at..state_at.saturating_add(len)) else {
                    break;
                };
                self.flavor(cpu, flavor, state, &mut t);
                at = state_at + len;
                if len == 0 {
                    break;
                }
            }
            out.push(t);
        }
        out
    }

    fn flavor(&self, cpu: u32, flavor: u32, s: &[u8], t: &mut CoreThread) {
        let word = |i: usize| self.u64_at(s, 8 * i);
        match (cpu, flavor) {
            (CPU_TYPE_X86_64, X86_THREAD_STATE) => {
                // (A header: the state's own flavor and count.)
                if let (Some(f), Some(_)) = (self.u32_at(s, 0), self.u32_at(s, 4))
                    && let Some(rest) = s.get(8..)
                {
                    self.flavor(cpu, f, rest, t);
                }
            }
            (CPU_TYPE_X86_64, X86_THREAD_STATE64) => {
                let mut r = [0u64; 21];
                for (i, v) in r.iter_mut().enumerate() {
                    let Some(w) = word(i) else { return };
                    *v = w;
                }
                t.state = Some(ThreadState::X64(r));
            }
            (CPU_TYPE_X86_64, X86_EXCEPTION_STATE64) => {
                // trapno (u16), cpu (u16), err (u32), faultvaddr (u64).
                let trap = self.u32_at(s, 0).map(|v| v & 0xFFFF);
                if let (Some(code), Some(err), Some(address)) =
                    (trap, self.u32_at(s, 4), self.u64_at(s, 8))
                {
                    t.exception = Some(ExceptionState {
                        address,
                        code,
                        detail: err,
                    });
                }
            }
            (CPU_TYPE_X86_64, X86_FLOAT_STATE64) => {
                t.vectors = (0..16)
                    .filter_map(|i| {
                        let b: [u8; 16] = s.get(168 + 16 * i..184 + 16 * i)?.try_into().ok()?;
                        Some(u128::from_le_bytes(b))
                    })
                    .collect();
            }
            (CPU_TYPE_ARM64, ARM_THREAD_STATE64) => {
                let mut r = [0u64; 33];
                for (i, v) in r.iter_mut().enumerate() {
                    let Some(w) = word(i) else { return };
                    *v = w;
                }
                let cpsr = self.u32_at(s, 33 * 8).unwrap_or(0);
                t.state = Some(ThreadState::Arm64 { regs: r, cpsr });
            }
            (CPU_TYPE_ARM64, ARM_EXCEPTION_STATE64) => {
                if let (Some(address), Some(esr), Some(exc)) =
                    (self.u64_at(s, 0), self.u32_at(s, 8), self.u32_at(s, 12))
                {
                    t.exception = Some(ExceptionState {
                        address,
                        code: esr,
                        detail: exc,
                    });
                }
            }
            (CPU_TYPE_ARM64, ARM_NEON_STATE64) => {
                t.vectors = (0..32)
                    .filter_map(|i| {
                        let b: [u8; 16] = s.get(16 * i..16 * i + 16)?.try_into().ok()?;
                        Some(match self.endian() {
                            Endian::Little => u128::from_le_bytes(b),
                            Endian::Big => u128::from_be_bytes(b),
                        })
                    })
                    .collect();
            }
            _ => {}
        }
    }

    /// The notes: (owner, data).
    pub fn notes(&self) -> Vec<(&'d [u8], &'d [u8])> {
        let data = self.macho.data();
        self.macho
            .load_commands()
            .into_iter()
            .filter(|lc| lc.cmd == LC_NOTE)
            .filter_map(|lc| {
                let owner = lc.data.get(8..24)?;
                let owner = &owner[..owner.iter().position(|&b| b == 0).unwrap_or(16)];
                let offset = usize::try_from(self.u64_at(lc.data, 24)?).ok()?;
                let size = usize::try_from(self.u64_at(lc.data, 32)?).ok()?;
                Some((owner, data.get(offset..offset.checked_add(size)?)?))
            })
            .collect()
    }

    fn note(&self, owner: &[u8]) -> Option<&'d [u8]> {
        self.notes()
            .into_iter()
            .find(|(o, _)| *o == owner)
            .map(|(_, d)| d)
    }

    /// How many bits of a code address are the address (`addrable bits`;
    /// version 3's for low, user addresses).
    pub fn addressable_bits(&self) -> Option<u8> {
        let d = self.note(b"addrable bits")?;
        let version = self.u32_at(d, 0)?;
        let bits = if version >= 3 {
            self.u32_at(d, 8)
                .filter(|&b| b != 0)
                .or_else(|| self.u32_at(d, 4))?
        } else {
            self.u32_at(d, 4)?
        };
        u8::try_from(bits).ok().filter(|b| (1..64).contains(b))
    }

    /// The images loaded (`all image infos`).
    pub fn images(&self) -> Vec<ImageInfo> {
        let Some(d) = self.note(b"all image infos") else {
            return Vec::new();
        };
        let data = self.macho.data();
        let (Some(count), Some(entries), Some(size)) =
            (self.u32_at(d, 4), self.u64_at(d, 8), self.u32_at(d, 16))
        else {
            return Vec::new();
        };
        let (Ok(mut at), Ok(size)) = (usize::try_from(entries), usize::try_from(size)) else {
            return Vec::new();
        };
        let mut out = Vec::new();
        for _ in 0..count.min(1 << 16) {
            let Some(e) = data.get(at..at.saturating_add(size)) else {
                break;
            };
            let (Some(path_at), Some(load)) = (self.u64_at(e, 0), self.u64_at(e, 24)) else {
                break;
            };
            let Some(uuid) = e.get(8..24).and_then(|u| <[u8; 16]>::try_from(u).ok()) else {
                break;
            };
            let path = usize::try_from(path_at)
                .ok()
                .and_then(|p| data.get(p..))
                .map(|b| &b[..b.iter().position(|&c| c == 0).unwrap_or(b.len().min(4096))])
                .map(|b| String::from_utf8_lossy(b).into_owned())
                .unwrap_or_default();
            out.push(ImageInfo {
                path,
                uuid,
                load_address: load,
            });
            at += size;
        }
        out
    }
}
