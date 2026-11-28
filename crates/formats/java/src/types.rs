//! Descriptors (JVMS 4.3) and generic signatures (JVMS 4.7.9.1), parsed
//! and written as Java (and javap) writes them: `java.util.List<? extends
//! T>[]`, `<T extends java.lang.Comparable<? super T>>`.
//!
//! Names stay in their binary form but for the separators (`/` becomes
//! `.`; a nested class's `$` stays, as javap writes it).

use std::fmt::{self, Display, Write as _};

use turbonop_core::{Error, ErrorKind, Result};

/// Nesting deeper than this (arrays of generics of arrays ...) is taken
/// for damage: a signature's parse recurses once per level.
const MAX_DEPTH: usize = 256;

/// A primitive type, or `void`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Primitive {
    /// `B`.
    Byte,
    /// `C`.
    Char,
    /// `D`.
    Double,
    /// `F`.
    Float,
    /// `I`.
    Int,
    /// `J`.
    Long,
    /// `S`.
    Short,
    /// `Z`.
    Boolean,
    /// `V` (a method's result only).
    Void,
}

impl Primitive {
    /// The primitive for descriptor character `c`.
    pub fn from_char(c: u8) -> Option<Self> {
        Some(match c {
            b'B' => Primitive::Byte,
            b'C' => Primitive::Char,
            b'D' => Primitive::Double,
            b'F' => Primitive::Float,
            b'I' => Primitive::Int,
            b'J' => Primitive::Long,
            b'S' => Primitive::Short,
            b'Z' => Primitive::Boolean,
            b'V' => Primitive::Void,
            _ => return None,
        })
    }

    /// Its Java keyword.
    pub fn name(self) -> &'static str {
        match self {
            Primitive::Byte => "byte",
            Primitive::Char => "char",
            Primitive::Double => "double",
            Primitive::Float => "float",
            Primitive::Int => "int",
            Primitive::Long => "long",
            Primitive::Short => "short",
            Primitive::Boolean => "boolean",
            Primitive::Void => "void",
        }
    }

    /// How many local variable slots it takes (2 for long and double).
    pub fn slots(self) -> u16 {
        match self {
            Primitive::Long | Primitive::Double => 2,
            Primitive::Void => 0,
            _ => 1,
        }
    }
}

/// A field descriptor's type: `dims` array dimensions of a primitive or a
/// class.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct FieldType<'a> {
    /// Array dimensions (0 for none).
    pub dims: u8,
    /// The element type.
    pub base: BaseType<'a>,
}

/// A descriptor's element type.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum BaseType<'a> {
    /// A primitive (or `void`).
    Primitive(Primitive),
    /// A class, by its internal name (`java/lang/String`).
    Class(&'a str),
}

impl FieldType<'_> {
    /// How many local variable slots it takes.
    pub fn slots(&self) -> u16 {
        match (self.dims, self.base) {
            (0, BaseType::Primitive(p)) => p.slots(),
            _ => 1,
        }
    }
}

impl Display for FieldType<'_> {
    /// As Java writes it: `int`, `java.lang.String[][]`.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.base {
            BaseType::Primitive(p) => f.write_str(p.name())?,
            BaseType::Class(n) => write_binary_name(f, n)?,
        }
        for _ in 0..self.dims {
            f.write_str("[]")?;
        }
        Ok(())
    }
}

/// `name` with `/` written as `.`.
fn write_binary_name(f: &mut impl fmt::Write, name: &str) -> fmt::Result {
    for part in name.split('/').enumerate() {
        if part.0 > 0 {
            f.write_char('.')?;
        }
        f.write_str(part.1)?;
    }
    Ok(())
}

/// An internal name (`java/lang/String`) as Java writes it.
pub fn java_name(internal: &str) -> String {
    internal.replace('/', ".")
}

fn error(at: usize, what: &'static str) -> Error {
    Error::new(ErrorKind::Malformed, at as u64, what)
}

/// The field type at the start of `s`, and what follows it.
pub fn parse_field_type(s: &str) -> Result<(FieldType<'_>, &str)> {
    let b = s.as_bytes();
    let mut dims = 0usize;
    while b.get(dims) == Some(&b'[') {
        dims += 1;
    }
    let dims_u8 = u8::try_from(dims).map_err(|_| error(0, "array of more than 255 dimensions"))?;
    let c = *b.get(dims).ok_or_else(|| error(dims, "descriptor"))?;
    if c == b'L' {
        let end = s[dims..]
            .find(';')
            .ok_or_else(|| error(dims, "class name in a descriptor"))?
            + dims;
        let name = &s[dims + 1..end];
        if name.is_empty() {
            return Err(error(dims, "class name in a descriptor"));
        }
        return Ok((
            FieldType {
                dims: dims_u8,
                base: BaseType::Class(name),
            },
            &s[end + 1..],
        ));
    }
    match Primitive::from_char(c) {
        Some(Primitive::Void) if dims > 0 => Err(error(dims, "array of void")),
        Some(p) => Ok((
            FieldType {
                dims: dims_u8,
                base: BaseType::Primitive(p),
            },
            &s[dims + 1..],
        )),
        None => Err(error(dims, "descriptor")),
    }
}

/// A field descriptor, which must be all of `s` (and not `void`).
pub fn parse_field_descriptor(s: &str) -> Result<FieldType<'_>> {
    let (t, rest) = parse_field_type(s)?;
    if !rest.is_empty() || t.base == BaseType::Primitive(Primitive::Void) {
        return Err(error(s.len() - rest.len(), "field descriptor"));
    }
    Ok(t)
}

/// A method descriptor: `(params)result`.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct MethodDescriptor<'a> {
    /// The parameters' types.
    pub params: Vec<FieldType<'a>>,
    /// The result's type (`void` is [`Primitive::Void`]).
    pub result: FieldType<'a>,
}

impl MethodDescriptor<'_> {
    /// How many local variable slots the parameters take.
    pub fn param_slots(&self) -> u32 {
        self.params.iter().map(|p| u32::from(p.slots())).sum()
    }

    /// The parameters as javap writes them: `(int, java.lang.String[])`.
    pub fn params_text(&self) -> String {
        let mut s = String::from("(");
        for (i, p) in self.params.iter().enumerate() {
            if i > 0 {
                s.push_str(", ");
            }
            let _ = write!(s, "{p}");
        }
        s.push(')');
        s
    }
}

/// Parses a method descriptor, which must be all of `s`.
pub fn parse_method_descriptor(s: &str) -> Result<MethodDescriptor<'_>> {
    let mut rest = s
        .strip_prefix('(')
        .ok_or_else(|| error(0, "method descriptor"))?;
    let mut params = Vec::new();
    while !rest.starts_with(')') {
        let (t, r) = parse_field_type(rest)?;
        if t.base == BaseType::Primitive(Primitive::Void) {
            return Err(error(s.len() - rest.len(), "void parameter"));
        }
        params.push(t);
        rest = r;
    }
    let (result, r) = parse_field_type(&rest[1..])?;
    if !r.is_empty() {
        return Err(error(s.len() - r.len(), "method descriptor"));
    }
    Ok(MethodDescriptor { params, result })
}

/// A generic type (JVMS 4.7.9.1's JavaTypeSignature, or `void`).
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum Type<'a> {
    /// A primitive (or `void`, a result only).
    Primitive(Primitive),
    /// A class type, with its type arguments and outer classes'.
    Class(ClassType<'a>),
    /// A type variable.
    Variable(&'a str),
    /// An array of the element type.
    Array(Box<Type<'a>>),
}

/// A class type: `outer.name<args>`.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct ClassType<'a> {
    /// The enclosing class type, for a nested class written with its
    /// outer class's arguments (`Outer<T>.Inner`).
    pub outer: Option<Box<ClassType<'a>>>,
    /// The name: the internal name (`java/util/Map$Entry`) without an
    /// outer type; the simple name after one.
    pub name: &'a str,
    /// Its type arguments.
    pub args: Vec<TypeArgument<'a>>,
}

/// A type argument.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum TypeArgument<'a> {
    /// `*`: `?`.
    Any,
    /// A type.
    Exact(Type<'a>),
    /// `+`: `? extends T`.
    Extends(Type<'a>),
    /// `-`: `? super T`.
    Super(Type<'a>),
}

/// A type parameter: a name and its bounds.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct TypeParameter<'a> {
    /// Its name.
    pub name: &'a str,
    /// Its class bound (absent when it has only interface bounds).
    pub class_bound: Option<Type<'a>>,
    /// Its interface bounds.
    pub interface_bounds: Vec<Type<'a>>,
}

/// A class's signature.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct ClassSignature<'a> {
    /// Its type parameters.
    pub type_params: Vec<TypeParameter<'a>>,
    /// Its superclass.
    pub superclass: ClassType<'a>,
    /// Its interfaces.
    pub interfaces: Vec<ClassType<'a>>,
}

/// A method's signature.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct MethodSignature<'a> {
    /// Its type parameters.
    pub type_params: Vec<TypeParameter<'a>>,
    /// Its parameters' types.
    pub params: Vec<Type<'a>>,
    /// Its result.
    pub result: Type<'a>,
    /// What it throws.
    pub throws: Vec<Type<'a>>,
}

impl Display for Type<'_> {
    /// As javap writes it.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Type::Primitive(p) => f.write_str(p.name()),
            Type::Class(c) => c.fmt(f),
            Type::Variable(v) => f.write_str(v),
            Type::Array(t) => write!(f, "{t}[]"),
        }
    }
}

impl Display for ClassType<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if let Some(o) = &self.outer {
            write!(f, "{o}.")?;
        }
        write_binary_name(f, self.name)?;
        if !self.args.is_empty() {
            f.write_char('<')?;
            for (i, a) in self.args.iter().enumerate() {
                if i > 0 {
                    f.write_str(", ")?;
                }
                write!(f, "{a}")?;
            }
            f.write_char('>')?;
        }
        Ok(())
    }
}

impl Display for TypeArgument<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            TypeArgument::Any => f.write_str("?"),
            TypeArgument::Exact(t) => t.fmt(f),
            TypeArgument::Extends(t) => write!(f, "? extends {t}"),
            TypeArgument::Super(t) => write!(f, "? super {t}"),
        }
    }
}

impl ClassType<'_> {
    /// Whether it's `java.lang.Object` (with no arguments).
    pub fn is_object(&self) -> bool {
        self.outer.is_none() && self.name == "java/lang/Object" && self.args.is_empty()
    }
}

impl Type<'_> {
    /// Whether it's `java.lang.Object`.
    pub fn is_object(&self) -> bool {
        matches!(self, Type::Class(c) if c.is_object())
    }
}

impl TypeParameter<'_> {
    /// As javap writes it: `T extends java.lang.Object & java.io.Serializable`
    /// (with `verbose`, an `Object` class bound is written).
    pub fn text(&self, verbose: bool) -> String {
        let mut s = self.name.to_string();
        let mut sep = " extends ";
        if let Some(b) = &self.class_bound
            && (verbose || !b.is_object())
        {
            let _ = write!(s, "{sep}{b}");
            sep = " & ";
        }
        for b in &self.interface_bounds {
            let _ = write!(s, "{sep}{b}");
            sep = " & ";
        }
        s
    }
}

/// Type parameters as javap writes them before a declaration: `<T, U
/// extends T> ` (with its space), or nothing.
pub fn type_params_text(params: &[TypeParameter<'_>], verbose: bool) -> String {
    if params.is_empty() {
        return String::new();
    }
    let list: Vec<String> = params.iter().map(|p| p.text(verbose)).collect();
    format!("<{}>", list.join(", "))
}

struct Parser<'a> {
    s: &'a str,
    pos: usize,
    depth: usize,
}

impl<'a> Parser<'a> {
    fn peek(&self) -> Option<u8> {
        self.s.as_bytes().get(self.pos).copied()
    }

    fn eat(&mut self, c: u8) -> bool {
        if self.peek() == Some(c) {
            self.pos += 1;
            true
        } else {
            false
        }
    }

    fn expect(&mut self, c: u8, what: &'static str) -> Result<()> {
        if self.eat(c) {
            Ok(())
        } else {
            Err(error(self.pos, what))
        }
    }

    fn deeper(&mut self) -> Result<()> {
        self.depth += 1;
        if self.depth > MAX_DEPTH {
            return Err(error(self.pos, "signature nested too deeply"));
        }
        Ok(())
    }

    /// An identifier: up to one of `.;[/<>:`.
    fn identifier(&mut self) -> Result<&'a str> {
        let start = self.pos;
        while let Some(c) = self.peek() {
            if matches!(c, b'.' | b';' | b'[' | b'/' | b'<' | b'>' | b':') {
                break;
            }
            self.pos += 1;
        }
        if self.pos == start {
            return Err(error(start, "identifier in a signature"));
        }
        Ok(&self.s[start..self.pos])
    }

    fn java_type(&mut self) -> Result<Type<'a>> {
        match self.peek() {
            Some(b'L' | b'T' | b'[') => self.reference_type(),
            Some(c) => match Primitive::from_char(c) {
                Some(p) => {
                    self.pos += 1;
                    Ok(Type::Primitive(p))
                }
                None => Err(error(self.pos, "type in a signature")),
            },
            None => Err(error(self.pos, "type in a signature")),
        }
    }

    fn reference_type(&mut self) -> Result<Type<'a>> {
        self.deeper()?;
        let t = match self.peek() {
            Some(b'L') => Type::Class(self.class_type()?),
            Some(b'T') => {
                self.pos += 1;
                let name = self.identifier()?;
                self.expect(b';', "type variable")?;
                Type::Variable(name)
            }
            Some(b'[') => {
                self.pos += 1;
                let t = self.java_type()?;
                if t == Type::Primitive(Primitive::Void) {
                    return Err(error(self.pos, "array of void"));
                }
                Type::Array(Box::new(t))
            }
            _ => return Err(error(self.pos, "reference type in a signature")),
        };
        self.depth -= 1;
        Ok(t)
    }

    fn class_type(&mut self) -> Result<ClassType<'a>> {
        self.expect(b'L', "class type")?;
        // The package and outermost name, through the last `/`.
        let start = self.pos;
        loop {
            self.identifier()?;
            if !self.eat(b'/') {
                break;
            }
        }
        let mut t = ClassType {
            outer: None,
            name: &self.s[start..self.pos],
            args: self.type_args()?,
        };
        while self.eat(b'.') {
            let name = self.identifier()?;
            t = ClassType {
                outer: Some(Box::new(t)),
                name,
                args: self.type_args()?,
            };
        }
        self.expect(b';', "class type")?;
        Ok(t)
    }

    fn type_args(&mut self) -> Result<Vec<TypeArgument<'a>>> {
        let mut args = Vec::new();
        if !self.eat(b'<') {
            return Ok(args);
        }
        while !self.eat(b'>') {
            let a = match self.peek() {
                Some(b'*') => {
                    self.pos += 1;
                    TypeArgument::Any
                }
                Some(b'+') => {
                    self.pos += 1;
                    TypeArgument::Extends(self.reference_type()?)
                }
                Some(b'-') => {
                    self.pos += 1;
                    TypeArgument::Super(self.reference_type()?)
                }
                _ => TypeArgument::Exact(self.reference_type()?),
            };
            args.push(a);
        }
        if args.is_empty() {
            return Err(error(self.pos, "empty type arguments"));
        }
        Ok(args)
    }

    fn type_params(&mut self) -> Result<Vec<TypeParameter<'a>>> {
        let mut params = Vec::new();
        if !self.eat(b'<') {
            return Ok(params);
        }
        while !self.eat(b'>') {
            let name = self.identifier()?;
            self.expect(b':', "type parameter's class bound")?;
            let class_bound = match self.peek() {
                Some(b'L' | b'T' | b'[') => Some(self.reference_type()?),
                _ => None,
            };
            let mut interface_bounds = Vec::new();
            while self.eat(b':') {
                interface_bounds.push(self.reference_type()?);
            }
            params.push(TypeParameter {
                name,
                class_bound,
                interface_bounds,
            });
        }
        if params.is_empty() {
            return Err(error(self.pos, "empty type parameters"));
        }
        Ok(params)
    }

    fn end(&self) -> Result<()> {
        if self.pos == self.s.len() {
            Ok(())
        } else {
            Err(error(self.pos, "signature"))
        }
    }
}

fn parser(s: &str) -> Parser<'_> {
    Parser {
        s,
        pos: 0,
        depth: 0,
    }
}

/// Parses a class signature.
pub fn parse_class_signature(s: &str) -> Result<ClassSignature<'_>> {
    let mut p = parser(s);
    let type_params = p.type_params()?;
    let superclass = p.class_type()?;
    let mut interfaces = Vec::new();
    while p.peek().is_some() {
        interfaces.push(p.class_type()?);
    }
    p.end()?;
    Ok(ClassSignature {
        type_params,
        superclass,
        interfaces,
    })
}

/// Parses a method signature.
pub fn parse_method_signature(s: &str) -> Result<MethodSignature<'_>> {
    let mut p = parser(s);
    let type_params = p.type_params()?;
    p.expect(b'(', "method signature")?;
    let mut params = Vec::new();
    while !p.eat(b')') {
        let t = p.java_type()?;
        if t == Type::Primitive(Primitive::Void) {
            return Err(error(p.pos, "void parameter"));
        }
        params.push(t);
    }
    let result = p.java_type()?;
    let mut throws = Vec::new();
    while p.eat(b'^') {
        match p.peek() {
            Some(b'L' | b'T') => throws.push(p.reference_type()?),
            _ => return Err(error(p.pos, "throws signature")),
        }
    }
    p.end()?;
    Ok(MethodSignature {
        type_params,
        params,
        result,
        throws,
    })
}

/// Parses a field signature (a reference type signature; javap also
/// takes a primitive).
pub fn parse_field_signature(s: &str) -> Result<Type<'_>> {
    let mut p = parser(s);
    let t = p.java_type()?;
    p.end()?;
    if t == Type::Primitive(Primitive::Void) {
        return Err(error(0, "void field signature"));
    }
    Ok(t)
}

/// What a Signature attribute's text parses as: javap reads a class's
/// signature as a field type if it is one (a superclass alone).
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum AnySignature<'a> {
    /// A class's.
    Class(ClassSignature<'a>),
    /// A method's.
    Method(MethodSignature<'a>),
    /// A field's (or a record component's, or a local variable's).
    Field(Type<'a>),
}

/// Parses a signature of any kind, as javap does: a method's if it starts
/// with `(` or (with type parameters) has one after them; else a field's
/// if that's all of it; else a class's.
pub fn parse_any_signature(s: &str) -> Result<AnySignature<'_>> {
    if s.starts_with('(') {
        return parse_method_signature(s).map(AnySignature::Method);
    }
    if s.starts_with('<') {
        // A method's type parameters are followed by `(`.
        let mut p = parser(s);
        p.type_params()?;
        return if p.peek() == Some(b'(') {
            parse_method_signature(s).map(AnySignature::Method)
        } else {
            parse_class_signature(s).map(AnySignature::Class)
        };
    }
    if let Ok(t) = parse_field_signature(s) {
        return Ok(AnySignature::Field(t));
    }
    parse_class_signature(s).map(AnySignature::Class)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn descriptors() {
        let t = parse_field_descriptor("[[Ljava/lang/String;").unwrap();
        assert_eq!(t.to_string(), "java.lang.String[][]");
        assert_eq!(t.slots(), 1);
        assert_eq!(parse_field_descriptor("J").unwrap().slots(), 2);
        assert_eq!(
            parse_field_descriptor("Lfoo/Bar$Baz;").unwrap().to_string(),
            "foo.Bar$Baz"
        );
        for bad in [
            "",
            "V",
            "[V",
            "L;",
            "Lfoo",
            "Q",
            "II",
            "[",
            "Ljava/lang/String;X",
        ] {
            assert!(parse_field_descriptor(bad).is_err(), "{bad}");
        }
        let many = format!("{}I", "[".repeat(256));
        assert!(parse_field_descriptor(&many).is_err());
        let m = parse_method_descriptor("(IJ[Ljava/lang/String;D)V").unwrap();
        assert_eq!(m.params_text(), "(int, long, java.lang.String[], double)");
        assert_eq!(m.result.to_string(), "void");
        assert_eq!(m.param_slots(), 6);
        assert_eq!(parse_method_descriptor("()[I").unwrap().params_text(), "()");
        for bad in ["", "()", "(V)V", "(I", "I)V", "()VV", "(I)[V"] {
            assert!(parse_method_descriptor(bad).is_err(), "{bad}");
        }
    }

    fn roundtrip_class(s: &str) -> String {
        let c = parse_class_signature(s).unwrap();
        let mut out = type_params_text(&c.type_params, true);
        out.push_str(&format!(" extends {}", c.superclass));
        if !c.interfaces.is_empty() {
            let list: Vec<String> = c.interfaces.iter().map(ToString::to_string).collect();
            out.push_str(&format!(" implements {}", list.join(", ")));
        }
        out
    }

    #[test]
    fn signatures() {
        assert_eq!(
            roundtrip_class(
                "<K::Ljava/lang/Comparable<-TK;>;V:Ljava/lang/Number;:Ljava/io/Serializable;>Ljava/lang/Object;Ljava/util/Map$Entry<TK;Ljava/util/List<TV;>;>;"
            ),
            "<K extends java.lang.Comparable<? super K>, V extends java.lang.Number & java.io.Serializable> extends java.lang.Object implements java.util.Map$Entry<K, java.util.List<V>>"
        );
        let t =
            parse_field_signature("Lfixture/Generics<TK;TV;>.Node<Ljava/lang/String;>;").unwrap();
        assert_eq!(
            t.to_string(),
            "fixture.Generics<K, V>.Node<java.lang.String>"
        );
        let t = parse_field_signature("[Ljava/util/Map<-TK;*>;").unwrap();
        assert_eq!(t.to_string(), "java.util.Map<? super K, ?>[]");
        let m = parse_method_signature(
            "<T:TK;>(Ljava/util/Collection<+TT;>;[I)TT;^TE;^Ljava/io/IOException;",
        )
        .unwrap();
        assert_eq!(type_params_text(&m.type_params, true), "<T extends K>");
        assert_eq!(m.params[0].to_string(), "java.util.Collection<? extends T>");
        assert_eq!(m.params[1].to_string(), "int[]");
        assert_eq!(m.result, Type::Variable("T"));
        assert_eq!(m.throws.len(), 2);
        // An Object class bound is written only when verbose.
        let m = parse_method_signature("<A:Ljava/lang/Object;>()V").unwrap();
        assert_eq!(m.type_params[0].text(false), "A");
        assert_eq!(m.type_params[0].text(true), "A extends java.lang.Object");
        for bad in [
            "",
            "<>Ljava/lang/Object;",
            "Ljava/lang/Object",
            "Lfoo<>;",
            "TT",
            "[V",
            "<T>()V",
            "(V)V",
            "()V^I",
            "LA;.B;",
        ] {
            assert!(parse_any_signature(bad).is_err(), "{bad}");
        }
        assert!(matches!(
            parse_any_signature("Ljava/lang/Object;").unwrap(),
            AnySignature::Field(_)
        ));
        assert!(matches!(
            parse_any_signature("Ljava/lang/Object;Ljava/lang/Runnable;").unwrap(),
            AnySignature::Class(_)
        ));
        assert!(matches!(
            parse_any_signature("<T:Ljava/lang/Object;>Ljava/lang/Object;").unwrap(),
            AnySignature::Class(_)
        ));
        assert!(matches!(
            parse_any_signature("<T:Ljava/lang/Object;>()TT;").unwrap(),
            AnySignature::Method(_)
        ));
        // Deep nesting is refused, not a stack overflow.
        let deep = format!("{}I", "[".repeat(100_000));
        assert!(parse_field_signature(&deep).is_err());
        let deep = format!("{}{}", "LA<".repeat(10_000), ">;".repeat(10_000));
        assert!(parse_field_signature(&deep).is_err());
        assert_eq!(java_name("java/util/Map$Entry"), "java.util.Map$Entry");
    }
}
