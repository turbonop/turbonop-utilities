//! ARM EHABI unwinding (Arm's "Exception Handling ABI for the Arm
//! Architecture"): `.ARM.exidx` index entries, their `.ARM.extab` table
//! entries, and the unwind opcodes, applied to a frame.
//!
//! Registers are by DWARF number (r0..r15 are 0..15; d0..d31 are
//! 256..287), as in [`crate::dwarf_cfi`], and the result is the same
//! [`Frame`]. iWMMX registers popped are skipped over, not restored. A
//! return address keeps its Thumb bit (bit 0), for the caller to strip.
//!
//! `tests/arm_ehabi_readobj.rs` compares every index entry (function, table entry,
//! model, personality, opcode bytes) with llvm-readobj --unwind on ARM and
//! Thumb images, and unwinds from each. (A personality routine's own data
//! is read in GNU's layout, opcodes first, which llvm-readobj doesn't
//! decode.)

use alloc::vec::Vec;

use turbonop_core::Endian;

use crate::dwarf_cfi::{Frame, Registers};
use crate::{Error, Memory, Result};

const SP: u16 = 13;
const LR: u16 = 14;
const PC: u16 = 15;
const D0: u16 = 256;

pub use turbonop_elf::arm_exidx::{Entry, Model};

/// An `.ARM.exidx` section (see [`turbonop_elf::arm_exidx::Exidx`]),
/// its table entries read from [`Memory`].
#[derive(Clone, Copy, Debug)]
pub struct Exidx<'d> {
    pub data: &'d [u8],
    /// The section's address.
    pub address: u64,
    pub endian: Endian,
}

impl Exidx<'_> {
    fn index(&self) -> turbonop_elf::arm_exidx::Exidx<'_> {
        turbonop_elf::arm_exidx::Exidx {
            data: self.data,
            address: self.address,
            endian: self.endian,
        }
    }

    pub fn len(&self) -> usize {
        self.index().len()
    }

    pub fn is_empty(&self) -> bool {
        self.index().is_empty()
    }

    /// Entry `i`; `extab` reads table entries (by address).
    pub fn entry(&self, i: usize, extab: &dyn Memory) -> Option<Entry> {
        let endian = self.endian;
        self.index().entry(i, &|a| {
            let mut b = [0u8; 4];
            extab.read(a, &mut b).then(|| match endian {
                Endian::Little => u32::from_le_bytes(b),
                Endian::Big => u32::from_be_bytes(b),
            })
        })
    }

    /// The index of the entry covering `pc` (the last starting at or
    /// before it; entries are sorted by function).
    pub fn lookup(&self, pc: u64) -> Option<usize> {
        self.index().lookup(pc)
    }
}

/// Runs an entry's opcodes on `registers` (r13 is the frame's SP): they
/// become the caller's.
pub fn unwind(
    entry: &Entry,
    endian: Endian,
    registers: &mut Registers,
    memory: &dyn Memory,
) -> Result<Frame> {
    let bad = Error::Encoding {
        encoding: entry.opcodes.first().copied().map_or(0, u32::from),
    };
    if entry.model == Model::CantUnwind {
        return Err(bad);
    }
    let read = |at: u64| -> Result<u32> {
        let mut b = [0u8; 4];
        if !memory.read(at, &mut b) {
            return Err(Error::Memory { address: at });
        }
        Ok(match endian {
            Endian::Little => u32::from_le_bytes(b),
            Endian::Big => u32::from_be_bytes(b),
        })
    };
    let mut vsp = registers.get(SP).ok_or(Error::Register { register: SP })?;
    let mut saved = Vec::new();
    let mut pc_set = false;
    let mut sp_popped = false;
    let ops = &entry.opcodes;
    let mut i = 0;
    let next = |i: &mut usize| -> Result<u8> {
        let b = *ops.get(*i).ok_or(bad)?;
        *i += 1;
        Ok(b)
    };
    // Pops the core registers in `mask` (bit n: r(n)), lowest first.
    let mut pop_core = |mask: u16, vsp: &mut u64, registers: &mut Registers| -> Result<()> {
        let start = *vsp;
        for r in 0..16u16 {
            if mask & (1 << r) != 0 {
                let v = read(*vsp)?;
                saved.push((r, *vsp));
                registers.set(r, Some(u64::from(v)));
                *vsp = vsp.wrapping_add(4);
                if r == PC {
                    pc_set = true;
                }
            }
        }
        // (Popping SP sets it to the value popped.)
        if mask & (1 << SP) != 0 {
            sp_popped = true;
            *vsp = registers.get(SP).unwrap_or(start);
        }
        Ok(())
    };
    let pop_vfp =
        |first: u16, count: u16, vsp: &mut u64, registers: &mut Registers| -> Result<()> {
            for d in first..first + count {
                let (a, b) = (read(*vsp)?, read(vsp.wrapping_add(4))?);
                // (Words in memory order: low then high, little-endian.)
                let v = match endian {
                    Endian::Little => u64::from(a) | u64::from(b) << 32,
                    Endian::Big => u64::from(b) | u64::from(a) << 32,
                };
                registers.set(D0 + d, Some(v));
                *vsp = vsp.wrapping_add(8);
            }
            Ok(())
        };
    while i < ops.len() {
        let op = next(&mut i)?;
        match op {
            0x00..=0x3F => vsp = vsp.wrapping_add((u64::from(op & 0x3F) << 2) + 4),
            0x40..=0x7F => vsp = vsp.wrapping_sub((u64::from(op & 0x3F) << 2) + 4),
            0x80..=0x8F => {
                let mask = u16::from(op & 0xF) << 8 | u16::from(next(&mut i)?);
                if mask == 0 {
                    // Refuse to unwind.
                    return Err(bad);
                }
                pop_core(mask << 4, &mut vsp, registers)?;
            }
            0x9D | 0x9F => return Err(bad),
            0x90..=0x9F => {
                vsp = registers.get(u16::from(op & 0xF)).ok_or(Error::Register {
                    register: u16::from(op & 0xF),
                })?;
            }
            0xA0..=0xAF => {
                let n = u16::from(op & 7);
                let mut mask = ((1u16 << (n + 1)) - 1) << 4;
                if op & 8 != 0 {
                    mask |= 1 << LR;
                }
                pop_core(mask, &mut vsp, registers)?;
            }
            0xB0 => break,
            0xB1 => {
                let mask = next(&mut i)?;
                if mask == 0 || mask & 0xF0 != 0 {
                    return Err(bad);
                }
                pop_core(u16::from(mask), &mut vsp, registers)?;
            }
            0xB2 => {
                let mut v = 0u64;
                let mut shift = 0;
                loop {
                    let b = next(&mut i)?;
                    if shift < 64 {
                        v |= u64::from(b & 0x7F) << shift;
                    }
                    shift += 7;
                    if b & 0x80 == 0 {
                        break;
                    }
                }
                vsp = vsp.wrapping_add(0x204).wrapping_add(v << 2);
            }
            0xB3 | 0xC8 | 0xC9 => {
                let b = next(&mut i)?;
                let (start, count) = (u16::from(b >> 4), u16::from(b & 0xF) + 1);
                let first = if op == 0xC8 { 16 + start } else { start };
                pop_vfp(first, count, &mut vsp, registers)?;
                // (FSTMFDX leaves a pad word.)
                if op == 0xB3 {
                    vsp = vsp.wrapping_add(4);
                }
            }
            0xB8..=0xBF | 0xD0..=0xD7 => {
                pop_vfp(8, u16::from(op & 7) + 1, &mut vsp, registers)?;
                if op <= 0xBF {
                    vsp = vsp.wrapping_add(4);
                }
            }
            // iWMMX: wR registers (8 bytes each) and wCGR (4 bytes each).
            0xC6 => {
                let b = next(&mut i)?;
                vsp = vsp.wrapping_add(8 * (u64::from(b & 0xF) + 1));
            }
            0xC7 => {
                let mask = next(&mut i)?;
                if mask == 0 || mask & 0xF0 != 0 {
                    return Err(bad);
                }
                vsp = vsp.wrapping_add(4 * u64::from(mask.count_ones()));
            }
            0xC0..=0xC5 => vsp = vsp.wrapping_add(8 * (u64::from(op & 7) + 1)),
            // Spare.
            _ => return Err(bad),
        }
    }
    if !sp_popped {
        registers.set(SP, Some(vsp));
    }
    let return_address = if pc_set {
        registers.get(PC)
    } else {
        let lr = registers.get(LR);
        registers.set(PC, lr);
        lr
    };
    Ok(Frame {
        cfa: registers.get(SP).unwrap_or(vsp),
        return_address,
        return_address_signed: false,
        saved,
    })
}
