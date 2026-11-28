//! Attributes (JVMS 4.7): every standard one, and the JDK's own
//! (ModuleHashes, ModuleTarget, ModuleResolution, CharacterRangeTable,
//! SourceID, CompilationID). An attribute this reader doesn't know is
//! kept as bytes; one it can't read keeps its error, and the class still
//! reads.

use turbonop_core::{Error, ErrorKind, Result};

use crate::annotations::{
    Annotation, ElementValue, TypeAnnotation, parse_annotations, parse_element_value,
    parse_parameter_annotations, parse_type_annotations,
};
use crate::constant_pool::ConstantPool;
use crate::reader::Cursor;
use crate::stackmap::{StackMapFrame, parse_stack_map_table};

/// An attribute: its name, where it is, its bytes, and what they hold.
#[derive(Clone, Debug, PartialEq)]
pub struct Attribute<'d> {
    /// Its name's Utf8.
    pub name_index: u16,
    /// Where its header (name and length) is in the class file.
    pub offset: u64,
    /// Its data (after the six-byte header).
    pub data: &'d [u8],
    /// What the data holds.
    pub value: AttributeValue<'d>,
}

impl Attribute<'_> {
    /// Its size in the class file, header included.
    pub fn size(&self) -> u64 {
        6 + self.data.len() as u64
    }
}

/// An exception handler (a Code attribute's exception table entry).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ExceptionHandler {
    /// Where the range it covers starts.
    pub start_pc: u16,
    /// Where it ends (exclusive).
    pub end_pc: u16,
    /// The handler.
    pub handler_pc: u16,
    /// The Class it catches, or 0 for any (`finally`).
    pub catch_type: u16,
}

/// A method's code.
#[derive(Clone, Debug, PartialEq)]
pub struct Code<'d> {
    /// The operand stack's depth.
    pub max_stack: u16,
    /// The local variable slots.
    pub max_locals: u16,
    /// The bytecode.
    pub code: &'d [u8],
    /// Where the bytecode is in the class file.
    pub code_offset: u64,
    /// The exception table.
    pub exception_table: Vec<ExceptionHandler>,
    /// Its attributes (LineNumberTable, LocalVariableTable, StackMapTable,
    /// type annotations).
    pub attributes: Vec<Attribute<'d>>,
}

/// A nested class (InnerClasses).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct InnerClass {
    /// The nested class.
    pub inner_class: u16,
    /// Its enclosing class, or 0 (local and anonymous classes).
    pub outer_class: u16,
    /// Its simple name's Utf8, or 0 (anonymous).
    pub inner_name: u16,
    /// Its flags as declared.
    pub flags: u16,
}

/// A line number entry.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct LineNumber {
    /// Where the line's code starts.
    pub start_pc: u16,
    /// The line.
    pub line: u16,
}

/// A local variable (LocalVariableTable, or LocalVariableTypeTable with a
/// signature for the descriptor).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct LocalVariable {
    /// Where it's live from.
    pub start_pc: u16,
    /// For how long.
    pub length: u16,
    /// Its name's Utf8.
    pub name: u16,
    /// Its descriptor's (or signature's) Utf8.
    pub descriptor: u16,
    /// Its slot.
    pub index: u16,
}

/// A bootstrap method.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BootstrapMethod {
    /// Its MethodHandle.
    pub method_ref: u16,
    /// Its static arguments (constants).
    pub arguments: Vec<u16>,
}

/// A method parameter (MethodParameters).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct MethodParameter {
    /// Its name's Utf8, or 0.
    pub name: u16,
    /// Its flags (final, synthetic, mandated).
    pub flags: u16,
}

/// A record component.
#[derive(Clone, Debug, PartialEq)]
pub struct RecordComponent<'d> {
    /// Its name's Utf8.
    pub name: u16,
    /// Its descriptor's Utf8.
    pub descriptor: u16,
    /// Its attributes (Signature, annotations).
    pub attributes: Vec<Attribute<'d>>,
}

/// A module's `requires`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Requires {
    /// The Module.
    pub module: u16,
    /// Its flags (transitive, static, synthetic, mandated).
    pub flags: u16,
    /// The version compiled against (Utf8), or 0.
    pub version: u16,
}

/// A module's `exports` or `opens`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Exports {
    /// The Package.
    pub package: u16,
    /// Its flags (synthetic, mandated).
    pub flags: u16,
    /// The Modules it's qualified to (none: to all).
    pub to: Vec<u16>,
}

/// A module's `provides`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Provides {
    /// The service (a Class).
    pub service: u16,
    /// Its implementations (Classes).
    pub with: Vec<u16>,
}

/// The Module attribute.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Module {
    /// The Module.
    pub name: u16,
    /// Its flags (open, synthetic, mandated).
    pub flags: u16,
    /// Its version (Utf8), or 0.
    pub version: u16,
    /// What it requires.
    pub requires: Vec<Requires>,
    /// What it exports.
    pub exports: Vec<Exports>,
    /// What it opens.
    pub opens: Vec<Exports>,
    /// The services it uses (Classes).
    pub uses: Vec<u16>,
    /// The services it provides.
    pub provides: Vec<Provides>,
}

/// A CharacterRangeTable entry (javac's `-Xjcov`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CharacterRange {
    /// Where the code starts.
    pub start_pc: u16,
    /// Where it ends (inclusive).
    pub end_pc: u16,
    /// The source position it starts at (line << 10 | column).
    pub character_range_start: u32,
    /// The source position it ends at.
    pub character_range_end: u32,
    /// What it is (statement, block, assignment, ...).
    pub flags: u16,
}

/// What an attribute holds.
#[derive(Clone, Debug, PartialEq)]
pub enum AttributeValue<'d> {
    /// `ConstantValue`: the constant.
    ConstantValue(u16),
    /// `Code`.
    Code(Code<'d>),
    /// `StackMapTable`.
    StackMapTable(Vec<StackMapFrame>),
    /// `Exceptions`: the Classes a method throws.
    Exceptions(Vec<u16>),
    /// `InnerClasses`.
    InnerClasses(Vec<InnerClass>),
    /// `EnclosingMethod`: the class, and the method (a NameAndType) or 0.
    EnclosingMethod {
        /// The enclosing class.
        class: u16,
        /// The enclosing method, or 0 (an initializer's class).
        method: u16,
    },
    /// `Synthetic`.
    Synthetic,
    /// `Signature`: its Utf8.
    Signature(u16),
    /// `SourceFile`: its Utf8.
    SourceFile(u16),
    /// `SourceDebugExtension`: its bytes (modified UTF-8, SMAP).
    SourceDebugExtension(&'d [u8]),
    /// `LineNumberTable`.
    LineNumberTable(Vec<LineNumber>),
    /// `LocalVariableTable`.
    LocalVariableTable(Vec<LocalVariable>),
    /// `LocalVariableTypeTable`.
    LocalVariableTypeTable(Vec<LocalVariable>),
    /// `Deprecated`.
    Deprecated,
    /// `RuntimeVisibleAnnotations`.
    RuntimeVisibleAnnotations(Vec<Annotation>),
    /// `RuntimeInvisibleAnnotations`.
    RuntimeInvisibleAnnotations(Vec<Annotation>),
    /// `RuntimeVisibleParameterAnnotations`.
    RuntimeVisibleParameterAnnotations(Vec<Vec<Annotation>>),
    /// `RuntimeInvisibleParameterAnnotations`.
    RuntimeInvisibleParameterAnnotations(Vec<Vec<Annotation>>),
    /// `RuntimeVisibleTypeAnnotations`.
    RuntimeVisibleTypeAnnotations(Vec<TypeAnnotation>),
    /// `RuntimeInvisibleTypeAnnotations`.
    RuntimeInvisibleTypeAnnotations(Vec<TypeAnnotation>),
    /// `AnnotationDefault`.
    AnnotationDefault(ElementValue),
    /// `BootstrapMethods`.
    BootstrapMethods(Vec<BootstrapMethod>),
    /// `MethodParameters`.
    MethodParameters(Vec<MethodParameter>),
    /// `Module`.
    Module(Module),
    /// `ModulePackages`: Packages.
    ModulePackages(Vec<u16>),
    /// `ModuleMainClass`: a Class.
    ModuleMainClass(u16),
    /// `NestHost`: a Class.
    NestHost(u16),
    /// `NestMembers`: Classes.
    NestMembers(Vec<u16>),
    /// `Record`.
    Record(Vec<RecordComponent<'d>>),
    /// `PermittedSubclasses`: Classes.
    PermittedSubclasses(Vec<u16>),
    /// `ModuleHashes` (the JDK's): the algorithm (Utf8) and each module's
    /// hash.
    ModuleHashes {
        /// The algorithm's Utf8.
        algorithm: u16,
        /// Each Module and its hash.
        hashes: Vec<(u16, &'d [u8])>,
    },
    /// `ModuleTarget` (the JDK's): the target platform (Utf8), or 0.
    ModuleTarget(u16),
    /// `ModuleResolution` (the JDK's): flags.
    ModuleResolution(u16),
    /// `CharacterRangeTable` (javac's `-Xjcov`).
    CharacterRangeTable(Vec<CharacterRange>),
    /// `SourceID` (javac's): a Utf8.
    SourceId(u16),
    /// `CompilationID` (javac's): a Utf8.
    CompilationId(u16),
    /// An attribute this reader doesn't know.
    Unknown,
    /// An attribute it couldn't read: why.
    Malformed(Error),
}

/// Where attributes are: what may hold what.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Context {
    /// A class, field, method, or record component.
    Member,
    /// A Code attribute (whose attributes hold no Code).
    Code,
}

/// The attributes table at `c` (its count, then each).
pub(crate) fn parse_attributes<'d>(
    c: &mut Cursor<'d>,
    cp: &ConstantPool<'d>,
    context: Context,
    depth: usize,
) -> Result<Vec<Attribute<'d>>> {
    let n = c.count16(6, "attributes")?;
    let mut out = Vec::with_capacity(n);
    for _ in 0..n {
        let offset = c.offset();
        let name_index = c.u16("attribute name")?;
        let len = c.u32("attribute length")? as usize;
        let data_offset = c.offset();
        let data = c.bytes(len, "attribute data")?;
        let value = match cp.utf8_bytes(name_index) {
            Ok(name) => parse_value(name, data, data_offset, cp, context, depth)
                .unwrap_or_else(AttributeValue::Malformed),
            Err(e) => AttributeValue::Malformed(e),
        };
        out.push(Attribute {
            name_index,
            offset,
            data,
            value,
        });
    }
    Ok(out)
}

fn u16_only(data: &[u8], base: u64, what: &'static str) -> Result<u16> {
    let mut c = Cursor::with_base(data, base);
    let v = c.u16(what)?;
    c.finish(what)?;
    Ok(v)
}

fn empty(data: &[u8], base: u64, what: &'static str) -> Result<()> {
    Cursor::with_base(data, base).finish(what)
}

fn local_variables(c: &mut Cursor<'_>) -> Result<Vec<LocalVariable>> {
    let n = c.count16(10, "local variables")?;
    let what = "local variable";
    (0..n)
        .map(|_| {
            Ok(LocalVariable {
                start_pc: c.u16(what)?,
                length: c.u16(what)?,
                name: c.u16(what)?,
                descriptor: c.u16(what)?,
                index: c.u16(what)?,
            })
        })
        .collect()
}

fn exports(c: &mut Cursor<'_>) -> Result<Vec<Exports>> {
    let n = c.count16(6, "exports")?;
    let what = "exports";
    (0..n)
        .map(|_| {
            Ok(Exports {
                package: c.u16(what)?,
                flags: c.u16(what)?,
                to: c.u16_table(what)?,
            })
        })
        .collect()
}

fn parse_value<'d>(
    name: &[u8],
    data: &'d [u8],
    base: u64,
    cp: &ConstantPool<'d>,
    context: Context,
    depth: usize,
) -> Result<AttributeValue<'d>> {
    let mut c = Cursor::with_base(data, base);
    let v = match name {
        b"ConstantValue" => {
            return Ok(AttributeValue::ConstantValue(u16_only(
                data,
                base,
                "ConstantValue",
            )?));
        }
        b"Code" => {
            if context == Context::Code {
                return Err(c.error(ErrorKind::Malformed, "Code attribute in a Code attribute"));
            }
            let max_stack = c.u16("max_stack")?;
            let max_locals = c.u16("max_locals")?;
            let len = c.u32("code_length")? as usize;
            let code_offset = c.offset();
            let code = c.bytes(len, "code")?;
            let n = c.count16(8, "exception table")?;
            let what = "exception table";
            let mut exception_table = Vec::with_capacity(n);
            for _ in 0..n {
                exception_table.push(ExceptionHandler {
                    start_pc: c.u16(what)?,
                    end_pc: c.u16(what)?,
                    handler_pc: c.u16(what)?,
                    catch_type: c.u16(what)?,
                });
            }
            let attributes = parse_attributes(&mut c, cp, Context::Code, depth + 1)?;
            AttributeValue::Code(Code {
                max_stack,
                max_locals,
                code,
                code_offset,
                exception_table,
                attributes,
            })
        }
        b"StackMapTable" => AttributeValue::StackMapTable(parse_stack_map_table(&mut c)?),
        b"Exceptions" => AttributeValue::Exceptions(c.u16_table("Exceptions")?),
        b"InnerClasses" => {
            let n = c.count16(8, "InnerClasses")?;
            let what = "InnerClasses";
            let mut v = Vec::with_capacity(n);
            for _ in 0..n {
                v.push(InnerClass {
                    inner_class: c.u16(what)?,
                    outer_class: c.u16(what)?,
                    inner_name: c.u16(what)?,
                    flags: c.u16(what)?,
                });
            }
            AttributeValue::InnerClasses(v)
        }
        b"EnclosingMethod" => AttributeValue::EnclosingMethod {
            class: c.u16("EnclosingMethod")?,
            method: c.u16("EnclosingMethod")?,
        },
        b"Synthetic" => {
            empty(data, base, "Synthetic")?;
            return Ok(AttributeValue::Synthetic);
        }
        b"Signature" => {
            return Ok(AttributeValue::Signature(u16_only(
                data,
                base,
                "Signature",
            )?));
        }
        b"SourceFile" => {
            return Ok(AttributeValue::SourceFile(u16_only(
                data,
                base,
                "SourceFile",
            )?));
        }
        b"SourceDebugExtension" => return Ok(AttributeValue::SourceDebugExtension(data)),
        b"LineNumberTable" => {
            let n = c.count16(4, "LineNumberTable")?;
            let mut v = Vec::with_capacity(n);
            for _ in 0..n {
                v.push(LineNumber {
                    start_pc: c.u16("LineNumberTable")?,
                    line: c.u16("LineNumberTable")?,
                });
            }
            AttributeValue::LineNumberTable(v)
        }
        b"LocalVariableTable" => AttributeValue::LocalVariableTable(local_variables(&mut c)?),
        b"LocalVariableTypeTable" => {
            AttributeValue::LocalVariableTypeTable(local_variables(&mut c)?)
        }
        b"Deprecated" => {
            empty(data, base, "Deprecated")?;
            return Ok(AttributeValue::Deprecated);
        }
        b"RuntimeVisibleAnnotations" => {
            AttributeValue::RuntimeVisibleAnnotations(parse_annotations(&mut c)?)
        }
        b"RuntimeInvisibleAnnotations" => {
            AttributeValue::RuntimeInvisibleAnnotations(parse_annotations(&mut c)?)
        }
        b"RuntimeVisibleParameterAnnotations" => {
            AttributeValue::RuntimeVisibleParameterAnnotations(parse_parameter_annotations(&mut c)?)
        }
        b"RuntimeInvisibleParameterAnnotations" => {
            AttributeValue::RuntimeInvisibleParameterAnnotations(parse_parameter_annotations(
                &mut c,
            )?)
        }
        b"RuntimeVisibleTypeAnnotations" => {
            AttributeValue::RuntimeVisibleTypeAnnotations(parse_type_annotations(&mut c)?)
        }
        b"RuntimeInvisibleTypeAnnotations" => {
            AttributeValue::RuntimeInvisibleTypeAnnotations(parse_type_annotations(&mut c)?)
        }
        b"AnnotationDefault" => AttributeValue::AnnotationDefault(parse_element_value(&mut c, 0)?),
        b"BootstrapMethods" => {
            let n = c.count16(4, "BootstrapMethods")?;
            let mut v = Vec::with_capacity(n);
            for _ in 0..n {
                v.push(BootstrapMethod {
                    method_ref: c.u16("BootstrapMethods")?,
                    arguments: c.u16_table("BootstrapMethods")?,
                });
            }
            AttributeValue::BootstrapMethods(v)
        }
        b"MethodParameters" => {
            let n = usize::from(c.u8("MethodParameters")?);
            c.check_count(n, 4, "MethodParameters")?;
            let mut v = Vec::with_capacity(n);
            for _ in 0..n {
                v.push(MethodParameter {
                    name: c.u16("MethodParameters")?,
                    flags: c.u16("MethodParameters")?,
                });
            }
            AttributeValue::MethodParameters(v)
        }
        b"Module" => {
            let what = "Module";
            let name = c.u16(what)?;
            let flags = c.u16(what)?;
            let version = c.u16(what)?;
            let n = c.count16(6, "requires")?;
            let mut requires = Vec::with_capacity(n);
            for _ in 0..n {
                requires.push(Requires {
                    module: c.u16(what)?,
                    flags: c.u16(what)?,
                    version: c.u16(what)?,
                });
            }
            let exports_ = exports(&mut c)?;
            let opens = exports(&mut c)?;
            let uses = c.u16_table("uses")?;
            let n = c.count16(4, "provides")?;
            let mut provides = Vec::with_capacity(n);
            for _ in 0..n {
                provides.push(Provides {
                    service: c.u16(what)?,
                    with: c.u16_table("provides")?,
                });
            }
            AttributeValue::Module(Module {
                name,
                flags,
                version,
                requires,
                exports: exports_,
                opens,
                uses,
                provides,
            })
        }
        b"ModulePackages" => AttributeValue::ModulePackages(c.u16_table("ModulePackages")?),
        b"ModuleMainClass" => {
            return Ok(AttributeValue::ModuleMainClass(u16_only(
                data,
                base,
                "ModuleMainClass",
            )?));
        }
        b"NestHost" => return Ok(AttributeValue::NestHost(u16_only(data, base, "NestHost")?)),
        b"NestMembers" => AttributeValue::NestMembers(c.u16_table("NestMembers")?),
        b"Record" => {
            if depth > 0 {
                return Err(c.error(
                    ErrorKind::Malformed,
                    "Record attribute in a record component",
                ));
            }
            let n = c.count16(6, "Record")?;
            let mut v = Vec::with_capacity(n);
            for _ in 0..n {
                let name = c.u16("record component")?;
                let descriptor = c.u16("record component")?;
                let attributes = parse_attributes(&mut c, cp, Context::Member, depth + 1)?;
                v.push(RecordComponent {
                    name,
                    descriptor,
                    attributes,
                });
            }
            AttributeValue::Record(v)
        }
        b"PermittedSubclasses" => {
            AttributeValue::PermittedSubclasses(c.u16_table("PermittedSubclasses")?)
        }
        b"ModuleHashes" => {
            let algorithm = c.u16("ModuleHashes")?;
            let n = c.count16(4, "ModuleHashes")?;
            let mut hashes = Vec::with_capacity(n);
            for _ in 0..n {
                let m = c.u16("ModuleHashes")?;
                let len = usize::from(c.u16("ModuleHashes")?);
                hashes.push((m, c.bytes(len, "ModuleHashes")?));
            }
            AttributeValue::ModuleHashes { algorithm, hashes }
        }
        b"ModuleTarget" => {
            return Ok(AttributeValue::ModuleTarget(u16_only(
                data,
                base,
                "ModuleTarget",
            )?));
        }
        b"ModuleResolution" => {
            return Ok(AttributeValue::ModuleResolution(u16_only(
                data,
                base,
                "ModuleResolution",
            )?));
        }
        b"CharacterRangeTable" => {
            let n = c.count16(14, "CharacterRangeTable")?;
            let what = "CharacterRangeTable";
            let mut v = Vec::with_capacity(n);
            for _ in 0..n {
                v.push(CharacterRange {
                    start_pc: c.u16(what)?,
                    end_pc: c.u16(what)?,
                    character_range_start: c.u32(what)?,
                    character_range_end: c.u32(what)?,
                    flags: c.u16(what)?,
                });
            }
            AttributeValue::CharacterRangeTable(v)
        }
        b"SourceID" => return Ok(AttributeValue::SourceId(u16_only(data, base, "SourceID")?)),
        b"CompilationID" => {
            return Ok(AttributeValue::CompilationId(u16_only(
                data,
                base,
                "CompilationID",
            )?));
        }
        _ => return Ok(AttributeValue::Unknown),
    };
    c.finish("attribute's data longer than its contents")?;
    Ok(v)
}

/// Attributes' lookups by kind.
pub trait AttributesExt<'d> {
    /// The attributes.
    fn attrs(&self) -> &[Attribute<'d>];

    /// The first attribute whose value `f` selects.
    fn find_map<'a, T>(&'a self, f: impl Fn(&'a AttributeValue<'d>) -> Option<T>) -> Option<T>
    where
        'd: 'a,
    {
        self.attrs().iter().find_map(|a| f(&a.value))
    }

    /// The Signature attribute's Utf8, if there is one.
    fn signature(&self) -> Option<u16> {
        self.find_map(|v| match v {
            AttributeValue::Signature(s) => Some(*s),
            _ => None,
        })
    }

    /// The Code attribute, if there is one.
    fn code(&self) -> Option<&Code<'d>> {
        self.attrs().iter().find_map(|a| match &a.value {
            AttributeValue::Code(c) => Some(c),
            _ => None,
        })
    }

    /// Every attribute that couldn't be read, with its error.
    fn malformed(&self) -> Vec<Error> {
        let mut out = Vec::new();
        for a in self.attrs() {
            match &a.value {
                AttributeValue::Malformed(e) => out.push(*e),
                AttributeValue::Code(c) => out.extend(c.attributes.malformed()),
                AttributeValue::Record(r) => {
                    for rc in r {
                        out.extend(rc.attributes.malformed());
                    }
                }
                _ => {}
            }
        }
        out
    }
}

impl<'d> AttributesExt<'d> for Vec<Attribute<'d>> {
    fn attrs(&self) -> &[Attribute<'d>] {
        self
    }
}

impl<'d> AttributesExt<'d> for [Attribute<'d>] {
    fn attrs(&self) -> &[Attribute<'d>] {
        self
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A pool of Utf8s, `names[i]` at index `i + 1`.
    fn pool(names: &[&str]) -> Vec<u8> {
        let mut b = u16::try_from(names.len() + 1)
            .unwrap()
            .to_be_bytes()
            .to_vec();
        for n in names {
            b.push(1);
            b.extend_from_slice(&u16::try_from(n.len()).unwrap().to_be_bytes());
            b.extend_from_slice(n.as_bytes());
        }
        b
    }

    fn attrs(names: &[&str], items: &[(u16, &[u8])]) -> (Vec<u8>, Vec<u8>) {
        let mut a = u16::try_from(items.len()).unwrap().to_be_bytes().to_vec();
        for (n, d) in items {
            a.extend_from_slice(&n.to_be_bytes());
            a.extend_from_slice(&u32::try_from(d.len()).unwrap().to_be_bytes());
            a.extend_from_slice(d);
        }
        (pool(names), a)
    }

    fn parse<'d>(p: &'d [u8], a: &'d [u8]) -> Vec<Attribute<'d>> {
        let cp = ConstantPool::parse(&mut Cursor::new(p)).unwrap();
        let mut c = Cursor::new(a);
        let v = parse_attributes(&mut c, &cp, Context::Member, 0).unwrap();
        c.finish("x").unwrap();
        v
    }

    #[test]
    fn simple_attributes() {
        let names = [
            "ConstantValue",
            "Synthetic",
            "Deprecated",
            "Signature",
            "SourceFile",
            "SourceDebugExtension",
            "Exceptions",
            "NestHost",
            "Custom",
            "EnclosingMethod",
            "MethodParameters",
            "ModuleTarget",
        ];
        let (p, a) = attrs(
            &names,
            &[
                (1, &[0, 9]),
                (2, &[]),
                (3, &[]),
                (4, &[0, 4]),
                (5, &[0, 5]),
                (6, b"SMAP\n"),
                (7, &[0, 2, 0, 1, 0, 2]),
                (8, &[0, 7]),
                (9, &[1, 2, 3]),
                (10, &[0, 1, 0, 0]),
                (11, &[2, 0, 1, 0, 0x10, 0, 0, 0x80, 0]),
                (12, &[0, 3]),
                // Damaged: too long, too short, bad name.
                (1, &[0, 9, 0]),
                (4, &[0]),
                (99, &[]),
            ],
        );
        let v = parse(&p, &a);
        let values: Vec<&AttributeValue<'_>> = v.iter().map(|a| &a.value).collect();
        assert_eq!(values[0], &AttributeValue::ConstantValue(9));
        assert_eq!(values[1], &AttributeValue::Synthetic);
        assert_eq!(values[2], &AttributeValue::Deprecated);
        assert_eq!(values[3], &AttributeValue::Signature(4));
        assert_eq!(values[4], &AttributeValue::SourceFile(5));
        assert_eq!(values[5], &AttributeValue::SourceDebugExtension(b"SMAP\n"));
        assert_eq!(values[6], &AttributeValue::Exceptions(vec![1, 2]));
        assert_eq!(values[7], &AttributeValue::NestHost(7));
        assert_eq!(values[8], &AttributeValue::Unknown);
        assert_eq!(
            values[9],
            &AttributeValue::EnclosingMethod {
                class: 1,
                method: 0
            }
        );
        assert_eq!(
            values[10],
            &AttributeValue::MethodParameters(vec![
                MethodParameter {
                    name: 1,
                    flags: 0x10
                },
                MethodParameter {
                    name: 0,
                    flags: 0x8000
                }
            ])
        );
        assert_eq!(values[11], &AttributeValue::ModuleTarget(3));
        assert!(matches!(values[12], AttributeValue::Malformed(_)));
        assert!(matches!(values[13], AttributeValue::Malformed(_)));
        assert!(matches!(values[14], AttributeValue::Malformed(_)));
        assert_eq!(v.malformed().len(), 3);
        assert_eq!(v.signature(), Some(4));
        assert_eq!(v[0].size(), 8);
        assert_eq!(v[0].offset, 2);
    }

    #[test]
    fn code_and_its_attributes() {
        let names = [
            "Code",
            "LineNumberTable",
            "StackMapTable",
            "LocalVariableTable",
        ];
        let mut code = vec![0, 2, 0, 3, 0, 0, 0, 1, 0xB1]; // stack 2, locals 3, return
        code.extend_from_slice(&[0, 1, 0, 0, 0, 1, 0, 1, 0, 0]); // one handler
        code.extend_from_slice(&[0, 3]);
        code.extend_from_slice(&[0, 2, 0, 0, 0, 6, 0, 1, 0, 0, 0, 7]);
        code.extend_from_slice(&[0, 3, 0, 0, 0, 3, 0, 1, 0]);
        code.extend_from_slice(&[0, 4, 0, 0, 0, 12, 0, 1, 0, 0, 0, 1, 0, 2, 0, 3, 0, 0]);
        let (p, a) = attrs(&names, &[(1, &code)]);
        let v = parse(&p, &a);
        let c = v.code().unwrap();
        assert_eq!((c.max_stack, c.max_locals, c.code), (2, 3, &[0xB1][..]));
        assert_eq!(c.code_offset, 2 + 6 + 8);
        assert_eq!(c.exception_table[0].handler_pc, 1);
        assert_eq!(
            c.attributes[0].value,
            AttributeValue::LineNumberTable(vec![LineNumber {
                start_pc: 0,
                line: 7
            }])
        );
        assert!(matches!(&c.attributes[1].value, AttributeValue::StackMapTable(f) if f.len() == 1));
        assert!(
            matches!(&c.attributes[2].value, AttributeValue::LocalVariableTable(l) if l[0].descriptor == 3)
        );
        assert!(v.malformed().is_empty());
        // A Code inside a Code: refused, not recursed into.
        let len = u32::try_from(code.len()).unwrap();
        let mut outer = code[..code.len() - 41].to_vec();
        outer.extend_from_slice(&[0, 1, 0, 1]);
        outer.extend_from_slice(&len.to_be_bytes());
        outer.extend_from_slice(&code);
        let (p, a) = attrs(&names, &[(1, &outer)]);
        let v = parse(&p, &a);
        assert!(matches!(
            v.code().unwrap().attributes[0].value,
            AttributeValue::Malformed(_)
        ));
    }

    #[test]
    fn module_and_record() {
        let names = ["Module", "Record", "Signature", "ModuleHashes"];
        let module = [
            0, 1, 0, 0x20, 0, 0, // name #1, open, no version
            0, 1, 0, 2, 0x80, 0, 0, 3, // requires #2 mandated, version #3
            0, 1, 0, 4, 0, 0, 0, 1, 0, 5, // exports #4 to #5
            0, 0, // opens
            0, 1, 0, 6, // uses #6
            0, 1, 0, 7, 0, 2, 0, 8, 0, 9, // provides #7 with #8, #9
        ];
        let record = [0, 1, 0, 1, 0, 2, 0, 1, 0, 3, 0, 0, 0, 2, 0, 9];
        let hashes = [0, 1, 0, 1, 0, 2, 0, 2, 0xAB, 0xCD];
        let (p, a) = attrs(&names, &[(1, &module), (2, &record), (4, &hashes)]);
        let v = parse(&p, &a);
        let AttributeValue::Module(m) = &v[0].value else {
            panic!("{:?}", v[0].value)
        };
        assert_eq!(m.flags, 0x20);
        assert_eq!(m.requires[0].version, 3);
        assert_eq!(m.exports[0].to, [5]);
        assert!(m.opens.is_empty());
        assert_eq!(m.provides[0].with, [8, 9]);
        let AttributeValue::Record(r) = &v[1].value else {
            panic!()
        };
        assert_eq!(r[0].attributes.signature(), Some(9));
        assert_eq!(
            v[2].value,
            AttributeValue::ModuleHashes {
                algorithm: 1,
                hashes: vec![(2, &[0xAB, 0xCD][..])]
            }
        );
    }
}
