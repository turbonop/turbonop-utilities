//! AArch64 (ARM64) instruction decoder and formatter.
//!
//! The decode tables are generated from Arm's Machine Readable
//! Specification (AARCHMRS) and LLVM's AArch64 target (see
//! `tables/a64.txt` and `tools/arm64/extract.py`): every encoding with its
//! fixed bits, constraints, UNDEFINED conditions, alias preferences, and
//! assembler template. Output follows the Microsoft
//! disassembler's syntax (`dumpbin /disasm`).
//!
//! Each instruction also has its operands, typed
//! ([`Instruction::detailed_operands`]), what it reads and writes
//! ([`Instruction::register_accesses`], [`Instruction::memory_accesses`]),
//! and how control leaves it ([`Instruction::flow`]): from LLVM's records
//! for each encoding (`tables/a64-access.txt`, from
//! `tools/arm64/access.py`), checked against llvm-mc. [`Arm64`] is the
//! decoder as [`turbonop_isa`]'s traits see it.

#![no_std]
#![allow(clippy::cast_possible_truncation, clippy::cast_possible_wrap)]
#![warn(missing_docs)]

extern crate alloc;

mod access;
mod alias;
mod format;
mod operand;
mod tables;

pub use access::{Element, Register, RegisterSet};
pub use format::{format, format_into, format_with};
pub use operand::{
    Access, AddressingMode, Arrangement, Condition, MAX_OPERANDS, MemoryOperand, Operand,
    OperandKind, Operands, Predication, RegisterList, RegisterOperand, ShiftKind, SystemRegister,
    ZaSlice,
};
pub use turbonop_isa::{Reference, Resolver};

pub use isa::{Arm64, Syntax};
pub use tables::opcodes;
pub use turbonop_isa::DecodeError;

mod isa;

/// Which instruction an instruction is: its encoding in Arm's
/// specification (an encoding in parts is one opcode). An alias is its
/// own encoding: `mov x0, x1` is [`opcodes::MOV_ORR_64_LOG_SHIFT`]. Each
/// has a constant in [`opcodes`], named for it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Opcode(pub(crate) u16);

impl Opcode {
    /// The encoding's name in Arm's specification (`ADD_64_addsub_imm`).
    pub fn name(self) -> &'static str {
        encoding(usize::from(tables::OPCODE_FIRST[usize::from(self.0)])).name()
    }

    /// Its mnemonic, as the architecture writes it (`add`; `b` for
    /// `b.<cond>`).
    pub fn mnemonic(self) -> &'static str {
        tables::OPCODE_MNEMONICS[usize::from(self.0)]
    }

    /// Every opcode, in order.
    pub fn all() -> impl Iterator<Item = Opcode> {
        (0..tables::OPCODE_FIRST.len() as u16).map(Opcode)
    }

    pub(crate) fn attrs(self) -> u8 {
        tables::OPCODE_ATTRS[usize::from(self.0)]
    }
}

use tables::{BUCKETS, Encoding, NONE, encoding, ms};

/// A decoded instruction: its word (as its four bytes, little-endian, as
/// A64 code is whatever the data's byte order) and its encoding. Where it
/// is isn't part of it: what depends on that (a branch's target, an
/// `adrp`'s page) is asked with the address.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Instruction {
    bytes: [u8; 4],
    encoding: u16,
}

const _: () = assert!(size_of::<Instruction>() == 6);

/// An instruction at an address: what formatting and operands work on.
#[derive(Clone, Copy)]
pub(crate) struct Inst {
    pub(crate) inst: Instruction,
    pub(crate) word: u32,
    pub(crate) address: u64,
}

impl core::ops::Deref for Inst {
    type Target = Instruction;

    #[inline]
    fn deref(&self) -> &Instruction {
        &self.inst
    }
}

impl Instruction {
    /// The instruction's word.
    #[inline]
    pub fn word(&self) -> u32 {
        u32::from_le_bytes(self.bytes)
    }

    /// Its bytes, as encoded.
    #[inline]
    pub fn bytes(&self) -> &[u8; 4] {
        &self.bytes
    }

    /// Which instruction it is.
    #[inline]
    pub fn opcode(&self) -> Opcode {
        Opcode(tables::OPCODE_OF[usize::from(self.encoding)])
    }

    #[inline]
    pub(crate) fn at(self, address: u64) -> Inst {
        Inst {
            inst: self,
            word: self.word(),
            address,
        }
    }

    /// The encoding's name in ARM's ISA description, such as
    /// `ADD_64_addsub_imm`.
    #[inline]
    pub fn encoding_name(&self) -> &'static str {
        self.encoding().name()
    }

    /// The instruction class: `general`, `float`, `advsimd`, `sve`, ...
    #[inline]
    pub fn class(&self) -> &'static str {
        tables::CLASSES[usize::from(self.encoding().class)]
    }

    /// The architecture feature the instruction needs, such as `FEAT_SVE2`
    /// (empty for the base instruction set).
    #[inline]
    pub fn feature(&self) -> &'static str {
        tables::FEATURES[usize::from(self.encoding().feature)]
    }

    /// Whether the encoding is CONSTRAINED UNPREDICTABLE (such as a
    /// load pair with writeback into one of its registers). Microsoft
    /// marks these with a `?` before the mnemonic.
    /// So are words whose should-be bits (Arm's `(0)` and `(1)`) differ.
    pub fn is_unpredictable(&self) -> bool {
        let e = self.encoding();
        let w = self.word();
        let unpredictable = |e: &Encoding| e.unpred(w) || !should_hold(e, w);
        unpredictable(e) || (e.alias_of != NONE && unpredictable(encoding(usize::from(e.alias_of))))
    }

    /// Whether the word's should-be bits (Arm's `(0)` and `(1)`) are as
    /// the encoding has them. When they aren't, the instruction is
    /// CONSTRAINED UNPREDICTABLE: decoded (and marked), not rejected.
    pub fn should_bits_hold(&self) -> bool {
        let e = self.encoding();
        let w = self.word();
        should_hold(e, w)
            && (e.alias_of == NONE || should_hold(encoding(usize::from(e.alias_of)), w))
    }

    fn encoding(&self) -> &'static Encoding {
        encoding(usize::from(self.encoding))
    }
}

/// Whether a word's should-be bits are as the encoding would have them.
fn should_hold(e: &Encoding, w: u32) -> bool {
    w & e.should.0 == e.should.1
}

fn matches(e: &Encoding, w: u32) -> bool {
    w & e.mask == e.value && e.ne().iter().all(|&(m, v)| w & m != v) && !e.undef(w)
}

/// Decodes the instruction `word`.
///
/// # Errors
///
/// [`DecodeError::Undefined`]: it isn't an allocated, defined encoding.
pub fn decode(word: u32) -> Result<Instruction, DecodeError> {
    let candidates = BUCKETS[(word >> 21) as usize];
    // The base encoding: the most specific match that is not an alias,
    // among those whose should-be bits hold, else (CONSTRAINED
    // UNPREDICTABLE) among those whose don't.
    let best = |exact: bool| {
        let mut base: Option<u16> = None;
        for &i in candidates {
            let e = encoding(usize::from(i));
            if e.alias_of != NONE || !matches(e, word) || should_hold(e, word) != exact {
                continue;
            }
            let better = base.is_none_or(|b| {
                let b = encoding(usize::from(b));
                e.mask.count_ones() + e.should.0.count_ones()
                    > b.mask.count_ones() + b.should.0.count_ones()
            });
            if better {
                base = Some(i);
            }
        }
        base
    };
    let base = best(true)
        .or_else(|| best(false))
        .ok_or(DecodeError::Undefined)?;
    // A preferred alias of it, if any.
    let mut chosen = base;
    for &i in candidates {
        let e = encoding(usize::from(i));
        // (An encoding can be in parts, one per operand layout, all with its
        // name: an alias is of any of them.)
        let of_base = e.alias_of != NONE
            && (e.alias_of == base
                || encoding(usize::from(e.alias_of)).group == encoding(usize::from(base)).group);
        if !of_base || e.ms == ms::NOT_PREFERRED {
            continue;
        }
        // Where Microsoft has its own preference, the alias's "not equal"
        // constraints (such as "cond != 111x") do not apply either.
        let (rn, rm) = ((word >> 5) & 0x1F, (word >> 16) & 0x1F);
        let ms_prefers = match e.ms {
            ms::CINC => Some(rn == rm && rn != 31),
            ms::CNEG => Some(rn == rm),
            ms::CSET => Some(rn == 31 && rm == 31),
            _ => None,
        };
        let preferred = match ms_prefers {
            Some(p) => p && word & e.mask == e.value,
            None => w_matches_alias(e, word) && e.prefer(word),
        };
        if preferred {
            chosen = i;
            break;
        }
    }
    let inst = Instruction {
        bytes: word.to_le_bytes(),
        encoding: chosen,
    };
    if format::is_valid(&inst.at(0)) {
        return Ok(inst);
    }
    // An alias whose operands do not render (such as a TLBI operation
    // with no name) falls back to the instruction it stands for.
    let inst = Instruction {
        encoding: base,
        ..inst
    };
    // Reserved operand values (such as an arrangement marked RESERVED)
    // make the encoding invalid.
    if format::is_valid(&inst.at(0)) {
        Ok(inst)
    } else {
        Err(DecodeError::Undefined)
    }
}

fn w_matches_alias(e: &Encoding, w: u32) -> bool {
    w & e.mask == e.value && e.ne().iter().all(|&(m, v)| w & m != v)
}

/// For tools: every encoding whose fixed bits match `word`, whether its
/// constraints hold, and how it renders the word.
#[doc(hidden)]
pub fn explain(word: u32) -> alloc::string::String {
    let mut out = alloc::string::String::new();
    for &i in BUCKETS[(word >> 21) as usize] {
        let e = encoding(usize::from(i));
        if word & e.mask != e.value {
            continue;
        }
        out.push_str(if matches(e, word) {
            "match"
        } else {
            "rejected (ne/undef)"
        });
        out.push_str(if e.alias_of == NONE { "" } else { " alias" });
        out.push('\n');
        let inst = Instruction {
            bytes: word.to_le_bytes(),
            encoding: i,
        };
        out.push_str(&format::explain_encoding(&inst.at(0)));
    }
    out
}

/// For tests: the first encoding matching `word` whose quick validity
/// check (what [`decode`] uses) disagrees with rendering it in full, if
/// any.
#[doc(hidden)]
pub fn validity_disagrees(word: u32) -> Option<&'static str> {
    for &i in BUCKETS[(word >> 21) as usize] {
        let e = encoding(usize::from(i));
        if word & e.mask != e.value {
            continue;
        }
        let inst = Instruction {
            bytes: word.to_le_bytes(),
            encoding: i,
        }
        .at(0);
        if format::is_valid(&inst) != format::renders_fully(&inst) {
            return Some(e.name());
        }
    }
    None
}

/// Every encoding in the tables: its name and fixed bits (`word & mask ==
/// value`). For generating test cases.
pub fn encodings() -> impl Iterator<Item = (&'static str, u32, u32)> {
    tables::encodings().map(|e| (e.name(), e.mask, e.value))
}
