//! ZIP archives (PKWARE's APPNOTE 6.3.10), as JARs, APKs, AABs, NuGet
//! packages, Office documents, and `.zip` files hold them.
//!
//! - [`Archive::parse`] finds the end of central directory record (and
//!   ZIP64's record and locator) and reads every central directory entry.
//!   Data before the archive (a self-extractor's program, a JMOD's
//!   header) is allowed: offsets are taken relative to where the central
//!   directory really is when they don't match where it's recorded.
//! - [`Entry`]: an entry's central directory record, with ZIP64's sizes
//!   and offset applied, its name decoded (UTF-8, Info-ZIP's Unicode path,
//!   or code page 437), times, attributes, and [`extra`] fields.
//! - [`Archive::local_header`], [`Archive::data_descriptor`],
//!   [`Archive::extent`]: an entry's local header, data, and descriptor,
//!   where each lies (for sizing).
//! - [`Archive::read`]: an entry's data, stored or deflated (inflated with
//!   `miniz_oxide`), its size and CRC-32 checked. Other methods (bzip2,
//!   LZMA, zstd, ...) and encrypted entries are named, not decoded.
//!   [`Archive::par_map`] and [`Archive::read_all`] work on every core.
//!
//! Multi-disk (split and spanned) archives are refused: their parts aren't
//! in one buffer. Everything is read from the borrowed bytes and every
//! read is checked: damaged archives give errors, never a panic.

#![warn(missing_docs)]

#[cfg(test)]
use turbonop_global_alloc as _;

mod crc;
pub mod extra;
mod text;

pub use crc::{crc32, crc32_update};
pub use extra::{ExtraField, ExtraFields, Zip64Values};
pub use text::{DosDateTime, decode_cp437};

use std::borrow::Cow;
use std::ops::Range;
use std::sync::OnceLock;

use turbonop_core::{Error, ErrorKind, Result};

/// A local file header's signature (`PK\3\4`).
pub const LOCAL_HEADER_SIGNATURE: u32 = 0x0403_4B50;
/// A central directory header's signature (`PK\1\2`).
pub const CENTRAL_HEADER_SIGNATURE: u32 = 0x0201_4B50;
/// The end of central directory record's signature (`PK\5\6`).
pub const END_OF_CENTRAL_DIRECTORY_SIGNATURE: u32 = 0x0605_4B50;
/// The ZIP64 end of central directory record's signature (`PK\6\6`).
pub const ZIP64_END_OF_CENTRAL_DIRECTORY_SIGNATURE: u32 = 0x0606_4B50;
/// The ZIP64 end of central directory locator's signature (`PK\6\7`).
pub const ZIP64_LOCATOR_SIGNATURE: u32 = 0x0706_4B50;
/// A data descriptor's (optional) signature (`PK\7\8`), also a spanned
/// archive's first four bytes.
pub const DATA_DESCRIPTOR_SIGNATURE: u32 = 0x0807_4B50;
/// The archive extra data record's signature (`PK\6\8`), before an
/// encrypted central directory.
pub const ARCHIVE_EXTRA_DATA_SIGNATURE: u32 = 0x0806_4B50;
/// The central directory's digital signature record (`PK\5\5`).
pub const DIGITAL_SIGNATURE_SIGNATURE: u32 = 0x0505_4B50;

/// The end of central directory record's size, without its comment.
pub const END_OF_CENTRAL_DIRECTORY_SIZE: usize = 22;
/// The ZIP64 locator's size.
pub const ZIP64_LOCATOR_SIZE: usize = 20;
/// The ZIP64 end of central directory record's size, without its
/// extensible data.
pub const ZIP64_END_OF_CENTRAL_DIRECTORY_SIZE: usize = 56;
/// A central directory header's size, before its name, extra, and comment.
pub const CENTRAL_HEADER_SIZE: usize = 46;
/// A local header's size, before its name and extra.
pub const LOCAL_HEADER_SIZE: usize = 30;

/// General-purpose flag bits.
pub mod flags {
    /// The entry is encrypted (traditional PKWARE encryption, or with
    /// [`STRONG_ENCRYPTION`], PKWARE's strong encryption; WinZip's AES has
    /// an AES extra field).
    pub const ENCRYPTED: u16 = 1 << 0;
    /// Imploding's 8K dictionary, or deflate's compression option (with
    /// bit 2: normal, maximum, fast, super fast).
    pub const OPTION_1: u16 = 1 << 1;
    /// Imploding's three Shannon-Fano trees, or deflate's option.
    pub const OPTION_2: u16 = 1 << 2;
    /// The local header's CRC-32 and sizes are zero; a data descriptor
    /// after the data has them.
    pub const DATA_DESCRIPTOR: u16 = 1 << 3;
    /// Enhanced deflating (method 8; reserved).
    pub const ENHANCED_DEFLATE: u16 = 1 << 4;
    /// Compressed patched data.
    pub const PATCHED: u16 = 1 << 5;
    /// PKWARE's strong encryption.
    pub const STRONG_ENCRYPTION: u16 = 1 << 6;
    /// The name and comment are UTF-8 (the "language encoding flag").
    pub const UTF8: u16 = 1 << 11;
    /// The central directory is encrypted, and local headers' values
    /// masked.
    pub const MASKED_HEADERS: u16 = 1 << 13;
}

/// The compression methods' numbers.
pub mod method {
    /// No compression.
    pub const STORED: u16 = 0;
    /// Shrunk (LZW).
    pub const SHRUNK: u16 = 1;
    /// Deflated.
    pub const DEFLATED: u16 = 8;
    /// Deflate64.
    pub const DEFLATE64: u16 = 9;
    /// bzip2.
    pub const BZIP2: u16 = 12;
    /// LZMA.
    pub const LZMA: u16 = 14;
    /// Zstandard.
    pub const ZSTD: u16 = 93;
    /// xz.
    pub const XZ: u16 = 95;
    /// AES encrypted (WinZip): the real method is in the AES extra field.
    pub const AES: u16 = 99;
}

/// The name of compression method `m`.
pub fn method_name(m: u16) -> Cow<'static, str> {
    Cow::Borrowed(match m {
        0 => "stored",
        1 => "shrunk",
        2..=5 => {
            return Cow::Owned(format!("reduced (factor {})", m - 1));
        }
        6 => "imploded",
        7 => "tokenized",
        8 => "deflated",
        9 => "deflate64",
        10 => "PKWARE DCL imploded",
        12 => "bzip2",
        14 => "LZMA",
        16 => "IBM z/OS CMPSC",
        18 => "IBM TERSE",
        19 => "IBM LZ77 z",
        20 | 93 => "zstd",
        94 => "MP3",
        95 => "xz",
        96 => "JPEG",
        97 => "WavPack",
        98 => "PPMd",
        99 => "AES encrypted",
        _ => return Cow::Owned(format!("method {m}")),
    })
}

/// The name of the host system in a "version made by" field's high byte.
pub fn host_system_name(host: u8) -> Cow<'static, str> {
    Cow::Borrowed(match host {
        0 => "MS-DOS",
        1 => "Amiga",
        2 => "OpenVMS",
        3 => "Unix",
        4 => "VM/CMS",
        5 => "Atari ST",
        6 => "OS/2 HPFS",
        7 => "Macintosh",
        8 => "Z-System",
        9 => "CP/M",
        10 => "Windows NTFS",
        11 => "MVS",
        12 => "VSE",
        13 => "Acorn RISC OS",
        14 => "VFAT",
        15 => "alternate MVS",
        16 => "BeOS",
        17 => "Tandem",
        18 => "OS/400",
        19 => "OS X",
        _ => return Cow::Owned(format!("host {host}")),
    })
}

fn le16(data: &[u8], at: usize, what: &'static str) -> Result<u16> {
    data.get(at..at.wrapping_add(2))
        .and_then(|b| b.try_into().ok())
        .map(u16::from_le_bytes)
        .ok_or(Error::new(ErrorKind::Truncated, at as u64, what))
}

fn le32(data: &[u8], at: usize, what: &'static str) -> Result<u32> {
    data.get(at..at.wrapping_add(4))
        .and_then(|b| b.try_into().ok())
        .map(u32::from_le_bytes)
        .ok_or(Error::new(ErrorKind::Truncated, at as u64, what))
}

fn le64(data: &[u8], at: usize, what: &'static str) -> Result<u64> {
    data.get(at..at.wrapping_add(8))
        .and_then(|b| b.try_into().ok())
        .map(u64::from_le_bytes)
        .ok_or(Error::new(ErrorKind::Truncated, at as u64, what))
}

fn bytes<'d>(data: &'d [u8], at: usize, len: usize, what: &'static str) -> Result<&'d [u8]> {
    at.checked_add(len)
        .and_then(|end| data.get(at..end))
        .ok_or(Error::new(ErrorKind::Truncated, at as u64, what))
}

fn to_usize(v: u64, what: &'static str) -> Result<usize> {
    usize::try_from(v).map_err(|_| Error::new(ErrorKind::Overflow, v, what))
}

/// The ZIP64 end of central directory record.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Zip64EndOfCentralDirectory<'d> {
    /// Where it is in the file.
    pub offset: u64,
    /// Its size field: the record's size less 12.
    pub record_size: u64,
    /// The version (and host) that made it.
    pub version_made_by: u16,
    /// The version needed to extract.
    pub version_needed: u16,
    /// This disk's number.
    pub disk: u32,
    /// The disk the central directory starts on.
    pub central_directory_disk: u32,
    /// Entries on this disk.
    pub disk_entries: u64,
    /// Entries in all.
    pub total_entries: u64,
    /// The central directory's size.
    pub central_directory_size: u64,
    /// Its offset (from the start of the archive's first disk).
    pub central_directory_offset: u64,
    /// The extensible data (version 2's encryption and compression of the
    /// central directory, or vendors').
    pub extensible_data: &'d [u8],
    /// The locator's count of disks.
    pub total_disks: u32,
}

/// The end of central directory record (with ZIP64's, when there's one).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct EndOfCentralDirectory<'d> {
    /// Where it is in the file.
    pub offset: u64,
    /// This disk's number.
    pub disk: u16,
    /// The disk the central directory starts on.
    pub central_directory_disk: u16,
    /// Entries on this disk (0xFFFF: see ZIP64's).
    pub disk_entries: u16,
    /// Entries in all (0xFFFF: see ZIP64's).
    pub total_entries: u16,
    /// The central directory's size (0xFFFFFFFF: see ZIP64's).
    pub central_directory_size: u32,
    /// Its offset (0xFFFFFFFF: see ZIP64's).
    pub central_directory_offset: u32,
    /// The archive's comment.
    pub comment: &'d [u8],
    /// The ZIP64 record, if there's a locator.
    pub zip64: Option<Zip64EndOfCentralDirectory<'d>>,
}

impl EndOfCentralDirectory<'_> {
    /// Entries in all (ZIP64's count if there is one).
    pub fn entries(&self) -> u64 {
        self.zip64
            .map_or(u64::from(self.total_entries), |z| z.total_entries)
    }

    /// The central directory's size.
    pub fn central_directory_size(&self) -> u64 {
        self.zip64
            .map_or(u64::from(self.central_directory_size), |z| {
                z.central_directory_size
            })
    }

    /// The central directory's recorded offset.
    pub fn central_directory_offset(&self) -> u64 {
        self.zip64
            .map_or(u64::from(self.central_directory_offset), |z| {
                z.central_directory_offset
            })
    }
}

/// An entry: its central directory record, with ZIP64's values applied.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Entry<'d> {
    /// Its index in the central directory.
    pub index: usize,
    /// Where its central directory record is in the file.
    pub central_offset: u64,
    /// The version (low byte, times ten) and host system (high byte) that
    /// made it.
    pub version_made_by: u16,
    /// The version needed to extract it.
    pub version_needed: u16,
    /// General-purpose flags ([`flags`]).
    pub flags: u16,
    /// The compression method ([`method`]).
    pub method: u16,
    /// The modification date and time.
    pub modified: DosDateTime,
    /// The CRC-32 of the uncompressed data.
    pub crc32: u32,
    /// The compressed size.
    pub compressed_size: u64,
    /// The uncompressed size.
    pub uncompressed_size: u64,
    /// The disk it starts on.
    pub disk_start: u32,
    /// Internal attributes (bit 0: text).
    pub internal_attributes: u16,
    /// External attributes: the host's (MS-DOS attributes in the low byte;
    /// a Unix mode in the high 16 bits).
    pub external_attributes: u32,
    /// Its local header's offset, as recorded.
    pub local_header_offset: u64,
    /// Its name, as recorded.
    pub raw_name: &'d [u8],
    /// Its extra fields' data.
    pub extra: &'d [u8],
    /// Its comment, as recorded.
    pub raw_comment: &'d [u8],
    /// Whether a ZIP64 extra field supplied any value.
    pub zip64: bool,
}

impl<'d> Entry<'d> {
    /// Its name: UTF-8 if flag 11 says so, else Info-ZIP's Unicode path
    /// (if its CRC matches the name), else code page 437.
    pub fn name(&self) -> Cow<'d, str> {
        decode_text(
            self.raw_name,
            self.flags,
            self.unicode_text(extra::UNICODE_PATH),
        )
    }

    /// Its comment, decoded as its name is.
    pub fn comment(&self) -> Cow<'d, str> {
        decode_text(
            self.raw_comment,
            self.flags,
            self.unicode_text(extra::UNICODE_COMMENT),
        )
    }

    fn unicode_text(&self, id: u16) -> Option<&'d [u8]> {
        let raw = if id == extra::UNICODE_PATH {
            self.raw_name
        } else {
            self.raw_comment
        };
        self.extra_fields().flatten().find_map(|f| match f {
            ExtraField::UnicodeText {
                id: i,
                version: 1,
                name_crc32,
                text,
            } if i == id && name_crc32 == crc32(raw) => Some(text),
            _ => None,
        })
    }

    /// Its extra fields.
    pub fn extra_fields(&self) -> ExtraFields<'d> {
        let base = self.central_offset + CENTRAL_HEADER_SIZE as u64 + self.raw_name.len() as u64;
        ExtraFields::new(self.extra, base)
    }

    /// The host system that made it (the high byte of "version made by").
    pub fn host_system(&self) -> u8 {
        (self.version_made_by >> 8) as u8
    }

    /// Whether it's a directory: its name ends with `/` (or, from MS-DOS
    /// and Windows hosts, the directory attribute is set).
    pub fn is_dir(&self) -> bool {
        self.raw_name.ends_with(b"/")
            || (matches!(self.host_system(), 0 | 10 | 14) && self.external_attributes & 0x10 != 0)
    }

    /// Whether it's encrypted (any kind: traditional, strong, or AES).
    pub fn is_encrypted(&self) -> bool {
        self.flags & (flags::ENCRYPTED | flags::STRONG_ENCRYPTION | flags::MASKED_HEADERS) != 0
            || self.method == method::AES
    }

    /// Whether a data descriptor follows its data.
    pub fn has_data_descriptor(&self) -> bool {
        self.flags & flags::DATA_DESCRIPTOR != 0
    }

    /// Its Unix mode (permissions and file type), if a Unix host made it
    /// and recorded one.
    pub fn unix_mode(&self) -> Option<u32> {
        let mode = self.external_attributes >> 16;
        (matches!(self.host_system(), 3 | 19) && mode != 0).then_some(mode)
    }

    /// Its modification time from an extended timestamp field (seconds
    /// since 1970, UTC), if it has one.
    pub fn unix_modified(&self) -> Option<i32> {
        self.extra_fields().flatten().find_map(|f| match f {
            ExtraField::ExtendedTimestamp { modified, .. } => modified,
            _ => None,
        })
    }

    /// Its compression method's name.
    pub fn method_name(&self) -> Cow<'static, str> {
        method_name(self.method)
    }
}

fn decode_text<'d>(raw: &'d [u8], flags: u16, unicode: Option<&'d [u8]>) -> Cow<'d, str> {
    if flags & flags::UTF8 != 0 {
        String::from_utf8_lossy(raw)
    } else if let Some(u) = unicode {
        String::from_utf8_lossy(u)
    } else {
        decode_cp437(raw)
    }
}

/// An entry's local header.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct LocalHeader<'d> {
    /// Where it is in the file.
    pub offset: u64,
    /// The version needed to extract.
    pub version_needed: u16,
    /// General-purpose flags.
    pub flags: u16,
    /// The compression method.
    pub method: u16,
    /// The modification date and time.
    pub modified: DosDateTime,
    /// The CRC-32 (zero with a data descriptor).
    pub crc32: u32,
    /// The compressed size (zero with a data descriptor; ZIP64's if the
    /// field is all ones and there's a ZIP64 extra field).
    pub compressed_size: u64,
    /// The uncompressed size (likewise).
    pub uncompressed_size: u64,
    /// The name.
    pub raw_name: &'d [u8],
    /// The extra fields' data.
    pub extra: &'d [u8],
    /// Where the entry's data starts.
    pub data_offset: u64,
}

impl<'d> LocalHeader<'d> {
    /// Its extra fields.
    pub fn extra_fields(&self) -> ExtraFields<'d> {
        ExtraFields::new(
            self.extra,
            self.offset + LOCAL_HEADER_SIZE as u64 + self.raw_name.len() as u64,
        )
    }

    /// Whether it has a ZIP64 extra field (so that its data descriptor's
    /// sizes are eight bytes each).
    pub fn has_zip64(&self) -> bool {
        ExtraFields::find(self.extra, extra::ZIP64).is_some()
    }
}

/// A data descriptor: the CRC-32 and sizes, after the data.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct DataDescriptor {
    /// Where it is in the file.
    pub offset: u64,
    /// Whether it starts with its (optional) signature.
    pub signed: bool,
    /// Whether its sizes are eight bytes each (ZIP64).
    pub zip64: bool,
    /// The CRC-32.
    pub crc32: u32,
    /// The compressed size.
    pub compressed_size: u64,
    /// The uncompressed size.
    pub uncompressed_size: u64,
}

impl DataDescriptor {
    /// Its size in bytes.
    pub fn size(&self) -> u64 {
        u64::from(self.signed) * 4 + 4 + if self.zip64 { 16 } else { 8 }
    }
}

/// Where an entry's parts lie in the file.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EntryExtent {
    /// The local header (with its name and extra fields).
    pub local_header: Range<u64>,
    /// The (compressed) data.
    pub data: Range<u64>,
    /// The data descriptor, if there is one.
    pub descriptor: Option<Range<u64>>,
}

impl EntryExtent {
    /// From the local header's start to the end of the descriptor (or
    /// data).
    pub fn whole(&self) -> Range<u64> {
        self.local_header.start..self.descriptor.as_ref().map_or(self.data.end, |d| d.end)
    }
}

/// A ZIP archive: its end of central directory record and entries.
#[derive(Debug)]
pub struct Archive<'d> {
    data: &'d [u8],
    base: u64,
    end: EndOfCentralDirectory<'d>,
    central_directory: Range<u64>,
    entries: Vec<Entry<'d>>,
    by_name: OnceLock<Vec<u32>>,
}

/// How far from the end the end of central directory record can be: its
/// size, and the longest comment.
const MAX_EOCD_SEARCH: usize = END_OF_CENTRAL_DIRECTORY_SIZE + 0xFFFF;

/// Whether `data` looks like a ZIP archive: it starts with a local header
/// (or is an empty archive's end record), or has an end of central
/// directory record in its last 64 KiB.
pub fn is_zip(data: &[u8]) -> bool {
    data.starts_with(b"PK\x03\x04")
        || data.starts_with(b"PK\x05\x06")
        || find_end_record(data).is_some()
}

/// The offset of the end of central directory record: the last signature
/// whose comment ends exactly at the end of the data, else the last whose
/// comment fits (trailing bytes after the archive).
fn find_end_record(data: &[u8]) -> Option<usize> {
    if data.len() < END_OF_CENTRAL_DIRECTORY_SIZE {
        return None;
    }
    let last = data.len() - END_OF_CENTRAL_DIRECTORY_SIZE;
    let first = data.len().saturating_sub(MAX_EOCD_SEARCH);
    let sig = END_OF_CENTRAL_DIRECTORY_SIGNATURE.to_le_bytes();
    let mut fallback = None;
    for at in (first..=last).rev() {
        if data[at..at + 4] != sig {
            continue;
        }
        let comment = usize::from(u16::from_le_bytes([data[at + 20], data[at + 21]]));
        let end = at + END_OF_CENTRAL_DIRECTORY_SIZE + comment;
        if end == data.len() {
            return Some(at);
        }
        if end < data.len() && fallback.is_none() {
            fallback = Some(at);
        }
    }
    fallback
}

impl<'d> Archive<'d> {
    /// Reads the archive in `data`: its end of central directory record
    /// and every central directory entry.
    pub fn parse(data: &'d [u8]) -> Result<Self> {
        let eocd_at = find_end_record(data).ok_or(Error::new(
            ErrorKind::BadMagic,
            data.len() as u64,
            "end of central directory record",
        ))?;
        let what = "end of central directory record";
        let disk = le16(data, eocd_at + 4, what)?;
        let central_directory_disk = le16(data, eocd_at + 6, what)?;
        let disk_entries = le16(data, eocd_at + 8, what)?;
        let total_entries = le16(data, eocd_at + 10, what)?;
        let cd_size32 = le32(data, eocd_at + 12, what)?;
        let cd_offset32 = le32(data, eocd_at + 16, what)?;
        let comment_len = usize::from(le16(data, eocd_at + 20, what)?);
        let comment = bytes(
            data,
            eocd_at + END_OF_CENTRAL_DIRECTORY_SIZE,
            comment_len,
            "archive comment",
        )?;
        let mut end = EndOfCentralDirectory {
            offset: eocd_at as u64,
            disk,
            central_directory_disk,
            disk_entries,
            total_entries,
            central_directory_size: cd_size32,
            central_directory_offset: cd_offset32,
            comment,
            zip64: None,
        };
        // ZIP64: a locator just before the end record.
        let mut cd_end = eocd_at as u64;
        if let Some(loc_at) = eocd_at.checked_sub(ZIP64_LOCATOR_SIZE)
            && le32(data, loc_at, "ZIP64 locator")? == ZIP64_LOCATOR_SIGNATURE
        {
            let z = Self::parse_zip64(data, loc_at)?;
            cd_end = z.offset;
            end.zip64 = Some(z);
        }
        let multi = match end.zip64 {
            Some(z) => {
                z.disk != 0
                    || z.central_directory_disk != 0
                    || z.disk_entries != z.total_entries
                    || z.total_disks > 1
            }
            None => disk != 0 || central_directory_disk != 0 || disk_entries != total_entries,
        };
        if multi {
            return Err(Error::new(
                ErrorKind::Unsupported,
                eocd_at as u64,
                "multi-disk (split or spanned) archive",
            ));
        }
        let cd_size = end.central_directory_size();
        let recorded = end.central_directory_offset();
        // Where the central directory is: as recorded, or (data before the
        // archive) just before the end records.
        let actual = cd_end.checked_sub(cd_size).ok_or(Error::new(
            ErrorKind::Malformed,
            eocd_at as u64,
            "central directory size",
        ))?;
        let sig_at = |at: u64| {
            usize::try_from(at)
                .ok()
                .and_then(|a| le32(data, a, "").ok())
        };
        let cd_signature = |at: u64| {
            cd_size == 0
                || matches!(
                    sig_at(at),
                    Some(CENTRAL_HEADER_SIGNATURE | DIGITAL_SIGNATURE_SIGNATURE)
                )
        };
        let base = if recorded.checked_add(cd_size).is_some_and(|e| e <= cd_end)
            && cd_signature(recorded)
        {
            0
        } else if actual >= recorded && cd_signature(actual) {
            actual - recorded
        } else if sig_at(actual) == Some(ARCHIVE_EXTRA_DATA_SIGNATURE)
            || sig_at(recorded) == Some(ARCHIVE_EXTRA_DATA_SIGNATURE)
        {
            return Err(Error::new(
                ErrorKind::Unsupported,
                actual,
                "encrypted central directory",
            ));
        } else {
            return Err(Error::new(
                ErrorKind::Malformed,
                recorded,
                "central directory offset",
            ));
        };
        let cd_start = recorded + base;
        let central_directory = cd_start..cd_start + cd_size;
        let entries = Self::parse_entries(data, central_directory.clone(), end.entries())?;
        Ok(Self {
            data,
            base,
            end,
            central_directory,
            entries,
            by_name: OnceLock::new(),
        })
    }

    fn parse_zip64(data: &'d [u8], loc_at: usize) -> Result<Zip64EndOfCentralDirectory<'d>> {
        let what = "ZIP64 locator";
        let record_disk = le32(data, loc_at + 4, what)?;
        let recorded = le64(data, loc_at + 8, what)?;
        let total_disks = le32(data, loc_at + 16, what)?;
        if record_disk != 0 {
            return Err(Error::new(
                ErrorKind::Unsupported,
                loc_at as u64,
                "multi-disk (split or spanned) archive",
            ));
        }
        // As recorded, or (data before the archive) just before the
        // locator, with no extensible data.
        let is_record = |at: u64| {
            usize::try_from(at).is_ok_and(|a| {
                a < loc_at
                    && le32(data, a, "").ok() == Some(ZIP64_END_OF_CENTRAL_DIRECTORY_SIGNATURE)
            })
        };
        let at = if is_record(recorded) {
            recorded
        } else {
            let guess = (loc_at as u64).saturating_sub(ZIP64_END_OF_CENTRAL_DIRECTORY_SIZE as u64);
            if !is_record(guess) {
                return Err(Error::new(
                    ErrorKind::Malformed,
                    recorded,
                    "ZIP64 end of central directory record",
                ));
            }
            guess
        };
        let a = to_usize(at, "ZIP64 end of central directory record")?;
        let what = "ZIP64 end of central directory record";
        let record_size = le64(data, a + 4, what)?;
        let ext_len = record_size
            .checked_sub(ZIP64_END_OF_CENTRAL_DIRECTORY_SIZE as u64 - 12)
            .ok_or(Error::new(ErrorKind::Malformed, at + 4, what))?;
        let extensible_data = bytes(
            data,
            a + ZIP64_END_OF_CENTRAL_DIRECTORY_SIZE,
            to_usize(ext_len, what)?,
            "ZIP64 extensible data",
        )?;
        Ok(Zip64EndOfCentralDirectory {
            offset: at,
            record_size,
            version_made_by: le16(data, a + 12, what)?,
            version_needed: le16(data, a + 14, what)?,
            disk: le32(data, a + 16, what)?,
            central_directory_disk: le32(data, a + 20, what)?,
            disk_entries: le64(data, a + 24, what)?,
            total_entries: le64(data, a + 32, what)?,
            central_directory_size: le64(data, a + 40, what)?,
            central_directory_offset: le64(data, a + 48, what)?,
            extensible_data,
            total_disks,
        })
    }

    fn parse_entries(data: &'d [u8], cd: Range<u64>, declared: u64) -> Result<Vec<Entry<'d>>> {
        let start = to_usize(cd.start, "central directory")?;
        let end = to_usize(cd.end, "central directory")?;
        if end > data.len() {
            return Err(Error::new(
                ErrorKind::Truncated,
                cd.start,
                "central directory",
            ));
        }
        let mut entries = Vec::with_capacity(
            usize::try_from(declared)
                .unwrap_or(usize::MAX)
                .min((end - start) / CENTRAL_HEADER_SIZE),
        );
        let mut at = start;
        while at < end {
            let sig = le32(data, at, "central directory header")?;
            if sig == DIGITAL_SIGNATURE_SIGNATURE {
                // The central directory's signature record ends it.
                break;
            }
            if sig != CENTRAL_HEADER_SIGNATURE {
                return Err(Error::new(
                    ErrorKind::BadMagic,
                    at as u64,
                    "central directory header",
                ));
            }
            let (entry, next) = Self::parse_entry(data, at, entries.len())?;
            if next > end {
                return Err(Error::new(
                    ErrorKind::Truncated,
                    at as u64,
                    "central directory header past the central directory",
                ));
            }
            entries.push(entry);
            at = next;
        }
        // (Writers that don't do ZIP64 wrap counts past 65535.)
        let n = entries.len() as u64;
        if n != declared && n & 0xFFFF != declared {
            return Err(Error::new(
                ErrorKind::Malformed,
                cd.start,
                "central directory entry count",
            ));
        }
        Ok(entries)
    }

    fn parse_entry(data: &'d [u8], at: usize, index: usize) -> Result<(Entry<'d>, usize)> {
        let what = "central directory header";
        let h = bytes(data, at, CENTRAL_HEADER_SIZE, what)?;
        let u16_at = |o: usize| u16::from_le_bytes([h[o], h[o + 1]]);
        let u32_at = |o: usize| u32::from_le_bytes([h[o], h[o + 1], h[o + 2], h[o + 3]]);
        let name_len = usize::from(u16_at(28));
        let extra_len = usize::from(u16_at(30));
        let comment_len = usize::from(u16_at(32));
        let mut p = at + CENTRAL_HEADER_SIZE;
        let raw_name = bytes(data, p, name_len, "entry name")?;
        p += name_len;
        let extra = bytes(data, p, extra_len, "entry extra fields")?;
        p += extra_len;
        let raw_comment = bytes(data, p, comment_len, "entry comment")?;
        p += comment_len;
        let csize = u32_at(20);
        let usize_ = u32_at(24);
        let disk = u16_at(34);
        let offset = u32_at(42);
        let need = [
            usize_ == u32::MAX,
            csize == u32::MAX,
            offset == u32::MAX,
            disk == u16::MAX,
        ];
        let mut zip64 = false;
        let mut values = Zip64Values::default();
        if need.iter().any(|&n| n) {
            let field = ExtraFields::find(extra, extra::ZIP64);
            if let Some(f) = field {
                values = Zip64Values::parse(f, need).ok_or(Error::new(
                    ErrorKind::Truncated,
                    (at + CENTRAL_HEADER_SIZE + name_len) as u64,
                    "ZIP64 extra field",
                ))?;
                zip64 = true;
            }
        }
        let entry = Entry {
            index,
            central_offset: at as u64,
            version_made_by: u16_at(4),
            version_needed: u16_at(6),
            flags: u16_at(8),
            method: u16_at(10),
            modified: DosDateTime {
                time: u16_at(12),
                date: u16_at(14),
            },
            crc32: u32_at(16),
            compressed_size: values.compressed_size.unwrap_or(u64::from(csize)),
            uncompressed_size: values.uncompressed_size.unwrap_or(u64::from(usize_)),
            disk_start: values.disk_start.unwrap_or(u32::from(disk)),
            internal_attributes: u16_at(36),
            external_attributes: u32_at(38),
            local_header_offset: values.local_header_offset.unwrap_or(u64::from(offset)),
            raw_name,
            extra,
            raw_comment,
            zip64,
        };
        Ok((entry, p))
    }

    /// The archive's bytes (all of `data`, with whatever precedes the
    /// archive).
    pub fn data(&self) -> &'d [u8] {
        self.data
    }

    /// How many bytes precede the archive (a self-extractor's program, a
    /// JMOD's header): added to every recorded offset.
    pub fn base(&self) -> u64 {
        self.base
    }

    /// The end of central directory record.
    pub fn end_of_central_directory(&self) -> &EndOfCentralDirectory<'d> {
        &self.end
    }

    /// The archive's comment.
    pub fn comment(&self) -> &'d [u8] {
        self.end.comment
    }

    /// Where the central directory is in the file.
    pub fn central_directory(&self) -> Range<u64> {
        self.central_directory.clone()
    }

    /// Every entry, in central directory order.
    pub fn entries(&self) -> &[Entry<'d>] {
        &self.entries
    }

    /// How many entries there are.
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// Whether there are none.
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// The entry named `name` (the first, if several are: the central
    /// directory's order).
    pub fn find(&self, name: &str) -> Option<&Entry<'d>> {
        let index = self.by_name.get_or_init(|| {
            let mut v: Vec<u32> =
                (0..u32::try_from(self.entries.len()).unwrap_or(u32::MAX)).collect();
            let names: Vec<Cow<'d, str>> = self.entries.iter().map(Entry::name).collect();
            v.sort_by(|&a, &b| names[a as usize].cmp(&names[b as usize]));
            v
        });
        let at = index.partition_point(|&i| *self.entries[i as usize].name() < *name);
        let &i = index.get(at)?;
        let e = &self.entries[i as usize];
        (e.name() == name).then_some(e)
    }

    fn file_offset(&self, recorded: u64, what: &'static str) -> Result<usize> {
        recorded
            .checked_add(self.base)
            .and_then(|o| usize::try_from(o).ok())
            .filter(|&o| o <= self.data.len())
            .ok_or(Error::new(ErrorKind::Truncated, recorded, what))
    }

    /// `entry`'s local header.
    pub fn local_header(&self, entry: &Entry<'d>) -> Result<LocalHeader<'d>> {
        let at = self.file_offset(entry.local_header_offset, "local header")?;
        let data = self.data;
        let what = "local header";
        let h = bytes(data, at, LOCAL_HEADER_SIZE, what)?;
        let u16_at = |o: usize| u16::from_le_bytes([h[o], h[o + 1]]);
        let u32_at = |o: usize| u32::from_le_bytes([h[o], h[o + 1], h[o + 2], h[o + 3]]);
        if u32_at(0) != LOCAL_HEADER_SIGNATURE {
            return Err(Error::new(ErrorKind::BadMagic, at as u64, what));
        }
        let name_len = usize::from(u16_at(26));
        let extra_len = usize::from(u16_at(28));
        let raw_name = bytes(data, at + LOCAL_HEADER_SIZE, name_len, "local header name")?;
        let extra = bytes(
            data,
            at + LOCAL_HEADER_SIZE + name_len,
            extra_len,
            "local header extra fields",
        )?;
        let (csize, usize_) = (u32_at(18), u32_at(22));
        let mut compressed_size = u64::from(csize);
        let mut uncompressed_size = u64::from(usize_);
        if (csize == u32::MAX || usize_ == u32::MAX)
            && let Some(f) = ExtraFields::find(extra, extra::ZIP64)
        {
            // (A local header's ZIP64 field has both sizes.)
            if let Some(v) = Zip64Values::parse(f, [true, true, false, false]) {
                compressed_size = v.compressed_size.unwrap_or(compressed_size);
                uncompressed_size = v.uncompressed_size.unwrap_or(uncompressed_size);
            }
        }
        Ok(LocalHeader {
            offset: at as u64,
            version_needed: u16_at(4),
            flags: u16_at(6),
            method: u16_at(8),
            modified: DosDateTime {
                time: u16_at(10),
                date: u16_at(12),
            },
            crc32: u32_at(14),
            compressed_size,
            uncompressed_size,
            raw_name,
            extra,
            data_offset: (at + LOCAL_HEADER_SIZE + name_len + extra_len) as u64,
        })
    }

    /// `entry`'s compressed data (its size the central directory's).
    pub fn raw_data(&self, entry: &Entry<'d>) -> Result<&'d [u8]> {
        let h = self.local_header(entry)?;
        self.raw_data_after(&h, entry)
    }

    fn raw_data_after(&self, h: &LocalHeader<'d>, entry: &Entry<'d>) -> Result<&'d [u8]> {
        let at = to_usize(h.data_offset, "entry data")?;
        let len = to_usize(entry.compressed_size, "entry data")?;
        bytes(self.data, at, len, "entry data")
    }

    /// `entry`'s data descriptor, if its flags say it has one: after the
    /// data, with or without its signature, its sizes four bytes each or
    /// (ZIP64) eight; the layout whose values match the central
    /// directory's is taken.
    pub fn data_descriptor(&self, entry: &Entry<'d>) -> Result<Option<DataDescriptor>> {
        if !entry.has_data_descriptor() {
            return Ok(None);
        }
        let h = self.local_header(entry)?;
        let at = h
            .data_offset
            .checked_add(entry.compressed_size)
            .ok_or(Error::new(
                ErrorKind::Overflow,
                h.data_offset,
                "data descriptor",
            ))?;
        let a = to_usize(at, "data descriptor")?;
        let data = self.data;
        let signed = le32(data, a, "data descriptor").ok() == Some(DATA_DESCRIPTOR_SIGNATURE);
        let read = |signed: bool, zip64: bool| -> Option<DataDescriptor> {
            let p = a + if signed { 4 } else { 0 };
            let crc = le32(data, p, "").ok()?;
            let (c, u) = if zip64 {
                (le64(data, p + 4, "").ok()?, le64(data, p + 12, "").ok()?)
            } else {
                (
                    u64::from(le32(data, p + 4, "").ok()?),
                    u64::from(le32(data, p + 8, "").ok()?),
                )
            };
            Some(DataDescriptor {
                offset: at,
                signed,
                zip64,
                crc32: crc,
                compressed_size: c,
                uncompressed_size: u,
            })
        };
        let matches = |d: &DataDescriptor| {
            d.crc32 == entry.crc32
                && d.compressed_size == entry.compressed_size
                && d.uncompressed_size == entry.uncompressed_size
        };
        // The likeliest layouts first: ZIP64's if the local header has the
        // field; signed if the signature is there (a CRC can match it).
        let wide = h.has_zip64();
        let mut layouts = vec![(signed, wide), (signed, !wide)];
        if signed {
            layouts.extend([(false, wide), (false, !wide)]);
        }
        let candidates: Vec<DataDescriptor> = layouts
            .into_iter()
            .filter_map(|(s, z)| read(s, z))
            .collect();
        match candidates.iter().find(|d| matches(d)) {
            Some(d) => Ok(Some(*d)),
            None => match candidates.first() {
                // (One that disagrees is still the descriptor.)
                Some(d) => Ok(Some(*d)),
                None => Err(Error::new(ErrorKind::Truncated, at, "data descriptor")),
            },
        }
    }

    /// Where `entry`'s local header, data, and data descriptor lie.
    pub fn extent(&self, entry: &Entry<'d>) -> Result<EntryExtent> {
        let h = self.local_header(entry)?;
        let data = h.data_offset
            ..h.data_offset
                .checked_add(entry.compressed_size)
                .filter(|&e| e <= self.data.len() as u64)
                .ok_or(Error::new(
                    ErrorKind::Truncated,
                    h.data_offset,
                    "entry data",
                ))?;
        let descriptor = self
            .data_descriptor(entry)?
            .map(|d| d.offset..d.offset + d.size());
        Ok(EntryExtent {
            local_header: h.offset..h.data_offset,
            data,
            descriptor,
        })
    }

    /// `entry`'s data, decompressed (borrowed if it's stored), with its
    /// size and CRC-32 checked against the central directory's.
    pub fn read(&self, entry: &Entry<'d>) -> Result<Cow<'d, [u8]>> {
        let at = entry.central_offset;
        if entry.is_encrypted() {
            return Err(Error::new(ErrorKind::Unsupported, at, "encrypted entry"));
        }
        let h = self.local_header(entry)?;
        let raw = self.raw_data_after(&h, entry)?;
        let out: Cow<'d, [u8]> = match entry.method {
            method::STORED => {
                if entry.compressed_size != entry.uncompressed_size {
                    return Err(Error::new(
                        ErrorKind::Malformed,
                        at,
                        "stored entry's sizes differ",
                    ));
                }
                Cow::Borrowed(raw)
            }
            method::DEFLATED => Cow::Owned(inflate(raw, entry.uncompressed_size, h.data_offset)?),
            _ => {
                return Err(Error::new(
                    ErrorKind::Unsupported,
                    at,
                    "compression method (only stored and deflated are read)",
                ));
            }
        };
        if crc32(&out) != entry.crc32 {
            return Err(Error::new(ErrorKind::Malformed, at, "CRC-32 mismatch"));
        }
        Ok(out)
    }

    /// The entry named `name`'s data, if there's such an entry.
    pub fn read_by_name(&self, name: &str) -> Option<Result<Cow<'d, [u8]>>> {
        self.find(name).map(|e| self.read(e))
    }

    /// `f` of every entry, in order, on every core (the largest entries
    /// started first).
    pub fn par_map<R: Send>(&self, f: impl Fn(&Entry<'d>) -> R + Sync) -> Vec<R>
    where
        Self: Sync,
    {
        let entries = &self.entries;
        turbonop_par::map_costed(
            entries.len(),
            |i| entries[i].uncompressed_size.max(entries[i].compressed_size) + 64,
            |i| f(&entries[i]),
        )
    }

    /// Every entry's data ([`read`](Self::read)), in order, on every core.
    pub fn read_all(&self) -> Vec<Result<Cow<'d, [u8]>>>
    where
        Self: Sync,
    {
        self.par_map(|e| self.read(e))
    }
}

/// Deflate's best ratio is 1032 to 1 (a 258-byte match in a bit or two):
/// an uncompressed size beyond it is damage, not data (and not worth
/// allocating).
const MAX_DEFLATE_RATIO: u64 = 1032;

/// `packed` (raw deflate) inflated to exactly `size` bytes.
fn inflate(packed: &[u8], size: u64, offset: u64) -> Result<Vec<u8>> {
    use miniz_oxide::inflate::TINFLStatus;
    use miniz_oxide::inflate::core::{DecompressorOxide, decompress, inflate_flags};
    if size > (packed.len() as u64).saturating_mul(MAX_DEFLATE_RATIO) + 1024 {
        return Err(Error::new(
            ErrorKind::Malformed,
            offset,
            "uncompressed size beyond deflate's ratio",
        ));
    }
    let mut out = vec![0u8; to_usize(size, "uncompressed size")?];
    let mut state = Box::new(DecompressorOxide::new());
    let (status, _, written) = decompress(
        &mut state,
        packed,
        &mut out,
        0,
        inflate_flags::TINFL_FLAG_USING_NON_WRAPPING_OUTPUT_BUF,
    );
    match status {
        TINFLStatus::Done if written as u64 == size => Ok(out),
        TINFLStatus::Done => Err(Error::new(
            ErrorKind::Malformed,
            offset,
            "deflated data shorter than its size",
        )),
        TINFLStatus::HasMoreOutput => Err(Error::new(
            ErrorKind::Malformed,
            offset,
            "deflated data longer than its size",
        )),
        TINFLStatus::NeedsMoreInput | TINFLStatus::FailedCannotMakeProgress => {
            Err(Error::new(ErrorKind::Truncated, offset, "deflated data"))
        }
        _ => Err(Error::new(ErrorKind::Malformed, offset, "deflated data")),
    }
}

#[cfg(test)]
mod tests;
