//! Bytes read by range, as they are asked for: a file that isn't mapped
//! into memory (WebAssembly has no mapping), whose parts a parser reads
//! one at a time rather than the file whole.

/// A source of bytes read by range.
pub trait Source: Sync {
    /// How many bytes it has.
    fn len(&self) -> u64;

    /// Whether it has none.
    fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Fills `buf` with the bytes from `offset`; false if it hasn't them
    /// all (past its end, or unreadable).
    fn fill(&self, offset: u64, buf: &mut [u8]) -> bool;
}

/// Bytes in memory, as a source (for tests, and for what is given either).
impl Source for &[u8] {
    fn len(&self) -> u64 {
        <[u8]>::len(self) as u64
    }

    fn fill(&self, offset: u64, buf: &mut [u8]) -> bool {
        let Ok(start) = usize::try_from(offset) else {
            return false;
        };
        match start
            .checked_add(buf.len())
            .and_then(|end| self.get(start..end))
        {
            Some(src) => {
                buf.copy_from_slice(src);
                true
            }
            None => false,
        }
    }
}

impl Source for alloc::vec::Vec<u8> {
    fn len(&self) -> u64 {
        self.as_slice().len() as u64
    }

    fn fill(&self, offset: u64, buf: &mut [u8]) -> bool {
        self.as_slice().fill(offset, buf)
    }
}
