//! Annotations (JVMS 4.7.16-4.7.22): element values of every kind,
//! parameter annotations, annotation defaults, and type annotations with
//! their targets and type paths.

use turbonop_core::{ErrorKind, Result};

use crate::reader::Cursor;

/// Element values nested deeper than this (arrays of annotations of
/// arrays ...) are taken for damage.
const MAX_DEPTH: usize = 128;

/// An annotation: its type and element-value pairs.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Annotation {
    /// Its type's descriptor (a Utf8: `Lfixture/Info;`).
    pub type_index: u16,
    /// Its elements: each name's Utf8, and value.
    pub elements: Vec<(u16, ElementValue)>,
}

/// An element's value.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ElementValue {
    /// A constant: its tag (`B C D F I J S Z s`) and constant pool index.
    Const {
        /// The tag.
        tag: u8,
        /// The constant (an Integer, Long, Float, Double, or Utf8).
        index: u16,
    },
    /// An enum constant (`e`).
    Enum {
        /// The enum type's descriptor (Utf8).
        type_name: u16,
        /// The constant's name (Utf8).
        const_name: u16,
    },
    /// A class (`c`): its return descriptor (Utf8: `Ljava/lang/Object;`,
    /// `V`).
    Class(u16),
    /// A nested annotation (`@`).
    Annotation(Annotation),
    /// An array (`[`).
    Array(Vec<ElementValue>),
}

impl ElementValue {
    /// Its tag.
    pub fn tag(&self) -> u8 {
        match self {
            ElementValue::Const { tag, .. } => *tag,
            ElementValue::Enum { .. } => b'e',
            ElementValue::Class(_) => b'c',
            ElementValue::Annotation(_) => b'@',
            ElementValue::Array(_) => b'[',
        }
    }
}

pub(crate) fn parse_annotation(c: &mut Cursor<'_>, depth: usize) -> Result<Annotation> {
    if depth > MAX_DEPTH {
        return Err(c.error(ErrorKind::Malformed, "annotations nested too deeply"));
    }
    let type_index = c.u16("annotation type")?;
    let n = c.count16(3, "annotation elements")?;
    let mut elements = Vec::with_capacity(n);
    for _ in 0..n {
        let name = c.u16("element name")?;
        elements.push((name, parse_element_value(c, depth + 1)?));
    }
    Ok(Annotation {
        type_index,
        elements,
    })
}

pub(crate) fn parse_element_value(c: &mut Cursor<'_>, depth: usize) -> Result<ElementValue> {
    if depth > MAX_DEPTH {
        return Err(c.error(ErrorKind::Malformed, "element values nested too deeply"));
    }
    let tag = c.u8("element value tag")?;
    Ok(match tag {
        b'B' | b'C' | b'D' | b'F' | b'I' | b'J' | b'S' | b'Z' | b's' => ElementValue::Const {
            tag,
            index: c.u16("element value")?,
        },
        b'e' => ElementValue::Enum {
            type_name: c.u16("enum element value")?,
            const_name: c.u16("enum element value")?,
        },
        b'c' => ElementValue::Class(c.u16("class element value")?),
        b'@' => ElementValue::Annotation(parse_annotation(c, depth + 1)?),
        b'[' => {
            let n = c.count16(3, "array element value")?;
            let mut values = Vec::with_capacity(n);
            for _ in 0..n {
                values.push(parse_element_value(c, depth + 1)?);
            }
            ElementValue::Array(values)
        }
        _ => return Err(c.error(ErrorKind::Malformed, "element value tag")),
    })
}

/// A `num_annotations`-prefixed table of annotations.
pub(crate) fn parse_annotations(c: &mut Cursor<'_>) -> Result<Vec<Annotation>> {
    let n = c.count16(4, "annotations")?;
    (0..n).map(|_| parse_annotation(c, 0)).collect()
}

/// Parameter annotations: a `u1` count of parameters, each a table.
pub(crate) fn parse_parameter_annotations(c: &mut Cursor<'_>) -> Result<Vec<Vec<Annotation>>> {
    let n = usize::from(c.u8("parameter annotations")?);
    c.check_count(n, 2, "parameter annotations")?;
    (0..n).map(|_| parse_annotations(c)).collect()
}

/// Where a type annotation is (JVMS 4.7.20.1).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum TargetInfo {
    /// `type_parameter_target` (0x00 class, 0x01 method).
    TypeParameter {
        /// The type parameter's index.
        index: u8,
    },
    /// `supertype_target` (0x10): 65535 for the superclass, else an
    /// interface's index.
    Supertype {
        /// The `implements` clause's index, or 65535.
        index: u16,
    },
    /// `type_parameter_bound_target` (0x11 class, 0x12 method).
    TypeParameterBound {
        /// The type parameter's index.
        param: u8,
        /// The bound's index.
        bound: u8,
    },
    /// `empty_target` (0x13 field, 0x14 method return or new object, 0x15
    /// receiver).
    Empty,
    /// `formal_parameter_target` (0x16).
    FormalParameter {
        /// The parameter's index.
        index: u8,
    },
    /// `throws_target` (0x17).
    Throws {
        /// The Exceptions attribute's index.
        index: u16,
    },
    /// `localvar_target` (0x40 local, 0x41 resource variable): the ranges
    /// where the variable lives.
    LocalVariable(Vec<LocalVarRange>),
    /// `catch_target` (0x42): the exception table entry.
    Catch {
        /// Its index.
        index: u16,
    },
    /// `offset_target` (0x43 instanceof, 0x44 new, 0x45 `::new`, 0x46
    /// `::name`).
    Offset {
        /// The instruction's offset.
        offset: u16,
    },
    /// `type_argument_target` (0x47 cast, 0x48 constructor call, 0x49
    /// method call, 0x4A `::new`, 0x4B `::name`).
    TypeArgument {
        /// The instruction's offset.
        offset: u16,
        /// The type argument's index.
        index: u8,
    },
}

/// One range of a local variable target.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct LocalVarRange {
    /// Where it starts.
    pub start_pc: u16,
    /// How long it lasts.
    pub length: u16,
    /// Its slot.
    pub index: u16,
}

/// A type path step (JVMS 4.7.20.2).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TypePathEntry {
    /// 0 deeper in an array, 1 deeper in a nested type, 2 a wildcard's
    /// bound, 3 a type argument.
    pub kind: u8,
    /// The type argument's index (kind 3), else 0.
    pub argument_index: u8,
}

/// A type annotation.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TypeAnnotation {
    /// The target type (JVMS Table 4.7.20-A and B).
    pub target_type: u8,
    /// The target.
    pub target: TargetInfo,
    /// Where in the type.
    pub path: Vec<TypePathEntry>,
    /// The annotation.
    pub annotation: Annotation,
}

/// The name javap gives target type `t` (`FIELD`, `METHOD_FORMAL_PARAMETER`).
pub fn target_type_name(t: u8) -> Option<&'static str> {
    Some(match t {
        0x00 => "CLASS_TYPE_PARAMETER",
        0x01 => "METHOD_TYPE_PARAMETER",
        0x10 => "CLASS_EXTENDS",
        0x11 => "CLASS_TYPE_PARAMETER_BOUND",
        0x12 => "METHOD_TYPE_PARAMETER_BOUND",
        0x13 => "FIELD",
        0x14 => "METHOD_RETURN",
        0x15 => "METHOD_RECEIVER",
        0x16 => "METHOD_FORMAL_PARAMETER",
        0x17 => "THROWS",
        0x40 => "LOCAL_VARIABLE",
        0x41 => "RESOURCE_VARIABLE",
        0x42 => "EXCEPTION_PARAMETER",
        0x43 => "INSTANCEOF",
        0x44 => "NEW",
        0x45 => "CONSTRUCTOR_REFERENCE",
        0x46 => "METHOD_REFERENCE",
        0x47 => "CAST",
        0x48 => "CONSTRUCTOR_INVOCATION_TYPE_ARGUMENT",
        0x49 => "METHOD_INVOCATION_TYPE_ARGUMENT",
        0x4A => "CONSTRUCTOR_REFERENCE_TYPE_ARGUMENT",
        0x4B => "METHOD_REFERENCE_TYPE_ARGUMENT",
        _ => return None,
    })
}

pub(crate) fn parse_type_annotation(c: &mut Cursor<'_>) -> Result<TypeAnnotation> {
    let target_type = c.u8("type annotation target")?;
    let what = "type annotation target";
    let target = match target_type {
        0x00 | 0x01 => TargetInfo::TypeParameter { index: c.u8(what)? },
        0x10 => TargetInfo::Supertype {
            index: c.u16(what)?,
        },
        0x11 | 0x12 => TargetInfo::TypeParameterBound {
            param: c.u8(what)?,
            bound: c.u8(what)?,
        },
        0x13..=0x15 => TargetInfo::Empty,
        0x16 => TargetInfo::FormalParameter { index: c.u8(what)? },
        0x17 => TargetInfo::Throws {
            index: c.u16(what)?,
        },
        0x40 | 0x41 => {
            let n = c.count16(6, what)?;
            let mut ranges = Vec::with_capacity(n);
            for _ in 0..n {
                ranges.push(LocalVarRange {
                    start_pc: c.u16(what)?,
                    length: c.u16(what)?,
                    index: c.u16(what)?,
                });
            }
            TargetInfo::LocalVariable(ranges)
        }
        0x42 => TargetInfo::Catch {
            index: c.u16(what)?,
        },
        0x43..=0x46 => TargetInfo::Offset {
            offset: c.u16(what)?,
        },
        0x47..=0x4B => TargetInfo::TypeArgument {
            offset: c.u16(what)?,
            index: c.u8(what)?,
        },
        _ => return Err(c.error(ErrorKind::Malformed, what)),
    };
    let n = usize::from(c.u8("type path")?);
    c.check_count(n, 2, "type path")?;
    let mut path = Vec::with_capacity(n);
    for _ in 0..n {
        let kind = c.u8("type path")?;
        let argument_index = c.u8("type path")?;
        if kind > 3 {
            return Err(c.error(ErrorKind::Malformed, "type path kind"));
        }
        path.push(TypePathEntry {
            kind,
            argument_index,
        });
    }
    let annotation = parse_annotation(c, 0)?;
    Ok(TypeAnnotation {
        target_type,
        target,
        path,
        annotation,
    })
}

pub(crate) fn parse_type_annotations(c: &mut Cursor<'_>) -> Result<Vec<TypeAnnotation>> {
    let n = c.count16(5, "type annotations")?;
    (0..n).map(|_| parse_type_annotation(c)).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_element_kind() {
        let bytes = [
            0, 1, 0, 6, // type #1, 6 elements
            0, 2, b'I', 0, 3, // id=I#3
            0, 4, b'e', 0, 5, 0, 6, // kind=e#5.#6
            0, 7, b'c', 0, 8, // type=c#8
            0, 9, b'@', 0, 10, 0, 0, // nested=@#10()
            0, 11, b'[', 0, 2, b's', 0, 12, b'Z', 0, 13, // values=[s#12,Z#13]
            0, 14, b'[', 0, 0, // empty=[]
        ];
        let mut c = Cursor::new(&bytes);
        let a = parse_annotation(&mut c, 0).unwrap();
        c.finish("x").unwrap();
        assert_eq!(a.type_index, 1);
        let tags: Vec<u8> = a.elements.iter().map(|e| e.1.tag()).collect();
        assert_eq!(tags, b"Iec@[[");
        assert_eq!(
            a.elements[4].1,
            ElementValue::Array(vec![
                ElementValue::Const {
                    tag: b's',
                    index: 12
                },
                ElementValue::Const {
                    tag: b'Z',
                    index: 13
                }
            ])
        );
        // A bad tag, and nesting too deep.
        assert!(parse_element_value(&mut Cursor::new(&[b'x', 0, 0]), 0).is_err());
        let mut deep = Vec::new();
        for _ in 0..1000 {
            deep.extend_from_slice(&[b'[', 0, 1]);
        }
        deep.extend_from_slice(&[b'I', 0, 1]);
        assert!(parse_element_value(&mut Cursor::new(&deep), 0).is_err());
    }

    #[test]
    fn type_annotation_targets() {
        let tail = [0, 0, 1, 0, 0]; // no path, @#1()
        let mut cases: Vec<(Vec<u8>, TargetInfo)> = vec![
            (vec![0x00, 2], TargetInfo::TypeParameter { index: 2 }),
            (
                vec![0x10, 0xFF, 0xFF],
                TargetInfo::Supertype { index: 65535 },
            ),
            (
                vec![0x12, 1, 2],
                TargetInfo::TypeParameterBound { param: 1, bound: 2 },
            ),
            (vec![0x14], TargetInfo::Empty),
            (vec![0x16, 3], TargetInfo::FormalParameter { index: 3 }),
            (vec![0x17, 0, 1], TargetInfo::Throws { index: 1 }),
            (
                vec![0x40, 0, 1, 0, 2, 0, 3, 0, 4],
                TargetInfo::LocalVariable(vec![LocalVarRange {
                    start_pc: 2,
                    length: 3,
                    index: 4,
                }]),
            ),
            (vec![0x42, 0, 5], TargetInfo::Catch { index: 5 }),
            (vec![0x44, 0, 9], TargetInfo::Offset { offset: 9 }),
            (
                vec![0x4B, 0, 9, 1],
                TargetInfo::TypeArgument {
                    offset: 9,
                    index: 1,
                },
            ),
        ];
        for (bytes, want) in cases.drain(..) {
            let mut b = bytes.clone();
            b.extend_from_slice(&tail);
            let t = parse_type_annotation(&mut Cursor::new(&b)).unwrap();
            assert_eq!(t.target, want, "{bytes:?}");
            assert!(target_type_name(t.target_type).is_some());
        }
        // A path.
        let b = [0x13, 2, 0, 0, 3, 1, 0, 1, 0, 0];
        let t = parse_type_annotation(&mut Cursor::new(&b)).unwrap();
        assert_eq!(
            t.path,
            [
                TypePathEntry {
                    kind: 0,
                    argument_index: 0
                },
                TypePathEntry {
                    kind: 3,
                    argument_index: 1
                }
            ]
        );
        assert!(parse_type_annotation(&mut Cursor::new(&[0x13, 1, 4, 0, 0, 1, 0, 0])).is_err());
        assert!(parse_type_annotation(&mut Cursor::new(&[0x30, 0, 0, 1, 0, 0])).is_err());
        assert_eq!(target_type_name(0x30), None);
    }
}
