//! Blobs that describe values: custom attribute values (II.23.3),
//! declarative security permission sets (II.23.1.3, II.22.11), and
//! marshalling descriptors (II.23.4).
//!
//! A custom attribute's value can't be decoded without the types of its
//! constructor's parameters, and an enum's needs the enum's underlying
//! type, which only the assembly that defines it knows. Enums this module
//! defines are looked up (by their `value__` field); any other is taken to
//! be `int32` (as most are), as System.Reflection.Metadata's decoder does
//! when its provider can't say. The type names here are rendered as
//! [`crate::names`] renders them.

use std::collections::HashMap;
use std::fmt::Write;

use turbonop_core::{Error, ErrorKind, Result};

use crate::metadata::Metadata;
use crate::names::quote;
use crate::reader::Cursor;
use crate::signature::{Primitive, TypeSig, element_type as E, parse_field_sig, parse_method_sig};
use crate::tables::{CustomAttributeRow, FieldRow, MemberRefRow, MethodDefRow, TableId, Token};

/// The type of a custom attribute argument.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum CaType {
    /// A primitive: `bool`, `char`, the integers, the floats, `string`.
    Primitive(Primitive),
    /// `System.Type` (a type's serialized name).
    Type(String),
    /// `object` (each value carries its own type).
    Object,
    /// An enum, by its name, and its underlying type.
    Enum(String, Primitive),
    /// A single-dimension array.
    SzArray(Box<CaType>),
}

impl CaType {
    /// Its name (as the signature names it, or the serialized name).
    pub fn name(&self) -> String {
        match self {
            CaType::Primitive(p) => p.il_name().into(),
            CaType::Type(n) | CaType::Enum(n, _) => n.clone(),
            CaType::Object => "object".into(),
            CaType::SzArray(e) => format!("{}[]", e.name()),
        }
    }
}

/// A custom attribute argument's value.
#[derive(Clone, Debug, PartialEq)]
pub enum CaValue {
    /// A `bool`.
    Bool(bool),
    /// A `char`.
    Char(u16),
    /// A signed integer (or an enum's value of a signed type).
    Int(i64),
    /// An unsigned integer.
    UInt(u64),
    /// A `float32`'s bits.
    Float32(u32),
    /// A `float64`'s bits.
    Float64(u64),
    /// A string (or a `System.Type`'s name); `None` for null.
    String(Option<String>),
    /// An array; `None` for null.
    Array(Option<Vec<CaArgument>>),
}

/// A custom attribute argument: its type and value.
#[derive(Clone, Debug, PartialEq)]
pub struct CaArgument {
    /// Its type (for an `object` parameter, the value's own).
    pub ty: CaType,
    /// Its value.
    pub value: CaValue,
}

/// A named argument: a field or property set.
#[derive(Clone, Debug, PartialEq)]
pub struct CaNamedArgument {
    /// A field (else a property).
    pub is_field: bool,
    /// Its name.
    pub name: String,
    /// Its value.
    pub arg: CaArgument,
}

/// A custom attribute's value: the constructor's arguments and the named
/// ones.
#[derive(Clone, Debug, PartialEq, Default)]
pub struct CustomAttributeValue {
    /// The constructor's arguments.
    pub fixed: Vec<CaArgument>,
    /// The fields and properties set.
    pub named: Vec<CaNamedArgument>,
}

fn arg_text(a: &CaArgument) -> String {
    let v = match &a.value {
        CaValue::Bool(b) => if *b { "true" } else { "false" }.to_string(),
        CaValue::Char(c) => c.to_string(),
        CaValue::Int(i) => i.to_string(),
        CaValue::UInt(u) => u.to_string(),
        CaValue::Float32(b) => format!("{b:#010x}"),
        CaValue::Float64(b) => format!("{b:#018x}"),
        CaValue::String(None) | CaValue::Array(None) => "null".into(),
        CaValue::String(Some(s)) => quote(s),
        CaValue::Array(Some(v)) => {
            format!(
                "[{}]",
                v.iter().map(arg_text).collect::<Vec<_>>().join(", ")
            )
        }
    };
    format!("{}({v})", a.ty.name())
}

impl CaArgument {
    /// As text: `type(value)`, as the oracle renders it (`int32(7)`,
    /// `string("x")`, `int32[]([int32(1)])`, floats as their bits).
    pub fn text(&self) -> String {
        arg_text(self)
    }
}

impl CustomAttributeValue {
    /// As text: `(fixed, ...) {field F=..., property P=...}`.
    pub fn text(&self) -> String {
        let mut s = format!(
            "({})",
            self.fixed
                .iter()
                .map(arg_text)
                .collect::<Vec<_>>()
                .join(", ")
        );
        if !self.named.is_empty() {
            let named: Vec<String> = self
                .named
                .iter()
                .map(|n| {
                    format!(
                        "{} {}={}",
                        if n.is_field { "field" } else { "property" },
                        n.name,
                        arg_text(&n.arg)
                    )
                })
                .collect();
            let _ = write!(s, " {{{}}}", named.join(", "));
        }
        s
    }
}

/// The underlying types of the enums a module defines, by the names
/// custom attributes give them (as signatures render them, and
/// serialized: `Ns.Outer+Inner`).
#[derive(Clone, Debug, Default)]
pub struct EnumTypes {
    by_name: HashMap<String, Primitive>,
}

impl EnumTypes {
    /// The enums `md` defines.
    pub fn of(md: &Metadata<'_>) -> Self {
        let mut by_name = HashMap::new();
        for (rid, _) in md.tables().iter::<crate::tables::TypeDefRow>() {
            for f in md.type_fields(rid) {
                let Some(row) = md.row::<FieldRow>(f) else {
                    continue;
                };
                if row.flags & 0x0010 != 0 || md.string(row.name) != "value__" {
                    continue;
                }
                let Some(p) = md
                    .blob(row.signature)
                    .ok()
                    .and_then(|b| parse_field_sig(b).ok())
                    .and_then(|t| match t {
                        TypeSig::Primitive(p) if is_integral(p) => Some(p),
                        _ => None,
                    })
                else {
                    break;
                };
                if let Ok(name) = md.type_def_name(rid) {
                    by_name.insert(format!("valuetype {name}"), p);
                    by_name.insert(name, p);
                }
                if let Ok(name) = serialized_name(md, rid) {
                    by_name.insert(name, p);
                }
                break;
            }
        }
        Self { by_name }
    }

    /// Adds the enums another assembly (`other`, its metadata) defines,
    /// as references to it name them: `[scope]Ns.Name` (a TypeRef whose
    /// scope is the AssemblyRef `scope`), and serialized (`Ns.Name`, which
    /// an assembly-qualified name matches up to its comma). For a facade,
    /// `scope` is the facade's name and `other` the assembly it forwards
    /// to.
    pub fn add_assembly(&mut self, other: &Metadata<'_>, scope: &str) {
        let theirs = Self::of(other);
        for (name, p) in theirs.by_name {
            if name.starts_with("valuetype ") {
                continue;
            }
            if name.contains('+') || !name.contains('/') {
                // Serialized (Ns.Outer+Inner), or a top-level type's name.
                self.by_name.entry(name.clone()).or_insert(p);
            }
            if !name.contains('+') {
                self.by_name.entry(format!("[{scope}]{name}")).or_insert(p);
            }
        }
    }

    /// The underlying type of the enum named `name` (int32 if it isn't
    /// one of these).
    pub fn underlying(&self, name: &str) -> Primitive {
        if let Some(&p) = self.by_name.get(name) {
            return p;
        }
        if let Some(comma) = name.find(',')
            && let Some(&p) = self.by_name.get(name[..comma].trim())
        {
            return p;
        }
        Primitive::I4
    }
}

fn is_integral(p: Primitive) -> bool {
    matches!(
        p,
        Primitive::Boolean
            | Primitive::Char
            | Primitive::I1
            | Primitive::U1
            | Primitive::I2
            | Primitive::U2
            | Primitive::I4
            | Primitive::U4
            | Primitive::I8
            | Primitive::U8
    )
}

/// TypeDef `rid`'s name as reflection serializes it: `Ns.Outer+Inner`.
pub fn serialized_name(md: &Metadata<'_>, rid: u32) -> Result<String> {
    let mut parts = Vec::new();
    let mut t = rid;
    for _ in 0..64 {
        let row: crate::tables::TypeDefRow =
            md.row(t)
                .ok_or(Error::new(ErrorKind::Malformed, u64::from(t), "TypeDef"))?;
        match md.enclosing_type(t) {
            Some(outer) => {
                parts.push(md.string(row.type_name).into_owned());
                t = outer;
            }
            None => {
                let ns = md.string(row.type_namespace);
                let name = md.string(row.type_name);
                parts.push(if ns.is_empty() {
                    name.into_owned()
                } else {
                    format!("{ns}.{name}")
                });
                parts.reverse();
                return Ok(parts.join("+"));
            }
        }
    }
    Err(Error::new(
        ErrorKind::Malformed,
        u64::from(rid),
        "names nest too deeply",
    ))
}

const MAX_CA_DEPTH: usize = 16;

fn bad(c: &Cursor<'_>, what: &'static str) -> Error {
    c.error(ErrorKind::Malformed, what)
}

fn ser_string(c: &mut Cursor<'_>) -> Result<Option<String>> {
    if c.peek() == Some(0xFF) {
        c.u8("serialized string")?;
        return Ok(None);
    }
    let n = c.compressed_u32("serialized string length")? as usize;
    Ok(Some(
        String::from_utf8_lossy(c.bytes(n, "serialized string")?).into_owned(),
    ))
}

/// A FieldOrPropType (a named argument's, or a boxed value's).
fn tagged_type(c: &mut Cursor<'_>, enums: &EnumTypes, depth: usize) -> Result<CaType> {
    if depth > MAX_CA_DEPTH {
        return Err(bad(c, "custom attribute type nesting"));
    }
    let tag = c.u8("custom attribute type")?;
    Ok(match tag {
        E::BOOLEAN..=E::STRING => {
            CaType::Primitive(Primitive::from_element_type(tag).ok_or_else(|| bad(c, "type"))?)
        }
        E::SYSTEM_TYPE => CaType::Type("System.Type".into()),
        E::BOXED => CaType::Object,
        E::ENUM => {
            let name = ser_string(c)?.ok_or_else(|| bad(c, "enum type name"))?;
            let u = enums.underlying(&name);
            CaType::Enum(name, u)
        }
        E::SZARRAY => CaType::SzArray(Box::new(tagged_type(c, enums, depth + 1)?)),
        _ => return Err(bad(c, "custom attribute type")),
    })
}

fn primitive_value(c: &mut Cursor<'_>, p: Primitive) -> Result<CaValue> {
    use Primitive as P;
    let w = "custom attribute value";
    Ok(match p {
        P::Boolean => CaValue::Bool(c.u8(w)? != 0),
        P::Char => CaValue::Char(c.u16(w)?),
        P::I1 => CaValue::Int(i64::from(c.u8(w)?.cast_signed())),
        P::U1 => CaValue::UInt(u64::from(c.u8(w)?)),
        P::I2 => CaValue::Int(i64::from(c.u16(w)?.cast_signed())),
        P::U2 => CaValue::UInt(u64::from(c.u16(w)?)),
        P::I4 => CaValue::Int(i64::from(c.i32(w)?)),
        P::U4 => CaValue::UInt(u64::from(c.u32(w)?)),
        P::I8 => CaValue::Int(c.u64(w)?.cast_signed()),
        P::U8 => CaValue::UInt(c.u64(w)?),
        P::R4 => CaValue::Float32(c.u32(w)?),
        P::R8 => CaValue::Float64(c.u64(w)?),
        P::String => CaValue::String(ser_string(c)?),
        _ => return Err(bad(c, "custom attribute value type")),
    })
}

fn argument(
    c: &mut Cursor<'_>,
    ty: &CaType,
    enums: &EnumTypes,
    depth: usize,
) -> Result<CaArgument> {
    if depth > MAX_CA_DEPTH {
        return Err(bad(c, "custom attribute value nesting"));
    }
    let ty = match ty {
        CaType::Object => tagged_type(c, enums, depth + 1)?,
        t => t.clone(),
    };
    let value = match &ty {
        CaType::Primitive(p) => primitive_value(c, *p)?,
        CaType::Enum(_, p) => primitive_value(c, *p)?,
        CaType::Type(_) => CaValue::String(ser_string(c)?),
        CaType::Object => return Err(bad(c, "a boxed object in a boxed object")),
        CaType::SzArray(elem) => {
            let n = c.i32("custom attribute array length")?;
            if n == -1 {
                CaValue::Array(None)
            } else if n < 0 {
                return Err(bad(c, "custom attribute array length"));
            } else {
                #[allow(clippy::cast_sign_loss)]
                let n = c.check_count(n as usize, 1, "custom attribute array length")?;
                let mut v = Vec::with_capacity(n);
                for _ in 0..n {
                    v.push(argument(c, elem, enums, depth + 1)?);
                }
                CaValue::Array(Some(v))
            }
        }
    };
    Ok(CaArgument { ty, value })
}

fn is_system_type(name: &str) -> bool {
    name == "System.Type" || (name.starts_with('[') && name.ends_with("]System.Type"))
}

/// A constructor parameter's type as a custom attribute argument's.
fn param_type(md: &Metadata<'_>, t: &TypeSig, enums: &EnumTypes, depth: usize) -> Result<CaType> {
    if depth > MAX_CA_DEPTH {
        return Err(Error::new(
            ErrorKind::Malformed,
            0,
            "custom attribute type nesting",
        ));
    }
    Ok(match t {
        TypeSig::Primitive(Primitive::Object) => CaType::Object,
        TypeSig::Primitive(p)
            if is_integral(*p)
                || matches!(p, Primitive::R4 | Primitive::R8 | Primitive::String) =>
        {
            CaType::Primitive(*p)
        }
        TypeSig::Class(tok) | TypeSig::ValueType(tok)
            if matches!(tok.table(), Some(TableId::TypeDef | TableId::TypeRef)) =>
        {
            let name = md.type_token_name(*tok);
            if is_system_type(&name) {
                CaType::Type(name)
            } else {
                let u = enums.underlying(&name);
                CaType::Enum(name, u)
            }
        }
        TypeSig::SzArray(e) => CaType::SzArray(Box::new(param_type(md, e, enums, depth + 1)?)),
        _ => {
            return Err(Error::new(
                ErrorKind::Malformed,
                0,
                "a custom attribute parameter type",
            ));
        }
    })
}

/// Decodes a value blob given its constructor's parameter types.
pub fn decode_value(
    blob: &[u8],
    params: &[CaType],
    enums: &EnumTypes,
) -> Result<CustomAttributeValue> {
    let mut c = Cursor::new(blob);
    if c.u16("custom attribute prolog")? != 1 {
        return Err(bad(&c, "custom attribute prolog"));
    }
    let mut v = CustomAttributeValue::default();
    for p in params {
        v.fixed.push(argument(&mut c, p, enums, 0)?);
    }
    let n = c.u16("custom attribute named argument count")?;
    for _ in 0..n {
        v.named.push(named_argument(&mut c, enums)?);
    }
    Ok(v)
}

fn named_argument(c: &mut Cursor<'_>, enums: &EnumTypes) -> Result<CaNamedArgument> {
    let is_field = match c.u8("named argument kind")? {
        E::FIELD => true,
        E::PROPERTY => false,
        _ => return Err(bad(c, "named argument kind")),
    };
    let ty = tagged_type(c, enums, 0)?;
    let name = ser_string(c)?.unwrap_or_default();
    let arg = argument(c, &ty, enums, 0)?;
    Ok(CaNamedArgument {
        is_field,
        name,
        arg,
    })
}

impl Metadata<'_> {
    /// The parameter types of a custom attribute's constructor (a
    /// MethodDef or MemberRef).
    pub fn attribute_params(&self, ctor: Token, enums: &EnumTypes) -> Result<Vec<CaType>> {
        let sig = match ctor.table() {
            Some(TableId::MethodDef) => self.row::<MethodDefRow>(ctor.rid()).map(|m| m.signature),
            Some(TableId::MemberRef) => self.row::<MemberRefRow>(ctor.rid()).map(|m| m.signature),
            _ => None,
        }
        .ok_or(Error::new(
            ErrorKind::Malformed,
            u64::from(ctor.0),
            "attribute constructor",
        ))?;
        let sig = parse_method_sig(self.blob(sig)?)?;
        sig.params
            .iter()
            .map(|p| param_type(self, p, enums, 0))
            .collect()
    }

    /// CustomAttribute `rid`'s value, decoded (see [`EnumTypes`]).
    pub fn custom_attribute_value(
        &self,
        rid: u32,
        enums: &EnumTypes,
    ) -> Result<CustomAttributeValue> {
        let row: CustomAttributeRow = self.row(rid).ok_or(Error::new(
            ErrorKind::Malformed,
            u64::from(rid),
            "CustomAttribute",
        ))?;
        let params = self.attribute_params(row.ty.token_or_null(), enums)?;
        decode_value(self.blob(row.value)?, &params, enums)
    }
}

/// A declarative security permission set.
#[derive(Clone, Debug, PartialEq)]
pub enum PermissionSet {
    /// The .NET 1.x form: XML, in UTF-16.
    Xml(String),
    /// The .NET 2.0 form (`.`): security attributes and their properties.
    Attributes(Vec<SecurityAttribute>),
}

/// A security attribute in a permission set.
#[derive(Clone, Debug, PartialEq)]
pub struct SecurityAttribute {
    /// Its type's serialized name.
    pub type_name: String,
    /// The fields and properties it sets.
    pub named: Vec<CaNamedArgument>,
}

/// Decodes a DeclSecurity row's permission set.
pub fn decode_permission_set(blob: &[u8], enums: &EnumTypes) -> Result<PermissionSet> {
    let mut c = Cursor::new(blob);
    if c.peek() != Some(b'.') {
        let units: Vec<u16> = blob
            .as_chunks::<2>()
            .0
            .iter()
            .map(|&b| u16::from_le_bytes(b))
            .collect();
        return Ok(PermissionSet::Xml(String::from_utf16_lossy(&units)));
    }
    c.u8("permission set")?;
    let n = c.count(2, "permission set attribute count")?;
    let mut out = Vec::with_capacity(n);
    for _ in 0..n {
        let type_name = ser_string(&mut c)?.unwrap_or_default();
        let len = c.compressed_u32("security attribute blob length")? as usize;
        let body = c.bytes(len, "security attribute")?;
        let mut b = Cursor::new(body);
        let count = b.count(1, "security attribute property count")?;
        let mut named = Vec::with_capacity(count);
        for _ in 0..count {
            named.push(named_argument(&mut b, enums)?);
        }
        out.push(SecurityAttribute { type_name, named });
    }
    Ok(PermissionSet::Attributes(out))
}

/// Native types (`NATIVE_TYPE_*`, II.23.4 and the runtime's).
#[allow(missing_docs)]
pub mod native_type {
    pub const BOOLEAN: u8 = 0x02;
    pub const I1: u8 = 0x03;
    pub const U1: u8 = 0x04;
    pub const I2: u8 = 0x05;
    pub const U2: u8 = 0x06;
    pub const I4: u8 = 0x07;
    pub const U4: u8 = 0x08;
    pub const I8: u8 = 0x09;
    pub const U8: u8 = 0x0A;
    pub const R4: u8 = 0x0B;
    pub const R8: u8 = 0x0C;
    pub const CURRENCY: u8 = 0x0F;
    pub const BSTR: u8 = 0x13;
    pub const LPSTR: u8 = 0x14;
    pub const LPWSTR: u8 = 0x15;
    pub const LPTSTR: u8 = 0x16;
    pub const FIXEDSYSSTRING: u8 = 0x17;
    pub const IUNKNOWN: u8 = 0x19;
    pub const IDISPATCH: u8 = 0x1A;
    pub const STRUCT: u8 = 0x1B;
    pub const INTERFACE: u8 = 0x1C;
    pub const SAFEARRAY: u8 = 0x1D;
    pub const FIXEDARRAY: u8 = 0x1E;
    pub const INT: u8 = 0x1F;
    pub const UINT: u8 = 0x20;
    pub const BYVALSTR: u8 = 0x22;
    pub const ANSIBSTR: u8 = 0x23;
    pub const TBSTR: u8 = 0x24;
    pub const VARIANTBOOL: u8 = 0x25;
    pub const FUNC: u8 = 0x26;
    pub const ASANY: u8 = 0x28;
    pub const ARRAY: u8 = 0x2A;
    pub const LPSTRUCT: u8 = 0x2B;
    pub const CUSTOMMARSHALER: u8 = 0x2C;
    pub const ERROR: u8 = 0x2D;
    pub const IINSPECTABLE: u8 = 0x2E;
    pub const HSTRING: u8 = 0x2F;
    pub const LPUTF8STR: u8 = 0x30;
    pub const MAX: u8 = 0x50;
}

/// A native type's name (`UnmanagedType`'s).
pub fn native_type_name(t: u8) -> Option<&'static str> {
    use native_type as N;
    Some(match t {
        N::BOOLEAN => "Bool",
        N::I1 => "I1",
        N::U1 => "U1",
        N::I2 => "I2",
        N::U2 => "U2",
        N::I4 => "I4",
        N::U4 => "U4",
        N::I8 => "I8",
        N::U8 => "U8",
        N::R4 => "R4",
        N::R8 => "R8",
        N::CURRENCY => "Currency",
        N::BSTR => "BStr",
        N::LPSTR => "LPStr",
        N::LPWSTR => "LPWStr",
        N::LPTSTR => "LPTStr",
        N::FIXEDSYSSTRING => "ByValTStr",
        N::IUNKNOWN => "IUnknown",
        N::IDISPATCH => "IDispatch",
        N::STRUCT => "Struct",
        N::INTERFACE => "Interface",
        N::SAFEARRAY => "SafeArray",
        N::FIXEDARRAY => "ByValArray",
        N::INT => "SysInt",
        N::UINT => "SysUInt",
        N::BYVALSTR => "VBByRefStr",
        N::ANSIBSTR => "AnsiBStr",
        N::TBSTR => "TBStr",
        N::VARIANTBOOL => "VariantBool",
        N::FUNC => "FunctionPtr",
        N::ASANY => "AsAny",
        N::ARRAY => "LPArray",
        N::LPSTRUCT => "LPStruct",
        N::CUSTOMMARSHALER => "CustomMarshaler",
        N::ERROR => "Error",
        N::IINSPECTABLE => "IInspectable",
        N::HSTRING => "HString",
        N::LPUTF8STR => "LPUTF8Str",
        _ => return None,
    })
}

/// A marshalling descriptor.
#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub struct MarshalDescriptor {
    /// The native type.
    pub native_type: u8,
    /// An array's element type (`LPArray`, `ByValArray`'s ArraySubType).
    pub element_type: Option<u8>,
    /// The parameter that holds the count (`LPArray`'s SizeParamIndex).
    pub param_index: Option<u32>,
    /// The count (`SizeConst`).
    pub count: Option<u32>,
    /// `LPArray`'s flags (whether SizeParamIndex was given).
    pub array_flags: Option<u32>,
    /// A safe array's VARTYPE.
    pub safe_array_type: Option<u32>,
    /// A safe array's user-defined element type.
    pub safe_array_user_type: Option<String>,
    /// An interface's IID parameter (`IidParameterIndex`).
    pub iid_param_index: Option<u32>,
    /// A custom marshaler's GUID, native type name, marshaler type, and
    /// cookie.
    pub custom: Option<[String; 4]>,
}

/// Decodes a FieldMarshal row's descriptor.
pub fn decode_marshal(blob: &[u8]) -> Result<MarshalDescriptor> {
    use native_type as N;
    let mut c = Cursor::new(blob);
    let mut d = MarshalDescriptor {
        native_type: c.u8("native type")?,
        ..MarshalDescriptor::default()
    };
    let opt = |c: &mut Cursor<'_>| -> Result<Option<u32>> {
        if c.is_empty() {
            Ok(None)
        } else {
            c.compressed_u32("marshal descriptor").map(Some)
        }
    };
    match d.native_type {
        N::ARRAY => {
            if !c.is_empty() {
                d.element_type = Some(c.u8("array element type")?);
            }
            d.param_index = opt(&mut c)?;
            d.count = opt(&mut c)?;
            d.array_flags = opt(&mut c)?;
        }
        N::FIXEDARRAY => {
            d.count = opt(&mut c)?;
            if !c.is_empty() {
                d.element_type = Some(c.u8("array element type")?);
            }
        }
        N::FIXEDSYSSTRING => d.count = opt(&mut c)?,
        N::SAFEARRAY => {
            d.safe_array_type = opt(&mut c)?;
            if !c.is_empty() {
                d.safe_array_user_type = ser_string(&mut c)?;
            }
        }
        N::INTERFACE | N::IUNKNOWN | N::IDISPATCH => d.iid_param_index = opt(&mut c)?,
        N::CUSTOMMARSHALER => {
            let mut s = || -> Result<String> { Ok(ser_string(&mut c)?.unwrap_or_default()) };
            d.custom = Some([s()?, s()?, s()?, s()?]);
        }
        _ => {}
    }
    Ok(d)
}

#[cfg(test)]
#[allow(clippy::cast_possible_truncation)]
mod tests {
    use super::*;

    fn prim(p: Primitive) -> CaType {
        CaType::Primitive(p)
    }

    #[test]
    fn fixed_and_named_arguments() {
        let enums = EnumTypes::default();
        // ("ab", 7, null string, int32[] {1, 2}) {field F = (object)5,
        // property P = Ns.E(3)}
        let mut b = vec![
            1, 0, 2, b'a', b'b', 7, 0, 0, 0, 0xFF, 2, 0, 0, 0, 1, 0, 0, 0, 2, 0, 0, 0,
        ];
        b.extend_from_slice(&[2, 0]);
        b.extend_from_slice(&[E::FIELD, E::BOXED, 1, b'F', E::I4, 5, 0, 0, 0]);
        b.extend_from_slice(&[
            E::PROPERTY,
            E::ENUM,
            4,
            b'N',
            b's',
            b'.',
            b'E',
            1,
            b'P',
            3,
            0,
            0,
            0,
        ]);
        let params = [
            prim(Primitive::String),
            prim(Primitive::I4),
            prim(Primitive::String),
            CaType::SzArray(Box::new(prim(Primitive::I4))),
        ];
        let v = decode_value(&b, &params, &enums).unwrap();
        assert_eq!(
            v.text(),
            r#"(string("ab"), int32(7), string(null), int32[]([int32(1), int32(2)])) {field F=int32(5), property P=Ns.E(3)}"#
        );
        for n in 0..b.len() {
            assert!(decode_value(&b[..n], &params, &enums).is_err(), "{n}");
        }
        assert!(decode_value(&[2, 0, 0, 0], &[], &enums).is_err());
        // A huge array length.
        assert!(
            decode_value(
                &[1, 0, 0xFF, 0xFF, 0xFF, 0x7F],
                &[params[3].clone()],
                &enums
            )
            .is_err()
        );
        // Nested boxed arrays: object[] holding object[]s, deeper than
        // the limit.
        let mut deep = vec![1, 0];
        for _ in 0..100 {
            deep.extend_from_slice(&[E::SZARRAY]);
        }
        deep.push(E::I4);
        assert!(decode_value(&deep, &[CaType::Object], &enums).is_err());
    }

    #[test]
    fn permission_sets_and_marshal() {
        let enums = EnumTypes::default();
        let mut b = vec![b'.', 1, 3, b'S', b'e', b'c'];
        let props = [1, E::PROPERTY, E::STRING, 1, b'L', 1, b'h'];
        b.push(props.len() as u8);
        b.extend_from_slice(&props);
        let PermissionSet::Attributes(a) = decode_permission_set(&b, &enums).unwrap() else {
            panic!()
        };
        assert_eq!(a[0].type_name, "Sec");
        assert_eq!(a[0].named[0].arg.text(), r#"string("h")"#);
        for n in 1..b.len() {
            assert!(decode_permission_set(&b[..n], &enums).is_err(), "{n}");
        }
        assert_eq!(
            decode_permission_set(&[b'<', 0, b'x', 0], &enums).unwrap(),
            PermissionSet::Xml("<x".into())
        );
        let m = decode_marshal(&[native_type::ARRAY, native_type::I4, 1]).unwrap();
        assert_eq!(
            (m.element_type, m.param_index, m.count),
            (Some(7), Some(1), None)
        );
        let m = decode_marshal(&[native_type::FIXEDARRAY, 4, native_type::I4]).unwrap();
        assert_eq!((m.count, m.element_type), (Some(4), Some(7)));
        let m = decode_marshal(&[native_type::CUSTOMMARSHALER, 0, 0, 1, b'M', 0]).unwrap();
        assert_eq!(m.custom.unwrap()[2], "M");
        assert!(decode_marshal(&[native_type::CUSTOMMARSHALER, 0]).is_err());
        assert!(decode_marshal(&[]).is_err());
        assert_eq!(native_type_name(native_type::LPWSTR), Some("LPWStr"));
    }
}
