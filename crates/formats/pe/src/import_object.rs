//! Short import objects (`IMPORT_OBJECT_HEADER`), the members of import
//! libraries: each describes one function or variable a DLL exports,
//! from which the linker synthesizes the import thunk and descriptors.
//!
//! The header is 20 bytes: `Sig1` (0), `Sig2` (0xFFFF), `Version` (0),
//! `Machine`, `TimeDateStamp`, `SizeOfData` (the names' size),
//! `OrdinalOrHint`, then 16 bits holding the [`ImportType`] (2 bits), the
//! [`ImportNameType`] (3 bits), and 11 reserved bits. The names follow,
//! each NUL-terminated: the symbol, the DLL, and (for
//! [`ImportNameType::ExportAs`], and in `lib.exe`'s ARM64EC imports by
//! ordinal) the name to import.
//!
//! What `lib /DEF` (MSVC 14.52) and `llvm-lib /DEF` (LLVM 24) put in them:
//!
//! - `TimeDateStamp`: `lib.exe`, the time it ran; with `/Brepro`, a value
//!   that differs per object (some hash; not reproduced here).
//!   `llvm-lib`: 0.
//! - `OrdinalOrHint`: for an import by ordinal, the ordinal. `lib.exe`
//!   imports by ordinal every export given one (`@n`), and gives each
//!   other its position among the named exports (all but `NONAME` ones,
//!   `PRIVATE` ones included) sorted by the names they're exported as
//!   (see `turbonop-moduledef`'s `imports`). `llvm-lib` imports by name
//!   an export given an ordinal without `NONAME`, with the ordinal as its
//!   hint, and gives the others hint 0.
//! - The name type: by name, except on x86, where the symbol is the name
//!   with `_` in front (unless it starts with `?` or `@`), imported without
//!   that prefix; and for ARM64EC code, the mangled symbol (`#f`,
//!   `?f@@$$h...`) with the plain name to import after the DLL's.

use alloc::vec::Vec;

use turbonop_core::{Error, ErrorKind, Result};

/// The size of the header.
pub const HEADER_SIZE: usize = 20;

/// What an import is (`IMPORT_OBJECT_TYPE`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum ImportType {
    Code,
    Data,
    Const,
    Unknown(u8),
}

impl ImportType {
    /// Its 2-bit value.
    pub fn bits(self) -> u16 {
        match self {
            ImportType::Code => 0,
            ImportType::Data => 1,
            ImportType::Const => 2,
            ImportType::Unknown(k) => u16::from(k & 3),
        }
    }
}

/// How the import's name is found (`IMPORT_OBJECT_NAME_TYPE`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum ImportNameType {
    /// By ordinal (`ordinal_or_hint` is the ordinal).
    Ordinal,
    /// The symbol's name as is.
    Name,
    /// The symbol's name without a leading `?`, `@`, or `_`.
    NoPrefix,
    /// As `NoPrefix`, and cut at the first `@`.
    Undecorate,
    /// The name given after the DLL's name.
    ExportAs,
    Unknown(u8),
}

impl ImportNameType {
    /// Its 3-bit value.
    pub fn bits(self) -> u16 {
        match self {
            ImportNameType::Ordinal => 0,
            ImportNameType::Name => 1,
            ImportNameType::NoPrefix => 2,
            ImportNameType::Undecorate => 3,
            ImportNameType::ExportAs => 4,
            ImportNameType::Unknown(k) => u16::from(k & 7),
        }
    }
}

/// A short import object.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ImportObject<'data> {
    pub version: u16,
    pub machine: u16,
    pub time_date_stamp: u32,
    /// The size of the data after the header (the names).
    pub size_of_data: u32,
    pub ordinal_or_hint: u16,
    pub kind: ImportType,
    pub name_type: ImportNameType,
    /// The 11 bits after the name type (0 in every import seen).
    pub reserved: u16,
    /// The public symbol the import defines (`__imp_` is added for the
    /// import address table entry).
    pub symbol: &'data [u8],
    pub dll: &'data [u8],
    /// A third name, after the DLL's: for [`ImportNameType::ExportAs`],
    /// the name to import.
    pub export_name: Option<&'data [u8]>,
}

/// Why an import object can't be written.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ImportWriteError {
    /// A name holds a NUL.
    NulInName,
    /// The names are too long for `SizeOfData`.
    TooLarge,
}

impl core::fmt::Display for ImportWriteError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.write_str(match self {
            ImportWriteError::NulInName => "a name holds a NUL",
            ImportWriteError::TooLarge => "the names are too long",
        })
    }
}

impl core::error::Error for ImportWriteError {}

impl<'data> ImportObject<'data> {
    /// Whether `data` starts with an import object's signature (an
    /// anonymous object header of version 0).
    pub fn is_import_object(data: &[u8]) -> bool {
        data.len() >= 6 && data[0..4] == [0, 0, 0xFF, 0xFF] && data[4..6] == [0, 0]
    }

    pub fn parse(data: &'data [u8]) -> Result<Self> {
        if data.len() < HEADER_SIZE || !Self::is_import_object(data) {
            return Err(Error::new(ErrorKind::BadMagic, 0, "import object header"));
        }
        let u16_at = |o: usize| u16::from_le_bytes([data[o], data[o + 1]]);
        let u32_at =
            |o: usize| u32::from_le_bytes([data[o], data[o + 1], data[o + 2], data[o + 3]]);
        let size_of_data = u32_at(12);
        let bits = u16_at(18);
        let kind = match (bits & 3) as u8 {
            0 => ImportType::Code,
            1 => ImportType::Data,
            2 => ImportType::Const,
            k => ImportType::Unknown(k),
        };
        let name_type = match ((bits >> 2) & 7) as u8 {
            0 => ImportNameType::Ordinal,
            1 => ImportNameType::Name,
            2 => ImportNameType::NoPrefix,
            3 => ImportNameType::Undecorate,
            4 => ImportNameType::ExportAs,
            k => ImportNameType::Unknown(k),
        };
        let end = HEADER_SIZE
            .saturating_add(usize::try_from(size_of_data).unwrap_or(usize::MAX))
            .min(data.len());
        // Each name ends with a NUL (or, damaged, with the data).
        let mut rest = &data[HEADER_SIZE..end];
        let mut next = || {
            if rest.is_empty() {
                return None;
            }
            let at = rest.iter().position(|&b| b == 0).unwrap_or(rest.len());
            let name = &rest[..at];
            rest = rest.get(at + 1..).unwrap_or(&[]);
            Some(name)
        };
        let symbol = next().unwrap_or(&[]);
        let dll = next().ok_or(Error::new(
            ErrorKind::Truncated,
            HEADER_SIZE as u64,
            "import object names",
        ))?;
        let export_name = next();
        Ok(Self {
            version: u16_at(4),
            machine: u16_at(6),
            time_date_stamp: u32_at(8),
            size_of_data,
            ordinal_or_hint: u16_at(16),
            kind,
            name_type,
            reserved: bits >> 5,
            symbol,
            dll,
            export_name,
        })
    }

    /// A new import (version 0, time stamp 0, no reserved bits) of
    /// `symbol` from `dll`.
    pub fn new(
        machine: u16,
        kind: ImportType,
        name_type: ImportNameType,
        ordinal_or_hint: u16,
        symbol: &'data [u8],
        dll: &'data [u8],
        export_name: Option<&'data [u8]>,
    ) -> Self {
        let mut i = Self {
            version: 0,
            machine,
            time_date_stamp: 0,
            size_of_data: 0,
            ordinal_or_hint,
            kind,
            name_type,
            reserved: 0,
            symbol,
            dll,
            export_name,
        };
        i.size_of_data = u32::try_from(i.names_len()).unwrap_or(u32::MAX);
        i
    }

    /// The names' size, each with its NUL.
    pub fn names_len(&self) -> usize {
        self.symbol.len() + self.dll.len() + 2 + self.export_name.map_or(0, |n| n.len() + 1)
    }

    /// The object's bytes, as [`write_into`](Self::write_into) writes them.
    pub fn to_vec(&self) -> core::result::Result<Vec<u8>, ImportWriteError> {
        let mut out = Vec::with_capacity(HEADER_SIZE + self.names_len());
        self.write_into(&mut out)?;
        Ok(out)
    }

    /// Appends the object to `out`: the header (every field as it is here,
    /// but `SizeOfData`, which is the names' size), then the names.
    pub fn write_into(&self, out: &mut Vec<u8>) -> core::result::Result<(), ImportWriteError> {
        let names = [Some(self.symbol), Some(self.dll), self.export_name];
        if names.iter().flatten().any(|n| n.contains(&0)) {
            return Err(ImportWriteError::NulInName);
        }
        let size = u32::try_from(self.names_len()).map_err(|_| ImportWriteError::TooLarge)?;
        out.extend_from_slice(&[0, 0, 0xFF, 0xFF]);
        out.extend_from_slice(&self.version.to_le_bytes());
        out.extend_from_slice(&self.machine.to_le_bytes());
        out.extend_from_slice(&self.time_date_stamp.to_le_bytes());
        out.extend_from_slice(&size.to_le_bytes());
        out.extend_from_slice(&self.ordinal_or_hint.to_le_bytes());
        let bits = self.kind.bits() | (self.name_type.bits() << 2) | (self.reserved << 5);
        out.extend_from_slice(&bits.to_le_bytes());
        for n in names.into_iter().flatten() {
            out.extend_from_slice(n);
            out.push(0);
        }
        Ok(())
    }

    /// The name the DLL exports, as the loader looks it up; `None` for
    /// imports by ordinal.
    pub fn import_name(&self) -> Option<&'data [u8]> {
        let strip = |n: &'data [u8]| match n.first() {
            Some(b'?' | b'@' | b'_') => &n[1..],
            _ => n,
        };
        Some(match self.name_type {
            ImportNameType::Ordinal => return None,
            ImportNameType::Name | ImportNameType::Unknown(_) => self.symbol,
            ImportNameType::NoPrefix => strip(self.symbol),
            ImportNameType::Undecorate => {
                let n = strip(self.symbol);
                n.split(|&b| b == b'@').next().unwrap_or(n)
            }
            ImportNameType::ExportAs => self.export_name.unwrap_or(self.symbol),
        })
    }

    /// The object's size: the header and its names.
    pub fn size(&self) -> u64 {
        HEADER_SIZE as u64 + u64::from(self.size_of_data)
    }
}

#[cfg(test)]
mod tests;
