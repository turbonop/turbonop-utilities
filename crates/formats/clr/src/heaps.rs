//! The metadata heaps (II.24.2.3 to II.24.2.5): `#Strings` (UTF-8, NUL
//! terminated), `#US` (user strings: UTF-16, length prefixed), `#GUID`
//! (16-byte GUIDs, numbered from 1), and `#Blob` (length-prefixed bytes).
//!
//! A heap that isn't there reads as empty: index 0 of each is the empty
//! (or null) entry, and any other index is an error.

use std::borrow::Cow;
use std::fmt;

use turbonop_core::{Error, ErrorKind, Result};

use crate::reader::decode_compressed_u32;

/// An index into `#Strings`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct StringIndex(pub u32);

/// An index into `#Blob`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct BlobIndex(pub u32);

/// An index into `#GUID` (1-based; 0 is null).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct GuidIndex(pub u32);

/// `#Strings`: identifiers, NUL-terminated UTF-8.
#[derive(Clone, Copy, Debug, Default)]
pub struct StringsHeap<'d> {
    data: &'d [u8],
}

impl<'d> StringsHeap<'d> {
    /// The heap over `data` (the stream's bytes).
    pub fn new(data: &'d [u8]) -> Self {
        Self { data }
    }

    /// The heap's bytes.
    pub fn data(&self) -> &'d [u8] {
        self.data
    }

    /// Its length without the alignment padding after the last string
    /// (the zeros past its NUL), as System.Reflection.Metadata reports it.
    pub fn unpadded_len(&self) -> usize {
        match self.data.iter().rposition(|&b| b != 0) {
            Some(i) if i + 1 < self.data.len() => i + 2,
            Some(_) => self.data.len(),
            None => self.data.len().min(1),
        }
    }

    /// The string at `index`, without its NUL. A string that runs to the
    /// heap's end without one ends there.
    pub fn get(&self, index: StringIndex) -> Result<&'d [u8]> {
        let at = index.0 as usize;
        if at == 0 && self.data.is_empty() {
            return Ok(&[]);
        }
        let rest = self
            .data
            .get(at..)
            .filter(|r| !r.is_empty())
            .ok_or(Error::new(
                ErrorKind::Malformed,
                u64::from(index.0),
                "#Strings index",
            ))?;
        Ok(&rest[..memchr_nul(rest)])
    }

    /// The string at `index` as text (invalid UTF-8 replaced).
    pub fn get_str(&self, index: StringIndex) -> Result<Cow<'d, str>> {
        Ok(String::from_utf8_lossy(self.get(index)?))
    }

    /// The string at `index` as text, or `""` if the index is bad.
    pub fn text(&self, index: StringIndex) -> Cow<'d, str> {
        self.get_str(index).unwrap_or(Cow::Borrowed(""))
    }

    /// Every string: its offset and bytes, in heap order (offset 0, the
    /// empty string, first).
    pub fn iter(&self) -> impl Iterator<Item = (u32, &'d [u8])> + 'd {
        let data = self.data;
        let mut at = 0usize;
        std::iter::from_fn(move || {
            let rest = data.get(at..).filter(|r| !r.is_empty())?;
            let n = memchr_nul(rest);
            let offset = u32::try_from(at).ok()?;
            at += n + 1;
            Some((offset, &rest[..n]))
        })
    }
}

fn memchr_nul(b: &[u8]) -> usize {
    b.iter().position(|&c| c == 0).unwrap_or(b.len())
}

/// A blob's length prefix and contents at `at` in `data`: the contents,
/// and the offset just past them.
fn blob_at<'a>(data: &'a [u8], at: usize, what: &'static str) -> Result<(&'a [u8], usize)> {
    let bad = || Error::new(ErrorKind::Malformed, at as u64, what);
    let rest = data.get(at..).ok_or_else(bad)?;
    let (len, n) = decode_compressed_u32(rest).ok_or_else(bad)?;
    let body =
        rest.get(n..n + len as usize)
            .ok_or(Error::new(ErrorKind::Truncated, at as u64, what))?;
    Ok((body, at + n + len as usize))
}

/// `#Blob`: signatures, constants, custom attribute values, and the rest,
/// each with a compressed length.
#[derive(Clone, Copy, Debug, Default)]
pub struct BlobHeap<'d> {
    data: &'d [u8],
}

impl<'d> BlobHeap<'d> {
    /// The heap over `data` (the stream's bytes).
    pub fn new(data: &'d [u8]) -> Self {
        Self { data }
    }

    /// The heap's bytes.
    pub fn data(&self) -> &'d [u8] {
        self.data
    }

    /// The blob at `index` (index 0, and any blob of an empty heap's index
    /// 0, is empty).
    pub fn get(&self, index: BlobIndex) -> Result<&'d [u8]> {
        if index.0 == 0 && self.data.is_empty() {
            return Ok(&[]);
        }
        Ok(blob_at(self.data, index.0 as usize, "#Blob index")?.0)
    }

    /// Every blob: its offset and contents, in heap order. Stops at the
    /// first that doesn't fit.
    pub fn iter(&self) -> impl Iterator<Item = (u32, &'d [u8])> + 'd {
        let data = self.data;
        let mut at = 0usize;
        std::iter::from_fn(move || {
            if at >= data.len() {
                return None;
            }
            let (body, next) = blob_at(data, at, "#Blob").ok()?;
            let offset = u32::try_from(at).ok()?;
            at = next;
            Some((offset, body))
        })
    }
}

/// A user string: UTF-16LE code units, and the terminal byte the compiler
/// adds (1 if any unit needs more than an 8-bit comparison).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct UserString<'d> {
    /// The UTF-16LE bytes (an even number).
    pub bytes: &'d [u8],
    /// The terminal byte, if the blob has one (its length is odd).
    pub terminal: Option<u8>,
}

impl UserString<'_> {
    /// Its UTF-16 code units.
    pub fn units(&self) -> impl Iterator<Item = u16> + '_ {
        self.bytes
            .as_chunks::<2>()
            .0
            .iter()
            .map(|&c| u16::from_le_bytes(c))
    }

    /// As text (unpaired surrogates replaced).
    pub fn to_string_lossy(&self) -> String {
        char::decode_utf16(self.units())
            .map(|c| c.unwrap_or(char::REPLACEMENT_CHARACTER))
            .collect()
    }
}

/// `#US`: the strings `ldstr` loads.
#[derive(Clone, Copy, Debug, Default)]
pub struct UserStringsHeap<'d> {
    data: &'d [u8],
}

impl<'d> UserStringsHeap<'d> {
    /// The heap over `data` (the stream's bytes).
    pub fn new(data: &'d [u8]) -> Self {
        Self { data }
    }

    /// The heap's bytes.
    pub fn data(&self) -> &'d [u8] {
        self.data
    }

    /// The string at `offset` (a `0x70` token's low 24 bits).
    pub fn get(&self, offset: u32) -> Result<UserString<'d>> {
        if offset == 0 && self.data.is_empty() {
            return Ok(UserString {
                bytes: &[],
                terminal: None,
            });
        }
        let (body, _) = blob_at(self.data, offset as usize, "#US offset")?;
        let even = body.len() & !1;
        Ok(UserString {
            bytes: &body[..even],
            terminal: (body.len() & 1 == 1).then(|| body[even]),
        })
    }

    /// Every string after the first (offset 0's empty one), as
    /// System.Reflection.Metadata enumerates them: padding zeros at the
    /// heap's end are empty strings too.
    pub fn iter(&self) -> impl Iterator<Item = (u32, UserString<'d>)> + 'd {
        let heap = *self;
        // The entry at 0, skipped.
        let mut at = blob_at(self.data, 0, "#US").map_or(usize::MAX, |(_, next)| next);
        std::iter::from_fn(move || {
            if at >= heap.data.len() {
                return None;
            }
            let offset = u32::try_from(at).ok()?;
            let s = heap.get(offset).ok()?;
            at = blob_at(heap.data, at, "#US").ok()?.1;
            Some((offset, s))
        })
    }
}

/// A GUID, in its in-memory byte order (the first three fields little
/// endian).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Guid(pub [u8; 16]);

impl fmt::Display for Guid {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let b = &self.0;
        write!(
            f,
            "{:08x}-{:04x}-{:04x}-{:02x}{:02x}-{:02x}{:02x}{:02x}{:02x}{:02x}{:02x}",
            u32::from_le_bytes([b[0], b[1], b[2], b[3]]),
            u16::from_le_bytes([b[4], b[5]]),
            u16::from_le_bytes([b[6], b[7]]),
            b[8],
            b[9],
            b[10],
            b[11],
            b[12],
            b[13],
            b[14],
            b[15]
        )
    }
}

/// `#GUID`: module version IDs, and in portable PDBs, languages and hash
/// algorithms.
#[derive(Clone, Copy, Debug, Default)]
pub struct GuidHeap<'d> {
    data: &'d [u8],
}

impl<'d> GuidHeap<'d> {
    /// The heap over `data` (the stream's bytes).
    pub fn new(data: &'d [u8]) -> Self {
        Self { data }
    }

    /// The heap's bytes.
    pub fn data(&self) -> &'d [u8] {
        self.data
    }

    /// How many GUIDs it holds.
    pub fn len(&self) -> u32 {
        u32::try_from(self.data.len() / 16).unwrap_or(u32::MAX)
    }

    /// Whether it holds none.
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// The GUID at `index` (1-based), or `None` for index 0.
    pub fn get(&self, index: GuidIndex) -> Result<Option<Guid>> {
        if index.0 == 0 {
            return Ok(None);
        }
        let at = (index.0 as usize - 1) * 16;
        let b = self.data.get(at..at + 16).ok_or(Error::new(
            ErrorKind::Malformed,
            u64::from(index.0),
            "#GUID index",
        ))?;
        let mut g = [0; 16];
        g.copy_from_slice(b);
        Ok(Some(Guid(g)))
    }
}

#[cfg(test)]
#[allow(clippy::cast_possible_truncation)]
mod tests {
    use super::*;

    #[test]
    fn strings() {
        let h = StringsHeap::new(b"\0Foo\0Bar\0Baz");
        assert_eq!(h.get(StringIndex(0)).unwrap(), b"");
        assert_eq!(h.get(StringIndex(1)).unwrap(), b"Foo");
        assert_eq!(h.get(StringIndex(2)).unwrap(), b"oo");
        assert_eq!(h.get(StringIndex(9)).unwrap(), b"Baz");
        assert!(h.get(StringIndex(12)).is_err());
        assert!(h.get(StringIndex(u32::MAX)).is_err());
        assert_eq!(h.text(StringIndex(99)), "");
        let all: Vec<_> = h.iter().collect();
        assert_eq!(all, [(0, &b""[..]), (1, b"Foo"), (5, b"Bar"), (9, b"Baz")]);
        assert_eq!(
            StringsHeap::new(b"\xff\0").get_str(StringIndex(0)).unwrap(),
            "\u{fffd}"
        );
        // No heap: only the empty string.
        let none = StringsHeap::default();
        assert_eq!(none.get(StringIndex(0)).unwrap(), b"");
        assert!(none.get(StringIndex(1)).is_err());
        assert_eq!(none.unpadded_len(), 0);
        assert_eq!(StringsHeap::new(b"\0ab\0\0\0").unpadded_len(), 4);
        assert_eq!(StringsHeap::new(b"\0ab").unpadded_len(), 3);
        assert_eq!(StringsHeap::new(b"\0\0\0\0").unpadded_len(), 1);
    }

    #[test]
    fn blobs() {
        let mut data = vec![0, 3, 1, 2, 3, 0x80, 0x80];
        data.extend(std::iter::repeat_n(7, 0x80));
        let h = BlobHeap::new(&data);
        assert_eq!(h.get(BlobIndex(0)).unwrap(), b"");
        assert_eq!(h.get(BlobIndex(1)).unwrap(), [1, 2, 3]);
        assert_eq!(h.get(BlobIndex(5)).unwrap().len(), 0x80);
        assert!(BlobHeap::new(&[0, 3, 1, 2]).get(BlobIndex(1)).is_err());
        assert!(h.get(BlobIndex(500)).is_err());
        let offsets: Vec<u32> = h.iter().map(|(o, _)| o).collect();
        assert_eq!(offsets, [0, 1, 5]);
        // A damaged length: 0xFF leads nothing.
        assert!(BlobHeap::new(&[0, 0xFF]).get(BlobIndex(1)).is_err());
        assert_eq!(BlobHeap::new(&[0, 0xFF]).iter().count(), 1);
        assert_eq!(BlobHeap::default().get(BlobIndex(0)).unwrap(), b"");
    }

    #[test]
    fn user_strings() {
        // "", "Hi" (with its terminal byte), then two padding zeros.
        let h = UserStringsHeap::new(&[0, 5, b'H', 0, b'i', 0, 0, 0, 0]);
        let s = h.get(1).unwrap();
        assert_eq!(s.to_string_lossy(), "Hi");
        assert_eq!(s.terminal, Some(0));
        let all: Vec<(u32, String)> = h.iter().map(|(o, s)| (o, s.to_string_lossy())).collect();
        assert_eq!(
            all,
            [(1, "Hi".into()), (7, String::new()), (8, String::new())]
        );
        assert!(h.get(9).is_err());
        let lone = UserStringsHeap::new(&[0, 2, 0x00, 0xD8]);
        assert_eq!(lone.get(1).unwrap().to_string_lossy(), "\u{fffd}");
        assert_eq!(UserStringsHeap::default().iter().count(), 0);
    }

    #[test]
    fn guids() {
        let mut data = [0u8; 32];
        data[16..].copy_from_slice(&[
            0xf2, 0x96, 0x69, 0xdc, 0x53, 0x6e, 0x8c, 0x42, 0xbb, 0x72, 0x5d, 0xdd, 0x5a, 0x14,
            0xc5, 0x57,
        ]);
        let h = GuidHeap::new(&data);
        assert_eq!(h.len(), 2);
        assert_eq!(h.get(GuidIndex(0)).unwrap(), None);
        assert_eq!(
            h.get(GuidIndex(2)).unwrap().unwrap().to_string(),
            "dc6996f2-6e53-428c-bb72-5ddd5a14c557"
        );
        assert!(h.get(GuidIndex(3)).is_err());
    }
}
