//! The DEX header (`header_item`): the magic and version, the checksum and
//! signature, and where the id tables and data are; version 041's
//! container fields.

use turbonop_core::{Error, ErrorKind, Result};

use crate::reader::Cursor;

/// The size of a version 035–040 header.
pub const HEADER_SIZE: u32 = 0x70;
/// The size of a version 041 header (with `container_size` and
/// `header_offset`).
pub const HEADER_SIZE_V41: u32 = 0x78;
/// `endian_tag` of a little-endian file.
pub const ENDIAN_CONSTANT: u32 = 0x1234_5678;
/// `endian_tag` of a byte-swapped file (which nothing writes, and ART
/// doesn't read).
pub const REVERSE_ENDIAN_CONSTANT: u32 = 0x7856_3412;
/// The versions this reads: every one ART has ever read, 035 to 041.
pub const VERSIONS: [u32; 6] = [35, 37, 38, 39, 40, 41];
/// The first version in the container format.
pub const CONTAINER_VERSION: u32 = 41;

/// Whether `data` starts with a DEX magic (`dex\n` and a version, any
/// version).
pub fn is_dex(data: &[u8]) -> bool {
    data.len() >= 8
        && data.starts_with(b"dex\n")
        && data[4..7].iter().all(u8::is_ascii_digit)
        && data[7] == 0
}

/// A DEX header.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Header {
    /// `dex\n` and the version's three digits and a NUL.
    pub magic: [u8; 8],
    /// The version (35 for `035`).
    pub version: u32,
    /// The Adler-32 of the file (of this section, in a container) after
    /// this field.
    pub checksum: u32,
    /// The SHA-1 of the file (section) after this field.
    pub signature: [u8; 20],
    /// The file's size (in a container, this section's: up to the next
    /// header).
    pub file_size: u32,
    /// The header's size.
    pub header_size: u32,
    /// [`ENDIAN_CONSTANT`] (or, byte-swapped, [`REVERSE_ENDIAN_CONSTANT`]).
    pub endian_tag: u32,
    /// The link section (unused by any format version).
    pub link_size: u32,
    /// Where the link section is.
    pub link_off: u32,
    /// Where the map list is.
    pub map_off: u32,
    /// How many strings.
    pub string_ids_size: u32,
    /// Where their ids are.
    pub string_ids_off: u32,
    /// How many types.
    pub type_ids_size: u32,
    /// Where their ids are.
    pub type_ids_off: u32,
    /// How many prototypes.
    pub proto_ids_size: u32,
    /// Where their ids are.
    pub proto_ids_off: u32,
    /// How many field references.
    pub field_ids_size: u32,
    /// Where their ids are.
    pub field_ids_off: u32,
    /// How many method references.
    pub method_ids_size: u32,
    /// Where their ids are.
    pub method_ids_off: u32,
    /// How many classes.
    pub class_defs_size: u32,
    /// Where their definitions are.
    pub class_defs_off: u32,
    /// The data section's size (0 in a container).
    pub data_size: u32,
    /// Where the data section is (0 in a container).
    pub data_off: u32,
    /// Version 041: the whole container's size (`None` before).
    pub container_size: Option<u32>,
    /// Version 041: this header's offset in the container.
    pub header_offset: Option<u32>,
}

impl Header {
    /// Parses the header at `offset` of `data` (the file, or the
    /// container).
    pub fn parse(data: &[u8], offset: usize) -> Result<Self> {
        let mut c = Cursor::at(data, offset);
        let magic: [u8; 8] = c.array("DEX magic")?;
        if !is_dex(&magic) {
            return Err(Error::new(ErrorKind::BadMagic, offset as u64, "DEX magic"));
        }
        let version = magic[4..7]
            .iter()
            .fold(0u32, |v, d| v * 10 + u32::from(d - b'0'));
        if !VERSIONS.contains(&version) {
            return Err(Error::new(
                ErrorKind::Unsupported,
                offset as u64 + 4,
                "DEX version",
            ));
        }
        let checksum = c.u32("checksum")?;
        let signature = c.array("signature")?;
        let file_size = c.u32("file_size")?;
        let header_size = c.u32("header_size")?;
        let endian_tag = c.u32("endian_tag")?;
        let mut h = Self {
            magic,
            version,
            checksum,
            signature,
            file_size,
            header_size,
            endian_tag,
            link_size: c.u32("link_size")?,
            link_off: c.u32("link_off")?,
            map_off: c.u32("map_off")?,
            string_ids_size: c.u32("string_ids_size")?,
            string_ids_off: c.u32("string_ids_off")?,
            type_ids_size: c.u32("type_ids_size")?,
            type_ids_off: c.u32("type_ids_off")?,
            proto_ids_size: c.u32("proto_ids_size")?,
            proto_ids_off: c.u32("proto_ids_off")?,
            field_ids_size: c.u32("field_ids_size")?,
            field_ids_off: c.u32("field_ids_off")?,
            method_ids_size: c.u32("method_ids_size")?,
            method_ids_off: c.u32("method_ids_off")?,
            class_defs_size: c.u32("class_defs_size")?,
            class_defs_off: c.u32("class_defs_off")?,
            data_size: c.u32("data_size")?,
            data_off: c.u32("data_off")?,
            container_size: None,
            header_offset: None,
        };
        if endian_tag == REVERSE_ENDIAN_CONSTANT {
            return Err(Error::new(
                ErrorKind::Unsupported,
                offset as u64 + 40,
                "byte-swapped DEX file",
            ));
        }
        if endian_tag != ENDIAN_CONSTANT {
            return Err(Error::new(
                ErrorKind::Malformed,
                offset as u64 + 40,
                "endian_tag",
            ));
        }
        let min = if version >= CONTAINER_VERSION {
            HEADER_SIZE_V41
        } else {
            HEADER_SIZE
        };
        if header_size < min {
            return Err(Error::new(
                ErrorKind::Malformed,
                offset as u64 + 36,
                "header_size",
            ));
        }
        if version >= CONTAINER_VERSION {
            h.container_size = Some(c.u32("container_size")?);
            h.header_offset = Some(c.u32("header_offset")?);
        }
        Ok(h)
    }

    /// The version as the magic spells it (`"035"`).
    pub fn version_str(&self) -> &str {
        std::str::from_utf8(&self.magic[4..7]).unwrap_or("???")
    }

    /// Whether this is a container format (version 041) header.
    pub fn is_container(&self) -> bool {
        self.version >= CONTAINER_VERSION
    }
}

/// The Adler-32 of `data` (zlib's), computed on every core for big inputs:
/// each piece's sums, then combined.
pub fn adler32(data: &[u8]) -> u32 {
    const PIECE: usize = 1 << 20;
    if data.len() <= 2 * PIECE {
        return adler32_update(1, data);
    }
    let pieces = turbonop_par::map_chunks(data, PIECE, |piece| {
        vec![(adler32_update(1, piece), piece.len())]
    });
    pieces
        .into_iter()
        .fold(1, |acc, (a, len)| adler32_combine(acc, a, len))
}

const MOD_ADLER: u32 = 65521;

/// `adler`, continued over `data`.
pub fn adler32_update(adler: u32, data: &[u8]) -> u32 {
    // 5552 bytes is the most that can be summed before the sums overflow
    // (zlib's NMAX).
    let (mut a, mut b) = (adler & 0xFFFF, adler >> 16);
    for chunk in data.chunks(5552) {
        for &x in chunk {
            a += u32::from(x);
            b += a;
        }
        a %= MOD_ADLER;
        b %= MOD_ADLER;
    }
    (b << 16) | a
}

/// The Adler-32 of two pieces' concatenation from each's (zlib's
/// `adler32_combine`): `len2` is the second's length.
pub fn adler32_combine(adler1: u32, adler2: u32, len2: usize) -> u32 {
    let rem = (len2 % MOD_ADLER as usize) as u64;
    let m = u64::from(MOD_ADLER);
    let a1 = u64::from(adler1 & 0xFFFF);
    let b1 = u64::from(adler1 >> 16);
    let a2 = u64::from(adler2 & 0xFFFF);
    let b2 = u64::from(adler2 >> 16);
    let mut a = a1 + a2 + m - 1;
    let mut b = (rem * a1) % m + b1 + b2 + m - rem;
    a %= m;
    b %= m;
    #[allow(clippy::cast_possible_truncation)]
    {
        ((b as u32) << 16) | a as u32
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn adler32_known_values() {
        assert_eq!(adler32(b""), 1);
        assert_eq!(adler32(b"a"), 0x0062_0062);
        assert_eq!(adler32(b"Wikipedia"), 0x11E6_0398);
        let big: Vec<u8> = (0..5_000_000u32).map(|i| (i * 7 + i / 255) as u8).collect();
        assert_eq!(adler32(&big), adler32_update(1, &big));
        for split in [0, 1, 5552, 65521, 1 << 20, big.len()] {
            let (x, y) = big.split_at(split);
            assert_eq!(
                adler32_combine(adler32_update(1, x), adler32_update(1, y), y.len()),
                adler32_update(1, &big),
                "{split}"
            );
        }
    }

    #[test]
    fn magic_and_versions() {
        assert!(is_dex(b"dex\n035\0"));
        assert!(is_dex(b"dex\n999\0rest"));
        assert!(!is_dex(b"dex\n03a\0"));
        assert!(!is_dex(b"dey\n035\0"));
        assert!(!is_dex(b"dex\n035"));
        let mut h = vec![0u8; 0x70];
        h[..8].copy_from_slice(b"dex\n036\0");
        assert_eq!(
            Header::parse(&h, 0).unwrap_err().kind,
            ErrorKind::Unsupported
        );
        h[..8].copy_from_slice(b"dex\n035\0");
        h[36..40].copy_from_slice(&0x70u32.to_le_bytes());
        h[40..44].copy_from_slice(&REVERSE_ENDIAN_CONSTANT.to_le_bytes());
        assert_eq!(
            Header::parse(&h, 0).unwrap_err().kind,
            ErrorKind::Unsupported
        );
        h[40..44].copy_from_slice(&ENDIAN_CONSTANT.to_le_bytes());
        let parsed = Header::parse(&h, 0).unwrap();
        assert_eq!((parsed.version, parsed.version_str()), (35, "035"));
        assert!(!parsed.is_container());
        h[36..40].copy_from_slice(&0x6Fu32.to_le_bytes());
        assert_eq!(Header::parse(&h, 0).unwrap_err().what, "header_size");
        // 041 wants the longer header, and has its fields.
        h[..8].copy_from_slice(b"dex\n041\0");
        h[36..40].copy_from_slice(&0x70u32.to_le_bytes());
        assert_eq!(Header::parse(&h, 0).unwrap_err().what, "header_size");
        h[36..40].copy_from_slice(&0x78u32.to_le_bytes());
        assert_eq!(Header::parse(&h, 0).unwrap_err().kind, ErrorKind::Truncated);
        h.extend_from_slice(&[0x10, 0, 0, 0, 0x20, 0, 0, 0]);
        let parsed = Header::parse(&h, 0).unwrap();
        assert_eq!(
            (parsed.container_size, parsed.header_offset),
            (Some(0x10), Some(0x20))
        );
        assert!(Header::parse(&h, 1).is_err());
    }
}
