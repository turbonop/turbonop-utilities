//! The constant pool (JVMS 4.4): every tag, and the lookups that resolve
//! an index to its text.

use std::borrow::Cow;

use turbonop_core::{Error, ErrorKind, Result};

use crate::reader::Cursor;

/// The constant pool tags.
pub mod tag {
    /// `CONSTANT_Utf8`.
    pub const UTF8: u8 = 1;
    /// `CONSTANT_Integer`.
    pub const INTEGER: u8 = 3;
    /// `CONSTANT_Float`.
    pub const FLOAT: u8 = 4;
    /// `CONSTANT_Long`.
    pub const LONG: u8 = 5;
    /// `CONSTANT_Double`.
    pub const DOUBLE: u8 = 6;
    /// `CONSTANT_Class`.
    pub const CLASS: u8 = 7;
    /// `CONSTANT_String`.
    pub const STRING: u8 = 8;
    /// `CONSTANT_Fieldref`.
    pub const FIELDREF: u8 = 9;
    /// `CONSTANT_Methodref`.
    pub const METHODREF: u8 = 10;
    /// `CONSTANT_InterfaceMethodref`.
    pub const INTERFACE_METHODREF: u8 = 11;
    /// `CONSTANT_NameAndType`.
    pub const NAME_AND_TYPE: u8 = 12;
    /// `CONSTANT_MethodHandle`.
    pub const METHOD_HANDLE: u8 = 15;
    /// `CONSTANT_MethodType`.
    pub const METHOD_TYPE: u8 = 16;
    /// `CONSTANT_Dynamic`.
    pub const DYNAMIC: u8 = 17;
    /// `CONSTANT_InvokeDynamic`.
    pub const INVOKE_DYNAMIC: u8 = 18;
    /// `CONSTANT_Module`.
    pub const MODULE: u8 = 19;
    /// `CONSTANT_Package`.
    pub const PACKAGE: u8 = 20;
}

/// A method handle's kind (JVMS 4.4.8, Table 5.4.3.5-A).
pub mod ref_kind {
    /// `REF_getField`.
    pub const GET_FIELD: u8 = 1;
    /// `REF_getStatic`.
    pub const GET_STATIC: u8 = 2;
    /// `REF_putField`.
    pub const PUT_FIELD: u8 = 3;
    /// `REF_putStatic`.
    pub const PUT_STATIC: u8 = 4;
    /// `REF_invokeVirtual`.
    pub const INVOKE_VIRTUAL: u8 = 5;
    /// `REF_invokeStatic`.
    pub const INVOKE_STATIC: u8 = 6;
    /// `REF_invokeSpecial`.
    pub const INVOKE_SPECIAL: u8 = 7;
    /// `REF_newInvokeSpecial`.
    pub const NEW_INVOKE_SPECIAL: u8 = 8;
    /// `REF_invokeInterface`.
    pub const INVOKE_INTERFACE: u8 = 9;
}

/// A method handle kind's name (`REF_invokeStatic`).
pub fn ref_kind_name(kind: u8) -> Option<&'static str> {
    Some(match kind {
        1 => "REF_getField",
        2 => "REF_getStatic",
        3 => "REF_putField",
        4 => "REF_putStatic",
        5 => "REF_invokeVirtual",
        6 => "REF_invokeStatic",
        7 => "REF_invokeSpecial",
        8 => "REF_newInvokeSpecial",
        9 => "REF_invokeInterface",
        _ => return None,
    })
}

/// One constant pool entry.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Constant<'d> {
    /// Index 0, and the slot after a long or double: no entry.
    Unusable,
    /// Modified UTF-8 text (see [`decode_modified_utf8`]).
    Utf8(&'d [u8]),
    /// An `int`.
    Integer(i32),
    /// A `float`'s bits.
    Float(u32),
    /// A `long`.
    Long(i64),
    /// A `double`'s bits.
    Double(u64),
    /// A class or interface (or array type): its name's Utf8.
    Class {
        /// Its name (internal form: `java/lang/String`, or an array's
        /// descriptor).
        name: u16,
    },
    /// A `String` constant.
    String {
        /// Its Utf8.
        string: u16,
    },
    /// A field reference.
    Fieldref {
        /// Its class.
        class: u16,
        /// Its name and type.
        name_and_type: u16,
    },
    /// A method reference (a class's).
    Methodref {
        /// Its class.
        class: u16,
        /// Its name and type.
        name_and_type: u16,
    },
    /// An interface method reference.
    InterfaceMethodref {
        /// Its interface.
        class: u16,
        /// Its name and type.
        name_and_type: u16,
    },
    /// A name and descriptor.
    NameAndType {
        /// The name's Utf8.
        name: u16,
        /// The descriptor's Utf8.
        descriptor: u16,
    },
    /// A method handle.
    MethodHandle {
        /// Its kind ([`ref_kind`]).
        kind: u8,
        /// The field or method it refers to.
        reference: u16,
    },
    /// A method type.
    MethodType {
        /// Its descriptor's Utf8.
        descriptor: u16,
    },
    /// A dynamically computed constant.
    Dynamic {
        /// Its bootstrap method (an index into BootstrapMethods).
        bootstrap: u16,
        /// Its name and type.
        name_and_type: u16,
    },
    /// A dynamically computed call site.
    InvokeDynamic {
        /// Its bootstrap method (an index into BootstrapMethods).
        bootstrap: u16,
        /// Its name and type.
        name_and_type: u16,
    },
    /// A module (module-info only).
    Module {
        /// Its name's Utf8.
        name: u16,
    },
    /// A package (module-info only).
    Package {
        /// Its name's Utf8 (internal form: `java/lang`).
        name: u16,
    },
}

impl Constant<'_> {
    /// Its tag.
    pub fn tag(&self) -> u8 {
        match self {
            Constant::Unusable => 0,
            Constant::Utf8(_) => tag::UTF8,
            Constant::Integer(_) => tag::INTEGER,
            Constant::Float(_) => tag::FLOAT,
            Constant::Long(_) => tag::LONG,
            Constant::Double(_) => tag::DOUBLE,
            Constant::Class { .. } => tag::CLASS,
            Constant::String { .. } => tag::STRING,
            Constant::Fieldref { .. } => tag::FIELDREF,
            Constant::Methodref { .. } => tag::METHODREF,
            Constant::InterfaceMethodref { .. } => tag::INTERFACE_METHODREF,
            Constant::NameAndType { .. } => tag::NAME_AND_TYPE,
            Constant::MethodHandle { .. } => tag::METHOD_HANDLE,
            Constant::MethodType { .. } => tag::METHOD_TYPE,
            Constant::Dynamic { .. } => tag::DYNAMIC,
            Constant::InvokeDynamic { .. } => tag::INVOKE_DYNAMIC,
            Constant::Module { .. } => tag::MODULE,
            Constant::Package { .. } => tag::PACKAGE,
        }
    }

    /// Its tag's name, as javap spells it (`Methodref`, `InvokeDynamic`).
    pub fn tag_name(&self) -> &'static str {
        tag_name(self.tag()).unwrap_or("Unusable")
    }
}

/// A tag's name, as javap spells it.
pub fn tag_name(t: u8) -> Option<&'static str> {
    Some(match t {
        tag::UTF8 => "Utf8",
        tag::INTEGER => "Integer",
        tag::FLOAT => "Float",
        tag::LONG => "Long",
        tag::DOUBLE => "Double",
        tag::CLASS => "Class",
        tag::STRING => "String",
        tag::FIELDREF => "Fieldref",
        tag::METHODREF => "Methodref",
        tag::INTERFACE_METHODREF => "InterfaceMethodref",
        tag::NAME_AND_TYPE => "NameAndType",
        tag::METHOD_HANDLE => "MethodHandle",
        tag::METHOD_TYPE => "MethodType",
        tag::DYNAMIC => "Dynamic",
        tag::INVOKE_DYNAMIC => "InvokeDynamic",
        tag::MODULE => "Module",
        tag::PACKAGE => "Package",
        _ => return None,
    })
}

/// Modified UTF-8 (JVMS 4.4.7) decoded: NUL is `C0 80`, and characters past
/// U+FFFF are UTF-16 surrogate pairs, each surrogate three bytes. Borrowed
/// when the bytes are also standard UTF-8 (ASCII always is); malformed
/// sequences and unpaired surrogates become U+FFFD.
pub fn decode_modified_utf8(bytes: &[u8]) -> Cow<'_, str> {
    // Standard UTF-8 with no encoded surrogates (lead 0xED, then 0xA0 to
    // 0xBF) and no overlong NUL (0xC0) reads the same either way.
    if !bytes.iter().any(|&b| b == 0xC0 || b == 0xED || b >= 0xF0)
        && let Ok(s) = std::str::from_utf8(bytes)
    {
        return Cow::Borrowed(s);
    }
    let mut units: Vec<u16> = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        let b = bytes[i];
        let cont = |k: usize| bytes.get(i + k).copied().filter(|c| c & 0xC0 == 0x80);
        if b < 0x80 {
            units.push(u16::from(b));
            i += 1;
        } else if b & 0xE0 == 0xC0
            && let Some(c1) = cont(1)
        {
            units.push((u16::from(b & 0x1F) << 6) | u16::from(c1 & 0x3F));
            i += 2;
        } else if b & 0xF0 == 0xE0
            && let (Some(c1), Some(c2)) = (cont(1), cont(2))
        {
            units.push(
                (u16::from(b & 0x0F) << 12) | (u16::from(c1 & 0x3F) << 6) | u16::from(c2 & 0x3F),
            );
            i += 3;
        } else {
            units.push(0xFFFD);
            i += 1;
        }
    }
    Cow::Owned(String::from_utf16_lossy(&units))
}

/// The constant pool.
#[derive(Clone, Debug, Default)]
pub struct ConstantPool<'d> {
    entries: Vec<Constant<'d>>,
    /// Each entry's offset in the class file.
    offsets: Vec<u32>,
}

fn bad(index: u16, what: &'static str) -> Error {
    Error::new(ErrorKind::Malformed, u64::from(index), what)
}

impl<'d> ConstantPool<'d> {
    /// Reads `constant_pool_count` and the entries from `c`.
    pub(crate) fn parse(c: &mut Cursor<'d>) -> Result<Self> {
        let count = usize::from(c.u16("constant_pool_count")?);
        // Each entry is at least three bytes.
        c.check_count(count.saturating_sub(1), 3, "constant pool")?;
        let mut entries = Vec::with_capacity(count.max(1));
        let mut offsets = Vec::with_capacity(count.max(1));
        entries.push(Constant::Unusable);
        offsets.push(0);
        while entries.len() < count {
            let at = c.offset();
            let t = c.u8("constant tag")?;
            let what = "constant";
            let e = match t {
                tag::UTF8 => {
                    let n = usize::from(c.u16(what)?);
                    Constant::Utf8(c.bytes(n, "Utf8 constant")?)
                }
                tag::INTEGER => Constant::Integer(c.i32(what)?),
                tag::FLOAT => Constant::Float(c.u32(what)?),
                tag::LONG => Constant::Long(c.u64(what)?.cast_signed()),
                tag::DOUBLE => Constant::Double(c.u64(what)?),
                tag::CLASS => Constant::Class { name: c.u16(what)? },
                tag::STRING => Constant::String {
                    string: c.u16(what)?,
                },
                tag::FIELDREF | tag::METHODREF | tag::INTERFACE_METHODREF => {
                    let class = c.u16(what)?;
                    let name_and_type = c.u16(what)?;
                    match t {
                        tag::FIELDREF => Constant::Fieldref {
                            class,
                            name_and_type,
                        },
                        tag::METHODREF => Constant::Methodref {
                            class,
                            name_and_type,
                        },
                        _ => Constant::InterfaceMethodref {
                            class,
                            name_and_type,
                        },
                    }
                }
                tag::NAME_AND_TYPE => Constant::NameAndType {
                    name: c.u16(what)?,
                    descriptor: c.u16(what)?,
                },
                tag::METHOD_HANDLE => Constant::MethodHandle {
                    kind: c.u8(what)?,
                    reference: c.u16(what)?,
                },
                tag::METHOD_TYPE => Constant::MethodType {
                    descriptor: c.u16(what)?,
                },
                tag::DYNAMIC | tag::INVOKE_DYNAMIC => {
                    let bootstrap = c.u16(what)?;
                    let name_and_type = c.u16(what)?;
                    if t == tag::DYNAMIC {
                        Constant::Dynamic {
                            bootstrap,
                            name_and_type,
                        }
                    } else {
                        Constant::InvokeDynamic {
                            bootstrap,
                            name_and_type,
                        }
                    }
                }
                tag::MODULE => Constant::Module { name: c.u16(what)? },
                tag::PACKAGE => Constant::Package { name: c.u16(what)? },
                _ => {
                    return Err(Error::new(ErrorKind::Malformed, at, "constant tag"));
                }
            };
            let wide = matches!(e, Constant::Long(_) | Constant::Double(_));
            entries.push(e);
            offsets.push(u32::try_from(at).unwrap_or(u32::MAX));
            if wide {
                // The next index is unusable; a long or double last of all
                // claims an index past the count (which javac never writes).
                if entries.len() >= count {
                    return Err(Error::new(
                        ErrorKind::Malformed,
                        at,
                        "long or double constant at the pool's end",
                    ));
                }
                entries.push(Constant::Unusable);
                offsets.push(u32::try_from(at).unwrap_or(u32::MAX));
            }
        }
        Ok(Self { entries, offsets })
    }

    /// `constant_pool_count`: one more than the last index.
    pub fn count(&self) -> usize {
        self.entries.len()
    }

    /// Every entry, by index (0 and the slots after longs and doubles are
    /// [`Constant::Unusable`]).
    pub fn entries(&self) -> &[Constant<'d>] {
        &self.entries
    }

    /// Where entry `index` is in the class file.
    pub fn offset(&self, index: u16) -> Option<u64> {
        self.offsets.get(usize::from(index)).map(|&o| u64::from(o))
    }

    /// Entry `index`, if it's a usable index.
    pub fn get(&self, index: u16) -> Option<&Constant<'d>> {
        self.entries
            .get(usize::from(index))
            .filter(|c| !matches!(c, Constant::Unusable))
    }

    /// Entry `index`, or an error naming `what`.
    pub fn entry(&self, index: u16, what: &'static str) -> Result<&Constant<'d>> {
        self.get(index).ok_or_else(|| bad(index, what))
    }

    /// The Utf8 at `index`, decoded.
    pub fn utf8(&self, index: u16) -> Result<Cow<'d, str>> {
        match self.get(index) {
            Some(Constant::Utf8(b)) => Ok(decode_modified_utf8(b)),
            _ => Err(bad(index, "Utf8 constant")),
        }
    }

    /// The Utf8 at `index`, as bytes.
    pub fn utf8_bytes(&self, index: u16) -> Result<&'d [u8]> {
        match self.get(index) {
            Some(Constant::Utf8(b)) => Ok(b),
            _ => Err(bad(index, "Utf8 constant")),
        }
    }

    /// The name (internal form) of the Class at `index`.
    pub fn class_name(&self, index: u16) -> Result<Cow<'d, str>> {
        match self.get(index) {
            Some(Constant::Class { name }) => self.utf8(*name),
            _ => Err(bad(index, "Class constant")),
        }
    }

    /// The name of the Module or Package at `index`.
    pub fn module_or_package_name(&self, index: u16) -> Result<Cow<'d, str>> {
        match self.get(index) {
            Some(Constant::Module { name } | Constant::Package { name }) => self.utf8(*name),
            _ => Err(bad(index, "Module or Package constant")),
        }
    }

    /// The name and descriptor of the NameAndType at `index`.
    pub fn name_and_type(&self, index: u16) -> Result<(Cow<'d, str>, Cow<'d, str>)> {
        match self.get(index) {
            Some(Constant::NameAndType { name, descriptor }) => {
                Ok((self.utf8(*name)?, self.utf8(*descriptor)?))
            }
            _ => Err(bad(index, "NameAndType constant")),
        }
    }

    /// A field or method reference's class, name, and descriptor.
    pub fn member_ref(&self, index: u16) -> Result<MemberRef<'d>> {
        match self.get(index) {
            Some(
                c @ (Constant::Fieldref {
                    class,
                    name_and_type,
                }
                | Constant::Methodref {
                    class,
                    name_and_type,
                }
                | Constant::InterfaceMethodref {
                    class,
                    name_and_type,
                }),
            ) => {
                let (name, descriptor) = self.name_and_type(*name_and_type)?;
                Ok(MemberRef {
                    tag: c.tag(),
                    class: self.class_name(*class)?,
                    name,
                    descriptor,
                })
            }
            _ => Err(bad(index, "field or method reference")),
        }
    }
}

/// A field or method reference, resolved.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MemberRef<'d> {
    /// Fieldref, Methodref, or InterfaceMethodref.
    pub tag: u8,
    /// The class (internal form).
    pub class: Cow<'d, str>,
    /// The member's name.
    pub name: Cow<'d, str>,
    /// Its descriptor.
    pub descriptor: Cow<'d, str>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn modified_utf8() {
        assert!(matches!(decode_modified_utf8(b"abc"), Cow::Borrowed("abc")));
        assert!(matches!(
            decode_modified_utf8("caf\u{e9}".as_bytes()),
            Cow::Borrowed(_)
        ));
        // NUL, two bytes.
        assert_eq!(decode_modified_utf8(&[b'a', 0xC0, 0x80, b'b']), "a\0b");
        // U+1F600 as a surrogate pair, three bytes each.
        assert_eq!(
            decode_modified_utf8(&[0xED, 0xA0, 0xBD, 0xED, 0xB8, 0x80]),
            "\u{1F600}"
        );
        // An unpaired surrogate, a stray continuation, a cut sequence; a raw
        // NUL is taken (as Java's readUTF takes it).
        assert_eq!(decode_modified_utf8(&[0xED, 0xA0, 0xBD]), "\u{FFFD}");
        assert_eq!(decode_modified_utf8(&[0]), "\0");
        assert_eq!(decode_modified_utf8(&[0, 0xC0, 0x80]), "\0\0");
        assert_eq!(decode_modified_utf8(&[0x80, b'x']), "\u{FFFD}x");
        assert_eq!(decode_modified_utf8(&[0xE6, 0x97]), "\u{FFFD}\u{FFFD}");
        // Standard UTF-8's four-byte form isn't modified UTF-8's.
        assert_eq!(
            decode_modified_utf8("\u{1F600}".as_bytes()),
            "\u{FFFD}\u{FFFD}\u{FFFD}\u{FFFD}"
        );
    }

    fn pool(bytes: &[u8]) -> Result<ConstantPool<'_>> {
        ConstantPool::parse(&mut Cursor::new(bytes))
    }

    #[test]
    fn every_tag() {
        let mut b = vec![0, 0];
        let mut n = 1u16;
        let mut add = |bytes: &[u8], slots: u16| {
            b.extend_from_slice(bytes);
            n += slots;
        };
        add(&[1, 0, 3, b'a', b'/', b'B'], 1); // 1 Utf8
        add(&[3, 0xFF, 0xFF, 0xFF, 0xFE], 1); // 2 Integer -2
        add(&[4, 0x3F, 0xC0, 0, 0], 1); // 3 Float 1.5
        add(&[5, 0, 0, 0, 0, 0, 0, 0, 7], 2); // 4 Long 7, 5 unusable
        add(&[6, 0x40, 0, 0, 0, 0, 0, 0, 0], 2); // 6 Double 2.0, 7
        add(&[7, 0, 1], 1); // 8 Class a/B
        add(&[8, 0, 1], 1); // 9 String
        add(&[1, 0, 1, b'f'], 1); // 10 Utf8 f
        add(&[1, 0, 1, b'I'], 1); // 11 Utf8 I
        add(&[12, 0, 10, 0, 11], 1); // 12 NameAndType f:I
        add(&[9, 0, 8, 0, 12], 1); // 13 Fieldref
        add(&[10, 0, 8, 0, 12], 1); // 14 Methodref
        add(&[11, 0, 8, 0, 12], 1); // 15 InterfaceMethodref
        add(&[15, 6, 0, 14], 1); // 16 MethodHandle
        add(&[16, 0, 11], 1); // 17 MethodType
        add(&[17, 0, 0, 0, 12], 1); // 18 Dynamic
        add(&[18, 0, 1, 0, 12], 1); // 19 InvokeDynamic
        add(&[19, 0, 10], 1); // 20 Module
        add(&[20, 0, 1], 1); // 21 Package
        b[..2].copy_from_slice(&n.to_be_bytes());
        let cp = pool(&b).unwrap();
        assert_eq!(cp.count(), 22);
        let tags: Vec<&str> = cp.entries().iter().map(Constant::tag_name).collect();
        assert_eq!(
            tags,
            [
                "Unusable",
                "Utf8",
                "Integer",
                "Float",
                "Long",
                "Unusable",
                "Double",
                "Unusable",
                "Class",
                "String",
                "Utf8",
                "Utf8",
                "NameAndType",
                "Fieldref",
                "Methodref",
                "InterfaceMethodref",
                "MethodHandle",
                "MethodType",
                "Dynamic",
                "InvokeDynamic",
                "Module",
                "Package"
            ]
        );
        assert_eq!(cp.get(2), Some(&Constant::Integer(-2)));
        assert_eq!(cp.get(4), Some(&Constant::Long(7)));
        assert_eq!(cp.get(5), None);
        assert_eq!(cp.get(0), None);
        assert_eq!(cp.get(22), None);
        assert_eq!(cp.class_name(8).unwrap(), "a/B");
        assert!(cp.class_name(9).is_err());
        let r = cp.member_ref(15).unwrap();
        assert_eq!(
            (r.tag, &*r.class, &*r.name, &*r.descriptor),
            (tag::INTERFACE_METHODREF, "a/B", "f", "I")
        );
        assert!(cp.member_ref(12).is_err());
        assert_eq!(cp.module_or_package_name(20).unwrap(), "f");
        assert_eq!(cp.module_or_package_name(21).unwrap(), "a/B");
        assert_eq!(cp.offset(1), Some(2));
        assert_eq!(cp.offset(2), Some(8));
        assert_eq!(ref_kind_name(6), Some("REF_invokeStatic"));
        assert_eq!(ref_kind_name(0), None);
        assert_eq!(tag_name(2), None);
    }

    #[test]
    fn damaged_pools() {
        // A bad tag.
        assert!(pool(&[0, 2, 2, 0, 0]).is_err());
        // Truncated.
        assert!(pool(&[0, 2, 1, 0, 5, b'a']).is_err());
        // A long whose second slot is past the count.
        assert!(pool(&[0, 2, 5, 0, 0, 0, 0, 0, 0, 0, 0]).is_err());
        // A count too big for the data.
        assert!(pool(&[0xFF, 0xFF, 1, 0, 0]).is_err());
        // Count 0 and 1: no entries.
        assert_eq!(pool(&[0, 0]).unwrap().count(), 1);
        assert_eq!(pool(&[0, 1]).unwrap().count(), 1);
    }
}
