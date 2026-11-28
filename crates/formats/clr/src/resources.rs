//! Managed resources: the `.resources` format that
//! `System.Resources.ResourceWriter` writes and `ResourceReader` reads
//! (the usual content of an embedded manifest resource), its entries, and
//! their sizes.
//!
//! A `.resources` file is a ResourceManager header (magic `0xBEEFCACE`,
//! the reader and resource set types), a RuntimeResourceSet header (the
//! version, the resource and type counts, the type names), padding to 8
//! bytes, the names' hashes and positions, the data section's offset, the
//! names (UTF-16, each with its data's offset), and the data: each value a
//! 7-bit-encoded type code and its bytes. An entry's size runs to the next
//! entry's data (by offset), or to the end.

use std::borrow::Cow;

use turbonop_core::{ErrorKind, Result};

use crate::reader::Cursor;

/// The ResourceManager header's magic.
pub const RESOURCES_MAGIC: u32 = 0xBEEF_CACE;

/// Type codes of version-2 resource files (`ResourceTypeCode`).
#[allow(missing_docs)]
pub mod type_code {
    pub const NULL: u32 = 0x00;
    pub const STRING: u32 = 0x01;
    pub const BOOLEAN: u32 = 0x02;
    pub const CHAR: u32 = 0x03;
    pub const BYTE: u32 = 0x04;
    pub const SBYTE: u32 = 0x05;
    pub const INT16: u32 = 0x06;
    pub const UINT16: u32 = 0x07;
    pub const INT32: u32 = 0x08;
    pub const UINT32: u32 = 0x09;
    pub const INT64: u32 = 0x0A;
    pub const UINT64: u32 = 0x0B;
    pub const SINGLE: u32 = 0x0C;
    pub const DOUBLE: u32 = 0x0D;
    pub const DECIMAL: u32 = 0x0E;
    pub const DATE_TIME: u32 = 0x0F;
    pub const TIME_SPAN: u32 = 0x10;
    pub const BYTE_ARRAY: u32 = 0x20;
    pub const STREAM: u32 = 0x21;
    /// Codes from here are indexes into the type table, offset by this.
    pub const START_OF_USER_TYPES: u32 = 0x40;
}

/// A built-in type code's name (`ResourceTypeCode`'s).
pub fn type_code_name(code: u32) -> Option<&'static str> {
    use type_code as T;
    Some(match code {
        T::NULL => "Null",
        T::STRING => "String",
        T::BOOLEAN => "Boolean",
        T::CHAR => "Char",
        T::BYTE => "Byte",
        T::SBYTE => "SByte",
        T::INT16 => "Int16",
        T::UINT16 => "UInt16",
        T::INT32 => "Int32",
        T::UINT32 => "UInt32",
        T::INT64 => "Int64",
        T::UINT64 => "UInt64",
        T::SINGLE => "Single",
        T::DOUBLE => "Double",
        T::DECIMAL => "Decimal",
        T::DATE_TIME => "DateTime",
        T::TIME_SPAN => "TimeSpan",
        T::BYTE_ARRAY => "ByteArray",
        T::STREAM => "Stream",
        _ => return None,
    })
}

/// A resource's value, decoded where its type is a built-in one.
#[derive(Clone, Debug, PartialEq)]
pub enum ResourceValue<'d> {
    /// Null.
    Null,
    /// A string.
    String(String),
    /// A bool.
    Bool(bool),
    /// A char.
    Char(u16),
    /// A signed integer.
    Int(i64),
    /// An unsigned integer.
    UInt(u64),
    /// A float32's bits.
    Single(u32),
    /// A float64's bits.
    Double(u64),
    /// A decimal's 16 bytes.
    Decimal([u8; 16]),
    /// A DateTime's 64 bits (ticks and kind).
    DateTime(u64),
    /// A TimeSpan's ticks.
    TimeSpan(i64),
    /// A byte array.
    Bytes(&'d [u8]),
    /// A stream's bytes.
    Stream(&'d [u8]),
    /// A serialized value of another type (its bytes).
    Other(&'d [u8]),
}

/// A resource.
#[derive(Clone, Debug, PartialEq)]
pub struct ResourceEntry<'d> {
    /// Its name.
    pub name: String,
    /// Its name's hash, as stored.
    pub hash: u32,
    /// Its type code (version 2), or its type's index (version 1; `None`
    /// for null).
    pub type_code: Option<u32>,
    /// Its type's name: `ResourceTypeCode.<Code>` for a built-in one (as
    /// `ResourceReader.GetResourceData` says), else the type table's.
    pub type_name: String,
    /// Where its data starts (its type code), from the file's start.
    pub offset: u32,
    /// Its size, type code included.
    pub size: u32,
    /// Its data after the type code.
    pub data: &'d [u8],
}

impl<'d> ResourceEntry<'d> {
    /// Its value, decoded for the built-in types.
    pub fn value(&self) -> Result<ResourceValue<'d>> {
        use type_code as T;
        let mut c = Cursor::new(self.data);
        let w = "resource value";
        let Some(code) = self.type_code else {
            return Ok(ResourceValue::Null);
        };
        Ok(match code {
            T::NULL => ResourceValue::Null,
            T::STRING => {
                let n = c.leb_u32(w)? as usize;
                ResourceValue::String(String::from_utf8_lossy(c.bytes(n, w)?).into_owned())
            }
            T::BOOLEAN => ResourceValue::Bool(c.u8(w)? != 0),
            T::CHAR => ResourceValue::Char(c.u16(w)?),
            T::BYTE => ResourceValue::UInt(u64::from(c.u8(w)?)),
            T::SBYTE => ResourceValue::Int(i64::from(c.u8(w)?.cast_signed())),
            T::INT16 => ResourceValue::Int(i64::from(c.u16(w)?.cast_signed())),
            T::UINT16 => ResourceValue::UInt(u64::from(c.u16(w)?)),
            T::INT32 => ResourceValue::Int(i64::from(c.i32(w)?)),
            T::UINT32 => ResourceValue::UInt(u64::from(c.u32(w)?)),
            T::INT64 => ResourceValue::Int(c.u64(w)?.cast_signed()),
            T::UINT64 => ResourceValue::UInt(c.u64(w)?),
            T::SINGLE => ResourceValue::Single(c.u32(w)?),
            T::DOUBLE => ResourceValue::Double(c.u64(w)?),
            T::DECIMAL => ResourceValue::Decimal(c.array(w)?),
            T::DATE_TIME => ResourceValue::DateTime(c.u64(w)?),
            T::TIME_SPAN => ResourceValue::TimeSpan(c.u64(w)?.cast_signed()),
            T::BYTE_ARRAY | T::STREAM => {
                let n = c.u32(w)? as usize;
                let b = c.bytes(n, w)?;
                if code == T::STREAM {
                    ResourceValue::Stream(b)
                } else {
                    ResourceValue::Bytes(b)
                }
            }
            _ => ResourceValue::Other(self.data),
        })
    }
}

/// A `.resources` file.
#[derive(Clone, Debug, PartialEq)]
pub struct ResourceSet<'d> {
    /// The ResourceManager header's version.
    pub manager_version: u32,
    /// The reader type's name (version 1 headers).
    pub reader_type: Option<String>,
    /// The resource set type's name (version 1 headers).
    pub set_type: Option<String>,
    /// The resource set's version (1 or 2).
    pub version: u32,
    /// The type table.
    pub types: Vec<String>,
    /// Where the data section starts.
    pub data_section: u32,
    /// The entries, in stored (hash) order.
    pub entries: Vec<ResourceEntry<'d>>,
}

fn binary_string<'d>(c: &mut Cursor<'d>, what: &'static str) -> Result<Cow<'d, str>> {
    let n = c.leb_u32(what)? as usize;
    Ok(String::from_utf8_lossy(c.bytes(n, what)?))
}

/// Whether `data` starts like a `.resources` file.
pub fn is_resources(data: &[u8]) -> bool {
    data.get(..4) == Some(&RESOURCES_MAGIC.to_le_bytes())
}

/// Parses a `.resources` file.
pub fn parse_resources(data: &[u8]) -> Result<ResourceSet<'_>> {
    let mut c = Cursor::new(data);
    if c.u32("resources magic")? != RESOURCES_MAGIC {
        return Err(c.error(ErrorKind::BadMagic, "resources magic"));
    }
    let manager_version = c.u32("resources header")?;
    let skip = c.u32("resources header")? as usize;
    let (mut reader_type, mut set_type) = (None, None);
    if manager_version > 1 {
        c.bytes(skip, "resources header")?;
    } else {
        let end = c.pos() + skip;
        reader_type = Some(binary_string(&mut c, "resource reader type")?.into_owned());
        set_type = Some(binary_string(&mut c, "resource set type")?.into_owned());
        c.seek(end, "resources header")?;
    }
    let version = c.u32("resource set version")?;
    if version != 1 && version != 2 {
        return Err(c.error(ErrorKind::Unsupported, "resource set version"));
    }
    let count = c.u32("resource count")? as usize;
    let count = c.check_count(count, 8, "resource count")?;
    let type_count = c.u32("resource type count")? as usize;
    let type_count = c.check_count(type_count, 1, "resource type count")?;
    let mut types = Vec::with_capacity(type_count);
    for _ in 0..type_count {
        types.push(binary_string(&mut c, "resource type name")?.into_owned());
    }
    // Padding ("PAD", then zeros) to 8.
    c.align(8, "resources padding")?;
    let mut hashes = Vec::with_capacity(count);
    for _ in 0..count {
        hashes.push(c.u32("resource name hashes")?);
    }
    let mut positions = Vec::with_capacity(count);
    for _ in 0..count {
        positions.push(c.u32("resource name positions")? as usize);
    }
    let data_section = c.u32("resource data section offset")?;
    let names_start = c.pos();
    let mut raw = Vec::with_capacity(count);
    for (i, &p) in positions.iter().enumerate() {
        let mut n = Cursor::new(data);
        n.seek(
            names_start
                .checked_add(p)
                .ok_or_else(|| c.error(ErrorKind::Overflow, "resource name"))?,
            "resource name",
        )?;
        let len = n.leb_u32("resource name")? as usize;
        let bytes = n.bytes(len, "resource name")?;
        let units: Vec<u16> = bytes
            .as_chunks::<2>()
            .0
            .iter()
            .map(|&b| u16::from_le_bytes(b))
            .collect();
        let offset = n.u32("resource data offset")?;
        let at = u64::from(data_section) + u64::from(offset);
        if at > data.len() as u64 {
            return Err(n.error(ErrorKind::Truncated, "resource data offset"));
        }
        #[allow(clippy::cast_possible_truncation)]
        raw.push((String::from_utf16_lossy(&units), hashes[i], at as u32));
    }
    let mut sorted: Vec<u32> = raw.iter().map(|r| r.2).collect();
    sorted.sort_unstable();
    sorted.dedup();
    let end = u32::try_from(data.len()).unwrap_or(u32::MAX);
    let mut entries = Vec::with_capacity(count);
    for (name, hash, at) in raw {
        let next = sorted
            .get(sorted.partition_point(|&o| o <= at))
            .copied()
            .unwrap_or(end);
        let mut d = Cursor::new(&data[at as usize..next as usize]);
        let code = d.leb_u32("resource type code")?;
        let (type_code, type_name) = if version == 1 {
            // A type index (-1, as 0xFFFFFFFF, for null).
            if code == u32::MAX {
                (None, "null".to_string())
            } else {
                let t = types
                    .get(code as usize)
                    .ok_or_else(|| d.error(ErrorKind::Malformed, "resource type index"))?;
                (Some(code), t.clone())
            }
        } else if code >= type_code::START_OF_USER_TYPES {
            let t = types
                .get((code - type_code::START_OF_USER_TYPES) as usize)
                .ok_or_else(|| d.error(ErrorKind::Malformed, "resource type code"))?;
            (Some(code), t.clone())
        } else {
            let n = type_code_name(code)
                .ok_or_else(|| d.error(ErrorKind::Malformed, "resource type code"))?;
            (Some(code), format!("ResourceTypeCode.{n}"))
        };
        let body = &d.data()[d.pos()..];
        entries.push(ResourceEntry {
            name,
            hash,
            type_code,
            type_name,
            offset: at,
            size: next - at,
            data: body,
        });
    }
    Ok(ResourceSet {
        manager_version,
        reader_type,
        set_type,
        version,
        types,
        data_section,
        entries,
    })
}

/// The hash `ResourceWriter` stores for a name (`FastResourceComparer`:
/// djb2 with xor, over UTF-16 units).
pub fn name_hash(name: &str) -> u32 {
    name.encode_utf16().fold(5381u32, |h, c| {
        (h.wrapping_shl(5).wrapping_add(h)) ^ u32::from(c)
    })
}

#[cfg(test)]
#[allow(clippy::cast_possible_truncation)]
mod tests {
    use super::*;

    fn leb(mut v: u32, out: &mut Vec<u8>) {
        loop {
            let b = (v & 0x7F) as u8;
            v >>= 7;
            if v == 0 {
                out.push(b);
                return;
            }
            out.push(b | 0x80);
        }
    }

    /// A version-2 file with "a" = "xy" and "b" = int32 7, as
    /// ResourceWriter lays it out.
    fn sample() -> Vec<u8> {
        let mut v = Vec::new();
        v.extend_from_slice(&RESOURCES_MAGIC.to_le_bytes());
        v.extend_from_slice(&1u32.to_le_bytes());
        let mut hdr = Vec::new();
        for s in [
            "System.Resources.ResourceReader",
            "System.Resources.RuntimeResourceSet",
        ] {
            leb(s.len() as u32, &mut hdr);
            hdr.extend_from_slice(s.as_bytes());
        }
        v.extend_from_slice(&(hdr.len() as u32).to_le_bytes());
        v.extend_from_slice(&hdr);
        v.extend_from_slice(&2u32.to_le_bytes());
        v.extend_from_slice(&2u32.to_le_bytes());
        v.extend_from_slice(&0u32.to_le_bytes());
        while !v.len().is_multiple_of(8) {
            v.push(b'P');
        }
        v.extend_from_slice(&name_hash("a").to_le_bytes());
        v.extend_from_slice(&name_hash("b").to_le_bytes());
        v.extend_from_slice(&0u32.to_le_bytes());
        v.extend_from_slice(&7u32.to_le_bytes());
        let data_section_at = v.len();
        v.extend_from_slice(&0u32.to_le_bytes());
        // Names: "a" -> 0, "b" -> 4.
        v.extend_from_slice(&[2, b'a', 0, 0, 0, 0, 0]);
        v.extend_from_slice(&[2, b'b', 0, 4, 0, 0, 0]);
        let data = v.len() as u32;
        v[data_section_at..data_section_at + 4].copy_from_slice(&data.to_le_bytes());
        v.extend_from_slice(&[1, 2, b'x', b'y']);
        v.extend_from_slice(&[8, 7, 0, 0, 0]);
        v
    }

    #[test]
    fn parses_entries() {
        let data = sample();
        assert!(is_resources(&data));
        let r = parse_resources(&data).unwrap();
        assert_eq!(r.version, 2);
        assert_eq!(
            r.reader_type.as_deref(),
            Some("System.Resources.ResourceReader")
        );
        let e: Vec<(&str, &str, u32)> = r
            .entries
            .iter()
            .map(|e| (e.name.as_str(), e.type_name.as_str(), e.size))
            .collect();
        assert_eq!(
            e,
            [
                ("a", "ResourceTypeCode.String", 4),
                ("b", "ResourceTypeCode.Int32", 5)
            ]
        );
        assert_eq!(
            r.entries[0].value().unwrap(),
            ResourceValue::String("xy".into())
        );
        assert_eq!(r.entries[1].value().unwrap(), ResourceValue::Int(7));
        for n in 0..data.len() - 5 {
            assert!(parse_resources(&data[..n]).is_err(), "{n}");
        }
        // A huge count.
        let mut d = data.clone();
        let at = 12 + 68 + 4;
        d[at..at + 4].copy_from_slice(&0x7FFF_FFFFu32.to_le_bytes());
        assert!(parse_resources(&d).is_err());
        // An unknown type code.
        let mut d = data.clone();
        let n = d.len();
        d[n - 5] = 0x1F;
        assert!(parse_resources(&d).is_err());
        // A user type with no type table entry.
        d[n - 5] = 0x40;
        assert!(parse_resources(&d).is_err());
    }

    #[test]
    fn values() {
        let e = |code: u32, data: &'static [u8]| ResourceEntry {
            name: String::new(),
            hash: 0,
            type_code: Some(code),
            type_name: String::new(),
            offset: 0,
            size: 0,
            data,
        };
        assert_eq!(
            e(type_code::SBYTE, &[0x9C]).value().unwrap(),
            ResourceValue::Int(-100)
        );
        assert_eq!(
            e(type_code::BYTE_ARRAY, &[2, 0, 0, 0, 9, 8])
                .value()
                .unwrap(),
            ResourceValue::Bytes(&[9, 8])
        );
        assert!(
            e(type_code::BYTE_ARRAY, &[9, 0, 0, 0, 9, 8])
                .value()
                .is_err()
        );
        assert!(e(type_code::DECIMAL, &[0; 15]).value().is_err());
        assert_eq!(
            e(0x40, &[1, 2]).value().unwrap(),
            ResourceValue::Other(&[1, 2])
        );
        assert_eq!(name_hash(""), 5381);
    }
}
