//! A class file (JVMS 4.1): its version, constant pool, flags, this and
//! super classes, interfaces, fields, methods, and attributes.

use std::borrow::Cow;
use std::ops::Range;

use turbonop_core::{Error, ErrorKind, Result};

use crate::access;
use crate::attributes::{Attribute, AttributeValue, AttributesExt, Context, parse_attributes};
use crate::constant_pool::ConstantPool;
use crate::reader::Cursor;

/// A class file's first four bytes.
pub const MAGIC: u32 = 0xCAFE_BABE;

/// The minor version of a class that uses preview features.
pub const PREVIEW_MINOR_VERSION: u16 = 0xFFFF;

/// The newest class file version this reader names (Java 27's).
pub const LATEST_MAJOR_VERSION: u16 = 71;

/// The Java release whose class files have major version `major` (`1.1`
/// to `1.4`, then `5` to `27`), if it's one this reader knows.
pub fn java_version_name(major: u16) -> Option<Cow<'static, str>> {
    Some(match major {
        45 => Cow::Borrowed("1.1"),
        46 => Cow::Borrowed("1.2"),
        47 => Cow::Borrowed("1.3"),
        48 => Cow::Borrowed("1.4"),
        49..=LATEST_MAJOR_VERSION => Cow::Owned((major - 44).to_string()),
        _ => return None,
    })
}

/// Whether `data` starts like a class file: the magic, then a major
/// version from Java 1.1's (45) to a little past the newest known (a fat
/// Mach-O file shares the magic, with an architecture count below 45).
pub fn is_class(data: &[u8]) -> bool {
    data.len() >= 10
        && data[..4] == MAGIC.to_be_bytes()
        && (45..=LATEST_MAJOR_VERSION + 20).contains(&u16::from_be_bytes([data[6], data[7]]))
}

/// A field or method.
#[derive(Clone, Debug, PartialEq)]
pub struct Member<'d> {
    /// Where it is in the class file.
    pub offset: u64,
    /// Its size there (header and attributes).
    pub size: u64,
    /// Its access flags.
    pub access_flags: u16,
    /// Its name's Utf8.
    pub name_index: u16,
    /// Its descriptor's Utf8.
    pub descriptor_index: u16,
    /// Its attributes.
    pub attributes: Vec<Attribute<'d>>,
}

impl<'d> Member<'d> {
    /// Its name.
    pub fn name(&self, cp: &ConstantPool<'d>) -> Result<Cow<'d, str>> {
        cp.utf8(self.name_index)
    }

    /// Its descriptor.
    pub fn descriptor(&self, cp: &ConstantPool<'d>) -> Result<Cow<'d, str>> {
        cp.utf8(self.descriptor_index)
    }

    /// Its code, if it's a method with a body.
    pub fn code(&self) -> Option<&crate::attributes::Code<'d>> {
        self.attributes.code()
    }
}

/// The parts of a class file, where each lies: for sizing.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ClassLayout {
    /// Magic and versions (8 bytes).
    pub header: Range<u64>,
    /// The constant pool, its count included.
    pub constant_pool: Range<u64>,
    /// Flags, this and super classes, interfaces.
    pub declaration: Range<u64>,
    /// The fields, their count included.
    pub fields: Range<u64>,
    /// The methods, their count included.
    pub methods: Range<u64>,
    /// The class's attributes, their count included.
    pub attributes: Range<u64>,
}

/// A class file.
#[derive(Clone, Debug)]
pub struct ClassFile<'d> {
    data: &'d [u8],
    /// The minor version (65535: preview features).
    pub minor_version: u16,
    /// The major version (Java 8's is 52, Java 21's 65).
    pub major_version: u16,
    /// The constant pool.
    pub constant_pool: ConstantPool<'d>,
    /// The class's access flags.
    pub access_flags: u16,
    /// This class (a Class).
    pub this_class: u16,
    /// Its superclass (a Class), or 0 (`java.lang.Object`, modules).
    pub super_class: u16,
    /// Its interfaces (Classes).
    pub interfaces: Vec<u16>,
    /// Its fields.
    pub fields: Vec<Member<'d>>,
    /// Its methods.
    pub methods: Vec<Member<'d>>,
    /// Its attributes.
    pub attributes: Vec<Attribute<'d>>,
    layout: ClassLayout,
}

fn members<'d>(
    c: &mut Cursor<'d>,
    cp: &ConstantPool<'d>,
    what: &'static str,
) -> Result<Vec<Member<'d>>> {
    let n = c.count16(8, what)?;
    let mut v = Vec::with_capacity(n);
    for _ in 0..n {
        let offset = c.offset();
        let access_flags = c.u16(what)?;
        let name_index = c.u16(what)?;
        let descriptor_index = c.u16(what)?;
        let attributes = parse_attributes(c, cp, Context::Member, 0)?;
        v.push(Member {
            offset,
            size: c.offset() - offset,
            access_flags,
            name_index,
            descriptor_index,
            attributes,
        });
    }
    Ok(v)
}

impl<'d> ClassFile<'d> {
    /// Reads the class file `data`. Attributes that can't be read are kept
    /// with their errors ([`AttributeValue::Malformed`]); the structure
    /// around them must be whole.
    pub fn parse(data: &'d [u8]) -> Result<Self> {
        let mut c = Cursor::new(data);
        if c.u32("class file magic")? != MAGIC {
            return Err(Error::new(ErrorKind::BadMagic, 0, "class file magic"));
        }
        let minor_version = c.u16("minor version")?;
        let major_version = c.u16("major version")?;
        if major_version < 45 {
            return Err(Error::new(
                ErrorKind::Unsupported,
                6,
                "class file version before 45",
            ));
        }
        let cp_start = c.offset();
        let constant_pool = ConstantPool::parse(&mut c)?;
        let decl_start = c.offset();
        let access_flags = c.u16("access flags")?;
        let this_class = c.u16("this_class")?;
        let super_class = c.u16("super_class")?;
        let interfaces = c.u16_table("interfaces")?;
        let fields_start = c.offset();
        let fields = members(&mut c, &constant_pool, "fields")?;
        let methods_start = c.offset();
        let methods = members(&mut c, &constant_pool, "methods")?;
        let attrs_start = c.offset();
        let attributes = parse_attributes(&mut c, &constant_pool, Context::Member, 0)?;
        let end = c.offset();
        c.finish("bytes after the class file")?;
        Ok(Self {
            data,
            minor_version,
            major_version,
            constant_pool,
            access_flags,
            this_class,
            super_class,
            interfaces,
            fields,
            methods,
            attributes,
            layout: ClassLayout {
                header: 0..cp_start,
                constant_pool: cp_start..decl_start,
                declaration: decl_start..fields_start,
                fields: fields_start..methods_start,
                methods: methods_start..attrs_start,
                attributes: attrs_start..end,
            },
        })
    }

    /// The class file's bytes.
    pub fn data(&self) -> &'d [u8] {
        self.data
    }

    /// Where its parts are.
    pub fn layout(&self) -> &ClassLayout {
        &self.layout
    }

    /// This class's name (internal form: `java/lang/String`).
    pub fn name(&self) -> Result<Cow<'d, str>> {
        self.constant_pool.class_name(self.this_class)
    }

    /// The superclass's name, if there is one.
    pub fn super_name(&self) -> Result<Option<Cow<'d, str>>> {
        if self.super_class == 0 {
            return Ok(None);
        }
        self.constant_pool.class_name(self.super_class).map(Some)
    }

    /// The interfaces' names.
    pub fn interface_names(&self) -> Result<Vec<Cow<'d, str>>> {
        self.interfaces
            .iter()
            .map(|&i| self.constant_pool.class_name(i))
            .collect()
    }

    /// The package (internal form: `java/lang`; empty for the unnamed
    /// package).
    pub fn package(&self) -> Result<String> {
        let n = self.name()?;
        Ok(n.rfind('/')
            .map_or_else(String::new, |i| n[..i].to_string()))
    }

    /// The SourceFile attribute's name, if there is one.
    pub fn source_file(&self) -> Option<Cow<'d, str>> {
        let i = self.attributes.find_map(|v| match v {
            AttributeValue::SourceFile(i) => Some(*i),
            _ => None,
        })?;
        self.constant_pool.utf8(i).ok()
    }

    /// Whether it's an interface.
    pub fn is_interface(&self) -> bool {
        self.access_flags & access::INTERFACE != 0
    }

    /// Whether it's a module descriptor (module-info).
    pub fn is_module(&self) -> bool {
        self.access_flags & access::MODULE != 0
    }

    /// Whether it uses preview features (minor version 65535).
    pub fn uses_preview_features(&self) -> bool {
        self.minor_version == PREVIEW_MINOR_VERSION && self.major_version >= 56
    }

    /// The Java release its version is, if it's one this reader knows.
    pub fn java_version(&self) -> Option<Cow<'static, str>> {
        java_version_name(self.major_version)
    }

    /// Every attribute anywhere (the class's, fields', methods', Code's,
    /// record components') that couldn't be read.
    pub fn malformed_attributes(&self) -> Vec<Error> {
        let mut out = self.attributes.malformed();
        for m in self.fields.iter().chain(&self.methods) {
            out.extend(m.attributes.malformed());
        }
        out
    }

    /// The method named `name` with descriptor `descriptor`.
    pub fn method(&self, name: &str, descriptor: &str) -> Option<&Member<'d>> {
        let cp = &self.constant_pool;
        self.methods.iter().find(|m| {
            m.name(cp).is_ok_and(|n| n == name) && m.descriptor(cp).is_ok_and(|d| d == descriptor)
        })
    }

    /// The field named `name`.
    pub fn field(&self, name: &str) -> Option<&Member<'d>> {
        let cp = &self.constant_pool;
        self.fields
            .iter()
            .find(|m| m.name(cp).is_ok_and(|n| n == name))
    }
}
