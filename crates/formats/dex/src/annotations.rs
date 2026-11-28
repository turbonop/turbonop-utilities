//! Annotations: a class's directory (`annotations_directory_item`) of
//! its own, its fields', its methods', and its parameters' annotations;
//! annotation sets (`annotation_set_item`), parameters' lists of them
//! (`annotation_set_ref_list`), and annotations (`annotation_item`).

use turbonop_core::{Error, ErrorKind, Result};

use crate::reader::Cursor;
use crate::value::{EncodedAnnotation, read_annotation};

/// Kept at build time only.
pub const VISIBILITY_BUILD: u8 = 0;
/// Visible at run time.
pub const VISIBILITY_RUNTIME: u8 = 1;
/// The system's (`dalvik.annotation.*`).
pub const VISIBILITY_SYSTEM: u8 = 2;

/// A visibility's name as dexdump writes it (`VISIBILITY_RUNTIME`).
pub fn visibility_name(v: u8) -> &'static str {
    match v {
        VISIBILITY_BUILD => "VISIBILITY_BUILD",
        VISIBILITY_RUNTIME => "VISIBILITY_RUNTIME",
        VISIBILITY_SYSTEM => "VISIBILITY_SYSTEM",
        _ => "VISIBILITY_UNKNOWN",
    }
}

/// A class's annotations directory.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct AnnotationsDirectory {
    /// Where it is.
    pub offset: u32,
    /// The class's annotation set (0: none).
    pub class_annotations_off: u32,
    /// Each annotated field's (index) annotation set.
    pub fields: Vec<(u32, u32)>,
    /// Each annotated method's (index) annotation set.
    pub methods: Vec<(u32, u32)>,
    /// Each method's (index) parameters' annotation set ref list.
    pub parameters: Vec<(u32, u32)>,
    /// Its size in bytes.
    pub size: u32,
}

/// An annotation and its visibility.
#[derive(Clone, Debug, PartialEq)]
pub struct AnnotationItem {
    /// Where it is.
    pub offset: u32,
    /// Its visibility ([`VISIBILITY_RUNTIME`] ...).
    pub visibility: u8,
    /// The annotation.
    pub annotation: EncodedAnnotation,
    /// Its size in bytes.
    pub size: u32,
}

fn aligned(off: u32, what: &'static str) -> Result<()> {
    if off.is_multiple_of(4) {
        Ok(())
    } else {
        Err(Error::new(ErrorKind::Malformed, u64::from(off), what))
    }
}

impl AnnotationsDirectory {
    /// Parses the annotations directory at `off` of `data`.
    pub fn parse(data: &[u8], off: u32) -> Result<Self> {
        aligned(off, "annotations directory alignment")?;
        let mut c = Cursor::at(data, off as usize);
        let class_annotations_off = c.u32("class_annotations_off")?;
        let nf = c.u32("fields_size")?;
        let nm = c.u32("annotated_methods_size")?;
        let np = c.u32("annotated_parameters_size")?;
        let mut pairs = |n: u32, what: &'static str| -> Result<Vec<(u32, u32)>> {
            let n = c.check_count(n, 8, what)?;
            let mut v = Vec::with_capacity(n);
            for _ in 0..n {
                v.push((c.u32(what)?, c.u32(what)?));
            }
            Ok(v)
        };
        let fields = pairs(nf, "field annotations")?;
        let methods = pairs(nm, "method annotations")?;
        let parameters = pairs(np, "parameter annotations")?;
        #[allow(clippy::cast_possible_truncation)]
        let size = (c.pos() - off as usize) as u32;
        Ok(Self {
            offset: off,
            class_annotations_off,
            fields,
            methods,
            parameters,
            size,
        })
    }
}

/// The annotation set at `off` of `data`: its annotations' offsets.
pub fn annotation_set(data: &[u8], off: u32) -> Result<Vec<u32>> {
    offsets(data, off, "annotation set")
}

/// The annotation set ref list at `off` of `data`: each parameter's
/// annotation set's offset (0: none).
pub fn annotation_set_ref_list(data: &[u8], off: u32) -> Result<Vec<u32>> {
    offsets(data, off, "annotation set ref list")
}

fn offsets(data: &[u8], off: u32, what: &'static str) -> Result<Vec<u32>> {
    aligned(off, what)?;
    let mut c = Cursor::at(data, off as usize);
    let n = c.u32(what)?;
    let n = c.check_count(n, 4, what)?;
    (0..n).map(|_| c.u32(what)).collect()
}

/// The annotation at `off` of `data`.
pub fn annotation_item(data: &[u8], off: u32) -> Result<AnnotationItem> {
    let mut c = Cursor::at(data, off as usize);
    let visibility = c.u8("annotation visibility")?;
    let annotation = read_annotation(&mut c, 0)?;
    #[allow(clippy::cast_possible_truncation)]
    let size = (c.pos() - off as usize) as u32;
    Ok(AnnotationItem {
        offset: off,
        visibility,
        annotation,
        size,
    })
}

/// The annotations of the set at `off` of `data`, in order.
pub fn annotations_of_set(data: &[u8], off: u32) -> Result<Vec<AnnotationItem>> {
    annotation_set(data, off)?
        .into_iter()
        .map(|a| annotation_item(data, a))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::value::EncodedValue;

    #[test]
    fn directories_sets_and_items() {
        let mut d = Vec::new();
        // The directory at 0: class set at 0x30, a field, a method, a
        // parameter list.
        for v in [0x30u32, 1, 1, 1, 7, 0x30, 8, 0x30, 9, 0x40] {
            d.extend_from_slice(&v.to_le_bytes());
        }
        d.resize(0x30, 0);
        d.extend_from_slice(&1u32.to_le_bytes());
        d.extend_from_slice(&0x50u32.to_le_bytes());
        d.resize(0x40, 0);
        d.extend_from_slice(&2u32.to_le_bytes());
        d.extend_from_slice(&0x30u32.to_le_bytes());
        d.extend_from_slice(&0u32.to_le_bytes());
        d.resize(0x50, 0);
        d.extend_from_slice(&[VISIBILITY_RUNTIME, 5, 1, 3, 0x04, 42]);
        let dir = AnnotationsDirectory::parse(&d, 0).unwrap();
        assert_eq!(dir.size, 40);
        assert_eq!(
            (
                dir.class_annotations_off,
                dir.fields.as_slice(),
                dir.methods.as_slice(),
                dir.parameters.as_slice()
            ),
            (0x30, &[(7, 0x30)][..], &[(8, 0x30)][..], &[(9, 0x40)][..])
        );
        assert_eq!(annotation_set_ref_list(&d, 0x40).unwrap(), [0x30, 0]);
        let items = annotations_of_set(&d, 0x30).unwrap();
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].visibility, VISIBILITY_RUNTIME);
        assert_eq!(items[0].size, 6);
        assert_eq!(items[0].annotation.type_idx, 5);
        assert_eq!(items[0].annotation.elements, [(3, EncodedValue::Int(42))]);
        assert_eq!(visibility_name(2), "VISIBILITY_SYSTEM");
        assert_eq!(visibility_name(9), "VISIBILITY_UNKNOWN");
        assert!(AnnotationsDirectory::parse(&d, 2).is_err());
        assert!(AnnotationsDirectory::parse(&d[..30], 0).is_err());
        assert!(annotation_set(&d, 0x52).is_err());
        assert!(annotation_item(&d[..0x54], 0x50).is_err());
    }
}
