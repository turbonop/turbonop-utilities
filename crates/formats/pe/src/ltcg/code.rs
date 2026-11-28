//! `.cil$ex`: a header (0x58 bytes: 0x5B, 0x58, then zeros, in every
//! object seen), then each function's IL, a unit after another in the
//! order of the functions in the source. A function's record gives where
//! its unit starts ([`super::FunctionIl::il_offset`]); functions nothing
//! refers to (inline, or `static`) keep their units but get no record.
//!
//! The IL is a bytecode that is not decoded here. Every unit opens with
//! the same two operations (`4F 1F`, a 32-bit value, `4F 20`, 0xFE: the
//! function's options, it seems), which is how units are found; their
//! count is checked against `.cil$sy`'s streams, one per unit
//! ([`IlUnits::agrees_with`]). See docs/ltcg-il.md.

use alloc::vec::Vec;

use super::Locals;

/// Where each function's IL unit starts in `.cil$ex`.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct IlUnits {
    /// The units' starts, in order.
    pub starts: Vec<u32>,
    /// The section's size: the last unit's end.
    pub len: u32,
}

/// The header's size.
pub const IL_HEADER: usize = 0x58;

/// Whether a unit opens at `at`: `4F 1F 80 xx xx xx xx 4F 20 80 FE 00`.
fn opens(ex: &[u8], at: usize) -> bool {
    matches!(
        ex.get(at..at + 12),
        Some([0x4F, 0x1F, 0x80, _, _, _, _, 0x4F, 0x20, 0x80, 0xFE, 0x00])
    )
}

impl IlUnits {
    /// The units of `ex` (`.cil$ex`), found by their opening.
    pub fn find(ex: &[u8]) -> Self {
        let len = u32::try_from(ex.len()).unwrap_or(u32::MAX);
        let mut starts = Vec::new();
        let mut at = IL_HEADER;
        while at + 12 <= ex.len() {
            if ex[at] == 0x4F && opens(ex, at) {
                if let Ok(a) = u32::try_from(at) {
                    starts.push(a);
                }
                at += 12;
            } else {
                at += 1;
            }
        }
        Self { starts, len }
    }

    pub fn len(&self) -> usize {
        self.starts.len()
    }

    pub fn is_empty(&self) -> bool {
        self.starts.is_empty()
    }

    /// Unit `i`'s start and end.
    pub fn range(&self, i: usize) -> Option<(u32, u32)> {
        let start = *self.starts.get(i)?;
        let end = self.starts.get(i + 1).copied().unwrap_or(self.len);
        Some((start, end))
    }

    /// The unit that starts at `offset` (a function's IL offset).
    pub fn index_of(&self, offset: u32) -> Option<usize> {
        self.starts.binary_search(&offset).ok()
    }

    /// The size of the unit that starts at `offset`.
    pub fn size_at(&self, offset: u32) -> Option<u32> {
        let (s, e) = self.range(self.index_of(offset)?)?;
        Some(e - s)
    }

    /// Whether there is a `.cil$sy` stream for every unit (and so the
    /// units found are the units: in every object seen, they agree).
    pub fn agrees_with(&self, locals: &Locals<'_>) -> bool {
        locals.walked.is_ok() && locals.functions.len() == self.starts.len()
    }
}
