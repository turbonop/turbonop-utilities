//! Why bytes didn't decode.

use core::fmt;

/// Why the bytes at an address aren't an instruction.
///
/// An encoding the architecture calls UNPREDICTABLE, or one whose
/// should-be bits don't hold, decodes: what it is says so
/// ([`Attributes::UNPREDICTABLE`](crate::Attributes::UNPREDICTABLE)).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum DecodeError {
    /// The bytes end before the instruction does: at least `needed` bytes
    /// (from its start) are needed to decode it, or to know more.
    Truncated {
        /// How many bytes, at least.
        needed: u8,
    },
    /// No instruction is encoded so.
    Undefined,
    /// An instruction is encoded so, but not in this configuration: a
    /// feature (extension) is off, or the mode doesn't have it.
    Unsupported,
}

impl fmt::Display for DecodeError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            DecodeError::Truncated { needed } => {
                write!(f, "truncated instruction ({needed} bytes needed)")
            }
            DecodeError::Undefined => f.write_str("undefined instruction"),
            DecodeError::Unsupported => {
                f.write_str("instruction not supported in this configuration")
            }
        }
    }
}

impl core::error::Error for DecodeError {}
