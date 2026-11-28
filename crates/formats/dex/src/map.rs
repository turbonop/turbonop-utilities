//! The map list (`map_list`): every section of the file, its kind, size,
//! and offset.

use turbonop_core::{Error, ErrorKind, Result};

use crate::reader::Cursor;

/// What a section holds.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum MapItemType {
    /// The header.
    HeaderItem,
    /// String ids.
    StringIdItem,
    /// Type ids.
    TypeIdItem,
    /// Prototype ids.
    ProtoIdItem,
    /// Field ids.
    FieldIdItem,
    /// Method ids.
    MethodIdItem,
    /// Class definitions.
    ClassDefItem,
    /// Call site ids.
    CallSiteIdItem,
    /// Method handles.
    MethodHandleItem,
    /// The map list.
    MapList,
    /// Type lists.
    TypeList,
    /// Annotation set reference lists (parameters' annotations).
    AnnotationSetRefList,
    /// Annotation sets.
    AnnotationSetItem,
    /// Class data.
    ClassDataItem,
    /// Code items.
    CodeItem,
    /// String data.
    StringDataItem,
    /// Debug information.
    DebugInfoItem,
    /// Annotations.
    AnnotationItem,
    /// Encoded arrays (static values, call sites).
    EncodedArrayItem,
    /// Annotations directories.
    AnnotationsDirectoryItem,
    /// Hidden-API flags.
    HiddenapiClassDataItem,
    /// A kind this doesn't know.
    Unknown(u16),
}

impl MapItemType {
    /// The kind of type code `v`.
    pub fn from_u16(v: u16) -> Self {
        match v {
            0x0000 => Self::HeaderItem,
            0x0001 => Self::StringIdItem,
            0x0002 => Self::TypeIdItem,
            0x0003 => Self::ProtoIdItem,
            0x0004 => Self::FieldIdItem,
            0x0005 => Self::MethodIdItem,
            0x0006 => Self::ClassDefItem,
            0x0007 => Self::CallSiteIdItem,
            0x0008 => Self::MethodHandleItem,
            0x1000 => Self::MapList,
            0x1001 => Self::TypeList,
            0x1002 => Self::AnnotationSetRefList,
            0x1003 => Self::AnnotationSetItem,
            0x2000 => Self::ClassDataItem,
            0x2001 => Self::CodeItem,
            0x2002 => Self::StringDataItem,
            0x2003 => Self::DebugInfoItem,
            0x2004 => Self::AnnotationItem,
            0x2005 => Self::EncodedArrayItem,
            0x2006 => Self::AnnotationsDirectoryItem,
            0xF000 => Self::HiddenapiClassDataItem,
            v => Self::Unknown(v),
        }
    }

    /// Its type code.
    pub fn code(self) -> u16 {
        match self {
            Self::HeaderItem => 0x0000,
            Self::StringIdItem => 0x0001,
            Self::TypeIdItem => 0x0002,
            Self::ProtoIdItem => 0x0003,
            Self::FieldIdItem => 0x0004,
            Self::MethodIdItem => 0x0005,
            Self::ClassDefItem => 0x0006,
            Self::CallSiteIdItem => 0x0007,
            Self::MethodHandleItem => 0x0008,
            Self::MapList => 0x1000,
            Self::TypeList => 0x1001,
            Self::AnnotationSetRefList => 0x1002,
            Self::AnnotationSetItem => 0x1003,
            Self::ClassDataItem => 0x2000,
            Self::CodeItem => 0x2001,
            Self::StringDataItem => 0x2002,
            Self::DebugInfoItem => 0x2003,
            Self::AnnotationItem => 0x2004,
            Self::EncodedArrayItem => 0x2005,
            Self::AnnotationsDirectoryItem => 0x2006,
            Self::HiddenapiClassDataItem => 0xF000,
            Self::Unknown(v) => v,
        }
    }

    /// Its name as the format document spells it (`TYPE_CODE_ITEM`).
    pub fn name(self) -> &'static str {
        match self {
            Self::HeaderItem => "TYPE_HEADER_ITEM",
            Self::StringIdItem => "TYPE_STRING_ID_ITEM",
            Self::TypeIdItem => "TYPE_TYPE_ID_ITEM",
            Self::ProtoIdItem => "TYPE_PROTO_ID_ITEM",
            Self::FieldIdItem => "TYPE_FIELD_ID_ITEM",
            Self::MethodIdItem => "TYPE_METHOD_ID_ITEM",
            Self::ClassDefItem => "TYPE_CLASS_DEF_ITEM",
            Self::CallSiteIdItem => "TYPE_CALL_SITE_ID_ITEM",
            Self::MethodHandleItem => "TYPE_METHOD_HANDLE_ITEM",
            Self::MapList => "TYPE_MAP_LIST",
            Self::TypeList => "TYPE_TYPE_LIST",
            Self::AnnotationSetRefList => "TYPE_ANNOTATION_SET_REF_LIST",
            Self::AnnotationSetItem => "TYPE_ANNOTATION_SET_ITEM",
            Self::ClassDataItem => "TYPE_CLASS_DATA_ITEM",
            Self::CodeItem => "TYPE_CODE_ITEM",
            Self::StringDataItem => "TYPE_STRING_DATA_ITEM",
            Self::DebugInfoItem => "TYPE_DEBUG_INFO_ITEM",
            Self::AnnotationItem => "TYPE_ANNOTATION_ITEM",
            Self::EncodedArrayItem => "TYPE_ENCODED_ARRAY_ITEM",
            Self::AnnotationsDirectoryItem => "TYPE_ANNOTATIONS_DIRECTORY_ITEM",
            Self::HiddenapiClassDataItem => "TYPE_HIDDENAPI_CLASS_DATA_ITEM",
            Self::Unknown(_) => "TYPE_UNKNOWN",
        }
    }
}

/// A section.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct MapItem {
    /// What it holds.
    pub kind: MapItemType,
    /// How many items.
    pub size: u32,
    /// Where it is.
    pub offset: u32,
}

/// The map list at `off` of `data`.
pub fn parse(data: &[u8], off: u32) -> Result<Vec<MapItem>> {
    if off == 0 {
        return Err(Error::new(ErrorKind::Malformed, 0, "map_off"));
    }
    if off % 4 != 0 {
        return Err(Error::new(
            ErrorKind::Malformed,
            u64::from(off),
            "map list alignment",
        ));
    }
    let mut c = Cursor::at(data, off as usize);
    let n = c.u32("map list size")?;
    let n = c.check_count(n, 12, "map list")?;
    let mut items = Vec::with_capacity(n);
    for _ in 0..n {
        let kind = MapItemType::from_u16(c.u16("map item type")?);
        c.skip(2, "map item")?;
        items.push(MapItem {
            kind,
            size: c.u32("map item size")?,
            offset: c.u32("map item offset")?,
        });
    }
    Ok(items)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn codes_round_trip() {
        for v in (0..=0x10u16).chain(0x1000..0x1004).chain(0x2000..0x2008).chain([0xF000, 0xFFFF]) {
            assert_eq!(MapItemType::from_u16(v).code(), v);
        }
        assert_eq!(MapItemType::from_u16(0x2001).name(), "TYPE_CODE_ITEM");
        assert_eq!(MapItemType::from_u16(0x9).name(), "TYPE_UNKNOWN");
    }

    #[test]
    fn damaged_lists() {
        let mut d = vec![0u8; 8];
        d.extend_from_slice(&1u32.to_le_bytes());
        d.extend_from_slice(&[0x01, 0x20, 0, 0, 3, 0, 0, 0, 0x40, 0, 0, 0]);
        let items = parse(&d, 8).unwrap();
        assert_eq!(
            items,
            [MapItem {
                kind: MapItemType::CodeItem,
                size: 3,
                offset: 0x40
            }]
        );
        assert!(parse(&d, 0).is_err());
        assert!(parse(&d, 2).is_err());
        d[8] = 2;
        assert_eq!(parse(&d, 8).unwrap_err().kind, ErrorKind::Truncated);
        d[8..12].copy_from_slice(&u32::MAX.to_le_bytes());
        assert!(parse(&d, 8).is_err());
    }
}
