//! AArch32 instruction decoding: A32 (ARM) and T32 (Thumb, 16- and 32-bit)
//! encodings, from Arm's Machine Readable Specification (AARCHMRS; see
//! `tables/aarch32.txt` and `tools/arm32/extract.py`).
//!
//! Decoding identifies each word's encoding (its name in Arm's
//! specification, its features, its assembler template, its operands'
//! fields) and whether it decodes at all; [`format`] writes it as text, in
//! the syntax of LLVM's disassembler. Neither panics.

#![no_std]

#[cfg(test)]
use turbonop_global_alloc as _;

extern crate alloc;

/// An encoding of the instruction set.
///
/// (Compact: there are thousands. Its name, template, feature, aliases,
/// operands, fields, and condition are indexes into tables they share,
/// read by its methods.)
#[derive(Debug)]
pub struct Encoding {
    name_at: u32,
    name_len: u8,
    mask: u32,
    value: u32,
    sb_mask: u32,
    sb_value: u32,
    /// An index into `CONDS`, or `NONE` (no condition: it holds).
    cond: u16,
    feature: u16,
    asm_at: u32,
    asm_len: u16,
    aliases: u16,
    operands: u16,
    fields: u16,
}

/// No condition.
const NONE: u16 = u16::MAX;

/// An operand rule a template names (`%{Rd__4}`): its display (`<Rd>`),
/// and its text (`R%{UInteger}`) or, for a choice, its alternatives,
/// each named for the field values it is for (`size_01_16`).
#[derive(Debug)]
pub struct Rule {
    pub id: &'static str,
    pub display: &'static str,
    pub text: Option<&'static str>,
    pub alts: &'static [(&'static str, &'static str)],
}

/// The rule `id`.
#[inline(never)]
pub fn rule(id: &str) -> Option<&'static Rule> {
    // (By tools/isa-gen's index of the rules by a hash of their IDs.)
    let slots = tables::RULE_INDEX.len();
    let mut s = rule_hash(id) as usize & (slots - 1);
    for _ in 0..slots {
        let r = tables::RULES.get(usize::from(tables::RULE_INDEX[s]))?;
        if r.id == id {
            return Some(r);
        }
        s = (s + 1) & (slots - 1);
    }
    None
}

/// The rule at `index` (by ID), which a template's reference names.
#[inline]
pub(crate) fn rule_at(index: usize) -> Option<&'static Rule> {
    tables::RULES.get(index)
}

/// The hash rule IDs are indexed by (FNV-1a; tools/isa-gen's `rule_hash`).
fn rule_hash(id: &str) -> u32 {
    id.bytes().fold(0x811c_9dc5u32, |h, b| {
        (h ^ u32::from(b)).wrapping_mul(0x0100_0193)
    })
}

impl Encoding {
    fn matches(&self, w: u32) -> bool {
        w & self.mask == self.value
            && (self.cond == NONE
                || tables::CONDS
                    .get(usize::from(self.cond))
                    .is_some_and(|c| c(w)))
    }

    /// Its name in Arm's specification, such as `ADD_i_T3`.
    #[inline]
    pub fn name(&self) -> &'static str {
        text(self.name_at, usize::from(self.name_len))
    }

    /// Its assembler template (Arm's syntax), each operand a reference to
    /// its rule: one character, U+0100 + the rule's index ([`template`]
    /// writes them as `%{rule}`).
    #[inline]
    pub fn asm(&self) -> &'static str {
        text(self.asm_at, usize::from(self.asm_len))
    }

    /// Its assembler template as Arm writes it, operands as `%{rule}`.
    pub fn template(&self) -> alloc::string::String {
        let mut out = alloc::string::String::new();
        for c in self.asm().chars() {
            match u32::from(c).checked_sub(0x100) {
                Some(i) => {
                    out.push_str("%{");
                    out.push_str(rule_at(i as usize).map_or("", |r| r.id));
                    out.push('}');
                }
                None => out.push(c),
            }
        }
        out
    }

    /// The architecture features it needs (`FEAT_FP16`), if any.
    #[inline]
    pub fn feature(&self) -> &'static str {
        tables::FEATURES
            .get(usize::from(self.feature))
            .copied()
            .unwrap_or("")
    }

    /// Its aliases' templates.
    #[inline]
    pub fn aliases(&self) -> &'static [&'static str] {
        tables::ALIASES
            .get(usize::from(self.aliases))
            .copied()
            .unwrap_or(&[])
    }

    /// Its operands (by their display, `<Rd>`), and the bits (hi, lo) of
    /// the fields each is encoded in, most significant first.
    #[inline]
    pub fn operands(&self) -> &'static [(&'static str, &'static [(u8, u8)])] {
        tables::OPERANDS
            .get(usize::from(self.operands))
            .copied()
            .unwrap_or(&[])
    }

    /// Its fields: name, hi, lo.
    #[inline]
    pub fn fields(&self) -> &'static [(&'static str, u8, u8)] {
        tables::FIELDS
            .get(usize::from(self.fields))
            .copied()
            .unwrap_or(&[])
    }

    /// The mnemonic, lowercased, without its optional parts (`add` for
    /// `ADD{<c>}{<q>}`).
    pub fn mnemonic(&self) -> &'static str {
        mnemonic_of(self.asm())
    }
}

/// `len` bytes of the encodings' text from `at`.
#[inline]
fn text(at: u32, len: usize) -> &'static str {
    let at = at as usize;
    tables::TEXT.get(at..at + len).unwrap_or("")
}

/// A template's mnemonic: its leading letters and digits.
pub fn mnemonic_of(template: &'static str) -> &'static str {
    let end = template
        .find(|c: char| !c.is_ascii_alphanumeric())
        .unwrap_or(template.len());
    &template[..end]
}

pub mod format;

mod tables {
    // (Generated: conditions as the table writes them.)
    #![allow(unused_parens, clippy::all)]
    use super::{Encoding, NONE, Rule};
    include!("generated/tables.rs");
}

/// A decoded instruction.
#[derive(Clone, Copy, Debug)]
pub struct Instruction {
    /// The word, or for a 16-bit Thumb instruction, its halfword (and for
    /// a 32-bit one, the first halfword in the top half).
    pub word: u32,
    /// 2 or 4.
    pub size: u8,
    /// T32 (Thumb), not A32.
    pub thumb: bool,
    pub encoding: &'static Encoding,
}

impl Instruction {
    /// Whether its "should be" bits are other than they should be
    /// (CONSTRAINED UNPREDICTABLE).
    pub fn breaks_should_be(&self) -> bool {
        self.word & self.encoding.sb_mask != self.encoding.sb_value
    }

    /// Its encoding's field `name` (`Rd`, `imm8`, ...), if it has one.
    pub fn field(&self, name: &str) -> Option<u32> {
        let &(_, hi, lo) = self.encoding.fields().iter().find(|f| f.0 == name)?;
        let width = u32::from(hi - lo) + 1;
        let v = self.word >> lo;
        Some(if width >= 32 {
            v
        } else {
            v & ((1 << width) - 1)
        })
    }
}

fn find(
    table: &'static [Encoding],
    (buckets, starts): (&'static [u16], &'static [u16]),
    key: (u32, u32),
    w: u32,
) -> Option<&'static Encoding> {
    let k = ((w >> key.0) & key.1) as usize;
    let start = usize::from(*starts.get(k)?);
    let end = usize::from(*starts.get(k + 1)?);
    buckets
        .get(start..end)?
        .iter()
        .map(|&i| &table[usize::from(i)])
        .find(|e| e.matches(w))
}

/// Decodes an A32 (ARM) word.
pub fn decode_a32(word: u32) -> Option<Instruction> {
    let encoding = find(
        &tables::A32,
        (&tables::A32_BUCKETS, &tables::A32_STARTS),
        tables::A32_KEY,
        word,
    )?;
    Some(Instruction {
        word,
        size: 4,
        thumb: false,
        encoding,
    })
}

/// Whether a T32 instruction starting with this halfword is 32 bits long.
pub fn is_t32_wide(first: u16) -> bool {
    first >> 11 >= 0b11101
}

/// Decodes a T32 (Thumb) instruction from its first halfword and, for a
/// 32-bit one, its second (`None` if it is missing).
pub fn decode_t32(first: u16, second: Option<u16>) -> Option<Instruction> {
    if is_t32_wide(first) {
        let word = u32::from(first) << 16 | u32::from(second?);
        let encoding = find(
            &tables::T32,
            (&tables::T32_BUCKETS, &tables::T32_STARTS),
            tables::T32_KEY,
            word,
        )?;
        Some(Instruction {
            word,
            size: 4,
            thumb: true,
            encoding,
        })
    } else {
        let word = u32::from(first);
        let encoding = find(
            &tables::T16,
            (&tables::T16_BUCKETS, &tables::T16_STARTS),
            tables::T16_KEY,
            word,
        )?;
        Some(Instruction {
            word,
            size: 2,
            thumb: true,
            encoding,
        })
    }
}

/// The number of encodings in each table (A32, T16, T32).
pub fn counts() -> (usize, usize, usize) {
    (tables::A32.len(), tables::T16.len(), tables::T32.len())
}

#[cfg(test)]
mod tests;
