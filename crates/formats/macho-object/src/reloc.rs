//! Relocation types, how they pair, and the addresses local relocations
//! leave in a section's bytes.
//!
//! A relocation is one entry or two. The pairs, in file order (LLVM adds
//! the second entry first and writes a section's entries in reverse):
//!
//! - `X86_64_RELOC_SUBTRACTOR` then `X86_64_RELOC_UNSIGNED` (and arm64's
//!   two of those names): the value is the second's target less the
//!   first's, plus what the bytes hold. Both entries have the same
//!   `r_address` and `r_length`.
//! - `ARM64_RELOC_ADDEND` then `ARM64_RELOC_BRANCH26`, `ARM64_RELOC_PAGE21`
//!   or `ARM64_RELOC_PAGEOFF12`: the addend entry's `r_symbolnum` is the
//!   addend (24 bits, signed), not a symbol or a section; the instruction
//!   holds none.
//! - The 32-bit targets' `*_RELOC_PAIR` follows the entry it completes
//!   (i386's `GENERIC_RELOC_SECTDIFF` and `GENERIC_RELOC_LOCAL_SECTDIFF`,
//!   scattered, whose pair's `r_value` is the subtrahend's address; arm's
//!   `ARM_RELOC_SECTDIFF`, `ARM_RELOC_HALF` and `ARM_RELOC_HALF_SECTDIFF`,
//!   whose pair's `r_address` holds the other half of the value).
//!
//! What a local relocation (`r_extern` 0, `r_symbolnum` a section's
//! ordinal) leaves in the bytes, as LLVM writes them:
//!
//! - x86_64 `UNSIGNED`: the target's address (its section's address plus
//!   its offset) plus the addend. `SUBTRACTOR` (local when the subtrahend
//!   has no symbol before it, as in debug sections): less the subtrahend's
//!   address. `SIGNED`, `SIGNED_1`, `SIGNED_2`, `SIGNED_4` and `BRANCH`
//!   (PC-relative, 4 bytes): the target's address less the address after
//!   the 4 bytes and the 1, 2 or 4 bytes of immediate after them. x86_64
//!   makes local relocations only where the target has no symbol before it
//!   in its section, and in debug sections, always.
//! - arm64 `UNSIGNED` (8 bytes; 4 or 8 in debug sections) and
//!   `SUBTRACTOR`: as x86_64's. LLVM makes no other local arm64 relocation
//!   (an instruction's would need its address encoded in its immediate
//!   fields); another tool's are listed as unadjusted.
//! - i386 (per LLVM's `X86MachObjectWriter`): a plain local `VANILLA`
//!   entry, the target's address, less the entry's own address if it is
//!   PC-relative; a plain external PC-relative one, the addend less the
//!   entry's own address; a scattered `VANILLA` one that isn't
//!   PC-relative, the address of its `r_value` plus the addend;
//!   `SECTDIFF` and `LOCAL_SECTDIFF` (scattered, and a `PAIR` after),
//!   the difference of the two `r_value`s' addresses plus the addend.
//!   Scattered PC-relative ones (LLVM leaves out the entry's section's
//!   address) and `PB_LA_PTR` and `TLV` aren't handled.
//! - The other 32-bit targets' (arm, PowerPC) `*_RELOC_VANILLA` that
//!   isn't PC-relative: plain, the target's address. Their PC-relative,
//!   scattered and paired encodings aren't handled.
//!
//! External relocations on x86_64 and arm64 leave only an addend in the
//! bytes, which doesn't move with the sections.

use alloc::vec::Vec;
use core::ops::Range;

use crate::consts::{CPU_TYPE_ARM64, CPU_TYPE_ARM64_32, CPU_TYPE_X86, CPU_TYPE_X86_64};
use crate::{Endian, Relocation, Section};

/// x86_64's relocation types (`X86_64_RELOC_*`).
pub mod x86_64 {
    /// An absolute address (4 or 8 bytes).
    pub const UNSIGNED: u8 = 0;
    /// A PC-relative 4-byte displacement (`RIP`-relative data).
    pub const SIGNED: u8 = 1;
    /// A `call` or `jmp` displacement.
    pub const BRANCH: u8 = 2;
    /// A `movq` of a GOT entry (the linker may make it a `leaq`).
    pub const GOT_LOAD: u8 = 3;
    /// Another reference to a GOT entry.
    pub const GOT: u8 = 4;
    /// The subtrahend of a difference: the `UNSIGNED` entry after it is
    /// the minuend.
    pub const SUBTRACTOR: u8 = 5;
    /// `SIGNED` with a 1-byte immediate after the displacement.
    pub const SIGNED_1: u8 = 6;
    /// `SIGNED` with a 2-byte immediate after.
    pub const SIGNED_2: u8 = 7;
    /// `SIGNED` with a 4-byte immediate after.
    pub const SIGNED_4: u8 = 8;
    /// A thread-local variable's descriptor (`movq _v@TLVP(%rip)`).
    pub const TLV: u8 = 9;
}

/// arm64's relocation types (`ARM64_RELOC_*`).
pub mod arm64 {
    /// An absolute address (4 or 8 bytes).
    pub const UNSIGNED: u8 = 0;
    /// The subtrahend of a difference: the `UNSIGNED` entry after it is
    /// the minuend.
    pub const SUBTRACTOR: u8 = 1;
    /// A `b` or `bl`'s 26-bit displacement.
    pub const BRANCH26: u8 = 2;
    /// An `adrp`'s page.
    pub const PAGE21: u8 = 3;
    /// The offset in its page (an `add` or a load's immediate).
    pub const PAGEOFF12: u8 = 4;
    /// An `adrp` of a GOT entry's page.
    pub const GOT_LOAD_PAGE21: u8 = 5;
    /// A load of a GOT entry from its page.
    pub const GOT_LOAD_PAGEOFF12: u8 = 6;
    /// A PC-relative pointer to a GOT entry (4 bytes).
    pub const POINTER_TO_GOT: u8 = 7;
    /// An `adrp` of a thread-local variable's descriptor's page.
    pub const TLVP_LOAD_PAGE21: u8 = 8;
    /// The descriptor's offset in its page.
    pub const TLVP_LOAD_PAGEOFF12: u8 = 9;
    /// The addend of the `BRANCH26`, `PAGE21` or `PAGEOFF12` entry after
    /// it, in its `r_symbolnum`.
    pub const ADDEND: u8 = 10;
    /// An authenticated pointer (arm64e): the bytes hold the addend (low
    /// 32 bits), discriminator, key, and diversity.
    pub const AUTHENTICATED_POINTER: u8 = 11;
}

/// The 32-bit targets' generic types (i386's `GENERIC_RELOC_*`; arm's and
/// PowerPC's `*_RELOC_VANILLA` and `*_RELOC_PAIR` are the same numbers).
pub mod generic {
    pub const VANILLA: u8 = 0;
    /// The second entry of a pair.
    pub const PAIR: u8 = 1;
    pub const SECTDIFF: u8 = 2;
    pub const PB_LA_PTR: u8 = 3;
    pub const LOCAL_SECTDIFF: u8 = 4;
    pub const TLV: u8 = 5;
}

/// arm's types (`ARM_RELOC_*`) beyond the generic ones.
pub mod arm {
    pub const SECTDIFF: u8 = 2;
    pub const LOCAL_SECTDIFF: u8 = 3;
    pub const PB_LA_PTR: u8 = 4;
    pub const BR24: u8 = 5;
    pub const THUMB_RELOC_BR22: u8 = 6;
    pub const THUMB_32BIT_BRANCH: u8 = 7;
    pub const HALF: u8 = 8;
    pub const HALF_SECTDIFF: u8 = 9;
}

/// The CPU's `SUBTRACTOR` type (none: 0xFF).
fn subtractor(cputype: u32) -> u8 {
    if cputype == CPU_TYPE_X86_64 {
        x86_64::SUBTRACTOR
    } else if is_arm64(cputype) {
        arm64::SUBTRACTOR
    } else {
        0xFF
    }
}

fn is_arm64(cputype: u32) -> bool {
    cputype == CPU_TYPE_ARM64 || cputype == CPU_TYPE_ARM64_32
}

/// Whether the entry after one of type `first` belongs to it (the second
/// entry of a pair), if the second is of type `second`.
pub fn pairs(cputype: u32, first: &Relocation, second: &Relocation) -> bool {
    if cputype == CPU_TYPE_X86_64 {
        first.kind == x86_64::SUBTRACTOR && second.kind == x86_64::UNSIGNED
    } else if is_arm64(cputype) {
        (first.kind == arm64::SUBTRACTOR && second.kind == arm64::UNSIGNED)
            || (first.kind == arm64::ADDEND
                && matches!(
                    second.kind,
                    arm64::BRANCH26 | arm64::PAGE21 | arm64::PAGEOFF12
                ))
    } else {
        second.kind == generic::PAIR
    }
}

/// A section's relocations grouped into fixups: each range is one entry or
/// a pair ([`pairs`]).
pub fn fixups(cputype: u32, relocations: &[Relocation]) -> Vec<Range<usize>> {
    let mut out = Vec::with_capacity(relocations.len());
    let mut i = 0;
    while i < relocations.len() {
        let n = if relocations
            .get(i + 1)
            .is_some_and(|next| pairs(cputype, &relocations[i], next))
        {
            2
        } else {
            1
        };
        out.push(i..i + n);
        i += n;
    }
    out
}

/// Whether a relocation's `r_symbolnum` is a section's ordinal (a plain,
/// local entry, not `R_ABS`, and not an entry whose field holds something
/// else: arm64's `ADDEND`, the 32-bit targets' `PAIR`).
pub fn names_section(cputype: u32, r: &Relocation) -> bool {
    if r.value.is_some() || r.external || r.symbolnum == 0 {
        return false;
    }
    if is_arm64(cputype) {
        r.kind != arm64::ADDEND
    } else if cputype == CPU_TYPE_X86_64 {
        true
    } else {
        r.kind != generic::PAIR
    }
}

/// How a relocation's bytes change when sections move.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AddressRule {
    /// They don't: an addend, or nothing.
    None,
    /// By the target section's move times `sign`, less the relocation's
    /// own section's move if `pcrel`.
    Target { sign: i64, pcrel: bool },
    /// Less the relocation's own section's move (i386's PC-relative
    /// external entries).
    Pc,
    /// By the move of the section `r_value` is in (i386's scattered
    /// `VANILLA`).
    Value,
    /// By the move of the section `r_value` is in, less that of the one
    /// the next entry's (its `PAIR`'s) `r_value` is in (i386's
    /// `SECTDIFF` and `LOCAL_SECTDIFF`).
    Difference,
    /// An encoding this module doesn't know.
    Unknown,
}

/// How `r`'s bytes change when sections move (see the module's notes).
pub fn address_rule(cputype: u32, r: &Relocation) -> AddressRule {
    if cputype == CPU_TYPE_X86 {
        return match (r.value.is_some(), r.kind, r.pcrel, r.external) {
            (false, generic::VANILLA, pcrel, false) => AddressRule::Target { sign: 1, pcrel },
            (false, generic::VANILLA, true, true) => AddressRule::Pc,
            (false, generic::VANILLA, false, true) => AddressRule::None,
            (true, generic::VANILLA, false, _) => AddressRule::Value,
            (true, generic::SECTDIFF | generic::LOCAL_SECTDIFF, false, _) => {
                AddressRule::Difference
            }
            (true, generic::PAIR, ..) => AddressRule::None,
            _ => AddressRule::Unknown,
        };
    }
    if r.value.is_some() {
        return AddressRule::Unknown;
    }
    if cputype == CPU_TYPE_X86_64 {
        if r.external {
            return AddressRule::None;
        }
        match (r.kind, r.pcrel) {
            (x86_64::UNSIGNED, false) => AddressRule::Target {
                sign: 1,
                pcrel: false,
            },
            (x86_64::SUBTRACTOR, false) => AddressRule::Target {
                sign: -1,
                pcrel: false,
            },
            (
                x86_64::SIGNED
                | x86_64::SIGNED_1
                | x86_64::SIGNED_2
                | x86_64::SIGNED_4
                | x86_64::BRANCH,
                true,
            ) if r.length == 2 => AddressRule::Target {
                sign: 1,
                pcrel: true,
            },
            _ => AddressRule::Unknown,
        }
    } else if is_arm64(cputype) {
        if r.external || r.kind == arm64::ADDEND {
            return AddressRule::None;
        }
        match (r.kind, r.pcrel) {
            (arm64::UNSIGNED, false) => AddressRule::Target {
                sign: 1,
                pcrel: false,
            },
            (arm64::SUBTRACTOR, false) => AddressRule::Target {
                sign: -1,
                pcrel: false,
            },
            _ => AddressRule::Unknown,
        }
    } else if r.pcrel || r.kind != generic::VANILLA {
        AddressRule::Unknown
    } else if r.external {
        AddressRule::None
    } else {
        AddressRule::Target {
            sign: 1,
            pcrel: false,
        }
    }
}

/// The `1 << length`-byte value at `at`.
pub(crate) fn value_at(bytes: &[u8], at: u32, length: u8, endian: Endian) -> Option<u64> {
    let n = 1usize << length.min(3);
    let at = at as usize;
    let b = bytes.get(at..at.checked_add(n)?)?;
    let mut v = 0u64;
    let mut put = |x: u8| v = (v << 8) | u64::from(x);
    match endian {
        Endian::Little => b.iter().rev().for_each(|&x| put(x)),
        Endian::Big => b.iter().for_each(|&x| put(x)),
    }
    Some(v)
}

/// Adds `delta` to the `1 << length`-byte value at `at` (wrapping at its
/// width). Whether it is inside the bytes.
pub(crate) fn add_at(bytes: &mut [u8], at: u32, length: u8, delta: i64, endian: Endian) -> bool {
    let Some(v) = value_at(bytes, at, length, endian) else {
        return false;
    };
    let n = 1usize << length.min(3);
    let v = v.wrapping_add(delta as u64);
    let at = at as usize;
    let out = &mut bytes[at..at + n];
    for (k, b) in out.iter_mut().enumerate() {
        let shift = match endian {
            Endian::Little => 8 * k,
            Endian::Big => 8 * (n - 1 - k),
        };
        *b = (v >> shift) as u8;
    }
    true
}

/// The address local relocation `index` of `section` encodes in the
/// section's bytes (its target's address plus the addend), where the
/// encoding is plain: x86_64's `UNSIGNED`, `SIGNED*` and `BRANCH`, arm64's
/// `UNSIGNED`, and the 32-bit targets' `VANILLA` that isn't PC-relative
/// (a PC-relative one's is relative to the section's address). `None`
/// for any other entry, and for either half of a `SUBTRACTOR` pair (whose
/// bytes hold a difference).
pub fn local_address(
    cputype: u32,
    endian: Endian,
    section: &Section<'_>,
    index: usize,
) -> Option<u64> {
    let r = section.relocations.get(index)?;
    if r.value.is_some() || r.external {
        return None;
    }
    if let Some(prev) = index.checked_sub(1).map(|i| &section.relocations[i])
        && pairs(cputype, prev, r)
        && prev.kind == subtractor(cputype)
    {
        return None;
    }
    let bytes = section.bytes()?;
    let v = value_at(bytes, r.address, r.length, endian)?;
    let width = 8 << r.length.min(3);
    let signed = |v: u64| -> i64 {
        if width == 64 {
            v as i64
        } else {
            ((v << (64 - width)) as i64) >> (64 - width)
        }
    };
    let pc_bias = |kind: u8| -> Option<u64> {
        Some(match kind {
            x86_64::SIGNED | x86_64::BRANCH => 4,
            x86_64::SIGNED_1 => 5,
            x86_64::SIGNED_2 => 6,
            x86_64::SIGNED_4 => 8,
            _ => return None,
        })
    };
    match address_rule(cputype, r) {
        AddressRule::Target {
            sign: 1,
            pcrel: false,
        } => Some(v),
        AddressRule::Target {
            sign: 1,
            pcrel: true,
        } if cputype == CPU_TYPE_X86_64 => {
            let next = section
                .addr
                .wrapping_add(u64::from(r.address))
                .wrapping_add(pc_bias(r.kind)?);
            Some(next.wrapping_add(signed(v) as u64))
        }
        _ => None,
    }
}
