//! The CLI header (`IMAGE_COR20_HEADER`, II.25.3.3): the runtime version,
//! where the metadata is, the flags, the entry point, and the other
//! directories (resources, the strong-name signature, v-table fixups, the
//! ReadyToRun header).

use turbonop_core::{ErrorKind, Result};

use crate::reader::Cursor;

/// The header's size.
pub const COR20_HEADER_SIZE: u32 = 72;

/// `COMIMAGE_FLAGS_*`.
pub mod flags {
    /// IL only.
    pub const ILONLY: u32 = 0x0000_0001;
    /// Must run in a 32-bit process.
    pub const BIT32_REQUIRED: u32 = 0x0000_0002;
    /// (Obsolete.) An IL library.
    pub const IL_LIBRARY: u32 = 0x0000_0004;
    /// Strong-name signed.
    pub const STRONG_NAME_SIGNED: u32 = 0x0000_0008;
    /// The entry point is native (an RVA, not a token).
    pub const NATIVE_ENTRYPOINT: u32 = 0x0000_0010;
    /// Track debug data.
    pub const TRACK_DEBUG_DATA: u32 = 0x0001_0000;
    /// Prefers a 32-bit process.
    pub const BIT32_PREFERRED: u32 = 0x0002_0000;
}

/// The flags' names.
pub const FLAG_NAMES: &[(u32, &str)] = &[
    (flags::ILONLY, "IL only"),
    (flags::BIT32_REQUIRED, "32-bit required"),
    (flags::IL_LIBRARY, "IL library"),
    (flags::STRONG_NAME_SIGNED, "strong-name signed"),
    (flags::NATIVE_ENTRYPOINT, "native entry point"),
    (flags::TRACK_DEBUG_DATA, "track debug data"),
    (flags::BIT32_PREFERRED, "32-bit preferred"),
];

/// A directory: (RVA, size).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct Directory {
    /// Its RVA.
    pub rva: u32,
    /// Its size.
    pub size: u32,
}

impl Directory {
    /// Whether it's empty (no RVA or no size).
    pub fn is_empty(&self) -> bool {
        self.rva == 0 || self.size == 0
    }
}

/// The CLI header.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct Cor20Header {
    /// Its size (72).
    pub cb: u32,
    /// The runtime's major version (2).
    pub major_runtime_version: u16,
    /// Its minor version (5).
    pub minor_runtime_version: u16,
    /// The metadata.
    pub metadata: Directory,
    /// Flags ([`flags`]).
    pub flags: u32,
    /// The entry point: a MethodDef (or File) token, or with
    /// [`flags::NATIVE_ENTRYPOINT`], an RVA.
    pub entry_point: u32,
    /// The managed resources.
    pub resources: Directory,
    /// The strong-name signature.
    pub strong_name_signature: Directory,
    /// (Unused.)
    pub code_manager_table: Directory,
    /// V-table fixups (for unmanaged callers).
    pub vtable_fixups: Directory,
    /// (Unused.)
    pub export_address_table_jumps: Directory,
    /// The ReadyToRun header (or, in old NGen images, the native header).
    pub managed_native_header: Directory,
}

impl Cor20Header {
    /// Parses the header.
    pub fn parse(data: &[u8], base: u64) -> Result<Self> {
        let mut c = Cursor::with_base(data, base);
        let cb = c.u32("CLI header")?;
        if cb < COR20_HEADER_SIZE {
            return Err(c.error(ErrorKind::Malformed, "CLI header size"));
        }
        let dir = |c: &mut Cursor<'_>| -> Result<Directory> {
            Ok(Directory {
                rva: c.u32("CLI header")?,
                size: c.u32("CLI header")?,
            })
        };
        Ok(Self {
            cb,
            major_runtime_version: c.u16("CLI header")?,
            minor_runtime_version: c.u16("CLI header")?,
            metadata: dir(&mut c)?,
            flags: c.u32("CLI header")?,
            entry_point: c.u32("CLI header")?,
            resources: dir(&mut c)?,
            strong_name_signature: dir(&mut c)?,
            code_manager_table: dir(&mut c)?,
            vtable_fixups: dir(&mut c)?,
            export_address_table_jumps: dir(&mut c)?,
            managed_native_header: dir(&mut c)?,
        })
    }

    /// The names of the flags that are set.
    pub fn flag_names(&self) -> Vec<&'static str> {
        FLAG_NAMES
            .iter()
            .filter(|(bit, _)| self.flags & bit != 0)
            .map(|&(_, n)| n)
            .collect()
    }
}

/// `COR_VTABLE_*` flags.
pub mod vtable {
    /// 32-bit slots.
    pub const BIT32: u16 = 0x01;
    /// 64-bit slots.
    pub const BIT64: u16 = 0x02;
    /// Called from unmanaged code.
    pub const FROM_UNMANAGED: u16 = 0x04;
    /// ... keeping the app domain.
    pub const FROM_UNMANAGED_RETAIN_APPDOMAIN: u16 = 0x08;
    /// Call the most derived method.
    pub const CALL_MOST_DERIVED: u16 = 0x10;
}

/// A v-table fixup: `count` slots at `rva`, each a method token the
/// loader replaces with a pointer.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct VTableFixup {
    /// The slots' RVA.
    pub rva: u32,
    /// How many.
    pub count: u16,
    /// `COR_VTABLE_*` flags ([`vtable`]).
    pub flags: u16,
}

/// Parses the v-table fixups directory.
pub fn parse_vtable_fixups(data: &[u8], base: u64) -> Result<Vec<VTableFixup>> {
    let mut c = Cursor::with_base(data, base);
    let mut out = Vec::with_capacity(data.len() / 8);
    while c.remaining() >= 8 {
        out.push(VTableFixup {
            rva: c.u32("v-table fixup")?,
            count: c.u16("v-table fixup")?,
            flags: c.u16("v-table fixup")?,
        });
    }
    Ok(out)
}

#[cfg(test)]
#[allow(clippy::cast_possible_truncation)]
mod tests {
    use super::*;

    #[test]
    fn parses() {
        let mut d = 72u32.to_le_bytes().to_vec();
        d.extend_from_slice(&2u16.to_le_bytes());
        d.extend_from_slice(&5u16.to_le_bytes());
        for v in [0x2050u32, 0x5D8, 9, 0x0600_0001] {
            d.extend_from_slice(&v.to_le_bytes());
        }
        d.resize(72, 0);
        let h = Cor20Header::parse(&d, 0).unwrap();
        assert_eq!((h.major_runtime_version, h.minor_runtime_version), (2, 5));
        assert_eq!(
            h.metadata,
            Directory {
                rva: 0x2050,
                size: 0x5D8
            }
        );
        assert_eq!(h.flag_names(), ["IL only", "strong-name signed"]);
        assert!(h.resources.is_empty());
        for n in 0..72 {
            assert!(Cor20Header::parse(&d[..n], 0).is_err());
        }
        d[0] = 8;
        assert!(Cor20Header::parse(&d, 0).is_err());
        let f = parse_vtable_fixups(&[1, 0, 0, 0, 2, 0, 5, 0, 9], 0).unwrap();
        assert_eq!(
            f,
            [VTableFixup {
                rva: 1,
                count: 2,
                flags: 5
            }]
        );
    }
}
