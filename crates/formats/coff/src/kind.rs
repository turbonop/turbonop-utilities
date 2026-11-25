//! What kind of object a file is, and objects the model doesn't cover,
//! passed through as they are.

use alloc::vec::Vec;

use turbonop_core::Result;

use crate::object::Object;

/// `{D1BAA1C7-BAEE-4BA9-AF20-FAF66AA4DCB8}`, the big-obj class ID, as
/// stored.
pub const BIGOBJ_CLASS_ID: [u8; 16] = [
    0xC7, 0xA1, 0xBA, 0xD1, 0xEE, 0xBA, 0xA9, 0x4B, 0xAF, 0x20, 0xFA, 0xF6, 0x6A, 0xA4, 0xDC, 0xB8,
];

/// What a file's header says it is.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Kind {
    /// A regular COFF object (`IMAGE_FILE_HEADER`): anything that doesn't
    /// start with an anonymous header's signature.
    Regular,
    /// A big-obj object (`ANON_OBJECT_HEADER_BIGOBJ`: version 2 or later,
    /// with the big-obj class ID).
    BigObj,
    /// A short import object (`IMPORT_OBJECT_HEADER`: an anonymous header
    /// of version 0).
    Import,
    /// A `/GL` object (an anonymous header with the LTCG class ID): the
    /// compiler's intermediate language.
    Ltcg,
    /// Any other anonymous object.
    Anonymous,
}

/// What kind of object `data` is, or `None` if it is too short to have
/// a header.
pub fn kind(data: &[u8]) -> Option<Kind> {
    if data.len() < 20 {
        return None;
    }
    // Anonymous headers start with machine 0 (`Sig1`) and 0xFFFF in the
    // section count's place (`Sig2`).
    if data[..4] != [0, 0, 0xFF, 0xFF] {
        return Some(Kind::Regular);
    }
    let version = u16::from_le_bytes([data[4], data[5]]);
    if version == 0 {
        return Some(Kind::Import);
    }
    if data.len() < 32 {
        return Some(Kind::Anonymous);
    }
    let class = &data[12..28];
    Some(
        if version >= 2 && class == BIGOBJ_CLASS_ID && data.len() >= 56 {
            Kind::BigObj
        } else if class == turbonop_pe::ltcg::CLASS_ID {
            Kind::Ltcg
        } else {
            Kind::Anonymous
        },
    )
}

/// An object file: a COFF object in the model, or anything else (import
/// objects, `/GL` objects, other anonymous objects) kept as its bytes, to
/// be written back as it is.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum File<'data> {
    /// A COFF object, regular or big-obj.
    Object(Object<'data>),
    /// An object the model doesn't cover.
    Opaque {
        /// What its header says it is.
        kind: Kind,
        /// Its bytes.
        data: &'data [u8],
    },
}

impl<'data> File<'data> {
    /// Reads `data`: a COFF object into the model, or (an import object
    /// or another anonymous object) as it is.
    pub fn parse(data: &'data [u8]) -> Result<Self> {
        match kind(data) {
            Some(kind @ (Kind::Import | Kind::Ltcg | Kind::Anonymous)) => {
                Ok(File::Opaque { kind, data })
            }
            _ => Object::parse(data).map(File::Object),
        }
    }

    /// Appends the file to `out`.
    pub fn write_to(&self, out: &mut Vec<u8>) -> Result<()> {
        match self {
            File::Object(object) => object.write_to(out),
            File::Opaque { data, .. } => {
                out.extend_from_slice(data);
                Ok(())
            }
        }
    }

    /// The file's bytes.
    pub fn write(&self) -> Result<Vec<u8>> {
        let mut out = Vec::new();
        self.write_to(&mut out)?;
        Ok(out)
    }
}
