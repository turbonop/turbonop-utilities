//! Bounds-checked little-endian reads, and ECMA-335's compressed integers
//! (II.23.2).

use turbonop_core::{Error, ErrorKind, Result};

/// A compressed unsigned integer (one, two, or four bytes: `0xxxxxxx`,
/// `10xxxxxx xxxxxxxx`, `110xxxxx` and three more), and how many bytes it
/// took; `None` if the data ends first or the first byte is no valid
/// lead (`111xxxxx`).
pub fn decode_compressed_u32(data: &[u8]) -> Option<(u32, usize)> {
    let b0 = *data.first()?;
    if b0 & 0x80 == 0 {
        Some((u32::from(b0), 1))
    } else if b0 & 0xC0 == 0x80 {
        let b1 = *data.get(1)?;
        Some(((u32::from(b0 & 0x3F) << 8) | u32::from(b1), 2))
    } else if b0 & 0xE0 == 0xC0 {
        let b = data.get(1..4)?;
        Some((
            (u32::from(b0 & 0x1F) << 24)
                | (u32::from(b[0]) << 16)
                | (u32::from(b[1]) << 8)
                | u32::from(b[2]),
            4,
        ))
    } else {
        None
    }
}

/// A compressed signed integer: the unsigned encoding of its bits rotated
/// left by one (the sign in bit 0), in 7, 14, or 29 bits.
pub fn decode_compressed_i32(data: &[u8]) -> Option<(i32, usize)> {
    let (u, n) = decode_compressed_u32(data)?;
    let bits = match n {
        1 => 6,
        2 => 13,
        _ => 28,
    };
    #[allow(clippy::cast_possible_wrap)]
    let v = (u >> 1) as i32;
    Some((if u & 1 != 0 { v - (1 << bits) } else { v }, n))
}

/// The compressed encoding of `v` (at most `0x1FFF_FFFF`).
pub fn encode_compressed_u32(v: u32, out: &mut Vec<u8>) {
    #[allow(clippy::cast_possible_truncation)]
    if v < 0x80 {
        out.push(v as u8);
    } else if v < 0x4000 {
        out.extend_from_slice(&[0x80 | (v >> 8) as u8, v as u8]);
    } else {
        out.extend_from_slice(&[
            0xC0 | ((v >> 24) & 0x1F) as u8,
            (v >> 16) as u8,
            (v >> 8) as u8,
            v as u8,
        ]);
    }
}

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

    pub(crate) fn pos(&self) -> usize {
        self.pos
    }

    pub(crate) fn data(&self) -> &'d [u8] {
        self.data
    }

    pub(crate) fn remaining(&self) -> usize {
        self.data.len() - self.pos
    }

    pub(crate) fn is_empty(&self) -> bool {
        self.pos >= self.data.len()
    }

    pub(crate) fn error(&self, kind: ErrorKind, what: &'static str) -> Error {
        Error::new(kind, self.base + self.pos as u64, what)
    }

    pub(crate) fn seek(&mut self, pos: usize, what: &'static str) -> Result<()> {
        if pos > self.data.len() {
            return Err(self.error(ErrorKind::Truncated, what));
        }
        self.pos = pos;
        Ok(())
    }

    pub(crate) fn align(&mut self, to: usize, what: &'static str) -> Result<()> {
        let to = self.pos.next_multiple_of(to);
        self.seek(to, what)
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

    pub(crate) fn peek(&self) -> Option<u8> {
        self.data.get(self.pos).copied()
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

    pub(crate) fn u64(&mut self, what: &'static str) -> Result<u64> {
        Ok(u64::from_le_bytes(self.array(what)?))
    }

    pub(crate) fn i32(&mut self, what: &'static str) -> Result<i32> {
        Ok(i32::from_le_bytes(self.array(what)?))
    }

    /// A little-endian unsigned integer of `width` bytes (1 to 4).
    pub(crate) fn uint(&mut self, width: usize, what: &'static str) -> Result<u32> {
        let b = self.bytes(width.min(4), what)?;
        Ok(b.iter().rev().fold(0u32, |v, &x| (v << 8) | u32::from(x)))
    }

    pub(crate) fn compressed_u32(&mut self, what: &'static str) -> Result<u32> {
        let (v, n) = decode_compressed_u32(&self.data[self.pos..])
            .ok_or_else(|| self.error(ErrorKind::Malformed, what))?;
        self.pos += n;
        Ok(v)
    }

    pub(crate) fn compressed_i32(&mut self, what: &'static str) -> Result<i32> {
        let (v, n) = decode_compressed_i32(&self.data[self.pos..])
            .ok_or_else(|| self.error(ErrorKind::Malformed, what))?;
        self.pos += n;
        Ok(v)
    }

    /// .NET's `BinaryReader.Read7BitEncodedInt`: LEB128, at most five
    /// bytes.
    pub(crate) fn leb_u32(&mut self, what: &'static str) -> Result<u32> {
        let mut v = 0u32;
        for i in 0..5 {
            let b = self.u8(what)?;
            if i == 4 && b > 0x0F {
                return Err(self.error(ErrorKind::Malformed, what));
            }
            v |= u32::from(b & 0x7F) << (7 * i);
            if b & 0x80 == 0 {
                return Ok(v);
            }
        }
        Err(self.error(ErrorKind::Malformed, what))
    }

    /// A compressed count of items, each at least `min_size` bytes, that
    /// must fit in what's left (so that a damaged count can't allocate
    /// without bound).
    pub(crate) fn count(&mut self, min_size: usize, what: &'static str) -> Result<usize> {
        let n = self.compressed_u32(what)? as usize;
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
}

#[cfg(test)]
#[allow(clippy::cast_possible_truncation)]
mod tests {
    use super::*;

    #[test]
    fn compressed_unsigned_examples_from_ecma() {
        // II.23.2's examples.
        for (v, bytes) in [
            (0x03, &[0x03][..]),
            (0x7F, &[0x7F]),
            (0x80, &[0x80, 0x80]),
            (0x2E57, &[0xAE, 0x57]),
            (0x3FFF, &[0xBF, 0xFF]),
            (0x4000, &[0xC0, 0x00, 0x40, 0x00]),
            (0x1FFF_FFFF, &[0xDF, 0xFF, 0xFF, 0xFF]),
        ] {
            assert_eq!(
                decode_compressed_u32(bytes),
                Some((v, bytes.len())),
                "{v:#x}"
            );
            let mut out = Vec::new();
            encode_compressed_u32(v, &mut out);
            assert_eq!(out, bytes);
        }
        assert_eq!(decode_compressed_u32(&[]), None);
        assert_eq!(decode_compressed_u32(&[0x80]), None);
        assert_eq!(decode_compressed_u32(&[0xC0, 0, 0]), None);
        assert_eq!(decode_compressed_u32(&[0xE0, 0, 0, 0]), None);
        assert_eq!(decode_compressed_u32(&[0xFF]), None);
    }

    #[test]
    fn compressed_signed_examples_from_ecma() {
        for (v, bytes) in [
            (3, &[0x06][..]),
            (-3, &[0x7B]),
            (64, &[0x80, 0x80]),
            (-64, &[0x01]),
            (8192, &[0xC0, 0x00, 0x40, 0x00]),
            (-8192, &[0x80, 0x01]),
            (268_435_455, &[0xDF, 0xFF, 0xFF, 0xFE]),
            (-268_435_456, &[0xC0, 0x00, 0x00, 0x01]),
        ] {
            assert_eq!(decode_compressed_i32(bytes), Some((v, bytes.len())), "{v}");
        }
    }

    #[test]
    fn cursor_reads_are_checked() {
        let mut c = Cursor::with_base(&[1, 2, 3, 4, 5], 0x100);
        assert_eq!(c.u16("a").unwrap(), 0x0201);
        assert_eq!(c.uint(3, "b").unwrap(), 0x05_0403);
        let e = c.u8("c").unwrap_err();
        assert_eq!(
            (e.kind, e.offset, e.what),
            (ErrorKind::Truncated, 0x105, "c")
        );
        let mut c = Cursor::new(&[0x80, 0x01]);
        assert_eq!(c.leb_u32("leb").unwrap(), 0x80);
        let mut c = Cursor::new(&[0xFF, 0xFF, 0xFF, 0xFF, 0x7F]);
        assert!(c.leb_u32("leb").is_err());
        let mut c = Cursor::new(&[0xFF, 0xFF, 0xFF, 0xFF, 0x0F]);
        assert_eq!(c.leb_u32("leb").unwrap(), u32::MAX);
        let mut c = Cursor::new(&[0x7F, 1, 2]);
        assert!(c.count(1, "n").is_err());
        let mut c = Cursor::new(&[2, 1, 2]);
        assert_eq!(c.count(1, "n").unwrap(), 2);
        assert!(Cursor::new(&[1]).bytes(usize::MAX, "x").is_err());
        let mut c = Cursor::new(&[1, 2, 3]);
        assert!(c.seek(4, "s").is_err());
        c.seek(1, "s").unwrap();
        c.align(4, "a").unwrap_err();
        assert_eq!(c.peek(), Some(2));
    }
}
