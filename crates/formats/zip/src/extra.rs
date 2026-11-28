//! Extra fields: the tagged records after a header's name (APPNOTE 4.5,
//! 4.6, and Info-ZIP's extra.fld).

use turbonop_core::{Error, ErrorKind, Result};

/// The ZIP64 extended information field.
pub const ZIP64: u16 = 0x0001;
/// The NTFS field: Windows FILETIMEs.
pub const NTFS: u16 = 0x000A;
/// PKWARE's strong encryption header.
pub const STRONG_ENCRYPTION: u16 = 0x0017;
/// Info-ZIP's extended timestamp ("UT"): Unix times.
pub const EXTENDED_TIMESTAMP: u16 = 0x5455;
/// Info-ZIP's old Unix field ("UX"): times, then a user and group.
pub const INFO_ZIP_UNIX_OLD: u16 = 0x5855;
/// Info-ZIP's Unicode comment ("uc").
pub const UNICODE_COMMENT: u16 = 0x6375;
/// Info-ZIP's Unicode path ("up").
pub const UNICODE_PATH: u16 = 0x7075;
/// Info-ZIP's new Unix field ("ux"): a user and group of any width.
pub const INFO_ZIP_UNIX: u16 = 0x7875;
/// WinZip's AES encryption field ("AE").
pub const AES: u16 = 0x9901;
/// The JAR marker (`jar` writes it, empty, on an archive's first entry).
pub const JAR_MARKER: u16 = 0xCAFE;
/// Android's alignment field (zipalign and apksigner pad with it).
pub const ANDROID_ALIGNMENT: u16 = 0xD935;

/// The name of an extra field's tag, if it's one this reader knows.
pub fn extra_field_name(id: u16) -> Option<&'static str> {
    Some(match id {
        ZIP64 => "ZIP64",
        0x0007 => "AV info",
        0x0009 => "OS/2",
        NTFS => "NTFS",
        0x000C => "OpenVMS",
        0x000D => "Unix",
        0x0014 => "PKCS#7 store",
        0x0015 => "X.509 certificate (file)",
        0x0016 => "X.509 certificate (central directory)",
        STRONG_ENCRYPTION => "strong encryption header",
        0x0018 => "record management controls",
        0x0019 => "PKCS#7 encryption recipients",
        0x0065 => "IBM S/390 (attributes, uncompressed)",
        0x0066 => "IBM S/390 (attributes, compressed)",
        0x4690 => "POSZIP 4690",
        EXTENDED_TIMESTAMP => "extended timestamp",
        INFO_ZIP_UNIX_OLD => "Info-ZIP Unix (old)",
        UNICODE_COMMENT => "Unicode comment",
        UNICODE_PATH => "Unicode path",
        INFO_ZIP_UNIX => "Info-ZIP Unix",
        AES => "AES encryption",
        JAR_MARKER => "JAR marker",
        ANDROID_ALIGNMENT => "Android alignment",
        _ => return None,
    })
}

/// One extra field, decoded if this reader knows its tag.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ExtraField<'d> {
    /// ZIP64's sizes and offsets: the raw field (which of its values are
    /// there depends on which of the header's fields are all ones: see
    /// [`Zip64Values::parse`]).
    Zip64(&'d [u8]),
    /// Info-ZIP's extended timestamp: Unix times, each present if its flag
    /// is set (and the field long enough: central directory copies carry
    /// only the modification time, with the local header's flags).
    ExtendedTimestamp {
        /// Which times the local header has: 1 modification, 2 access,
        /// 4 creation.
        flags: u8,
        /// The modification time (seconds since 1970, UTC).
        modified: Option<i32>,
        /// The access time.
        accessed: Option<i32>,
        /// The creation time.
        created: Option<i32>,
    },
    /// NTFS's times (FILETIMEs: 100 ns since 1601, UTC), from its tag 1.
    Ntfs {
        /// The modification time.
        modified: u64,
        /// The access time.
        accessed: u64,
        /// The creation time.
        created: u64,
    },
    /// Info-ZIP's Unicode path or comment: the UTF-8 text, valid while
    /// `name_crc32` is the CRC-32 of the header's own name (or comment).
    UnicodeText {
        /// Which: [`UNICODE_PATH`] or [`UNICODE_COMMENT`].
        id: u16,
        /// The field's version (1).
        version: u8,
        /// The CRC-32 of the header's text this replaces.
        name_crc32: u32,
        /// The text, UTF-8.
        text: &'d [u8],
    },
    /// Info-ZIP's Unix user and group (the new field: any width, up to 8
    /// bytes read).
    UnixOwner {
        /// The user.
        uid: u64,
        /// The group.
        gid: u64,
    },
    /// Info-ZIP's old Unix field: access and modification times, and
    /// (in local headers) a 16-bit user and group.
    UnixOld {
        /// The access time.
        accessed: i32,
        /// The modification time.
        modified: i32,
        /// The user and group, when present.
        owner: Option<(u16, u16)>,
    },
    /// WinZip's AES encryption: the entry's real compression method.
    Aes {
        /// 1 (AE-1, with a CRC) or 2 (AE-2, without).
        vendor_version: u16,
        /// The key: 1 128-bit, 2 192-bit, 3 256-bit.
        strength: u8,
        /// The compression method under the encryption.
        method: u16,
    },
    /// The JAR marker (empty).
    JarMarker,
    /// Android's alignment field: the alignment the data was padded to
    /// (the padding is the field's other bytes).
    AndroidAlignment {
        /// The alignment, in bytes.
        alignment: u16,
        /// How many bytes of padding follow.
        padding: usize,
    },
    /// A field this reader doesn't decode, or one too short for its tag.
    Other {
        /// Its tag.
        id: u16,
        /// Its data.
        data: &'d [u8],
    },
}

impl ExtraField<'_> {
    /// The field's tag.
    pub fn id(&self) -> u16 {
        match self {
            ExtraField::Zip64(_) => ZIP64,
            ExtraField::ExtendedTimestamp { .. } => EXTENDED_TIMESTAMP,
            ExtraField::Ntfs { .. } => NTFS,
            ExtraField::UnicodeText { id, .. } | ExtraField::Other { id, .. } => *id,
            ExtraField::UnixOwner { .. } => INFO_ZIP_UNIX,
            ExtraField::UnixOld { .. } => INFO_ZIP_UNIX_OLD,
            ExtraField::Aes { .. } => AES,
            ExtraField::JarMarker => JAR_MARKER,
            ExtraField::AndroidAlignment { .. } => ANDROID_ALIGNMENT,
        }
    }
}

fn le16(b: &[u8], at: usize) -> Option<u16> {
    Some(u16::from_le_bytes(b.get(at..at + 2)?.try_into().ok()?))
}

fn le32(b: &[u8], at: usize) -> Option<u32> {
    Some(u32::from_le_bytes(b.get(at..at + 4)?.try_into().ok()?))
}

fn le64(b: &[u8], at: usize) -> Option<u64> {
    Some(u64::from_le_bytes(b.get(at..at + 8)?.try_into().ok()?))
}

fn le_any(b: &[u8]) -> u64 {
    b.iter()
        .take(8)
        .rev()
        .fold(0u64, |v, &x| (v << 8) | u64::from(x))
}

#[allow(clippy::cast_possible_wrap)]
fn decode(id: u16, data: &[u8]) -> Option<ExtraField<'_>> {
    Some(match id {
        ZIP64 => ExtraField::Zip64(data),
        EXTENDED_TIMESTAMP => {
            let flags = *data.first()?;
            let mut at = 1;
            let mut next = |bit: u8| {
                if flags & bit == 0 {
                    return None;
                }
                let v = le32(data, at)? as i32;
                at += 4;
                Some(v)
            };
            let modified = next(1);
            let accessed = next(2);
            let created = next(4);
            ExtraField::ExtendedTimestamp {
                flags,
                modified,
                accessed,
                created,
            }
        }
        NTFS => {
            // A reserved u32, then tagged attributes; tag 1 is the times.
            let mut at = 4;
            loop {
                let tag = le16(data, at)?;
                let size = usize::from(le16(data, at + 2)?);
                let body = data.get(at + 4..at + 4 + size)?;
                if tag == 1 && size >= 24 {
                    break ExtraField::Ntfs {
                        modified: le64(body, 0)?,
                        accessed: le64(body, 8)?,
                        created: le64(body, 16)?,
                    };
                }
                at += 4 + size;
            }
        }
        UNICODE_PATH | UNICODE_COMMENT => ExtraField::UnicodeText {
            id,
            version: *data.first()?,
            name_crc32: le32(data, 1)?,
            text: data.get(5..)?,
        },
        INFO_ZIP_UNIX => {
            // Version 1, then each id's size and bytes.
            if *data.first()? != 1 {
                return None;
            }
            let uid_size = usize::from(*data.get(1)?);
            let uid = data.get(2..2 + uid_size)?;
            let gid_size = usize::from(*data.get(2 + uid_size)?);
            let gid = data.get(3 + uid_size..3 + uid_size + gid_size)?;
            ExtraField::UnixOwner {
                uid: le_any(uid),
                gid: le_any(gid),
            }
        }
        INFO_ZIP_UNIX_OLD => ExtraField::UnixOld {
            accessed: le32(data, 0)? as i32,
            modified: le32(data, 4)? as i32,
            owner: le16(data, 8).zip(le16(data, 10)),
        },
        AES => {
            if data.get(2..4)? != b"AE" {
                return None;
            }
            ExtraField::Aes {
                vendor_version: le16(data, 0)?,
                strength: *data.get(4)?,
                method: le16(data, 5)?,
            }
        }
        JAR_MARKER => ExtraField::JarMarker,
        ANDROID_ALIGNMENT => ExtraField::AndroidAlignment {
            alignment: le16(data, 0)?,
            padding: data.len() - 2,
        },
        _ => return None,
    })
}

/// The extra fields of a header's extra data, in order. A field whose
/// length runs past the data ends the iteration with an error; a field
/// too short for its tag's layout is [`ExtraField::Other`].
#[derive(Clone, Debug)]
pub struct ExtraFields<'d> {
    data: &'d [u8],
    pos: usize,
    /// The extra data's offset in the file (for errors).
    base: u64,
}

impl<'d> ExtraFields<'d> {
    /// The fields of `data`, whose offset in the file is `base`.
    pub fn new(data: &'d [u8], base: u64) -> Self {
        Self { data, pos: 0, base }
    }

    /// The data of the first field tagged `id`, if there is one.
    pub fn find(data: &'d [u8], id: u16) -> Option<&'d [u8]> {
        let mut pos = 0;
        while let (Some(tag), Some(size)) = (le16(data, pos), le16(data, pos + 2)) {
            let body = data.get(pos + 4..pos + 4 + usize::from(size))?;
            if tag == id {
                return Some(body);
            }
            pos += 4 + usize::from(size);
        }
        None
    }
}

impl<'d> Iterator for ExtraFields<'d> {
    type Item = Result<ExtraField<'d>>;

    fn next(&mut self) -> Option<Self::Item> {
        let rest = self.data.get(self.pos..)?;
        if rest.is_empty() {
            return None;
        }
        let at = self.base + self.pos as u64;
        let err = |what| Error::new(ErrorKind::Truncated, at, what);
        let (Some(id), Some(size)) = (le16(rest, 0), le16(rest, 2)) else {
            // (Some writers pad the extra data with a few zeros.)
            self.pos = self.data.len();
            return if rest.iter().all(|&b| b == 0) {
                None
            } else {
                Some(Err(err("extra field header")))
            };
        };
        let Some(body) = rest.get(4..4 + usize::from(size)) else {
            self.pos = self.data.len();
            return Some(Err(err("extra field data")));
        };
        self.pos += 4 + usize::from(size);
        Some(Ok(
            decode(id, body).unwrap_or(ExtraField::Other { id, data: body })
        ))
    }
}

/// The values a ZIP64 extra field holds: each of the header's fields that
/// is all ones (0xFFFFFFFF, or 0xFFFF for the disk) has its real value
/// here, in this order.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Zip64Values {
    /// The uncompressed size.
    pub uncompressed_size: Option<u64>,
    /// The compressed size.
    pub compressed_size: Option<u64>,
    /// The local header's offset (central directory only).
    pub local_header_offset: Option<u64>,
    /// The disk the entry starts on (central directory only).
    pub disk_start: Option<u32>,
}

impl Zip64Values {
    /// The values of `field` (a ZIP64 field's data) for a header whose
    /// fields named in `need` are all ones: uncompressed size, compressed
    /// size, local header offset, disk. `None` if the field is too short
    /// for what's needed.
    pub fn parse(field: &[u8], need: [bool; 4]) -> Option<Self> {
        let mut at = 0;
        let mut take8 = |needed: bool| -> Option<Option<u64>> {
            if !needed {
                return Some(None);
            }
            let v = le64(field, at)?;
            at += 8;
            Some(Some(v))
        };
        let uncompressed_size = take8(need[0])?;
        let compressed_size = take8(need[1])?;
        let local_header_offset = take8(need[2])?;
        let disk_start = if need[3] {
            Some(le32(field, at)?)
        } else {
            None
        };
        Some(Self {
            uncompressed_size,
            compressed_size,
            local_header_offset,
            disk_start,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn field(id: u16, data: &[u8]) -> Vec<u8> {
        let mut v = id.to_le_bytes().to_vec();
        v.extend_from_slice(&u16::try_from(data.len()).unwrap().to_le_bytes());
        v.extend_from_slice(data);
        v
    }

    #[test]
    fn decodes_each_kind() {
        let mut all = Vec::new();
        all.extend(field(EXTENDED_TIMESTAMP, &[3, 1, 0, 0, 0, 2, 0, 0, 0]));
        let mut ntfs = vec![0; 4];
        ntfs.extend_from_slice(&[9, 0, 1, 0, 0xAA]); // another tag first
        ntfs.extend_from_slice(&1u16.to_le_bytes());
        ntfs.extend_from_slice(&24u16.to_le_bytes());
        for t in [10u64, 20, 30] {
            ntfs.extend_from_slice(&t.to_le_bytes());
        }
        all.extend(field(NTFS, &ntfs));
        let mut up = vec![1];
        up.extend_from_slice(&0x1234_5678u32.to_le_bytes());
        up.extend_from_slice("é".as_bytes());
        all.extend(field(UNICODE_PATH, &up));
        all.extend(field(INFO_ZIP_UNIX, &[1, 4, 0xE8, 3, 0, 0, 2, 0x64, 0]));
        all.extend(field(
            INFO_ZIP_UNIX_OLD,
            &[1, 0, 0, 0, 2, 0, 0, 0, 5, 0, 6, 0],
        ));
        all.extend(field(AES, &[2, 0, b'A', b'E', 3, 8, 0]));
        all.extend(field(JAR_MARKER, &[]));
        all.extend(field(ANDROID_ALIGNMENT, &[4, 0, 0, 0, 0]));
        all.extend(field(0x1234, &[1, 2]));
        all.extend(field(AES, &[2, 0, b'X', b'X', 3, 8, 0]));
        let got: Vec<ExtraField<'_>> = ExtraFields::new(&all, 0).map(Result::unwrap).collect();
        assert_eq!(
            got,
            [
                ExtraField::ExtendedTimestamp {
                    flags: 3,
                    modified: Some(1),
                    accessed: Some(2),
                    created: None
                },
                ExtraField::Ntfs {
                    modified: 10,
                    accessed: 20,
                    created: 30
                },
                ExtraField::UnicodeText {
                    id: UNICODE_PATH,
                    version: 1,
                    name_crc32: 0x1234_5678,
                    text: "é".as_bytes()
                },
                ExtraField::UnixOwner {
                    uid: 1000,
                    gid: 100
                },
                ExtraField::UnixOld {
                    accessed: 1,
                    modified: 2,
                    owner: Some((5, 6))
                },
                ExtraField::Aes {
                    vendor_version: 2,
                    strength: 3,
                    method: 8
                },
                ExtraField::JarMarker,
                ExtraField::AndroidAlignment {
                    alignment: 4,
                    padding: 3
                },
                ExtraField::Other {
                    id: 0x1234,
                    data: &[1, 2]
                },
                ExtraField::Other {
                    id: AES,
                    data: &[2, 0, b'X', b'X', 3, 8, 0]
                },
            ]
        );
        assert_eq!(got[3].id(), INFO_ZIP_UNIX);
        assert_eq!(ExtraFields::find(&all, JAR_MARKER), Some(&[][..]));
        assert_eq!(ExtraFields::find(&all, 0x4321), None);
        assert_eq!(extra_field_name(ZIP64), Some("ZIP64"));
        assert_eq!(extra_field_name(0x4321), None);
    }

    #[test]
    fn central_extended_timestamp_has_only_the_modification_time() {
        let data = field(EXTENDED_TIMESTAMP, &[7, 9, 0, 0, 0]);
        let got: Vec<_> = ExtraFields::new(&data, 0).collect();
        assert_eq!(
            got,
            [Ok(ExtraField::ExtendedTimestamp {
                flags: 7,
                modified: Some(9),
                accessed: None,
                created: None
            })]
        );
    }

    #[test]
    fn damaged_fields() {
        // Runs past the end.
        let mut data = field(0x1234, &[1, 2, 3]);
        data.truncate(5);
        let got: Vec<_> = ExtraFields::new(&data, 100).collect();
        assert_eq!(got.len(), 1);
        assert_eq!(got[0].unwrap_err().offset, 100);
        // Zero padding is no field; other leftovers are an error.
        let mut data = field(JAR_MARKER, &[]);
        data.extend_from_slice(&[0, 0]);
        assert_eq!(ExtraFields::new(&data, 0).count(), 1);
        data[5] = 1;
        assert!(ExtraFields::new(&data, 0).nth(1).unwrap().is_err());
        // Too short for its kind.
        let data = field(EXTENDED_TIMESTAMP, &[]);
        assert!(matches!(
            ExtraFields::new(&data, 0).next(),
            Some(Ok(ExtraField::Other { .. }))
        ));
    }

    #[test]
    fn zip64_values_in_order() {
        let mut f = Vec::new();
        for v in [1u64, 2, 3] {
            f.extend_from_slice(&v.to_le_bytes());
        }
        f.extend_from_slice(&4u32.to_le_bytes());
        assert_eq!(
            Zip64Values::parse(&f, [true; 4]),
            Some(Zip64Values {
                uncompressed_size: Some(1),
                compressed_size: Some(2),
                local_header_offset: Some(3),
                disk_start: Some(4)
            })
        );
        // Only the offset: it's the first value.
        assert_eq!(
            Zip64Values::parse(&f, [false, false, true, false]),
            Some(Zip64Values {
                local_header_offset: Some(1),
                ..Zip64Values::default()
            })
        );
        assert_eq!(
            Zip64Values::parse(&f[..20], [true, true, true, false]),
            None
        );
    }
}
