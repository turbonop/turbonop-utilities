//! 32-bit ARM's exception index (Arm's "Exception Handling ABI for the
//! Arm Architecture"): `.ARM.exidx` entries, each a function and how to
//! unwind it, inline or in an `.ARM.extab` table entry (whose words a
//! caller reads). Unwinding by the opcodes is `turbonop-unwind`'s.

use alloc::vec::Vec;

use turbonop_core::Endian;

/// How an index entry says to unwind.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Model {
    /// `EXIDX_CANTUNWIND`: the function can't be unwound through.
    CantUnwind,
    /// Arm's compact model, with personality routine 0, 1, or 2
    /// (`__aeabi_unwind_cpp_pr0`..`pr2`).
    Compact { personality: u8, inline: bool },
    /// A personality routine of its own (its address), with opcodes in
    /// the GNU layout after it.
    Generic { personality: u64 },
}

/// An index entry.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Entry {
    pub function: u64,
    /// The `.ARM.extab` entry's address, if not inline.
    pub table: Option<u64>,
    pub model: Model,
    /// The unwind opcodes (with any trailing `finish` padding).
    pub opcodes: Vec<u8>,
}

impl Entry {
    /// The bytes of its table entry that are its unwind information: a
    /// generic model's personality word, and its opcodes' words (what
    /// follows them is the personality routine's data: GCC's LSDA). None
    /// for an inline entry.
    pub fn table_unwind_bytes(&self) -> u64 {
        if self.table.is_none() {
            return 0;
        }
        let words = self.opcodes.len().div_ceil(4) as u64 * 4;
        match self.model {
            Model::Generic { .. } => 4 + words,
            _ => words,
        }
    }
}

/// An `.ARM.exidx` section.
#[derive(Clone, Copy, Debug)]
pub struct Exidx<'d> {
    pub data: &'d [u8],
    /// The section's address.
    pub address: u64,
    pub endian: Endian,
}

/// The most extra words a table entry's opcodes may take.
const MAX_WORDS: u64 = 255;

impl Exidx<'_> {
    fn word(&self, at: usize) -> Option<u32> {
        let b: [u8; 4] = self.data.get(at..at.checked_add(4)?)?.try_into().ok()?;
        Some(match self.endian {
            Endian::Little => u32::from_le_bytes(b),
            Endian::Big => u32::from_be_bytes(b),
        })
    }

    pub fn len(&self) -> usize {
        self.data.len() / 8
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Entry `i`; `extab` reads a table entry's word (by address, in the
    /// image's byte order).
    pub fn entry(&self, i: usize, extab: &dyn Fn(u64) -> Option<u32>) -> Option<Entry> {
        let at = i.checked_mul(8)?;
        let w0 = self.word(at)?;
        let w1 = self.word(at + 4)?;
        let here = self.address.wrapping_add(at as u64);
        let function = prel31(w0, here);
        if w1 == 1 {
            return Some(Entry {
                function,
                table: None,
                model: Model::CantUnwind,
                opcodes: Vec::new(),
            });
        }
        if w1 & 0x8000_0000 != 0 {
            let personality = ((w1 >> 24) & 0xF) as u8;
            return Some(Entry {
                function,
                table: None,
                model: Model::Compact {
                    personality,
                    inline: true,
                },
                opcodes: bytes_of(w1, 3).collect(),
            });
        }
        let table = prel31(w1, here.wrapping_add(4));
        let t0 = extab(table)?;
        let mut opcodes = Vec::new();
        let (model, first, extra_at) = if t0 & 0x8000_0000 != 0 {
            let personality = ((t0 >> 24) & 0xF) as u8;
            let model = Model::Compact {
                personality,
                inline: false,
            };
            if personality == 0 {
                opcodes.extend(bytes_of(t0, 3));
                (model, 0, 0)
            } else {
                opcodes.extend(bytes_of(t0, 2));
                (model, (t0 >> 16) & 0xFF, table.wrapping_add(4))
            }
        } else {
            // A personality routine; then the GNU layout: a word count and
            // three opcode bytes, then the words.
            let personality = prel31(t0, table);
            let t1 = extab(table.wrapping_add(4))?;
            opcodes.extend(bytes_of(t1, 3));
            (
                Model::Generic { personality },
                t1 >> 24,
                table.wrapping_add(8),
            )
        };
        if u64::from(first) > MAX_WORDS {
            return None;
        }
        for k in 0..u64::from(first) {
            opcodes.extend(bytes_of(extab(extra_at.wrapping_add(4 * k))?, 4));
        }
        Some(Entry {
            function,
            table: Some(table),
            model,
            opcodes,
        })
    }

    /// The index of the entry covering `pc` (the last starting at or
    /// before it; entries are sorted by function).
    pub fn lookup(&self, pc: u64) -> Option<usize> {
        let (mut lo, mut hi) = (0usize, self.len());
        while lo < hi {
            let mid = lo + (hi - lo) / 2;
            let w0 = self.word(mid * 8)?;
            if prel31(w0, self.address.wrapping_add(mid as u64 * 8)) <= pc {
                lo = mid + 1;
            } else {
                hi = mid;
            }
        }
        lo.checked_sub(1)
    }
}

/// A 31-bit place-relative offset's target.
fn prel31(word: u32, place: u64) -> u64 {
    let offset = i64::from(((word << 1) as i32) >> 1);
    place.wrapping_add(offset as u64) & 0xFFFF_FFFF
}

/// A word's low `n` bytes, most significant first.
fn bytes_of(word: u32, n: usize) -> impl Iterator<Item = u8> {
    let b = word.to_be_bytes();
    (4 - n.min(4)..4).map(move |i| b[i])
}
