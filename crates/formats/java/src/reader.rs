//! Bounds-checked big-endian reads (class files are big-endian).

use turbonop_core::{Error, ErrorKind, Result};

/// A cursor over bytes; every read is checked, and errors carry the offset
/// (from the cursor's start, plus `base`) and what was being read.
#[derive(Clone, Debug)]
pub(crate) struct Cursor<'d> {
    data: &'d [u8],
    pos: usize,
    base: u64,
}

impl<'d> Cursor<'d> {
    pub(crate) fn new(data: &'d [u8]) -> Self {
        Self {
            data,
            pos: 0,
            base: 0,
        }
    }

    /// A cursor whose errors report offsets from `base`.
    pub(crate) fn with_base(data: &'d [u8], base: u64) -> Self {
        Self { data, pos: 0, base }
    }

    pub(crate) fn remaining(&self) -> usize {
        self.data.len() - self.pos
    }

    pub(crate) fn is_empty(&self) -> bool {
        self.pos >= self.data.len()
    }

    /// The offset errors report for the cursor's position.
    pub(crate) fn offset(&self) -> u64 {
        self.base + self.pos as u64
    }

    pub(crate) fn error(&self, kind: ErrorKind, what: &'static str) -> Error {
        Error::new(kind, self.offset(), what)
    }

    pub(crate) fn bytes(&mut self, n: usize, what: &'static str) -> Result<&'d [u8]> {
        let end = self
            .pos
            .checked_add(n)
            .filter(|&e| e <= self.data.len())
            .ok_or_else(|| self.error(ErrorKind::Truncated, what))?;
        let b = &self.data[self.pos..end];
        self.pos = end;
        Ok(b)
    }

    fn array<const N: usize>(&mut self, what: &'static str) -> Result<[u8; N]> {
        let b = self.bytes(N, what)?;
        let mut a = [0; N];
        a.copy_from_slice(b);
        Ok(a)
    }

    pub(crate) fn u8(&mut self, what: &'static str) -> Result<u8> {
        Ok(self.array::<1>(what)?[0])
    }

    pub(crate) fn u16(&mut self, what: &'static str) -> Result<u16> {
        Ok(u16::from_be_bytes(self.array(what)?))
    }

    pub(crate) fn u32(&mut self, what: &'static str) -> Result<u32> {
        Ok(u32::from_be_bytes(self.array(what)?))
    }

    pub(crate) fn i32(&mut self, what: &'static str) -> Result<i32> {
        Ok(i32::from_be_bytes(self.array(what)?))
    }

    pub(crate) fn u64(&mut self, what: &'static str) -> Result<u64> {
        Ok(u64::from_be_bytes(self.array(what)?))
    }

    /// A count of items each at least `min_size` bytes, which must fit in
    /// what's left (so that a damaged count can't allocate without bound).
    pub(crate) fn count16(&mut self, min_size: usize, what: &'static str) -> Result<usize> {
        let n = usize::from(self.u16(what)?);
        self.check_count(n, min_size, what)
    }

    pub(crate) fn check_count(
        &self,
        n: usize,
        min_size: usize,
        what: &'static str,
    ) -> Result<usize> {
        if n.checked_mul(min_size)
            .is_none_or(|need| need > self.remaining())
        {
            return Err(self.error(ErrorKind::Truncated, what));
        }
        Ok(n)
    }

    /// A table of `u16`s preceded by its `u16` count.
    pub(crate) fn u16_table(&mut self, what: &'static str) -> Result<Vec<u16>> {
        let n = self.count16(2, what)?;
        (0..n).map(|_| self.u16(what)).collect()
    }

    /// Fails unless every byte has been read.
    pub(crate) fn finish(&self, what: &'static str) -> Result<()> {
        if self.is_empty() {
            Ok(())
        } else {
            Err(self.error(ErrorKind::Malformed, what))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_are_checked() {
        let mut c = Cursor::with_base(&[0x12, 0x34, 0xFF, 0xFE, 1], 0x10);
        assert_eq!(c.u16("a").unwrap(), 0x1234);
        assert_eq!(c.u16("b").unwrap(), 0xFFFE);
        let e = c.u16("c").unwrap_err();
        assert_eq!(
            (e.kind, e.offset, e.what),
            (ErrorKind::Truncated, 0x14, "c")
        );
        assert!(c.finish("x").is_err());
        assert_eq!(c.u8("d").unwrap(), 1);
        c.finish("x").unwrap();
        let mut c = Cursor::new(&[0, 3, 0, 1, 0, 2]);
        assert!(c.count16(2, "n").is_err());
        let mut c = Cursor::new(&[0, 2, 0, 1, 0, 2]);
        assert_eq!(c.u16_table("t").unwrap(), [1, 2]);
        assert!(Cursor::new(&[1]).bytes(usize::MAX, "x").is_err());
        let mut c = Cursor::new(&[0; 12]);
        assert_eq!(c.u64("q").unwrap(), 0);
        assert_eq!(c.i32("i").unwrap(), 0);
        assert!(c.u32("u").is_err());
    }
}
