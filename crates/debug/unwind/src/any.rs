//! One unwinder for an image of any format and architecture: [`unwind`]
//! finds the unwind data the image has for a frame's code, and applies the
//! unwinder that reads it:
//!
//! - a PE image's exception directory (`.pdata`), on x64, ARM64, and ARM
//!   ([`crate::windows_x64`], [`crate::windows_arm64`],
//!   [`crate::windows_arm32`]); code with no entry is a leaf;
//! - a Mach-O image's compact unwind information (`__unwind_info`), on
//!   x86-64 and ARM64 ([`crate::apple_compact`]), and the FDEs in
//!   `__eh_frame` its encodings send it to;
//! - an ELF image's `.ARM.exidx`, on ARM ([`crate::arm_ehabi`]);
//! - and, for any image (an ELF image's, a MinGW-built PE image's),
//!   DWARF call frame information: `.eh_frame`, then `.debug_frame`
//!   ([`crate::dwarf_cfi`]).
//!
//! Registers are by DWARF number, whatever the unwinder's own numbering
//! (x86-64: RAX 0, RDX 1, RCX 2, RBX 3, RSI 4, RDI 5, RBP 6, RSP 7, R8..R15
//! 8..15, RIP 16, XMM0..XMM15 17..32; AArch64: X0..X30 0..30, SP 31, PC 32,
//! V0..V31 64..95; ARM: R0..R15 0..15, D0..D31 256..287; x86: EIP 8, ESP
//! 4; RISC-V: x0..x31 0..31, f0..f31 32..63, its PC [`RISCV_PC`];
//! LoongArch: r0..r31 0..31, f0..f31 32..63, its PC [`LOONGARCH_PC`];
//! PowerPC: r0..r31 0..31, f0..f31 32..63, LR 65, CTR 66, its PC
//! [`POWERPC_PC`]; MIPS: r0..r31 0..31, f0..f31 32..63, its PC
//! [`MIPS_PC`]).
//! 32-bit x86 frames need a PDB's frame data ([`crate::windows_x86`]), which an image doesn't have: they aren't
//! unwound here.

use alloc::vec::Vec;

use hexcavator_core::Arch;
use hexcavator_dwarf::cfi::{Cfi, CfiKind, Entry as CfiEntry};
use hexcavator_image::{Image, Section};

use crate::dwarf_cfi::{self, Frame, Registers, Target};
use crate::{Error, Memory, Result};

/// The unwind data a frame was unwound by.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Method {
    /// A PE image's `.pdata` entry (and its unwind information).
    Pdata,
    /// No unwind data: a leaf function (its return address in the link
    /// register, or at the stack pointer).
    Leaf,
    /// A Mach-O image's compact unwind encoding.
    Compact,
    /// An ARM EHABI index entry.
    ArmExidx,
    /// DWARF call frame information (an FDE in `.eh_frame` or
    /// `.debug_frame`).
    Dwarf(CfiKind),
}

/// A frame unwound: how, and what was found (its CFA, its caller's PC,
/// where each restored register was read from).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Unwound {
    pub method: Method,
    pub frame: Frame,
}

/// Where [`Registers`] keeps a RISC-V frame's PC: DWARF numbers none
/// (its x0 to x31 are 0 to 31, f0 to f31 32 to 63, and its CFI's return
/// address column is `ra`'s, 1), so this is past every number it does.
pub const RISCV_PC: u16 = Registers::LIMIT - 1;

/// Where [`Registers`] keeps a LoongArch frame's PC: likewise, DWARF
/// numbers none (r0 to r31 are 0 to 31, f0 to f31 32 to 63; its return
/// address column is `$ra`'s, 1).
pub const LOONGARCH_PC: u16 = Registers::LIMIT - 1;

/// Where [`Registers`] keeps a PowerPC frame's PC: likewise, DWARF numbers
/// none (its CFI's return address column is the link register's, 65).
pub const POWERPC_PC: u16 = Registers::LIMIT - 1;

/// Where [`Registers`] keeps a MIPS frame's PC: likewise, DWARF numbers
/// none (its CFI's return address column is `$ra`'s, 31).
pub const MIPS_PC: u16 = Registers::LIMIT - 1;

/// The DWARF numbers of an architecture's PC and stack pointer (RISC-V's
/// PC: [`RISCV_PC`]; LoongArch's: [`LOONGARCH_PC`]; PowerPC's:
/// [`POWERPC_PC`]; MIPS's: [`MIPS_PC`]).
pub fn registers_of(arch: Arch) -> Option<(u16, u16)> {
    match arch {
        Arch::X64 => Some((16, 7)),
        Arch::Arm64 => Some((32, 31)),
        Arch::Arm => Some((15, 13)),
        Arch::X86 => Some((8, 4)),
        Arch::RiscV32 | Arch::RiscV64 => Some((RISCV_PC, 2)),
        Arch::LoongArch32 | Arch::LoongArch64 => Some((LOONGARCH_PC, 3)),
        Arch::PowerPc | Arch::PowerPc64 => Some((POWERPC_PC, 1)),
        Arch::Mips | Arch::Mips64 => Some((MIPS_PC, 29)),
        _ => None,
    }
}

/// Unwinds one frame of code in `image`, loaded `bias` bytes from where
/// it was linked to be (a PE image's load address, since its addresses
/// are RVAs; an ELF or Mach-O image's slide): `registers` (the frame's)
/// become the caller's. `caller` says the PC is a return address (any
/// frame but the innermost), which may be just past its function's end:
/// its function is found by the byte before it.
///
/// # Errors
///
/// If the image has no unwind data for the PC (and it isn't a leaf), the
/// data is bad, or the stack can't be read.
pub fn unwind(
    image: &Image<'_>,
    bias: u64,
    caller: bool,
    registers: &mut Registers,
    memory: &dyn Memory,
) -> Result<Unwound> {
    let arch = image.arch();
    let (pc_register, _) = registers_of(arch).ok_or(Error::Unsupported { rva: 0 })?;
    let pc = registers.get(pc_register).ok_or(Error::Register {
        register: pc_register,
    })?;
    // (An ARM return address keeps its Thumb bit.)
    let pc = if arch == Arch::Arm { pc & !1 } else { pc };
    let address = pc.wrapping_sub(bias);
    let lookup = if caller {
        address.wrapping_sub(1)
    } else {
        address
    };
    if let Some(pe) = image.pe()
        && let Some(u) = pdata(image, pe, bias, lookup, registers, memory)?
    {
        return Ok(with_pc(u, pc_register, registers));
    }
    if image.macho().is_some()
        && let Some(u) = compact(image, bias, lookup, registers, memory)?
    {
        return Ok(with_pc(u, pc_register, registers));
    }
    if arch == Arch::Arm
        && image.elf().is_some()
        && let Some(u) = exidx(image, lookup, registers, memory)?
    {
        return Ok(with_pc(u, pc_register, registers));
    }
    let mut found = None;
    for kind in [CfiKind::EhFrame, CfiKind::DebugFrame] {
        found = dwarf(image, kind, lookup, None, registers, memory)?;
        if found.is_some() {
            break;
        }
    }
    let found = match found {
        Some(u) => u,
        // A PE image's code that no `.pdata` entry covers is a leaf; so is
        // RISC-V, LoongArch, PowerPC, and MIPS code without CFI (its
        // return address in `ra`, or the link register).
        None if (image.pe().is_some() && matches!(arch, Arch::X64 | Arch::Arm64 | Arch::Arm))
            || matches!(
                arch,
                Arch::RiscV32
                    | Arch::RiscV64
                    | Arch::LoongArch32
                    | Arch::LoongArch64
                    | Arch::PowerPc
                    | Arch::PowerPc64
                    | Arch::Mips
                    | Arch::Mips64
            ) =>
        {
            leaf(arch, registers, memory)?
        }
        None => {
            return Err(Error::UnwindData {
                rva: u32::try_from(address).unwrap_or(u32::MAX),
            });
        }
    };
    Ok(with_pc(found, pc_register, registers))
}

/// `unwound`, with the caller's PC set to its return address (DWARF's
/// return address column is the link register on ARM, not the PC).
fn with_pc(unwound: Unwound, pc_register: u16, registers: &mut Registers) -> Unwound {
    if let Some(ra) = unwound.frame.return_address {
        registers.set(pc_register, Some(ra));
    }
    unwound
}

/// A leaf function's frame: its return address in the link register
/// (ARM64, ARM), or at the stack pointer (x64).
fn leaf(arch: Arch, registers: &mut Registers, memory: &dyn Memory) -> Result<Unwound> {
    let get =
        |registers: &Registers, r: u16| registers.get(r).ok_or(Error::Register { register: r });
    let frame = match arch {
        Arch::X64 => {
            let sp = get(registers, 7)?;
            let ra = crate::read_u64(memory, sp)?;
            let cfa = sp.wrapping_add(8);
            registers.set(16, Some(ra));
            registers.set(7, Some(cfa));
            Frame {
                cfa,
                return_address: Some(ra),
                return_address_signed: false,
                saved: alloc::vec![(16, sp)],
            }
        }
        Arch::Arm64
        | Arch::Arm
        | Arch::RiscV32
        | Arch::RiscV64
        | Arch::LoongArch32
        | Arch::LoongArch64
        | Arch::PowerPc
        | Arch::PowerPc64
        | Arch::Mips
        | Arch::Mips64 => {
            let (pc, sp, lr) = match arch {
                Arch::Arm64 => (32, 31, 30),
                Arch::Arm => (15, 13, 14),
                Arch::LoongArch32 | Arch::LoongArch64 => (LOONGARCH_PC, 3, 1),
                Arch::PowerPc | Arch::PowerPc64 => (POWERPC_PC, 1, 65),
                Arch::Mips | Arch::Mips64 => (MIPS_PC, 29, 31),
                _ => (RISCV_PC, 2, 1),
            };
            let ra = get(registers, lr)?;
            let cfa = get(registers, sp)?;
            registers.set(pc, Some(ra));
            Frame {
                cfa,
                return_address: Some(ra),
                return_address_signed: false,
                saved: Vec::new(),
            }
        }
        _ => return Err(Error::Unsupported { rva: 0 }),
    };
    Ok(Unwound {
        method: Method::Leaf,
        frame,
    })
}

// ------------------------------------------------------------------- PE

/// A register number from an index into an unwinder's registers.
fn reg(i: usize) -> u16 {
    u16::try_from(i).unwrap_or(u16::MAX)
}

/// x64's general registers: Windows' order (RAX RCX RDX RBX RSP RBP RSI
/// RDI R8..R15), by DWARF number.
const X64_DWARF: [u16; 16] = [0, 2, 1, 3, 7, 6, 4, 5, 8, 9, 10, 11, 12, 13, 14, 15];

/// The frame by the `.pdata` entry covering `rva`, if one does.
fn pdata(
    image: &Image<'_>,
    pe: &hexcavator_pe::PeFile<'_>,
    bias: u64,
    rva: u64,
    registers: &mut Registers,
    memory: &dyn Memory,
) -> Result<Option<Unwound>> {
    use hexcavator_pe::consts::directory::EXCEPTION;
    let Some(dir) = pe.data_directory(EXCEPTION) else {
        return Ok(None);
    };
    let (table, size) = (dir.virtual_address.get(), dir.size.get());
    let Ok(rva) = u32::try_from(rva) else {
        return Ok(None);
    };
    let get =
        |registers: &Registers, r: u16| registers.get(r).ok_or(Error::Register { register: r });
    let frame = match image.arch() {
        Arch::X64 => {
            use crate::windows_x64 as w;
            let Some(f) = w::lookup(pe, table, size, rva) else {
                return Ok(None);
            };
            let mut c = w::Context {
                rip: get(registers, 16)?,
                ..w::Context::default()
            };
            for (i, &d) in X64_DWARF.iter().enumerate() {
                c.gpr[i] = registers.get(d).unwrap_or(0);
            }
            for (i, x) in c.xmm.iter_mut().enumerate() {
                *x = registers.get(17 + reg(i)).map_or(0, u128::from);
            }
            let f = w::unwind(pe, bias, f, &mut c, memory)?;
            let mut saved = Vec::new();
            for (i, &d) in X64_DWARF.iter().enumerate() {
                registers.set(d, Some(c.gpr[i]));
                if let Some(at) = f.saved_gpr[i] {
                    saved.push((d, at));
                }
            }
            for (i, at) in f.saved_xmm.iter().enumerate() {
                if let Some(at) = at {
                    saved.push((17 + reg(i), *at));
                }
            }
            registers.set(16, Some(c.rip));
            Frame {
                cfa: c.rsp(),
                return_address: (c.rip != 0).then_some(c.rip),
                return_address_signed: false,
                saved,
            }
        }
        Arch::Arm64 => {
            use crate::windows_arm64 as w;
            let Some(f) = w::lookup(pe, table, size, rva) else {
                return Ok(None);
            };
            let mut c = w::Context {
                pc: get(registers, 32)?,
                sp: get(registers, 31)?,
                ..w::Context::default()
            };
            for (i, x) in c.x.iter_mut().enumerate() {
                *x = registers.get(reg(i)).unwrap_or(0);
            }
            for (i, v) in c.v.iter_mut().enumerate() {
                *v = registers.get(64 + reg(i)).map_or(0, u128::from);
            }
            let f = w::unwind(pe, bias, f, &mut c, memory)?;
            let mut saved = Vec::new();
            for (i, x) in c.x.iter().enumerate() {
                registers.set(reg(i), Some(*x));
                if let Some(at) = f.saved_x[i] {
                    saved.push((reg(i), at));
                }
            }
            for (i, at) in f.saved_v.iter().enumerate() {
                if let Some(at) = at {
                    saved.push((64 + reg(i), *at));
                }
            }
            registers.set(31, Some(c.sp));
            registers.set(32, Some(c.pc));
            Frame {
                cfa: c.sp,
                return_address: (c.pc != 0).then_some(c.pc),
                return_address_signed: f.signed_return,
                saved,
            }
        }
        Arch::Arm => {
            use crate::windows_arm32 as w;
            let Some(f) = w::lookup(pe, table, size, rva) else {
                return Ok(None);
            };
            let Ok(base) = u32::try_from(bias) else {
                return Ok(None);
            };
            let mut c = w::Context::default();
            for (i, r) in c.r.iter_mut().enumerate() {
                *r = registers
                    .get(reg(i))
                    .map_or(0, |v| u32::try_from(v & 0xFFFF_FFFF).unwrap_or(0));
            }
            for (i, d) in c.d.iter_mut().enumerate() {
                *d = registers.get(256 + reg(i)).unwrap_or(0);
            }
            let f = w::unwind(pe, base, f, &mut c, memory)?;
            let mut saved = Vec::new();
            for (i, r) in c.r.iter().enumerate() {
                registers.set(reg(i), Some(u64::from(*r)));
                if let Some(at) = f.saved_r.get(i).copied().flatten() {
                    saved.push((reg(i), u64::from(at)));
                }
            }
            for (i, at) in f.saved_d.iter().enumerate() {
                if let Some(at) = at {
                    saved.push((256 + reg(i), u64::from(*at)));
                }
            }
            let pc = c.r[15];
            Frame {
                cfa: u64::from(c.r[13]),
                return_address: (pc != 0).then_some(u64::from(pc)),
                return_address_signed: false,
                saved,
            }
        }
        _ => return Ok(None),
    };
    Ok(Some(Unwound {
        method: Method::Pdata,
        frame,
    }))
}

// ---------------------------------------------------------------- Mach-O

/// The frame by the compact unwind encoding for `address`, if the image
/// has one (or the FDE it sends to).
fn compact(
    image: &Image<'_>,
    bias: u64,
    address: u64,
    registers: &mut Registers,
    memory: &dyn Memory,
) -> Result<Option<Unwound>> {
    use crate::apple_compact::{self as c, Step};
    let Some(m) = image.macho() else {
        return Ok(None);
    };
    let Some(info) = section(image, b"__unwind_info")
        .and_then(|s| section_bytes(image, &s))
        .and_then(|d| hexcavator_macho::unwind_info::UnwindInfo::parse(d, image.endian()))
    else {
        return Ok(None);
    };
    // (Its offsets are from the image's base: `__TEXT`'s address.)
    let base = m
        .segments()
        .iter()
        .find(|s| s.fileoff == 0 && s.filesize > 0)
        .map_or(0, |s| s.vmaddr);
    let Some(offset) = address
        .checked_sub(base)
        .and_then(|o| u32::try_from(o).ok())
    else {
        return Ok(None);
    };
    let Some(found) = info.lookup(offset) else {
        return Ok(None);
    };
    let start = bias
        .wrapping_add(base)
        .wrapping_add(u64::from(found.entry.function));
    let step = match image.arch() {
        Arch::X64 => c::unwind_x86_64(found.entry.encoding, start, registers, memory)?,
        Arch::Arm64 => c::unwind_arm64(found.entry.encoding, registers, memory)?,
        _ => return Ok(None),
    };
    match step {
        Step::Unwound(frame) => Ok(Some(Unwound {
            method: Method::Compact,
            frame,
        })),
        Step::Dwarf { eh_frame_offset } => dwarf(
            image,
            CfiKind::EhFrame,
            address,
            Some(u64::from(eh_frame_offset)),
            registers,
            memory,
        ),
        // (No unwind information: an ARM64 leaf's return address is in
        // LR.)
        Step::None if image.arch() == Arch::Arm64 => leaf(Arch::Arm64, registers, memory).map(Some),
        Step::None => Ok(None),
    }
}

// ------------------------------------------------------------------- ELF

/// The frame by the `.ARM.exidx` entry covering `address`, if one does.
fn exidx(
    image: &Image<'_>,
    address: u64,
    registers: &mut Registers,
    memory: &dyn Memory,
) -> Result<Option<Unwound>> {
    use crate::arm_ehabi;
    let Some(s) = section(image, b".ARM.exidx") else {
        return Ok(None);
    };
    let Some(data) = section_bytes(image, &s) else {
        return Ok(None);
    };
    let index = arm_ehabi::Exidx {
        data,
        address: s.addr,
        endian: image.endian(),
    };
    let Some(i) = index.lookup(address) else {
        return Ok(None);
    };
    // (Table entries are read from the image: they're in it.)
    let Some(entry) = index.entry(i, &ImageMemory(image)) else {
        return Ok(None);
    };
    let frame = arm_ehabi::unwind(&entry, image.endian(), registers, memory)?;
    Ok(Some(Unwound {
        method: Method::ArmExidx,
        frame,
    }))
}

/// The image's bytes, as [`Memory`] at its own addresses.
struct ImageMemory<'a, 'd>(&'a Image<'d>);

impl Memory for ImageMemory<'_, '_> {
    fn read(&self, address: u64, buf: &mut [u8]) -> bool {
        match self.0.bytes_at(address, buf.len() as u64) {
            Some(b) => {
                buf.copy_from_slice(b);
                true
            }
            None => false,
        }
    }
}

// ----------------------------------------------------------------- DWARF

/// The section of the image named `name` (`.eh_frame`, or Mach-O's
/// `__eh_frame`).
fn section<'d>(image: &Image<'d>, name: &[u8]) -> Option<Section<'d>> {
    image.sections().into_iter().find(|s| s.name == name)
}

/// A section's bytes in the file (a debug section isn't loaded).
fn section_bytes<'d>(image: &Image<'d>, s: &Section<'d>) -> Option<&'d [u8]> {
    let start = usize::try_from(s.file_offset).ok()?;
    let len = if s.vm_size > 0 {
        s.file_size.min(s.vm_size)
    } else {
        s.file_size
    };
    let len = usize::try_from(len).ok()?;
    image.data().get(start..start.checked_add(len)?)
}

/// The frame by the FDE (in the image's CFI of `kind`) covering
/// `address`, or the one at `offset`, if there is one.
fn dwarf(
    image: &Image<'_>,
    kind: CfiKind,
    address: u64,
    offset: Option<u64>,
    registers: &mut Registers,
    memory: &dyn Memory,
) -> Result<Option<Unwound>> {
    let names: &[&[u8]] = match kind {
        CfiKind::EhFrame => &[b".eh_frame", b"__eh_frame"],
        CfiKind::DebugFrame => &[b".debug_frame", b"__debug_frame"],
    };
    let Some(s) = names.iter().find_map(|n| section(image, n)) else {
        return Ok(None);
    };
    let Some(data) = section_bytes(image, &s) else {
        return Ok(None);
    };
    let arch = image.arch();
    let Some((_, sp)) = registers_of(arch) else {
        return Ok(None);
    };
    let address_size = arch.pointer_size().unwrap_or(8);
    let cfi = Cfi {
        data,
        kind,
        endian: image.endian(),
        address: s.addr,
        address_size,
        text_base: None,
        data_base: None,
    };
    let found = match offset {
        Some(o) => usize::try_from(o)
            .ok()
            .and_then(|o| cfi.entry_at(o))
            .and_then(|(e, _)| match e {
                CfiEntry::Fde(f) => {
                    let cie = cfi.entry_at(usize::try_from(f.cie).ok()?)?.0;
                    match cie {
                        CfiEntry::Cie(c) => Some((c, f)),
                        CfiEntry::Fde(_) => None,
                    }
                }
                CfiEntry::Cie(_) => None,
            }),
        None => cfi.find(address),
    };
    let Some((cie, fde)) = found else {
        return Ok(None);
    };
    let Some(row) = hexcavator_dwarf::cfi::row_at(&cfi, &cie, &fde, address) else {
        return Ok(None);
    };
    let target = Target {
        endian: image.endian(),
        address_size,
        stack_pointer: sp,
    };
    let frame = dwarf_cfi::unwind(&row, cie.return_address_register, target, registers, memory)?;
    Ok(Some(Unwound {
        method: Method::Dwarf(kind),
        frame,
    }))
}
