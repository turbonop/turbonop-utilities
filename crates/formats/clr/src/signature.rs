//! Signatures (II.23.2): of methods (and the method pointers in them, and
//! `calli`'s stand-alone ones), fields, properties, locals, type
//! specifications, and method specifications (generic instantiations).
//!
//! Types nest (pointers to arrays of generic instantiations, function
//! pointers taking function pointers); a nesting deeper than
//! [`MAX_DEPTH`] is an error, as is any count larger than the bytes left
//! could hold.

use turbonop_core::{ErrorKind, Result};

use crate::reader::Cursor;
use crate::tables::{TableId, Token};

/// The deepest a type may nest.
pub const MAX_DEPTH: usize = 128;

/// Element types (`ELEMENT_TYPE_*`).
#[allow(missing_docs)]
pub mod element_type {
    pub const END: u8 = 0x00;
    pub const VOID: u8 = 0x01;
    pub const BOOLEAN: u8 = 0x02;
    pub const CHAR: u8 = 0x03;
    pub const I1: u8 = 0x04;
    pub const U1: u8 = 0x05;
    pub const I2: u8 = 0x06;
    pub const U2: u8 = 0x07;
    pub const I4: u8 = 0x08;
    pub const U4: u8 = 0x09;
    pub const I8: u8 = 0x0A;
    pub const U8: u8 = 0x0B;
    pub const R4: u8 = 0x0C;
    pub const R8: u8 = 0x0D;
    pub const STRING: u8 = 0x0E;
    pub const PTR: u8 = 0x0F;
    pub const BYREF: u8 = 0x10;
    pub const VALUETYPE: u8 = 0x11;
    pub const CLASS: u8 = 0x12;
    pub const VAR: u8 = 0x13;
    pub const ARRAY: u8 = 0x14;
    pub const GENERICINST: u8 = 0x15;
    pub const TYPEDBYREF: u8 = 0x16;
    pub const I: u8 = 0x18;
    pub const U: u8 = 0x19;
    pub const FNPTR: u8 = 0x1B;
    pub const OBJECT: u8 = 0x1C;
    pub const SZARRAY: u8 = 0x1D;
    pub const MVAR: u8 = 0x1E;
    pub const CMOD_REQD: u8 = 0x1F;
    pub const CMOD_OPT: u8 = 0x20;
    pub const INTERNAL: u8 = 0x21;
    pub const MODIFIER: u8 = 0x40;
    pub const SENTINEL: u8 = 0x41;
    pub const PINNED: u8 = 0x45;
    /// (Custom attributes.) `System.Type`.
    pub const SYSTEM_TYPE: u8 = 0x50;
    /// (Custom attributes.) A boxed value.
    pub const BOXED: u8 = 0x51;
    /// (Custom attributes.) A field.
    pub const FIELD: u8 = 0x53;
    /// (Custom attributes.) A property.
    pub const PROPERTY: u8 = 0x54;
    /// (Custom attributes.) An enum.
    pub const ENUM: u8 = 0x55;
}

/// Calling conventions (a signature's first byte).
#[allow(missing_docs)]
pub mod calling_convention {
    pub const DEFAULT: u8 = 0x00;
    pub const C: u8 = 0x01;
    pub const STDCALL: u8 = 0x02;
    pub const THISCALL: u8 = 0x03;
    pub const FASTCALL: u8 = 0x04;
    pub const VARARG: u8 = 0x05;
    pub const FIELD: u8 = 0x06;
    pub const LOCAL_SIG: u8 = 0x07;
    pub const PROPERTY: u8 = 0x08;
    pub const UNMANAGED: u8 = 0x09;
    pub const GENERIC_INST: u8 = 0x0A;
    pub const NATIVE_VARARG: u8 = 0x0B;
    /// The low four bits: the kind.
    pub const MASK: u8 = 0x0F;
    pub const GENERIC: u8 = 0x10;
    pub const HAS_THIS: u8 = 0x20;
    pub const EXPLICIT_THIS: u8 = 0x40;
}

/// The primitive types.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[allow(missing_docs)]
pub enum Primitive {
    Void,
    Boolean,
    Char,
    I1,
    U1,
    I2,
    U2,
    I4,
    U4,
    I8,
    U8,
    R4,
    R8,
    String,
    Object,
    IntPtr,
    UIntPtr,
    TypedReference,
}

impl Primitive {
    /// The primitive an element type is.
    pub fn from_element_type(e: u8) -> Option<Self> {
        use element_type as E;
        Some(match e {
            E::VOID => Self::Void,
            E::BOOLEAN => Self::Boolean,
            E::CHAR => Self::Char,
            E::I1 => Self::I1,
            E::U1 => Self::U1,
            E::I2 => Self::I2,
            E::U2 => Self::U2,
            E::I4 => Self::I4,
            E::U4 => Self::U4,
            E::I8 => Self::I8,
            E::U8 => Self::U8,
            E::R4 => Self::R4,
            E::R8 => Self::R8,
            E::STRING => Self::String,
            E::OBJECT => Self::Object,
            E::I => Self::IntPtr,
            E::U => Self::UIntPtr,
            E::TYPEDBYREF => Self::TypedReference,
            _ => return None,
        })
    }

    /// Its element type.
    pub fn element_type(self) -> u8 {
        use element_type as E;
        match self {
            Self::Void => E::VOID,
            Self::Boolean => E::BOOLEAN,
            Self::Char => E::CHAR,
            Self::I1 => E::I1,
            Self::U1 => E::U1,
            Self::I2 => E::I2,
            Self::U2 => E::U2,
            Self::I4 => E::I4,
            Self::U4 => E::U4,
            Self::I8 => E::I8,
            Self::U8 => E::U8,
            Self::R4 => E::R4,
            Self::R8 => E::R8,
            Self::String => E::STRING,
            Self::Object => E::OBJECT,
            Self::IntPtr => E::I,
            Self::UIntPtr => E::U,
            Self::TypedReference => E::TYPEDBYREF,
        }
    }

    /// Its name, as IL spells it (`int32`, `native int`, `typedref`).
    pub fn il_name(self) -> &'static str {
        match self {
            Self::Void => "void",
            Self::Boolean => "bool",
            Self::Char => "char",
            Self::I1 => "int8",
            Self::U1 => "uint8",
            Self::I2 => "int16",
            Self::U2 => "uint16",
            Self::I4 => "int32",
            Self::U4 => "uint32",
            Self::I8 => "int64",
            Self::U8 => "uint64",
            Self::R4 => "float32",
            Self::R8 => "float64",
            Self::String => "string",
            Self::Object => "object",
            Self::IntPtr => "native int",
            Self::UIntPtr => "native uint",
            Self::TypedReference => "typedref",
        }
    }

    /// Its size in bytes, for the fixed-size ones.
    pub fn size(self) -> Option<u32> {
        Some(match self {
            Self::Boolean | Self::I1 | Self::U1 => 1,
            Self::Char | Self::I2 | Self::U2 => 2,
            Self::I4 | Self::U4 | Self::R4 => 4,
            Self::I8 | Self::U8 | Self::R8 => 8,
            _ => return None,
        })
    }
}

/// An array's shape: its rank, and the sizes and lower bounds of its
/// first dimensions.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct ArrayShape {
    /// How many dimensions.
    pub rank: u32,
    /// Sizes of the first dimensions.
    pub sizes: Vec<u32>,
    /// Lower bounds of the first dimensions.
    pub lower_bounds: Vec<i32>,
}

/// A type in a signature.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum TypeSig {
    /// A primitive.
    Primitive(Primitive),
    /// `T*`.
    Ptr(Box<TypeSig>),
    /// `T&`.
    ByRef(Box<TypeSig>),
    /// A pinned local.
    Pinned(Box<TypeSig>),
    /// A value type (TypeDef, TypeRef, or TypeSpec token).
    ValueType(Token),
    /// A class.
    Class(Token),
    /// A type's generic parameter (`!n`).
    Var(u32),
    /// A method's generic parameter (`!!n`).
    MVar(u32),
    /// A single-dimension, zero-based array.
    SzArray(Box<TypeSig>),
    /// A general array.
    Array(Box<TypeSig>, ArrayShape),
    /// A generic type instantiated.
    GenericInst {
        /// Whether it's a value type (else a class).
        value_type: bool,
        /// The generic type.
        ty: Token,
        /// Its arguments.
        args: Vec<TypeSig>,
    },
    /// A function pointer.
    FnPtr(Box<MethodSig>),
    /// A type with a custom modifier (`modreq` or `modopt`).
    Modified {
        /// `modreq` (else `modopt`).
        required: bool,
        /// The modifier type.
        modifier: Token,
        /// The type modified.
        ty: Box<TypeSig>,
    },
}

/// A method signature (and a property's, whose kind is `PROPERTY`).
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct MethodSig {
    /// The first byte: the calling convention and `HASTHIS`,
    /// `EXPLICITTHIS`, `GENERIC` ([`calling_convention`]).
    pub header: u8,
    /// How many generic parameters (with `GENERIC`).
    pub generic_params: u32,
    /// The return type.
    pub ret: TypeSig,
    /// The parameters' types.
    pub params: Vec<TypeSig>,
    /// Where a vararg call site's extra arguments start (the `SENTINEL`).
    pub sentinel: Option<usize>,
}

impl MethodSig {
    /// The calling convention (the header's low four bits).
    pub fn kind(&self) -> u8 {
        self.header & calling_convention::MASK
    }

    /// Whether it takes `this`.
    pub fn has_this(&self) -> bool {
        self.header & calling_convention::HAS_THIS != 0
    }

    /// Whether `this` is the first parameter explicitly.
    pub fn explicit_this(&self) -> bool {
        self.header & calling_convention::EXPLICIT_THIS != 0
    }

    /// How many parameters are required (before the sentinel).
    pub fn required_params(&self) -> usize {
        self.sentinel.unwrap_or(self.params.len())
    }
}

/// A stand-alone signature (a StandAloneSig row's).
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum StandAloneSig {
    /// Locals (`LOCAL_SIG`).
    Locals(Vec<TypeSig>),
    /// A field's (C++/CLI writes these).
    Field(TypeSig),
    /// A method's (`calli`'s).
    Method(MethodSig),
}

fn type_def_or_ref(c: &mut Cursor<'_>) -> Result<Token> {
    let v = c.compressed_u32("TypeDefOrRef in a signature")?;
    let table = match v & 3 {
        0 => TableId::TypeDef,
        1 => TableId::TypeRef,
        2 => TableId::TypeSpec,
        _ => return Err(c.error(ErrorKind::Malformed, "TypeDefOrRef in a signature")),
    };
    Ok(Token::new(table, v >> 2))
}

fn ty(c: &mut Cursor<'_>, depth: usize) -> Result<TypeSig> {
    use element_type as E;
    if depth > MAX_DEPTH {
        return Err(c.error(ErrorKind::Malformed, "signature nesting"));
    }
    let d = depth + 1;
    let e = c.u8("element type")?;
    if let Some(p) = Primitive::from_element_type(e) {
        return Ok(TypeSig::Primitive(p));
    }
    Ok(match e {
        E::PTR => TypeSig::Ptr(Box::new(ty(c, d)?)),
        E::BYREF => TypeSig::ByRef(Box::new(ty(c, d)?)),
        E::PINNED => TypeSig::Pinned(Box::new(ty(c, d)?)),
        E::VALUETYPE => TypeSig::ValueType(type_def_or_ref(c)?),
        E::CLASS => TypeSig::Class(type_def_or_ref(c)?),
        E::VAR => TypeSig::Var(c.compressed_u32("generic parameter number")?),
        E::MVAR => TypeSig::MVar(c.compressed_u32("generic parameter number")?),
        E::SZARRAY => TypeSig::SzArray(Box::new(ty(c, d)?)),
        E::ARRAY => {
            let elem = ty(c, d)?;
            let rank = c.compressed_u32("array rank")?;
            let n = c.count(1, "array sizes")?;
            let sizes = (0..n)
                .map(|_| c.compressed_u32("array size"))
                .collect::<Result<_>>()?;
            let n = c.count(1, "array lower bounds")?;
            let lower_bounds = (0..n)
                .map(|_| c.compressed_i32("array lower bound"))
                .collect::<Result<_>>()?;
            TypeSig::Array(
                Box::new(elem),
                ArrayShape {
                    rank,
                    sizes,
                    lower_bounds,
                },
            )
        }
        E::GENERICINST => {
            let value_type = match c.u8("generic instantiation kind")? {
                E::VALUETYPE => true,
                E::CLASS => false,
                _ => return Err(c.error(ErrorKind::Malformed, "generic instantiation kind")),
            };
            let generic = type_def_or_ref(c)?;
            let n = c.count(1, "generic arguments")?;
            if n == 0 {
                return Err(c.error(ErrorKind::Malformed, "generic arguments"));
            }
            let args = (0..n).map(|_| ty(c, d)).collect::<Result<_>>()?;
            TypeSig::GenericInst {
                value_type,
                ty: generic,
                args,
            }
        }
        E::FNPTR => TypeSig::FnPtr(Box::new(method(c, d)?)),
        E::CMOD_REQD | E::CMOD_OPT => {
            let modifier = type_def_or_ref(c)?;
            TypeSig::Modified {
                required: e == E::CMOD_REQD,
                modifier,
                ty: Box::new(ty(c, d)?),
            }
        }
        _ => return Err(c.error(ErrorKind::Malformed, "element type")),
    })
}

fn method(c: &mut Cursor<'_>, depth: usize) -> Result<MethodSig> {
    use calling_convention as CC;
    let header = c.u8("method signature header")?;
    match header & CC::MASK {
        CC::FIELD | CC::LOCAL_SIG | CC::GENERIC_INST => {
            return Err(c.error(ErrorKind::Malformed, "method signature header"));
        }
        _ => {}
    }
    let generic_params = if header & CC::GENERIC != 0 {
        c.compressed_u32("generic parameter count")?
    } else {
        0
    };
    let n = c.count(1, "parameter count")?;
    let ret = ty(c, depth)?;
    let mut params = Vec::with_capacity(n);
    let mut sentinel = None;
    for i in 0..n {
        if c.peek() == Some(element_type::SENTINEL) {
            if sentinel.is_some() {
                return Err(c.error(ErrorKind::Malformed, "second sentinel"));
            }
            c.u8("sentinel")?;
            sentinel = Some(i);
        }
        params.push(ty(c, depth)?);
    }
    Ok(MethodSig {
        header,
        generic_params,
        ret,
        params,
        sentinel,
    })
}

/// A method signature (MethodDef's, MemberRef's for a method, or
/// `calli`'s), or a property's.
pub fn parse_method_sig(blob: &[u8]) -> Result<MethodSig> {
    method(&mut Cursor::new(blob), 0)
}

/// A field signature: `FIELD`, then the type (custom modifiers part of it).
pub fn parse_field_sig(blob: &[u8]) -> Result<TypeSig> {
    let mut c = Cursor::new(blob);
    if c.u8("field signature header")? & calling_convention::MASK != calling_convention::FIELD {
        return Err(c.error(ErrorKind::Malformed, "field signature header"));
    }
    ty(&mut c, 0)
}

/// A property signature: `PROPERTY` (perhaps with `HASTHIS`), the
/// parameter count, the type, the indexer's parameters.
pub fn parse_property_sig(blob: &[u8]) -> Result<MethodSig> {
    let sig = parse_method_sig(blob)?;
    if sig.kind() != calling_convention::PROPERTY {
        return Err(turbonop_core::Error::new(
            ErrorKind::Malformed,
            0,
            "property signature header",
        ));
    }
    Ok(sig)
}

fn locals(c: &mut Cursor<'_>) -> Result<Vec<TypeSig>> {
    let n = c.count(1, "local count")?;
    (0..n).map(|_| ty(c, 0)).collect()
}

/// A local variable signature: `LOCAL_SIG`, the count, the types.
pub fn parse_local_sig(blob: &[u8]) -> Result<Vec<TypeSig>> {
    let mut c = Cursor::new(blob);
    if c.u8("local signature header")? != calling_convention::LOCAL_SIG {
        return Err(c.error(ErrorKind::Malformed, "local signature header"));
    }
    locals(&mut c)
}

/// A StandAloneSig row's signature: locals, a method's, or a field's.
pub fn parse_standalone_sig(blob: &[u8]) -> Result<StandAloneSig> {
    match blob.first().map(|h| h & calling_convention::MASK) {
        Some(calling_convention::LOCAL_SIG) => parse_local_sig(blob).map(StandAloneSig::Locals),
        Some(calling_convention::FIELD) => parse_field_sig(blob).map(StandAloneSig::Field),
        _ => parse_method_sig(blob).map(StandAloneSig::Method),
    }
}

/// A TypeSpec's signature: a type.
pub fn parse_type_spec(blob: &[u8]) -> Result<TypeSig> {
    ty(&mut Cursor::new(blob), 0)
}

/// A MethodSpec's instantiation: `GENERICINST`, the count, the type
/// arguments.
pub fn parse_method_spec(blob: &[u8]) -> Result<Vec<TypeSig>> {
    let mut c = Cursor::new(blob);
    if c.u8("method spec header")? != calling_convention::GENERIC_INST {
        return Err(c.error(ErrorKind::Malformed, "method spec header"));
    }
    let n = c.count(1, "type argument count")?;
    (0..n).map(|_| ty(&mut c, 0)).collect()
}

/// A type at the start of `blob`, and how many bytes it took (for
/// signatures embedded in others, as a local constant's).
pub fn parse_type_prefix(blob: &[u8]) -> Result<(TypeSig, usize)> {
    let mut c = Cursor::new(blob);
    let t = ty(&mut c, 0)?;
    Ok((t, c.pos()))
}

#[cfg(test)]
#[allow(clippy::cast_possible_truncation)]
mod tests {
    use super::*;
    use element_type as E;

    fn td(rid: u32) -> Token {
        Token::new(TableId::TypeDef, rid)
    }
    fn tr(rid: u32) -> Token {
        Token::new(TableId::TypeRef, rid)
    }

    #[test]
    fn fields() {
        assert_eq!(
            parse_field_sig(&[6, E::I4]).unwrap(),
            TypeSig::Primitive(Primitive::I4)
        );
        // volatile int: modreq(IsVolatile) int32.
        assert_eq!(
            parse_field_sig(&[6, E::CMOD_REQD, 0x09, E::I4]).unwrap(),
            TypeSig::Modified {
                required: true,
                modifier: Token::new(TableId::TypeRef, 2),
                ty: Box::new(TypeSig::Primitive(Primitive::I4))
            }
        );
        assert!(parse_field_sig(&[7, E::I4]).is_err());
        assert!(parse_field_sig(&[6]).is_err());
        assert!(parse_field_sig(&[6, 0x17]).is_err());
    }

    #[test]
    fn methods() {
        // instance void (int32, class [x]T&, !0[], !!1[0...,0...])
        let blob = [
            0x20,
            4,
            E::VOID,
            E::I4,
            E::BYREF,
            E::CLASS,
            0x05,
            E::SZARRAY,
            E::VAR,
            0,
            E::ARRAY,
            E::MVAR,
            1,
            2,
            0,
            2,
            0,
            0,
        ];
        let m = parse_method_sig(&blob).unwrap();
        assert!(m.has_this() && !m.explicit_this());
        assert_eq!(m.kind(), 0);
        assert_eq!(m.params.len(), 4);
        assert_eq!(m.params[1], TypeSig::ByRef(Box::new(TypeSig::Class(tr(1)))));
        assert_eq!(
            m.params[3],
            TypeSig::Array(
                Box::new(TypeSig::MVar(1)),
                ArrayShape {
                    rank: 2,
                    sizes: vec![],
                    lower_bounds: vec![0, 0]
                }
            )
        );
        for n in 0..blob.len() {
            assert!(parse_method_sig(&blob[..n]).is_err(), "{n}");
        }
        // Generic: generic<2> !!0 (!!1).
        let m = parse_method_sig(&[0x10, 2, 1, E::MVAR, 0, E::MVAR, 1]).unwrap();
        assert_eq!((m.generic_params, m.ret.clone()), (2, TypeSig::MVar(0)));
        // A vararg call site: (int32, ..., string, float64).
        let m = parse_method_sig(&[5, 3, E::VOID, E::I4, E::SENTINEL, E::STRING, E::R8]).unwrap();
        assert_eq!((m.sentinel, m.required_params()), (Some(1), 1));
        assert!(parse_method_sig(&[5, 3, 1, 8, 0x41, 0x0E, 0x41, 0x0D]).is_err());
        // Counts larger than the blob.
        assert!(parse_method_sig(&[0, 0x7F, 1]).is_err());
        assert!(parse_method_sig(&[6, 0, 1]).is_err());
        assert!(parse_method_sig(&[0x0A, 0, 1]).is_err());
    }

    #[test]
    fn generic_instantiations_and_function_pointers() {
        // valuetype Span`1<int32>
        assert_eq!(
            parse_type_spec(&[E::GENERICINST, E::VALUETYPE, 0x05, 1, E::I4]).unwrap(),
            TypeSig::GenericInst {
                value_type: true,
                ty: tr(1),
                args: vec![TypeSig::Primitive(Primitive::I4)]
            }
        );
        assert!(parse_type_spec(&[E::GENERICINST, E::VALUETYPE, 0x05, 0]).is_err());
        assert!(parse_type_spec(&[E::GENERICINST, E::I4, 0x05, 1, E::I4]).is_err());
        // method unmanaged cdecl int32 *(int32)
        let t = parse_type_spec(&[E::FNPTR, 1, 1, E::I4, E::I4]).unwrap();
        let TypeSig::FnPtr(m) = t else { panic!() };
        assert_eq!((m.kind(), m.params.len()), (1, 1));
        // A TypeDefOrRef with tag 3.
        assert!(parse_type_spec(&[E::CLASS, 0x07]).is_err());
        assert_eq!(
            parse_type_spec(&[E::CLASS, 0x08]).unwrap(),
            TypeSig::Class(td(2))
        );
    }

    #[test]
    fn locals_specs_and_standalone() {
        // (int32& pinned, typedref)
        let l = parse_local_sig(&[7, 2, E::PINNED, E::BYREF, E::I4, E::TYPEDBYREF]).unwrap();
        assert_eq!(
            l[0],
            TypeSig::Pinned(Box::new(TypeSig::ByRef(Box::new(TypeSig::Primitive(
                Primitive::I4
            )))))
        );
        assert_eq!(l[1], TypeSig::Primitive(Primitive::TypedReference));
        assert!(parse_local_sig(&[6, 0]).is_err());
        assert!(matches!(
            parse_standalone_sig(&[7, 0]).unwrap(),
            StandAloneSig::Locals(v) if v.is_empty()
        ));
        assert!(matches!(
            parse_standalone_sig(&[6, E::I4]).unwrap(),
            StandAloneSig::Field(_)
        ));
        assert!(matches!(
            parse_standalone_sig(&[2, 0, E::VOID]).unwrap(),
            StandAloneSig::Method(_)
        ));
        assert!(parse_standalone_sig(&[]).is_err());
        assert_eq!(parse_method_spec(&[0x0A, 1, E::OBJECT]).unwrap().len(), 1);
        assert!(parse_method_spec(&[0x0B, 1, E::OBJECT]).is_err());
        assert_eq!(
            parse_property_sig(&[0x28, 0, E::R8]).unwrap().ret,
            TypeSig::Primitive(Primitive::R8)
        );
        assert!(parse_property_sig(&[0x20, 0, E::R8]).is_err());
        assert_eq!(parse_type_prefix(&[E::I4, 1, 2]).unwrap().1, 1);
    }

    #[test]
    fn deep_nesting_is_an_error_not_a_crash() {
        let mut deep = vec![E::PTR; 100_000];
        deep.push(E::I4);
        assert!(parse_type_spec(&deep).is_err());
        let mut ok = vec![E::PTR; MAX_DEPTH];
        ok.push(E::I4);
        assert!(parse_type_spec(&ok).is_ok());
        // Function pointers nest too.
        let mut fp = Vec::new();
        for _ in 0..10_000 {
            fp.extend_from_slice(&[E::FNPTR, 0, 0]);
        }
        fp.push(E::VOID);
        assert!(parse_type_spec(&fp).is_err());
    }
}
