//! Names as IL spells them: types (`Ns.Outer/Inner`, `[Assembly]Ns.Type`,
//! `class [System.Runtime]System.Collections.Generic.List`1<int32>`),
//! signatures (`instance void (int32, string)`), and members
//! (`instance void [System.Runtime]System.Object::.ctor()`), close to
//! ildasm's.
//!
//! A method signature renders as its calling convention (`instance`,
//! `explicit`, `vararg`, `unmanaged cdecl`, ...; `generic<N>` for a
//! generic method), its return type, and its parameters (`...` where a
//! vararg call site's extra arguments start). Arrays render their bounds
//! as `lo...hi` (or `lo...` with no size), modifiers follow the type they
//! modify (`int32 modreq(IsVolatile)`), and function pointers are
//! `method <signature> *(<parameters>)`.

use std::fmt::Write;

use turbonop_core::{Error, ErrorKind, Result};

use crate::metadata::Metadata;
use crate::signature::{
    MethodSig, TypeSig, calling_convention as CC, parse_field_sig, parse_method_sig,
    parse_method_spec, parse_type_spec,
};
use crate::tables::{
    AssemblyRefRow, FieldRow, MemberRefRow, MethodDefRow, MethodSpecRow, ModuleRefRow, TableId,
    Token, TypeDefRow, TypeRefRow, TypeSpecRow,
};

/// How deep names may nest (types in types, TypeRefs in TypeRefs, type
/// specifications in type specifications).
const MAX_DEPTH: usize = 64;

fn too_deep(token: Token) -> Error {
    Error::new(
        ErrorKind::Malformed,
        u64::from(token.0),
        "names nest too deeply",
    )
}

fn no_row(token: Token) -> Error {
    Error::new(
        ErrorKind::Malformed,
        u64::from(token.0),
        "a token with no row",
    )
}

/// `s` quoted, as a C# string literal would be (with `\u` escapes for
/// anything outside printable ASCII), from its UTF-16 units.
pub fn quote_utf16(units: impl IntoIterator<Item = u16>) -> String {
    let mut out = String::from("\"");
    for u in units {
        match u {
            0x22 => out.push_str("\\\""),
            0x5C => out.push_str("\\\\"),
            0x0A => out.push_str("\\n"),
            0x0D => out.push_str("\\r"),
            0x09 => out.push_str("\\t"),
            0x00 => out.push_str("\\0"),
            0x20..=0x7E => out.push(char::from_u32(u32::from(u)).unwrap_or('?')),
            _ => {
                let _ = write!(out, "\\u{u:04x}");
            }
        }
    }
    out.push('"');
    out
}

/// `s` quoted (see [`quote_utf16`]).
pub fn quote(s: &str) -> String {
    quote_utf16(s.encode_utf16())
}

/// A signature's calling convention as a prefix (`instance `, `vararg `,
/// `generic<2> `, ...).
pub fn signature_prefix(header: u8, generic_params: u32) -> String {
    let mut s = String::new();
    if header & CC::HAS_THIS != 0 {
        s.push_str("instance ");
    }
    if header & CC::EXPLICIT_THIS != 0 {
        s.push_str("explicit ");
    }
    match header & CC::MASK {
        CC::DEFAULT | CC::PROPERTY => {}
        CC::C => s.push_str("unmanaged cdecl "),
        CC::STDCALL => s.push_str("unmanaged stdcall "),
        CC::THISCALL => s.push_str("unmanaged thiscall "),
        CC::FASTCALL => s.push_str("unmanaged fastcall "),
        CC::VARARG => s.push_str("vararg "),
        CC::UNMANAGED => s.push_str("unmanaged "),
        k => {
            let _ = write!(s, "callconv({k}) ");
        }
    }
    if generic_params > 0 {
        let _ = write!(s, "generic<{generic_params}> ");
    }
    s
}

impl<'d> Metadata<'d> {
    fn type_def_name_at(&self, rid: u32, depth: usize) -> Result<String> {
        let token = Token::new(TableId::TypeDef, rid);
        if depth > MAX_DEPTH {
            return Err(too_deep(token));
        }
        let t: TypeDefRow = self.row(rid).ok_or_else(|| no_row(token))?;
        let name = self.string(t.type_name);
        if let Some(outer) = self.enclosing_type(rid) {
            return Ok(format!(
                "{}/{name}",
                self.type_def_name_at(outer, depth + 1)?
            ));
        }
        let ns = self.string(t.type_namespace);
        Ok(if ns.is_empty() {
            name.into_owned()
        } else {
            format!("{ns}.{name}")
        })
    }

    /// TypeDef `rid`'s name: `Ns.Name`, or `Ns.Outer/Inner` for a nested
    /// type.
    pub fn type_def_name(&self, rid: u32) -> Result<String> {
        self.type_def_name_at(rid, 0)
    }

    fn type_ref_name_at(&self, rid: u32, depth: usize) -> Result<String> {
        let token = Token::new(TableId::TypeRef, rid);
        if depth > MAX_DEPTH {
            return Err(too_deep(token));
        }
        let t: TypeRefRow = self.row(rid).ok_or_else(|| no_row(token))?;
        let name = self.string(t.type_name);
        let ns = self.string(t.type_namespace);
        let full = if ns.is_empty() {
            name.to_string()
        } else {
            format!("{ns}.{name}")
        };
        let scope = t.resolution_scope;
        if scope.is_null() {
            return Ok(full);
        }
        Ok(match scope.table() {
            Some(TableId::AssemblyRef) => {
                let a: AssemblyRefRow = self
                    .row(scope.rid())
                    .ok_or_else(|| no_row(scope.token_or_null()))?;
                format!("[{}]{full}", self.string(a.name))
            }
            Some(TableId::ModuleRef) => {
                let m: ModuleRefRow = self
                    .row(scope.rid())
                    .ok_or_else(|| no_row(scope.token_or_null()))?;
                format!("[.module {}]{full}", self.string(m.name))
            }
            Some(TableId::TypeRef) => {
                format!("{}/{name}", self.type_ref_name_at(scope.rid(), depth + 1)?)
            }
            _ => full,
        })
    }

    /// TypeRef `rid`'s name: `[Assembly]Ns.Name`, `[.module M]Ns.Name`, or
    /// `[Assembly]Ns.Outer/Inner`.
    pub fn type_ref_name(&self, rid: u32) -> Result<String> {
        self.type_ref_name_at(rid, 0)
    }

    fn type_spec_at(&self, rid: u32, depth: usize) -> Result<String> {
        let token = Token::new(TableId::TypeSpec, rid);
        if depth > MAX_DEPTH {
            return Err(too_deep(token));
        }
        let s: TypeSpecRow = self.row(rid).ok_or_else(|| no_row(token))?;
        let sig = parse_type_spec(self.blob(s.signature)?)?;
        self.type_sig_at(&sig, depth + 1)
    }

    /// TypeSpec `rid`'s type.
    pub fn type_spec_name(&self, rid: u32) -> Result<String> {
        self.type_spec_at(rid, 0)
    }

    fn plain_at(&self, token: Token, depth: usize) -> Result<String> {
        match token.table() {
            Some(TableId::TypeDef) => self.type_def_name_at(token.rid(), depth),
            Some(TableId::TypeRef) => self.type_ref_name_at(token.rid(), depth),
            Some(TableId::TypeSpec) => self.type_spec_at(token.rid(), depth),
            _ => Err(no_row(token)),
        }
    }

    /// A TypeDef, TypeRef, or TypeSpec token's type, without `class` or
    /// `valuetype`: `-` for a null token, `undecodable` for a TypeSpec
    /// whose signature doesn't decode, the token in hex for another table.
    pub fn type_token_name(&self, token: Token) -> String {
        if token.is_null() {
            return "-".into();
        }
        match token.table() {
            Some(TableId::TypeDef | TableId::TypeRef) => self
                .plain_at(token, 0)
                .unwrap_or_else(|_| "undecodable".into()),
            Some(TableId::TypeSpec) => self
                .type_spec_at(token.rid(), 0)
                .unwrap_or_else(|_| "undecodable".into()),
            _ => token.to_string(),
        }
    }

    /// A type's name as a signature has it, `class`/`valuetype` prefixed
    /// (a TypeSpec's type has neither).
    fn sig_type_token(&self, token: Token, kind: &str, depth: usize) -> Result<String> {
        if token.table() == Some(TableId::TypeSpec) {
            return self.type_spec_at(token.rid(), depth);
        }
        Ok(format!("{kind}{}", self.plain_at(token, depth)?))
    }

    fn type_sig_at(&self, t: &TypeSig, depth: usize) -> Result<String> {
        if depth > MAX_DEPTH {
            return Err(Error::new(ErrorKind::Malformed, 0, "names nest too deeply"));
        }
        let d = depth + 1;
        Ok(match t {
            TypeSig::Primitive(p) => p.il_name().into(),
            TypeSig::Ptr(e) => format!("{}*", self.type_sig_at(e, d)?),
            TypeSig::ByRef(e) => format!("{}&", self.type_sig_at(e, d)?),
            TypeSig::Pinned(e) => format!("{} pinned", self.type_sig_at(e, d)?),
            TypeSig::ValueType(tok) => self.sig_type_token(*tok, "valuetype ", d)?,
            TypeSig::Class(tok) => self.sig_type_token(*tok, "class ", d)?,
            TypeSig::Var(n) => format!("!{n}"),
            TypeSig::MVar(n) => format!("!!{n}"),
            TypeSig::SzArray(e) => format!("{}[]", self.type_sig_at(e, d)?),
            TypeSig::Array(e, shape) => {
                let mut dims = Vec::new();
                for i in 0..shape.rank as usize {
                    let lo = shape.lower_bounds.get(i).copied();
                    let size = shape.sizes.get(i).copied();
                    dims.push(match (lo, size) {
                        (lo, Some(size)) => {
                            let lo = i64::from(lo.unwrap_or(0));
                            format!("{lo}...{}", lo + i64::from(size) - 1)
                        }
                        (Some(lo), None) => format!("{lo}..."),
                        (None, None) => String::new(),
                    });
                    if dims.len() > 1 << 16 {
                        return Err(Error::new(ErrorKind::Malformed, 0, "array rank"));
                    }
                }
                format!("{}[{}]", self.type_sig_at(e, d)?, dims.join(","))
            }
            TypeSig::GenericInst {
                value_type,
                ty,
                args,
            } => {
                let kind = if *value_type { "valuetype " } else { "class " };
                if ty.table() == Some(TableId::TypeSpec) {
                    // (System.Reflection.Metadata doesn't allow one.)
                    return Err(Error::new(
                        ErrorKind::Malformed,
                        u64::from(ty.0),
                        "a generic instantiation of a TypeSpec",
                    ));
                }
                let args = args
                    .iter()
                    .map(|a| self.type_sig_at(a, d))
                    .collect::<Result<Vec<_>>>()?;
                format!(
                    "{}<{}>",
                    self.sig_type_token(*ty, kind, d)?,
                    args.join(", ")
                )
            }
            TypeSig::FnPtr(m) => format!(
                "method {}{} *{}",
                signature_prefix(m.header, m.generic_params),
                self.type_sig_at(&m.ret, d)?,
                self.params_at(m, d)?
            ),
            TypeSig::Modified {
                required,
                modifier,
                ty,
            } => format!(
                "{} {}({})",
                self.type_sig_at(ty, d)?,
                if *required { "modreq" } else { "modopt" },
                self.plain_at(*modifier, d)?
            ),
        })
    }

    /// A signature's type, as IL spells it.
    pub fn type_sig_name(&self, t: &TypeSig) -> Result<String> {
        self.type_sig_at(t, 0)
    }

    fn params_at(&self, m: &MethodSig, depth: usize) -> Result<String> {
        let mut list = m
            .params
            .iter()
            .map(|p| self.type_sig_at(p, depth))
            .collect::<Result<Vec<_>>>()?;
        if let Some(s) = m.sentinel
            && s < list.len()
        {
            list.insert(s, "...".into());
        }
        Ok(format!("({})", list.join(", ")))
    }

    /// A method signature: `instance void (int32, string)`.
    pub fn method_sig_name(&self, m: &MethodSig) -> Result<String> {
        Ok(format!(
            "{}{} {}",
            signature_prefix(m.header, m.generic_params),
            self.type_sig_at(&m.ret, 0)?,
            self.params_at(m, 0)?
        ))
    }

    /// A method as a member: `instance void Ns.Type::Name(int32)`.
    pub fn member_sig_name(&self, m: &MethodSig, parent: &str, name: &str) -> Result<String> {
        Ok(format!(
            "{}{} {parent}::{name}{}",
            signature_prefix(m.header, m.generic_params),
            self.type_sig_at(&m.ret, 0)?,
            self.params_at(m, 0)?
        ))
    }

    fn member_ref_parent(&self, parent: Token) -> Result<String> {
        Ok(match parent.table() {
            Some(TableId::ModuleRef) => {
                let m: ModuleRefRow = self.row(parent.rid()).ok_or_else(|| no_row(parent))?;
                format!("[.module {}]", self.string(m.name))
            }
            Some(TableId::MethodDef) => {
                let owner = self
                    .method_owner(parent.rid())
                    .ok_or_else(|| no_row(parent))?;
                self.type_token_name(Token::new(TableId::TypeDef, owner))
            }
            _ => self.type_token_name(parent),
        })
    }

    /// A MethodDef, Field, MemberRef, or MethodSpec token, as IL names it:
    /// `instance void [System.Runtime]System.Object::.ctor()`,
    /// `int32 Ns.Type::field`, `!!0 Ns.Type::M(!!0)<int32>`.
    pub fn member_name(&self, token: Token) -> Result<String> {
        self.member_name_at(token, 0)
    }

    fn member_name_at(&self, token: Token, depth: usize) -> Result<String> {
        if depth > 1 {
            // (A MethodSpec of a MethodSpec.)
            return Err(too_deep(token));
        }
        match token.table() {
            Some(TableId::MethodDef) => {
                let m: MethodDefRow = self.row(token.rid()).ok_or_else(|| no_row(token))?;
                let owner = self
                    .method_owner(token.rid())
                    .ok_or_else(|| no_row(token))?;
                let sig = parse_method_sig(self.blob(m.signature)?)?;
                self.member_sig_name(&sig, &self.type_def_name(owner)?, &self.string(m.name))
            }
            Some(TableId::Field) => {
                let f: FieldRow = self.row(token.rid()).ok_or_else(|| no_row(token))?;
                let owner = self.field_owner(token.rid()).ok_or_else(|| no_row(token))?;
                let sig = parse_field_sig(self.blob(f.signature)?)?;
                Ok(format!(
                    "{} {}::{}",
                    self.type_sig_name(&sig)?,
                    self.type_def_name(owner)?,
                    self.string(f.name)
                ))
            }
            Some(TableId::MemberRef) => {
                let r: MemberRefRow = self.row(token.rid()).ok_or_else(|| no_row(token))?;
                let parent = self.member_ref_parent(r.class.token_or_null())?;
                let blob = self.blob(r.signature)?;
                if blob.first() == Some(&CC::FIELD) {
                    let sig = parse_field_sig(blob)?;
                    Ok(format!(
                        "{} {parent}::{}",
                        self.type_sig_name(&sig)?,
                        self.string(r.name)
                    ))
                } else {
                    let sig = parse_method_sig(blob)?;
                    self.member_sig_name(&sig, &parent, &self.string(r.name))
                }
            }
            Some(TableId::MethodSpec) => {
                let s: MethodSpecRow = self.row(token.rid()).ok_or_else(|| no_row(token))?;
                let method = self.member_name_at(s.method.token_or_null(), depth + 1)?;
                let args = parse_method_spec(self.blob(s.instantiation)?)?
                    .iter()
                    .map(|a| self.type_sig_name(a))
                    .collect::<Result<Vec<_>>>()?;
                Ok(format!("{method}<{}>", args.join(", ")))
            }
            _ => Ok(token.to_string()),
        }
    }

    /// Whether a MemberRef names a field (its signature's first byte).
    pub fn member_ref_is_field(&self, rid: u32) -> bool {
        self.row::<MemberRefRow>(rid)
            .and_then(|r| self.blob(r.signature).ok())
            .is_some_and(|b| b.first() == Some(&CC::FIELD))
    }
}

#[cfg(test)]
#[allow(clippy::cast_possible_truncation)]
mod tests {
    use super::*;
    use crate::signature::{ArrayShape, Primitive, element_type as E};
    use crate::tables::CodedIndexKind;
    use crate::testutil::MetadataBuilder;

    #[test]
    fn quoting() {
        assert_eq!(quote("a\"b\\c\n\r\t\0"), r#""a\"b\\c\n\r\t\0""#);
        assert_eq!(quote("\u{e9}\u{2603}"), r#""\u00e9\u2603""#);
        assert_eq!(quote("\u{1F600}"), r#""\ud83d\ude00""#);
    }

    #[test]
    fn prefixes() {
        assert_eq!(signature_prefix(0x20, 0), "instance ");
        assert_eq!(
            signature_prefix(0x60 | 0x10, 2),
            "instance explicit generic<2> "
        );
        assert_eq!(signature_prefix(5, 0), "vararg ");
        assert_eq!(signature_prefix(1, 0), "unmanaged cdecl ");
        assert_eq!(signature_prefix(0x28, 0), "instance ");
        assert_eq!(signature_prefix(0x0C, 0), "callconv(12) ");
    }

    fn sample() -> Vec<u8> {
        let mut b = MetadataBuilder::new();
        let runtime = b.string("System.Runtime");
        let system = b.string("System");
        let object = b.string("Object");
        let list = b.string("List`1");
        let generic = b.string("System.Collections.Generic");
        let outer = b.string("Outer");
        let inner = b.string("Inner");
        let ns = b.string("Ns");
        let native = b.string("native.dll");
        let f = b.string("f");
        let ctor = b.string(".ctor");
        b.row(TableId::AssemblyRef, &[8, 0, 0, 0, 0, 0, runtime, 0, 0]);
        b.row(TableId::ModuleRef, &[native]);
        let asm = CodedIndexKind::ResolutionScope
            .encode(Token::new(TableId::AssemblyRef, 1))
            .unwrap();
        let module = CodedIndexKind::ResolutionScope
            .encode(Token::new(TableId::ModuleRef, 1))
            .unwrap();
        let in_typeref = CodedIndexKind::ResolutionScope
            .encode(Token::new(TableId::TypeRef, 3))
            .unwrap();
        b.row(TableId::TypeRef, &[asm, object, system]); // 1
        b.row(TableId::TypeRef, &[asm, list, generic]); // 2
        b.row(TableId::TypeRef, &[module, outer, ns]); // 3
        b.row(TableId::TypeRef, &[in_typeref, inner, ns]); // 4
        b.row(TableId::TypeRef, &[0, object, 0]); // 5
        // <Module>, Ns.Outer, Outer/Inner (nested), with a field and a
        // method.
        let sig_f = b.blob(&[6, E::SZARRAY, E::VAR, 0]);
        let sig_m = b.blob(&[0x20, 1, E::VOID, E::CLASS, 0x0D]); // class TypeRef 3
        b.row(TableId::TypeDef, &[0, 0, 0, 0, 1, 1]);
        b.row(TableId::TypeDef, &[0, outer, ns, 0, 1, 1]);
        b.row(TableId::TypeDef, &[0, inner, ns, 0, 2, 2]);
        b.row(TableId::Field, &[0, f, sig_f]);
        b.row(TableId::MethodDef, &[0, 0, 0, ctor, sig_m, 1]);
        b.row(TableId::NestedClass, &[3, 2]);
        // List`1<object> and a TypeSpec naming itself.
        let spec = b.blob(&[E::GENERICINST, E::CLASS, 0x09, 1, E::OBJECT]);
        b.row(TableId::TypeSpec, &[spec]);
        let selfish = b.blob(&[E::CLASS, 0x0A]); // TypeSpec 2
        b.row(TableId::TypeSpec, &[selfish]);
        // MemberRefs: List`1<object>::.ctor(), a field of a ModuleRef, and
        // a vararg call site of MethodDef 1.
        let mr_parent = |t: Token| CodedIndexKind::MemberRefParent.encode(t).unwrap();
        let sig_ctor = b.blob(&[0x20, 0, E::VOID]);
        b.row(
            TableId::MemberRef,
            &[mr_parent(Token::new(TableId::TypeSpec, 1)), ctor, sig_ctor],
        );
        let sig_field = b.blob(&[6, E::I4]);
        b.row(
            TableId::MemberRef,
            &[mr_parent(Token::new(TableId::ModuleRef, 1)), f, sig_field],
        );
        let sig_va = b.blob(&[0x25, 2, E::VOID, E::I4, E::SENTINEL, E::R8]);
        b.row(
            TableId::MemberRef,
            &[mr_parent(Token::new(TableId::MethodDef, 1)), ctor, sig_va],
        );
        let spec_sig = b.blob(&[0x0A, 2, E::I4, E::MVAR, 0]);
        let mdor = CodedIndexKind::MethodDefOrRef
            .encode(Token::new(TableId::MemberRef, 1))
            .unwrap();
        b.row(TableId::MethodSpec, &[mdor, spec_sig]);
        b.build()
    }

    #[test]
    fn type_names() {
        let data = sample();
        let md = Metadata::parse(&data).unwrap();
        assert_eq!(
            md.type_ref_name(1).unwrap(),
            "[System.Runtime]System.Object"
        );
        assert_eq!(md.type_ref_name(3).unwrap(), "[.module native.dll]Ns.Outer");
        assert_eq!(
            md.type_ref_name(4).unwrap(),
            "[.module native.dll]Ns.Outer/Inner"
        );
        assert_eq!(md.type_ref_name(5).unwrap(), "Object");
        assert_eq!(md.type_def_name(1).unwrap(), "");
        assert_eq!(md.type_def_name(3).unwrap(), "Ns.Outer/Inner");
        assert!(md.type_def_name(9).is_err());
        assert_eq!(
            md.type_token_name(Token::new(TableId::TypeSpec, 1)),
            "class [System.Runtime]System.Collections.Generic.List`1<object>"
        );
        assert_eq!(
            md.type_token_name(Token::new(TableId::TypeSpec, 2)),
            "undecodable"
        );
        assert_eq!(md.type_token_name(Token::new(TableId::TypeDef, 0)), "-");
        assert_eq!(md.type_token_name(Token(0x0400_0001)), "0x04000001");
        let arr = TypeSig::Array(
            Box::new(TypeSig::Primitive(Primitive::I4)),
            ArrayShape {
                rank: 3,
                sizes: vec![4],
                lower_bounds: vec![1, -2],
            },
        );
        assert_eq!(md.type_sig_name(&arr).unwrap(), "int32[1...4,-2...,]");
        let fnptr = crate::signature::parse_type_spec(&[
            E::FNPTR,
            0x05,
            2,
            E::I4,
            E::SENTINEL,
            E::PTR,
            E::U1,
            E::I,
        ])
        .unwrap();
        assert_eq!(
            md.type_sig_name(&fnptr).unwrap(),
            "method vararg int32 *(..., uint8*, native int)"
        );
    }

    #[test]
    fn member_names() {
        let data = sample();
        let md = Metadata::parse(&data).unwrap();
        assert_eq!(
            md.member_name(Token::new(TableId::MethodDef, 1)).unwrap(),
            "instance void Ns.Outer::.ctor(class [.module native.dll]Ns.Outer)"
        );
        assert_eq!(
            md.member_name(Token::new(TableId::Field, 1)).unwrap(),
            "!0[] Ns.Outer::f"
        );
        assert_eq!(
            md.member_name(Token::new(TableId::MemberRef, 1)).unwrap(),
            "instance void class [System.Runtime]System.Collections.Generic.List`1<object>::.ctor()"
        );
        assert_eq!(
            md.member_name(Token::new(TableId::MemberRef, 2)).unwrap(),
            "int32 [.module native.dll]::f"
        );
        assert!(md.member_ref_is_field(2) && !md.member_ref_is_field(1));
        assert_eq!(
            md.member_name(Token::new(TableId::MemberRef, 3)).unwrap(),
            "instance vararg void Ns.Outer::.ctor(int32, ..., float64)"
        );
        assert_eq!(
            md.member_name(Token::new(TableId::MethodSpec, 1)).unwrap(),
            "instance void class [System.Runtime]System.Collections.Generic.List`1<object>::.ctor()<int32, !!0>"
        );
        assert!(md.member_name(Token::new(TableId::MethodDef, 2)).is_err());
        assert_eq!(md.member_name(Token(0x7000_0001)).unwrap(), "0x70000001");
    }
}
