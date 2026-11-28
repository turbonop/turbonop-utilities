//! Bounds-checked little-endian reads, and DEX's LEB128 integers
//! (`uleb128`, `sleb128`, `uleb128p1`: at most five bytes, 32 bits).

use turbonop_core::{Error, ErrorKind, Result};

/// An unsigned LEB128 of at most five bytes, and how many bytes it took;
/// `None` if the data ends first. (As ART reads them, a fifth byte's
/// high bits are dropped.)
pub fn decode_uleb128(data: &[u8]) -> Option<(u32, usize)> {
    let mut v = 0u32;
    for i in 0..5 {
        let b = *data.get(i)?;
        v |= u32::from(b & 0x7F) << (7 * i);
        if b & 0x80 == 0 {
            return Some((v, i + 1));
        }
    }
    // A fifth byte with its continuation bit set: ART stops there too.
    Some((v, 5))
}

/// A signed LEB128 of at most five bytes, and how many bytes it took.
pub fn decode_sleb128(data: &[u8]) -> Option<(i32, usize)> {
    let mut v = 0u32;
    for i in 0..5 {
        let b = *data.get(i)?;
        v |= u32::from(b & 0x7F) << (7 * i);
        if b & 0x80 == 0 || i == 4 {
            let bits = 7 * (i + 1);
            #[allow(clippy::cast_possible_wrap)]
            let v = if bits < 32 {
                ((v << (32 - bits)) as i32) >> (32 - bits)
            } else {
                v as i32
            };
            return Some((v, i + 1));
        }
    }
    None
}

/// The unsigned LEB128 encoding of `v`.
pub fn encode_uleb128(mut v: u32, out: &mut Vec<u8>) {
    loop {
        #[allow(clippy::cast_possible_truncation)]
        let b = (v & 0x7F) as u8;
        v >>= 7;
        if v == 0 {
            out.push(b);
            return;
        }
        out.push(b | 0x80);
    }
}

/// The signed LEB128 encoding of `v`.
pub fn encode_sleb128(mut v: i32, out: &mut Vec<u8>) {
    loop {
        #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
        let b = (v & 0x7F) as u8;
        v >>= 7;
        if (v == 0 && b & 0x40 == 0) || (v == -1 && b & 0x40 != 0) {
            out.push(b);
            return;
        }
        out.push(b | 0x80);
    }
}

/// A cursor over the file's bytes; every read is checked, and errors carry
/// the offset (in the file: cursors index the whole file) and what was
/// being read.
#[derive(Clone, Debug)]
pub(crate) struct Cursor<'d> {
    data: &'d [u8],
    pos: usize,
}

impl<'d> Cursor<'d> {
    /// A cursor at `pos` (which may be past the end: reads then fail).
    pub(crate) fn at(data: &'d [u8], pos: usize) -> Self {
        Self { data, pos }
    }

    pub(crate) fn pos(&self) -> usize {
        self.pos
    }

    pub(crate) fn remaining(&self) -> usize {
        self.data.len().saturating_sub(self.pos)
    }

    pub(crate) fn error(&self, kind: ErrorKind, what: &'static str) -> Error {
        Error::new(kind, self.pos as u64, what)
    }

    pub(crate) fn skip(&mut self, n: usize, what: &'static str) -> Result<()> {
        self.bytes(n, what).map(|_| ())
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

    pub(crate) fn array<const N: usize>(&mut self, what: &'static str) -> Result<[u8; N]> {
        let b = self.bytes(N, what)?;
        let mut a = [0; N];
        a.copy_from_slice(b);
        Ok(a)
    }

    pub(crate) fn u8(&mut self, what: &'static str) -> Result<u8> {
        Ok(self.array::<1>(what)?[0])
    }

    pub(crate) fn u16(&mut self, what: &'static str) -> Result<u16> {
        Ok(u16::from_le_bytes(self.array(what)?))
    }

    pub(crate) fn u32(&mut self, what: &'static str) -> Result<u32> {
        Ok(u32::from_le_bytes(self.array(what)?))
    }

    pub(crate) fn uleb128(&mut self, what: &'static str) -> Result<u32> {
        let rest = self.data.get(self.pos..).unwrap_or_default();
        let (v, n) = decode_uleb128(rest).ok_or_else(|| self.error(ErrorKind::Truncated, what))?;
        self.pos += n;
        Ok(v)
    }

    pub(crate) fn sleb128(&mut self, what: &'static str) -> Result<i32> {
        let rest = self.data.get(self.pos..).unwrap_or_default();
        let (v, n) = decode_sleb128(rest).ok_or_else(|| self.error(ErrorKind::Truncated, what))?;
        self.pos += n;
        Ok(v)
    }

    /// A `uleb128p1`: the value plus one, so that -1 (`NO_INDEX`) takes one
    /// byte. `None` is -1.
    pub(crate) fn uleb128p1(&mut self, what: &'static str) -> Result<Option<u32>> {
        Ok(self.uleb128(what)?.checked_sub(1))
    }

    /// Checks that `n` items of at least `min_size` bytes each fit in what's
    /// left (so that a damaged count can't allocate without bound).
    pub(crate) fn check_count(&self, n: u32, min_size: usize, what: &'static str) -> Result<usize> {
        let n = n as usize;
        if n.checked_mul(min_size)
            .is_none_or(|need| need > self.remaining())
        {
            return Err(self.error(ErrorKind::Truncated, what));
        }
        Ok(n)
    }
}

/// A little-endian `u16` at `pos`, if it's there.
pub(crate) fn u16_at(data: &[u8], pos: usize) -> Option<u16> {
    let b = data.get(pos..pos.checked_add(2)?)?;
    Some(u16::from_le_bytes([b[0], b[1]]))
}

/// A little-endian `u32` at `pos`, if it's there.
pub(crate) fn u32_at(data: &[u8], pos: usize) -> Option<u32> {
    let b = data.get(pos..pos.checked_add(4)?)?;
    Some(u32::from_le_bytes([b[0], b[1], b[2], b[3]]))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn uleb128_examples_from_the_format() {
        // The DEX format document's examples.
        for (v, bytes) in [
            (0, &[0x00][..]),
            (1, &[0x01]),
            (127, &[0x7F]),
            (16256, &[0x80, 0x7F]),
            (0x80, &[0x80, 0x01]),
            (u32::MAX, &[0xFF, 0xFF, 0xFF, 0xFF, 0x0F]),
        ] {
            assert_eq!(decode_uleb128(bytes), Some((v, bytes.len())), "{v}");
            let mut out = Vec::new();
            encode_uleb128(v, &mut out);
            assert_eq!(out, bytes);
        }
        assert_eq!(decode_uleb128(&[]), None);
        assert_eq!(decode_uleb128(&[0x80]), None);
        assert_eq!(decode_uleb128(&[0x80, 0x80, 0x80, 0x80]), None);
        // Five bytes at most, however the fifth ends.
        assert_eq!(
            decode_uleb128(&[0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0x01]),
            Some((u32::MAX, 5))
        );
    }

    #[test]
    fn sleb128_examples_from_the_format() {
        for (v, bytes) in [
            (0, &[0x00][..]),
            (1, &[0x01]),
            (-1, &[0x7F]),
            (-128, &[0x80, 0x7F]),
            (63, &[0x3F]),
            (-64, &[0x40]),
            (64, &[0xC0, 0x00]),
            (i32::MIN, &[0x80, 0x80, 0x80, 0x80, 0x78]),
            (i32::MAX, &[0xFF, 0xFF, 0xFF, 0xFF, 0x07]),
        ] {
            assert_eq!(decode_sleb128(bytes), Some((v, bytes.len())), "{v}");
            let mut out = Vec::new();
            encode_sleb128(v, &mut out);
            assert_eq!(out, bytes, "{v}");
        }
        assert_eq!(decode_sleb128(&[0x80]), None);
    }

    #[test]
    fn cursor_reads_are_checked() {
        let data = [1, 2, 3, 4, 5, 0x80, 0x01, 0x00];
        let mut c = Cursor::at(&data, 0);
        assert_eq!(c.u16("a").unwrap(), 0x0201);
        assert_eq!(c.u16("b").unwrap(), 0x0403);
        assert_eq!(c.u8("c").unwrap(), 5);
        assert_eq!(c.uleb128("d").unwrap(), 0x80);
        assert_eq!(c.uleb128p1("e").unwrap(), None);
        let e = c.u8("f").unwrap_err();
        assert_eq!((e.kind, e.offset, e.what), (ErrorKind::Truncated, 8, "f"));
        assert!(Cursor::at(&data, 100).u8("past").is_err());
        assert!(Cursor::at(&data, 0).bytes(usize::MAX, "x").is_err());
        assert!(Cursor::at(&data, 0).check_count(9, 1, "n").is_err());
        assert_eq!(Cursor::at(&data, 0).check_count(4, 2, "n").unwrap(), 4);
        assert!(Cursor::at(&data, 0).check_count(u32::MAX, usize::MAX, "n").is_err());
        assert_eq!(u16_at(&data, 7), None);
        assert_eq!(u32_at(&data, 4), Some(0x0001_8005));
        assert_eq!(u32_at(&data, usize::MAX), None);
    }
}
