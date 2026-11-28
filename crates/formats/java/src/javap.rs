//! A class written as JDK 21's `javap -v -p` writes it: the declaration,
//! versions and flags, the constant pool with each entry resolved, every
//! field and method with its attributes (code disassembled, constant pool
//! references resolved to names), and the class's attributes.
//!
//! The lines javap writes about the file rather than the class (its path,
//! modification time, size, and checksum) are left to the caller
//! ([`Options::file_header`]).

use std::fmt::Write as _;

use crate::access::{self, FlagKind, flag_names, modifiers};
use crate::annotations::{Annotation, ElementValue, TargetInfo, TypeAnnotation, target_type_name};
use crate::attributes::{Attribute, AttributeValue, AttributesExt, Code, LocalVariable, Module};
use crate::bytecode::{self, Operand, array_type_name};
use crate::class::{ClassFile, Member};
use crate::constant_pool::{Constant, ConstantPool, decode_modified_utf8, ref_kind_name, tag};
use crate::stackmap::{FrameKind, StackMapFrame, VerificationType};
use crate::types::{
    AnySignature, Type, parse_any_signature, parse_field_descriptor, parse_method_descriptor,
};

/// What to write besides the class.
#[derive(Clone, Debug, Default)]
pub struct Options {
    /// Lines to write first (javap's `Classfile ...`, `Last modified ...`,
    /// `SHA-256 checksum ...`; all but the first indented one level).
    pub file_header: Vec<String>,
}

/// javap's line writer: indentation two spaces a level, spaces held until
/// something follows them (no line ends with one), and comments set at
/// column 40 past the indentation.
struct Writer {
    out: String,
    line: String,
    /// The line's length in UTF-16 units, as Java counts it.
    line_len: usize,
    pending: usize,
    indent: usize,
}

const INDENT_WIDTH: usize = 2;
const TAB_COLUMN: usize = 40;

impl Writer {
    fn new() -> Self {
        Self {
            out: String::new(),
            line: String::new(),
            line_len: 0,
            pending: 0,
            indent: 0,
        }
    }

    fn print(&mut self, s: &str) {
        for c in s.chars() {
            match c {
                ' ' => self.pending += 1,
                '\n' => self.println(),
                _ => {
                    if self.line.is_empty() {
                        self.pending += self.indent * INDENT_WIDTH;
                    }
                    for _ in 0..self.pending {
                        self.line.push(' ');
                    }
                    self.line_len += self.pending;
                    self.pending = 0;
                    self.line.push(c);
                    self.line_len += c.len_utf16();
                }
            }
        }
    }

    fn println(&mut self) {
        self.pending = 0;
        self.out.push_str(&self.line);
        self.out.push('\n');
        self.line.clear();
        self.line_len = 0;
    }

    fn line(&mut self, s: &str) {
        self.print(s);
        self.println();
    }

    fn tab(&mut self) {
        let col = self.indent * INDENT_WIDTH + TAB_COLUMN;
        // (As javap's: the line so far, its indentation included once it
        // holds anything, and not the spaces held.)
        let len = self.line_len;
        self.pending += if col <= len { 1 } else { col - len };
    }

    fn indent(&mut self, by: isize) {
        self.indent = self.indent.saturating_add_signed(by);
    }
}

/// Java's `Float.toString` (JDK 19's shortest-decimal algorithm, which
/// Rust's formatting shares).
pub fn java_float(v: f32) -> String {
    if v.is_nan() {
        return "NaN".into();
    }
    if v.is_infinite() {
        return if v > 0.0 { "Infinity" } else { "-Infinity" }.into();
    }
    java_decimal(&format!("{v:e}"), &format!("{v:.1e}"), f64::from(v.abs()))
}

/// Java's `Double.toString`.
pub fn java_double(v: f64) -> String {
    if v.is_nan() {
        return "NaN".into();
    }
    if v.is_infinite() {
        return if v > 0.0 { "Infinity" } else { "-Infinity" }.into();
    }
    java_decimal(&format!("{v:e}"), &format!("{v:.1e}"), v.abs())
}

/// Rust's shortest `{:e}` form (`-1.2345e-5`) as Java writes it: plain
/// from 10^-3 to 10^7, else computerized scientific notation (`1.0E10`).
/// Java writes two digits at least, and when the shortest has one, the
/// two closest to the value (`4.9E-324`, not `5.0E-324`): `two` is the
/// value to two digits (`{:.1e}`).
fn java_decimal(shortest: &str, two: &str, abs: f64) -> String {
    let e = if shortest
        .split_once('e')
        .map_or(shortest, |(m, _)| m)
        .chars()
        .filter(char::is_ascii_digit)
        .count()
        == 1
    {
        two
    } else {
        shortest
    };
    let (neg, e) = e.strip_prefix('-').map_or((false, e), |r| (true, r));
    let (mantissa, exp) = e.split_once('e').unwrap_or((e, "0"));
    let exp: i32 = exp.parse().unwrap_or(0);
    let mut digits: String = mantissa.chars().filter(char::is_ascii_digit).collect();
    while digits.len() > 1 && digits.ends_with('0') {
        digits.pop();
    }
    let mut s = String::new();
    if neg {
        s.push('-');
    }
    if abs == 0.0 {
        s.push_str("0.0");
    } else if (1e-3..1e7).contains(&abs) {
        let point = exp + 1;
        if point <= 0 {
            s.push_str("0.");
            for _ in 0..-point {
                s.push('0');
            }
            s.push_str(&digits);
        } else {
            let p = point as usize;
            if p >= digits.len() {
                s.push_str(&digits);
                for _ in digits.len()..p {
                    s.push('0');
                }
                s.push_str(".0");
            } else {
                s.push_str(&digits[..p]);
                s.push('.');
                s.push_str(&digits[p..]);
            }
        }
    } else {
        s.push_str(&digits[..1]);
        s.push('.');
        if digits.len() > 1 {
            s.push_str(&digits[1..]);
        } else {
            s.push('0');
        }
        let _ = write!(s, "E{exp}");
    }
    s
}

/// A Utf8 constant as javap writes it: escapes for `\t \n \r \b \f " ' \`
/// and `\uXXXX` for other control characters.
pub fn escape_utf8(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '\t' => out.push_str("\\t"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\u{8}' => out.push_str("\\b"),
            '\u{c}' => out.push_str("\\f"),
            '"' => out.push_str("\\\""),
            '\'' => out.push_str("\\'"),
            '\\' => out.push_str("\\\\"),
            c if c.is_control() => {
                let _ = write!(out, "\\u{:04x}", u32::from(c));
            }
            c => out.push(c),
        }
    }
    out
}

fn is_java_identifier_start(c: char) -> bool {
    c.is_alphabetic() || c == '$' || c == '_'
}

fn is_java_identifier_part(c: char) -> bool {
    is_java_identifier_start(c)
        || c.is_numeric()
        // (Characters Java's identifiers ignore.)
        || matches!(u32::from(c), 0..=8 | 0x0E..=0x1B | 0x7F..=0x9F)
}

/// A name as javap's constant writer checks it: quoted (with `\ " n t`
/// escaped) unless it's identifiers separated by `/`.
fn check_name(name: &str) -> String {
    if name.is_empty() {
        return "\"\"".into();
    }
    let mut prev = '/';
    for c in name.chars() {
        if (prev == '/' && !is_java_identifier_start(c))
            || (c != '/' && !is_java_identifier_part(c))
        {
            let mut s = String::from("\"");
            for c in name.chars() {
                match c {
                    '\\' => s.push_str("\\\\"),
                    '"' => s.push_str("\\\""),
                    '\n' => s.push_str("\\n"),
                    '\t' => s.push_str("\\t"),
                    c => s.push(c),
                }
            }
            s.push('"');
            return s;
        }
        prev = c;
    }
    name.into()
}

struct Javap<'a, 'd> {
    cf: &'a ClassFile<'d>,
    cp: &'a ConstantPool<'d>,
    w: Writer,
}

fn err_text(e: &turbonop_core::Error) -> String {
    format!("<{e}>")
}

impl<'d> Javap<'_, 'd> {
    fn utf8(&self, i: u16) -> String {
        self.cp
            .utf8(i)
            .map_or_else(|e| err_text(&e), std::borrow::Cow::into_owned)
    }

    /// javap's `stringValue`: an entry's text.
    fn string_value(&self, i: u16) -> String {
        self.string_value_depth(i, 0)
    }

    fn string_value_depth(&self, i: u16, depth: usize) -> String {
        if depth > 8 {
            return format!("<#{i}>");
        }
        let cp = self.cp;
        let Some(c) = cp.get(i) else {
            return format!("<invalid constant #{i}>");
        };
        let class = |n: u16| match cp.get(n) {
            Some(Constant::Class { name }) => check_name(&self.utf8(*name)),
            _ => format!("<invalid class #{n}>"),
        };
        match *c {
            Constant::Unusable => format!("<#{i}>"),
            Constant::Utf8(b) => escape_utf8(&decode_modified_utf8(b)),
            Constant::Integer(v) => v.to_string(),
            Constant::Float(bits) => format!("{}f", java_float(f32::from_bits(bits))),
            Constant::Long(v) => format!("{v}l"),
            Constant::Double(bits) => format!("{}d", java_double(f64::from_bits(bits))),
            Constant::Class { name } => check_name(&self.utf8(name)),
            Constant::String { string } => self.string_value_depth(string, depth + 1),
            Constant::Fieldref {
                class: c,
                name_and_type,
            }
            | Constant::Methodref {
                class: c,
                name_and_type,
            }
            | Constant::InterfaceMethodref {
                class: c,
                name_and_type,
            } => format!(
                "{}.{}",
                class(c),
                self.string_value_depth(name_and_type, depth + 1)
            ),
            Constant::NameAndType { name, descriptor } => {
                format!("{}:{}", check_name(&self.utf8(name)), self.utf8(descriptor))
            }
            Constant::MethodHandle { kind, reference } => format!(
                "{} {}",
                ref_kind_name(kind).map_or_else(|| format!("<kind {kind}>"), str::to_string),
                self.string_value_depth(reference, depth + 1)
            ),
            Constant::MethodType { descriptor } => self.utf8(descriptor),
            Constant::Dynamic {
                bootstrap,
                name_and_type,
            }
            | Constant::InvokeDynamic {
                bootstrap,
                name_and_type,
            } => format!(
                "#{bootstrap}:{}",
                self.string_value_depth(name_and_type, depth + 1)
            ),
            Constant::Module { name } | Constant::Package { name } => check_name(&self.utf8(name)),
        }
    }

    /// javap's constant writer's `write`: a tag word and the text, with
    /// references to this class's own members shortened.
    fn constant_text(&self, i: u16) -> String {
        if i == 0 {
            return "#0".into();
        }
        let Some(c) = self.cp.get(i) else {
            return format!("#{i}");
        };
        let mut shown = i;
        if let Constant::Fieldref {
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
        } = *c
            && class == self.cf.this_class
        {
            shown = name_and_type;
        }
        let word = match c.tag() {
            tag::UTF8 => "Utf8",
            tag::INTEGER => "int",
            tag::FLOAT => "float",
            tag::LONG => "long",
            tag::DOUBLE => "double",
            tag::CLASS => "class",
            tag::STRING => "String",
            tag::FIELDREF => "Field",
            tag::METHOD_HANDLE => "MethodHandle",
            tag::METHOD_TYPE => "MethodType",
            tag::METHODREF => "Method",
            tag::INTERFACE_METHODREF => "InterfaceMethod",
            tag::INVOKE_DYNAMIC => "InvokeDynamic",
            tag::DYNAMIC => "Dynamic",
            tag::NAME_AND_TYPE => "NameAndType",
            tag::MODULE => "Module",
            tag::PACKAGE => "Package",
            _ => "(unknown tag)",
        };
        format!("{word} {}", self.string_value(shown))
    }

    fn class_java_name(&self, i: u16) -> String {
        match self.cp.class_name(i) {
            Ok(n) => n.replace('/', "."),
            Err(e) => err_text(&e),
        }
    }

    fn write(mut self, options: &Options) -> String {
        let cf = self.cf;
        for (i, l) in options.file_header.iter().enumerate() {
            if i == 1 {
                self.w.indent(1);
            }
            self.w.line(l);
        }
        if options.file_header.len() <= 1 {
            self.w.indent(1);
        }
        if let Some(sf) = cf.attributes.find_map(|v| match v {
            AttributeValue::SourceFile(i) => Some(*i),
            _ => None,
        }) {
            let s = format!("Compiled from \"{}\"", self.utf8(sf));
            self.w.line(&s);
        }
        self.w.indent(-1);
        self.write_declaration();
        self.w.println();
        self.w.indent(1);
        self.w.line(&format!("minor version: {}", cf.minor_version));
        self.w.line(&format!("major version: {}", cf.major_version));
        let flags = flag_names(cf.access_flags, FlagKind::Class);
        self.w.line(&format!(
            "flags: (0x{:04x}) {}",
            cf.access_flags,
            flags.join(", ")
        ));
        self.w.print(&format!("this_class: #{}", cf.this_class));
        if cf.this_class != 0 {
            self.w.tab();
            let s = format!("// {}", self.string_value(cf.this_class));
            self.w.print(&s);
        }
        self.w.println();
        self.w.print(&format!("super_class: #{}", cf.super_class));
        if cf.super_class != 0 {
            self.w.tab();
            let s = format!("// {}", self.string_value(cf.super_class));
            self.w.print(&s);
        }
        self.w.println();
        self.w.line(&format!(
            "interfaces: {}, fields: {}, methods: {}, attributes: {}",
            cf.interfaces.len(),
            cf.fields.len(),
            cf.methods.len(),
            cf.attributes.len()
        ));
        self.w.indent(-1);
        self.write_constant_pool();
        self.w.line("{");
        self.w.indent(1);
        for f in &cf.fields {
            self.write_field(f);
        }
        for (i, m) in cf.methods.iter().enumerate() {
            if i > 0 {
                self.w.println();
            }
            self.write_method(m);
        }
        self.w.indent(-1);
        self.w.line("}");
        self.write_attributes(&cf.attributes, None);
        self.w.out
    }

    fn write_declaration(&mut self) {
        let cf = self.cf;
        let class_flags = if cf.is_interface() {
            cf.access_flags & !access::ABSTRACT
        } else {
            cf.access_flags
        };
        let mods: String = [
            (access::PUBLIC, "public "),
            (access::FINAL, "final "),
            (access::ABSTRACT, "abstract "),
        ]
        .iter()
        .filter(|(b, _)| class_flags & b != 0)
        .map(|(_, s)| *s)
        .collect();
        self.w.print(&mods);
        if cf.is_module() {
            let module = cf.attributes.find_map(|v| match v {
                AttributeValue::Module(m) => Some(m),
                _ => None,
            });
            if let Some(m) = module {
                let name = match self.cp.get(m.name) {
                    Some(Constant::Module { name }) => self.utf8(*name),
                    _ => self.utf8(m.name),
                };
                if m.flags & access::OPEN != 0 {
                    self.w.print("open ");
                }
                self.w.print("module ");
                self.w.print(&name.replace('/', "."));
                if m.version != 0 {
                    let v = self.utf8(m.version);
                    self.w.print(&format!("@{v}"));
                }
                return;
            }
            let n = self.class_java_name(cf.this_class);
            self.w.print(&format!("class {n}"));
            return;
        }
        if !cf.is_interface() {
            self.w.print("class ");
        } else {
            self.w.print("interface ");
        }
        let name = self.class_java_name(cf.this_class);
        self.w.print(&name);
        let sig = cf.attributes.signature().map(|i| self.utf8(i));
        let parsed = sig.as_deref().map(parse_any_signature);
        match parsed {
            None | Some(Err(_)) => {
                if !cf.is_interface() && cf.super_class != 0 {
                    let s = self.class_java_name(cf.super_class);
                    if s != "java.lang.Object" {
                        self.w.print(&format!(" extends {s}"));
                    }
                }
                for (i, &iface) in cf.interfaces.iter().enumerate() {
                    let sep = match (i, cf.is_interface()) {
                        (0, false) => " implements ",
                        (0, true) => " extends ",
                        _ => ",",
                    };
                    let n = self.class_java_name(iface);
                    self.w.print(&format!("{sep}{n}"));
                }
            }
            Some(Ok(AnySignature::Class(c))) => {
                let mut s = crate::types::type_params_text(&c.type_params, true);
                // (javap writes `extends java.lang.Object` when verbose.)
                let _ = write!(s, " extends {}", c.superclass);
                if !c.interfaces.is_empty() {
                    let list: Vec<String> = c.interfaces.iter().map(ToString::to_string).collect();
                    let word = if cf.is_interface() {
                        " extends "
                    } else {
                        " implements "
                    };
                    let _ = write!(s, "{word}{}", list.join(", "));
                }
                self.w.print(&s);
            }
            Some(Ok(AnySignature::Field(t))) => {
                self.w.print(&format!(" extends {t}"));
            }
            Some(Ok(AnySignature::Method(_))) => {}
        }
    }

    fn write_constant_pool(&mut self) {
        self.w.line("Constant pool:");
        self.w.indent(1);
        let count = self.cp.count();
        let width = count.to_string().len() + 1;
        let mut i = 1usize;
        while i < count {
            let index = u16::try_from(i).unwrap_or(u16::MAX);
            let Some(c) = self.cp.get(index).copied() else {
                i += 1;
                continue;
            };
            let head = format!("{:>width$} = {:<18} ", format!("#{i}"), c.tag_name());
            self.w.print(&head);
            let sv = self.string_value(index);
            match c {
                Constant::Utf8(_)
                | Constant::Integer(_)
                | Constant::Float(_)
                | Constant::Long(_)
                | Constant::Double(_) => self.w.line(&sv),
                Constant::Class { name: n }
                | Constant::Module { name: n }
                | Constant::Package { name: n } => {
                    self.w.print(&format!("#{n}"));
                    self.w.tab();
                    self.w.line(&format!("// {sv}"));
                }
                Constant::String { string } => {
                    self.w.print(&format!("#{string}"));
                    self.w.tab();
                    self.w.line(&format!("// {sv}"));
                }
                Constant::Fieldref {
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
                } => {
                    self.w.print(&format!("#{class}.#{name_and_type}"));
                    self.w.tab();
                    self.w.line(&format!("// {sv}"));
                }
                Constant::NameAndType { name, descriptor } => {
                    self.w.print(&format!("#{name}:#{descriptor}"));
                    self.w.tab();
                    self.w.line(&format!("// {sv}"));
                }
                Constant::MethodHandle { kind, reference } => {
                    self.w.print(&format!("{kind}:#{reference}"));
                    self.w.tab();
                    self.w.line(&format!("// {sv}"));
                }
                Constant::MethodType { descriptor } => {
                    self.w.print(&format!("#{descriptor}"));
                    self.w.tab();
                    self.w.line(&format!("//  {sv}"));
                }
                Constant::Dynamic {
                    bootstrap,
                    name_and_type,
                }
                | Constant::InvokeDynamic {
                    bootstrap,
                    name_and_type,
                } => {
                    self.w.print(&format!("#{bootstrap}:#{name_and_type}"));
                    self.w.tab();
                    self.w.line(&format!("// {sv}"));
                }
                Constant::Unusable => self.w.println(),
            }
            i += if matches!(c, Constant::Long(_) | Constant::Double(_)) {
                2
            } else {
                1
            };
        }
        self.w.indent(-1);
    }

    /// A field type from a descriptor, or the signature's when there is one.
    fn field_type_text(&self, attrs: &[Attribute<'d>], descriptor: u16) -> String {
        if let Some(s) = attrs.signature().map(|i| self.utf8(i))
            && let Ok(AnySignature::Field(t)) = parse_any_signature(&s)
        {
            return t.to_string();
        }
        let d = self.utf8(descriptor);
        parse_field_descriptor(&d).map_or_else(|e| err_text(&e), |t| t.to_string())
    }

    fn write_field(&mut self, f: &Member<'d>) {
        let mods = modifiers(f.access_flags, FlagKind::Field);
        let t = self.field_type_text(&f.attributes, f.descriptor_index);
        let name = self.utf8(f.name_index);
        self.w.line(&format!("{mods}{t} {name};"));
        self.w.indent(1);
        let d = self.utf8(f.descriptor_index);
        self.w.line(&format!("descriptor: {d}"));
        self.w.line(&format!(
            "flags: (0x{:04x}) {}",
            f.access_flags,
            flag_names(f.access_flags, FlagKind::Field).join(", ")
        ));
        self.write_attributes(&f.attributes, None);
        self.w.indent(-1);
        self.w.println();
    }

    fn write_method(&mut self, m: &Member<'d>) {
        let cf = self.cf;
        let mut mods = modifiers(m.access_flags, FlagKind::Method);
        let name = self.utf8(m.name_index);
        // An interface's methods with bodies (but static and private ones)
        // are default methods, from Java 8's class files.
        if cf.is_interface()
            && m.access_flags & (access::ABSTRACT | access::STATIC | access::PRIVATE) == 0
            && name != "<clinit>"
            && cf.major_version >= 52
        {
            mods.push_str("default ");
        }
        let desc = self.utf8(m.descriptor_index);
        let sig = m.attributes.signature().map(|i| self.utf8(i));
        let msig = match sig.as_deref().map(parse_any_signature) {
            Some(Ok(AnySignature::Method(ms))) => Some(ms),
            _ => None,
        };
        let mut s = mods;
        let (ret, params) = if let Some(ms) = &msig {
            let tp = crate::types::type_params_text(&ms.type_params, true);
            if !tp.is_empty() {
                s.push_str(&tp);
                s.push(' ');
            }
            let list: Vec<String> = ms.params.iter().map(ToString::to_string).collect();
            (ms.result.to_string(), format!("({})", list.join(", ")))
        } else {
            match parse_method_descriptor(&desc) {
                Ok(d) => (d.result.to_string(), d.params_text()),
                Err(e) => (err_text(&e), String::new()),
            }
        };
        let params = if m.access_flags & access::VARARGS != 0 {
            match params.rfind("[]") {
                Some(i) if i > 0 => format!("{}...{}", &params[..i], &params[i + 2..]),
                _ => params,
            }
        } else {
            params
        };
        match &*name {
            "<init>" => {
                s.push_str(&self.class_java_name(cf.this_class));
                s.push_str(&params);
            }
            "<clinit>" => s.push_str("{}"),
            _ => {
                let _ = write!(s, "{ret} {name}{params}");
            }
        }
        let exceptions = m.attributes.find_map(|v| match v {
            AttributeValue::Exceptions(e) => Some(e),
            _ => None,
        });
        if let Some(e) = exceptions {
            s.push_str(" throws ");
            match msig.as_ref().filter(|ms| !ms.throws.is_empty()) {
                Some(ms) => {
                    // (javap writes a signature's thrown types as they are,
                    // slashes and all.)
                    let list: Vec<String> = ms.throws.iter().map(raw_type).collect();
                    s.push_str(&list.join(", "));
                }
                None => {
                    let list: Vec<String> = e.iter().map(|&c| self.class_java_name(c)).collect();
                    s.push_str(&list.join(", "));
                }
            }
        }
        s.push(';');
        self.w.line(&s);
        self.w.indent(1);
        self.w.line(&format!("descriptor: {desc}"));
        self.w.line(&format!(
            "flags: (0x{:04x}) {}",
            m.access_flags,
            flag_names(m.access_flags, FlagKind::Method).join(", ")
        ));
        let args = parse_method_descriptor(&desc).map_or_else(
            |e| err_text(&e),
            |d| (d.params.len() + usize::from(m.access_flags & access::STATIC == 0)).to_string(),
        );
        self.write_attributes(&m.attributes, Some(&args));
        self.w.indent(-1);
    }

    fn write_attributes(&mut self, attrs: &[Attribute<'d>], args_size: Option<&str>) {
        for a in attrs {
            self.write_attribute(a, args_size, None);
        }
    }

    fn write_attribute(
        &mut self,
        a: &Attribute<'d>,
        args_size: Option<&str>,
        code: Option<&Code<'d>>,
    ) {
        let w = |s: &mut Self, t: &str| s.w.line(t);
        match &a.value {
            AttributeValue::ConstantValue(i) => {
                let t = format!("ConstantValue: {}", self.constant_text(*i));
                w(self, &t);
            }
            AttributeValue::Code(c) => self.write_code(c, args_size.unwrap_or("?")),
            AttributeValue::StackMapTable(frames) => {
                w(
                    self,
                    &format!("StackMapTable: number_of_entries = {}", frames.len()),
                );
                self.w.indent(1);
                for f in frames {
                    self.write_frame(f);
                }
                self.w.indent(-1);
            }
            AttributeValue::Exceptions(e) => {
                w(self, "Exceptions:");
                self.w.indent(1);
                let list: Vec<String> = e.iter().map(|&c| self.class_java_name(c)).collect();
                let t = format!("throws {}", list.join(", "));
                w(self, &t);
                self.w.indent(-1);
            }
            AttributeValue::InnerClasses(v) => {
                if v.is_empty() {
                    return;
                }
                w(self, "InnerClasses:");
                self.w.indent(1);
                for ic in v {
                    let f = if ic.flags & access::INTERFACE != 0 {
                        ic.flags & !access::ABSTRACT
                    } else {
                        ic.flags
                    };
                    let mut s = modifiers(f, FlagKind::InnerClass);
                    if ic.inner_name != 0 {
                        let _ = write!(s, "#{}= ", ic.inner_name);
                    }
                    let _ = write!(s, "#{}", ic.inner_class);
                    if ic.outer_class != 0 {
                        let _ = write!(s, " of #{}", ic.outer_class);
                    }
                    s.push(';');
                    self.w.print(&s);
                    self.w.tab();
                    let mut c = String::from("// ");
                    if ic.inner_name != 0 {
                        let _ = write!(c, "{}=", self.utf8(ic.inner_name));
                    }
                    c.push_str(&self.constant_text(ic.inner_class));
                    if ic.outer_class != 0 {
                        let _ = write!(c, " of {}", self.constant_text(ic.outer_class));
                    }
                    w(self, &c);
                }
                self.w.indent(-1);
            }
            AttributeValue::EnclosingMethod { class, method } => {
                self.w
                    .print(&format!("EnclosingMethod: #{class}.#{method}"));
                self.w.tab();
                let mut s = format!("// {}", self.class_java_name(*class));
                if *method != 0 {
                    let n = match self.cp.name_and_type(*method) {
                        Ok((n, _)) => n.into_owned(),
                        Err(e) => err_text(&e),
                    };
                    let _ = write!(s, ".{n}");
                }
                w(self, &s);
            }
            AttributeValue::Synthetic => w(self, "Synthetic: true"),
            AttributeValue::Deprecated => w(self, "Deprecated: true"),
            AttributeValue::Signature(i) => {
                self.w.print(&format!("Signature: #{i}"));
                self.w.tab();
                let t = format!("// {}", self.utf8(*i));
                w(self, &t);
            }
            AttributeValue::SourceFile(i) => {
                let t = format!("SourceFile: \"{}\"", self.utf8(*i));
                w(self, &t);
            }
            AttributeValue::SourceDebugExtension(d) => {
                w(self, "SourceDebugExtension:");
                self.w.indent(1);
                let text = decode_modified_utf8(d);
                for l in text.split(['\r', '\n']).filter(|l| !l.is_empty()) {
                    w(self, l);
                }
                self.w.indent(-1);
            }
            AttributeValue::LineNumberTable(v) => {
                w(self, "LineNumberTable:");
                self.w.indent(1);
                for l in v {
                    w(self, &format!("line {}: {}", l.line, l.start_pc));
                }
                self.w.indent(-1);
            }
            AttributeValue::LocalVariableTable(v) => self.write_locals("LocalVariableTable:", v),
            AttributeValue::LocalVariableTypeTable(v) => {
                self.write_locals("LocalVariableTypeTable:", v);
            }
            AttributeValue::RuntimeVisibleAnnotations(v) => {
                self.write_annotations("RuntimeVisibleAnnotations:", v);
            }
            AttributeValue::RuntimeInvisibleAnnotations(v) => {
                self.write_annotations("RuntimeInvisibleAnnotations:", v);
            }
            AttributeValue::RuntimeVisibleParameterAnnotations(v) => {
                self.write_parameter_annotations("RuntimeVisibleParameterAnnotations:", v);
            }
            AttributeValue::RuntimeInvisibleParameterAnnotations(v) => {
                self.write_parameter_annotations("RuntimeInvisibleParameterAnnotations:", v);
            }
            AttributeValue::RuntimeVisibleTypeAnnotations(v) => {
                self.write_type_annotations("RuntimeVisibleTypeAnnotations:", v, code);
            }
            AttributeValue::RuntimeInvisibleTypeAnnotations(v) => {
                self.write_type_annotations("RuntimeInvisibleTypeAnnotations:", v, code);
            }
            AttributeValue::AnnotationDefault(v) => {
                w(self, "AnnotationDefault:");
                self.w.indent(1);
                self.w.print("default_value: ");
                self.write_element_value_both(v);
                self.w.indent(-1);
                self.w.println();
            }
            AttributeValue::BootstrapMethods(v) => {
                w(self, "BootstrapMethods:");
                for (i, b) in v.iter().enumerate() {
                    self.w.indent(1);
                    let t = format!("{i}: #{} {}", b.method_ref, self.string_value(b.method_ref));
                    w(self, &t);
                    self.w.indent(1);
                    w(self, "Method arguments:");
                    self.w.indent(1);
                    for &arg in &b.arguments {
                        let t = format!("#{arg} {}", self.string_value(arg));
                        w(self, &t);
                    }
                    self.w.indent(-3);
                }
            }
            AttributeValue::MethodParameters(v) => {
                w(self, "MethodParameters:");
                self.w.indent(1);
                w(self, &format!("{:<31}{}", "Name", "Flags"));
                for p in v {
                    let name = if p.name == 0 {
                        "<no name>".to_string()
                    } else {
                        self.string_value(p.name)
                    };
                    let mut flags = String::new();
                    if p.flags & access::FINAL != 0 {
                        flags.push_str("final ");
                    }
                    if p.flags & access::MANDATED != 0 {
                        flags.push_str("mandated ");
                    }
                    if p.flags & access::SYNTHETIC != 0 {
                        flags.push_str("synthetic");
                    }
                    w(self, &format!("{name:<31}{flags}"));
                }
                self.w.indent(-1);
            }
            AttributeValue::Module(m) => self.write_module(m),
            AttributeValue::ModulePackages(v) => {
                w(self, "ModulePackages:");
                self.w.indent(1);
                for &p in v {
                    self.w.print(&format!("#{p}"));
                    self.w.tab();
                    let n = match self.cp.module_or_package_name(p) {
                        Ok(n) => n.replace('/', "."),
                        Err(e) => err_text(&e),
                    };
                    w(self, &format!("// {n}"));
                }
                self.w.indent(-1);
            }
            AttributeValue::ModuleMainClass(c) => {
                self.w.print(&format!("ModuleMainClass: #{c}"));
                self.w.tab();
                let t = format!("// {}", self.class_java_name(*c));
                w(self, &t);
            }
            AttributeValue::NestHost(c) => {
                let t = format!("NestHost: {}", self.constant_text(*c));
                w(self, &t);
            }
            AttributeValue::NestMembers(v) => self.write_class_list("NestMembers:", v),
            AttributeValue::PermittedSubclasses(v) => {
                self.write_class_list("PermittedSubclasses:", v);
            }
            AttributeValue::Record(v) => {
                w(self, "Record:");
                self.w.indent(1);
                for rc in v {
                    let t = self.field_type_text(&rc.attributes, rc.descriptor);
                    let n = self.utf8(rc.name);
                    w(self, &format!("{t} {n};"));
                    self.w.indent(1);
                    let d = self.utf8(rc.descriptor);
                    w(self, &format!("descriptor: {d}"));
                    self.write_attributes(&rc.attributes, None);
                    self.w.println();
                    self.w.indent(-1);
                }
                self.w.indent(-1);
            }
            AttributeValue::ModuleHashes { algorithm, hashes } => {
                w(self, "ModuleHashes:");
                self.w.indent(1);
                self.w.print(&format!("algorithm: #{algorithm}"));
                self.w.tab();
                let t = format!("// {}", self.utf8(*algorithm));
                w(self, &t);
                self.w.print(&hashes.len().to_string());
                self.w.tab();
                w(self, "// hashes");
                for (m, h) in hashes {
                    self.w.print(&format!("#{m}"));
                    self.w.tab();
                    let t = format!("// {}", self.string_value(*m));
                    w(self, &t);
                    w(self, &format!("hash_length: {}", h.len()));
                    let hex: String = h.iter().map(|b| format!("{b:02x}")).collect();
                    w(self, &format!("hash: [{hex}]"));
                }
                self.w.indent(-1);
            }
            AttributeValue::ModuleTarget(i) => {
                w(self, "ModuleTarget:");
                self.w.indent(1);
                self.w.print(&format!("target_platform: #{i}"));
                if *i != 0 {
                    self.w.tab();
                    let t = format!("// {}", self.utf8(*i));
                    self.w.print(&t);
                }
                self.w.println();
                self.w.indent(-1);
            }
            AttributeValue::ModuleResolution(f) => {
                w(self, "ModuleResolution:");
                self.w.indent(1);
                self.w.print(&format!("{f:x}"));
                self.w.tab();
                let mut s = String::from("//");
                for (bit, n) in [
                    (1, " DO_NOT_RESOLVE_BY_DEFAULT"),
                    (2, " WARN_DEPRECATED"),
                    (4, " WARN_DEPRECATED_FOR_REMOVAL"),
                    (8, " WARN_INCUBATING"),
                ] {
                    if f & bit != 0 {
                        s.push_str(n);
                    }
                }
                w(self, &s);
                self.w.indent(-1);
            }
            AttributeValue::CharacterRangeTable(v) => {
                w(self, "CharacterRangeTable:");
                self.w.indent(1);
                for e in v {
                    self.w.print(&format!(
                        "    {:2}, {:2}, {:6x}, {:6x}, {:4x}",
                        e.start_pc,
                        e.end_pc,
                        e.character_range_start,
                        e.character_range_end,
                        e.flags
                    ));
                    self.w.tab();
                    let mut s = format!(
                        "// {:2}, {:2}, {:4}:{:02}, {:4}:{:02}",
                        e.start_pc,
                        e.end_pc,
                        e.character_range_start >> 10,
                        e.character_range_start & 0x3FF,
                        e.character_range_end >> 10,
                        e.character_range_end & 0x3FF
                    );
                    for (bit, n) in [
                        (0x0001, ", statement"),
                        (0x0002, ", block"),
                        (0x0004, ", assignment"),
                        (0x0008, ", flow-controller"),
                        (0x0010, ", flow-target"),
                        (0x0020, ", invoke"),
                        (0x0040, ", create"),
                        (0x0080, ", branch-true"),
                        (0x0100, ", branch-false"),
                    ] {
                        if e.flags & bit != 0 {
                            s.push_str(n);
                        }
                    }
                    w(self, &s);
                }
                self.w.indent(-1);
            }
            // (javap's writer holds these as a line it never ends, so they
            // never appear.)
            AttributeValue::SourceId(_) | AttributeValue::CompilationId(_) => {}
            AttributeValue::Unknown => self.write_raw(a, Some("unknown attribute")),
            AttributeValue::Malformed(e) => self.write_raw(a, Some(&e.to_string())),
        }
    }

    /// An attribute javap shows as bytes: its name, length, and hex.
    fn write_raw(&mut self, a: &Attribute<'d>, reason: Option<&str>) {
        let name = self.cp.utf8(a.name_index).map_or_else(
            |_| format!("attribute name = #{}", a.name_index),
            |n| n.into_owned(),
        );
        let mut s = format!("  {name}: length = 0x{:X}", a.data.len());
        if let Some(r) = reason {
            let _ = write!(s, " ({r})");
        }
        self.w.line(&s);
        self.w.print("   ");
        for (i, b) in a.data.iter().enumerate() {
            self.w.print(&format!("{b:02X}"));
            if (i + 1) % 16 == 0 {
                self.w.println();
                self.w.print("   ");
            } else {
                self.w.print(" ");
            }
        }
        self.w.println();
    }

    fn write_class_list(&mut self, head: &str, v: &[u16]) {
        self.w.line(head);
        self.w.indent(1);
        for &c in v {
            let t = self.string_value(c);
            self.w.line(&t);
        }
        self.w.indent(-1);
    }

    fn write_locals(&mut self, head: &str, v: &[LocalVariable]) {
        self.w.line(head);
        self.w.indent(1);
        self.w.line("Start  Length  Slot  Name   Signature");
        for l in v {
            let t = format!(
                "{:5} {:7} {:5} {:>5}   {}",
                l.start_pc,
                l.length,
                l.index,
                self.string_value(l.name),
                self.string_value(l.descriptor)
            );
            self.w.line(&t);
        }
        self.w.indent(-1);
    }

    fn write_module(&mut self, m: &Module) {
        let w = |s: &mut Self, t: &str| s.w.line(t);
        w(self, "Module:");
        self.w.indent(1);
        self.w.print(&format!("#{},{:x}", m.name, m.flags));
        self.w.tab();
        let mut s = format!("// {}", self.string_value(m.name));
        for (bit, n) in [
            (access::OPEN, " ACC_OPEN"),
            (access::MANDATED, " ACC_MANDATED"),
            (access::SYNTHETIC, " ACC_SYNTHETIC"),
        ] {
            if m.flags & bit != 0 {
                s.push_str(n);
            }
        }
        w(self, &s);
        self.write_index_line(m.version);
        self.w.print(&m.requires.len().to_string());
        self.w.tab();
        w(self, "// requires");
        self.w.indent(1);
        for r in &m.requires {
            self.w.print(&format!("#{},{:x}", r.module, r.flags));
            self.w.tab();
            let mut s = format!("// {}", self.string_value(r.module));
            for (bit, n) in [
                (access::TRANSITIVE, " ACC_TRANSITIVE"),
                (access::STATIC_PHASE, " ACC_STATIC_PHASE"),
                (access::SYNTHETIC, " ACC_SYNTHETIC"),
                (access::MANDATED, " ACC_MANDATED"),
            ] {
                if r.flags & bit != 0 {
                    s.push_str(n);
                }
            }
            w(self, &s);
            self.write_index_line(r.version);
        }
        self.w.indent(-1);
        for (list, word) in [(&m.exports, "exports"), (&m.opens, "opens")] {
            self.w.print(&list.len().to_string());
            self.w.tab();
            w(self, &format!("// {word}"));
            self.w.indent(1);
            for e in list {
                self.w.print(&format!("#{},{:x}", e.package, e.flags));
                self.w.tab();
                let mut s = format!("// {}", self.string_value(e.package));
                if e.flags & access::MANDATED != 0 {
                    s.push_str(" ACC_MANDATED");
                }
                if e.flags & access::SYNTHETIC != 0 {
                    s.push_str(" ACC_SYNTHETIC");
                }
                if e.to.is_empty() {
                    w(self, &s);
                } else {
                    let _ = write!(s, " to ... {}", e.to.len());
                    w(self, &s);
                    self.w.indent(1);
                    for &t in &e.to {
                        self.w.print(&format!("#{t}"));
                        self.w.tab();
                        let s = format!("// ... to {}", self.string_value(t));
                        w(self, &s);
                    }
                    self.w.indent(-1);
                }
            }
            self.w.indent(-1);
        }
        self.w.print(&m.uses.len().to_string());
        self.w.tab();
        w(self, "// uses");
        self.w.indent(1);
        for &u in &m.uses {
            self.w.print(&format!("#{u}"));
            self.w.tab();
            let s = format!("// {}", self.string_value(u));
            w(self, &s);
        }
        self.w.indent(-1);
        self.w.print(&m.provides.len().to_string());
        self.w.tab();
        w(self, "// provides");
        self.w.indent(1);
        for p in &m.provides {
            self.w.print(&format!("#{}", p.service));
            self.w.tab();
            let s = format!(
                "// {} with ... {}",
                self.string_value(p.service),
                p.with.len()
            );
            w(self, &s);
            self.w.indent(1);
            for &c in &p.with {
                self.w.print(&format!("#{c}"));
                self.w.tab();
                let s = format!("// ... with {}", self.string_value(c));
                w(self, &s);
            }
            self.w.indent(-1);
        }
        self.w.indent(-1);
        self.w.indent(-1);
    }

    fn write_index_line(&mut self, i: u16) {
        self.w.print(&format!("#{i}"));
        if i != 0 {
            self.w.tab();
            let s = format!("// {}", self.string_value(i));
            self.w.print(&s);
        }
        self.w.println();
    }

    fn write_frame(&mut self, f: &StackMapFrame) {
        let head = format!("frame_type = {} /* {} */", f.frame_type, f.type_name());
        self.w.line(&head);
        self.w.indent(1);
        match &f.kind {
            FrameKind::Same => {
                if f.frame_type == 251 {
                    self.w.line(&format!("offset_delta = {}", f.offset_delta));
                }
            }
            FrameKind::SameLocals1StackItem(t) => {
                if f.frame_type == 247 {
                    self.w.line(&format!("offset_delta = {}", f.offset_delta));
                }
                self.write_map("stack", std::slice::from_ref(t));
            }
            FrameKind::Chop(_) => self.w.line(&format!("offset_delta = {}", f.offset_delta)),
            FrameKind::Append(locals) => {
                self.w.line(&format!("offset_delta = {}", f.offset_delta));
                self.write_map("locals", locals);
            }
            FrameKind::Full { locals, stack } => {
                self.w.line(&format!("offset_delta = {}", f.offset_delta));
                self.write_map("locals", locals);
                self.write_map("stack", stack);
            }
        }
        self.w.indent(-1);
    }

    fn write_map(&mut self, name: &str, types: &[VerificationType]) {
        let mut s = format!("{name} = [");
        for (i, t) in types.iter().enumerate() {
            s.push(' ');
            match t {
                VerificationType::Top => s.push_str("top"),
                VerificationType::Integer => s.push_str("int"),
                VerificationType::Float => s.push_str("float"),
                VerificationType::Long => s.push_str("long"),
                VerificationType::Double => s.push_str("double"),
                VerificationType::Null => s.push_str("null"),
                VerificationType::UninitializedThis => s.push_str("this"),
                VerificationType::Object(c) => s.push_str(&self.constant_text(*c)),
                VerificationType::Uninitialized(o) => {
                    let _ = write!(s, "uninitialized {o}");
                }
            }
            s.push(if i + 1 == types.len() { ' ' } else { ',' });
        }
        s.push(']');
        self.w.line(&s);
    }

    fn write_code(&mut self, c: &Code<'d>, args_size: &str) {
        self.w.line("Code:");
        self.w.indent(1);
        self.w.line(&format!(
            "stack={}, locals={}, args_size={args_size}",
            c.max_stack, c.max_locals
        ));
        let mut pc = 0;
        while pc < c.code.len() {
            match bytecode::decode_at(c.code, pc) {
                // (javap names no reserved opcode.)
                Ok(i) if !matches!(i.opcode.value, 202 | 254 | 255) => {
                    self.write_instruction(&i);
                    pc += i.length as usize;
                }
                Ok(_) => {
                    self.w.line(&format!("{pc:4}: bytecode {}", c.code[pc]));
                    pc += 1;
                }
                Err(_) if bytecode::opcode(c.code[pc]).is_none() => {
                    self.w.line(&format!("{pc:4}: bytecode {}", c.code[pc]));
                    pc += 1;
                }
                Err(e) => {
                    let t = format!("Error: error at or after byte {pc}: {e}");
                    self.w.line(&t);
                    break;
                }
            }
        }
        if !c.exception_table.is_empty() {
            self.w.line("Exception table:");
            self.w.indent(1);
            self.w.line(" from    to  target type");
            for h in &c.exception_table {
                let mut s = format!(" {:5} {:5} {:5}   ", h.start_pc, h.end_pc, h.handler_pc);
                if h.catch_type == 0 {
                    s.push_str("any");
                } else {
                    let _ = write!(s, "Class {}", self.string_value(h.catch_type));
                }
                self.w.line(&s);
            }
            self.w.indent(-1);
        }
        for a in &c.attributes {
            self.write_attribute(a, None, Some(c));
        }
        self.w.indent(-1);
    }

    fn write_instruction(&mut self, i: &bytecode::Instruction) {
        let head = format!("{:4}: {:<13} ", i.pc, i.mnemonic());
        self.w.print(&head);
        let pc = i64::from(i.pc);
        match &i.operand {
            Operand::None => {}
            Operand::Value(v) => self.w.print(&v.to_string()),
            Operand::Local(l) => self.w.print(&l.to_string()),
            Operand::Iinc { index, delta } => self.w.print(&format!("{index}, {delta}")),
            Operand::Branch(d) => self.w.print(&(pc + i64::from(*d)).to_string()),
            Operand::Constant(c) => {
                self.w.print(&format!("#{c}"));
                self.w.tab();
                let t = format!("// {}", self.constant_text(*c));
                self.w.print(&t);
            }
            Operand::InvokeInterface { index, count } => self.write_ref_and_value(*index, *count),
            Operand::InvokeDynamic { index } => self.write_ref_and_value(*index, 0),
            Operand::MultiANewArray { index, dims } => self.write_ref_and_value(*index, *dims),
            Operand::NewArray(t) => {
                let n = array_type_name(*t).map_or_else(|| format!("[type {t}]"), str::to_string);
                self.w.print(&format!(" {n}"));
            }
            Operand::TableSwitch {
                default,
                low,
                high,
                offsets,
            } => {
                self.w.print(&format!("{{ // {low} to {high}"));
                self.w.indent(3);
                for (k, o) in offsets.iter().enumerate() {
                    let key = i64::from(*low) + k as i64;
                    self.w.print(&format!("\n{key:12}: {}", pc + i64::from(*o)));
                }
                self.w
                    .print(&format!("\n     default: {}\n}}", pc + i64::from(*default)));
                self.w.indent(-3);
            }
            Operand::LookupSwitch { default, pairs } => {
                self.w.print(&format!("{{ // {}", pairs.len()));
                self.w.indent(3);
                for (k, o) in pairs {
                    self.w.print(&format!("\n{k:12}: {}", pc + i64::from(*o)));
                }
                self.w
                    .print(&format!("\n     default: {}\n}}", pc + i64::from(*default)));
                self.w.indent(-3);
            }
        }
        self.w.println();
    }

    fn write_ref_and_value(&mut self, index: u16, value: u8) {
        self.w.print(&format!("#{index},  {value}"));
        self.w.tab();
        let t = format!("// {}", self.constant_text(index));
        self.w.print(&t);
    }

    fn write_annotations(&mut self, head: &str, v: &[Annotation]) {
        self.w.line(head);
        self.w.indent(1);
        for (i, a) in v.iter().enumerate() {
            self.w.print(&format!("{i}: "));
            self.write_annotation_both(a);
            self.w.println();
        }
        self.w.indent(-1);
    }

    fn write_parameter_annotations(&mut self, head: &str, v: &[Vec<Annotation>]) {
        self.w.line(head);
        self.w.indent(1);
        for (p, list) in v.iter().enumerate() {
            self.w.line(&format!("parameter {p}: "));
            self.w.indent(1);
            for (i, a) in list.iter().enumerate() {
                self.w.print(&format!("{i}: "));
                self.write_annotation_both(a);
                self.w.println();
            }
            self.w.indent(-1);
        }
        self.w.indent(-1);
    }

    fn write_type_annotations(
        &mut self,
        head: &str,
        v: &[TypeAnnotation],
        _code: Option<&Code<'d>>,
    ) {
        self.w.line(head);
        self.w.indent(1);
        for (i, t) in v.iter().enumerate() {
            self.w.print(&format!("{i}: "));
            self.write_annotation(&t.annotation, false);
            self.w.print(": ");
            let pos = self.position_text(t);
            self.w.print(&pos);
            self.w.println();
            self.w.indent(1);
            self.write_annotation(&t.annotation, true);
            self.w.indent(-1);
            self.w.println();
        }
        self.w.indent(-1);
    }

    fn position_text(&self, t: &TypeAnnotation) -> String {
        let mut s = target_type_name(t.target_type)
            .map_or_else(|| format!("UNKNOWN({:#x})", t.target_type), str::to_string);
        match &t.target {
            TargetInfo::Offset { offset } => {
                let _ = write!(s, ", offset={offset}");
            }
            TargetInfo::LocalVariable(ranges) => {
                s.push_str(", {");
                for (i, r) in ranges.iter().enumerate() {
                    if i != 0 {
                        s.push_str("; ");
                    }
                    let _ = write!(
                        s,
                        "start_pc={}, length={}, index={}",
                        r.start_pc, r.length, r.index
                    );
                }
                s.push('}');
            }
            TargetInfo::Catch { index } => {
                let _ = write!(s, ", exception_index={index}");
            }
            TargetInfo::TypeParameter { index } => {
                let _ = write!(s, ", param_index={index}");
            }
            TargetInfo::TypeParameterBound { param, bound } => {
                let _ = write!(s, ", param_index={param}, bound_index={bound}");
            }
            TargetInfo::Supertype { index } => {
                let _ = write!(s, ", type_index={index}");
            }
            TargetInfo::Throws { index } => {
                let _ = write!(s, ", type_index={index}");
            }
            TargetInfo::FormalParameter { index } => {
                let _ = write!(s, ", param_index={index}");
            }
            TargetInfo::TypeArgument { offset, index } => {
                let _ = write!(s, ", offset={offset}, type_index={index}");
            }
            TargetInfo::Empty => {}
        }
        if !t.path.is_empty() {
            let steps: Vec<String> = t
                .path
                .iter()
                .map(|p| match p.kind {
                    0 => "ARRAY".to_string(),
                    1 => "INNER_TYPE".to_string(),
                    2 => "WILDCARD".to_string(),
                    _ => format!("TYPE_ARGUMENT({})", p.argument_index),
                })
                .collect();
            let _ = write!(s, ", location=[{}]", steps.join(", "));
        }
        s
    }

    /// An annotation twice, as javap writes one: indices, then (indented)
    /// resolved.
    fn write_annotation_both(&mut self, a: &Annotation) {
        self.write_annotation(a, false);
        self.w.println();
        self.w.indent(1);
        self.write_annotation(a, true);
        self.w.indent(-1);
    }

    fn write_element_value_both(&mut self, v: &ElementValue) {
        self.write_element_value(v, false);
        self.w.println();
        self.w.indent(1);
        self.write_element_value(v, true);
        self.w.indent(-1);
    }

    fn write_annotation(&mut self, a: &Annotation, resolve: bool) {
        if resolve {
            let t = self.utf8(a.type_index);
            let d = parse_field_descriptor(&t)
                .map_or_else(|_| format!("#{}", a.type_index), |t| t.to_string());
            self.w.print(&d);
            if !a.elements.is_empty() {
                self.w.line("(");
                self.w.indent(1);
                for (n, v) in &a.elements {
                    let name = self.string_value(*n);
                    self.w.print(&format!("{name}="));
                    self.write_element_value(v, true);
                    self.w.println();
                }
                self.w.indent(-1);
                self.w.print(")");
            }
        } else {
            self.w.print(&format!("#{}(", a.type_index));
            for (i, (n, v)) in a.elements.iter().enumerate() {
                if i > 0 {
                    self.w.print(",");
                }
                self.w.print(&format!("#{n}="));
                self.write_element_value(v, false);
            }
            self.w.print(")");
        }
    }

    fn write_element_value(&mut self, v: &ElementValue, resolve: bool) {
        match v {
            ElementValue::Const { tag: t, index } => {
                if !resolve {
                    self.w.print(&format!("{}#{index}", char::from(*t)));
                    return;
                }
                let s = match t {
                    b'B' => format!("(byte) {}", self.string_value(*index)),
                    b'C' => match self.cp.get(*index) {
                        Some(Constant::Integer(c)) => {
                            let ch = char::from_u32((*c as u32) & 0xFFFF).unwrap_or('\u{FFFD}');
                            format!("'{ch}'")
                        }
                        _ => format!("'{}'", self.string_value(*index)),
                    },
                    b'S' => format!("(short) {}", self.string_value(*index)),
                    b'Z' => match self.cp.get(*index) {
                        Some(Constant::Integer(z)) => (*z != 0).to_string(),
                        _ => self.string_value(*index),
                    },
                    b's' => format!("\"{}\"", self.string_value(*index)),
                    _ => self.string_value(*index),
                };
                self.w.print(&s);
            }
            ElementValue::Enum {
                type_name,
                const_name,
            } => {
                if resolve {
                    let s = format!(
                        "{}.{}",
                        self.string_value(*type_name),
                        self.string_value(*const_name)
                    );
                    self.w.print(&s);
                } else {
                    self.w.print(&format!("e#{type_name}.#{const_name}"));
                }
            }
            ElementValue::Class(i) => {
                if resolve {
                    let s = format!("class {}", self.string_value(*i));
                    self.w.print(&s);
                } else {
                    self.w.print(&format!("c#{i}"));
                }
            }
            ElementValue::Annotation(a) => {
                self.w.print("@");
                self.write_annotation(a, resolve);
            }
            ElementValue::Array(values) => {
                self.w.print("[");
                for (i, v) in values.iter().enumerate() {
                    if i > 0 {
                        self.w.print(",");
                    }
                    self.write_element_value(v, resolve);
                }
                self.w.print("]");
            }
        }
    }
}

/// A type as javap's signature types write themselves: with the internal
/// names' slashes.
fn raw_type(t: &Type<'_>) -> String {
    t.to_string_internal()
}

impl Type<'_> {
    fn to_string_internal(&self) -> String {
        match self {
            Type::Primitive(p) => p.name().to_string(),
            Type::Variable(v) => (*v).to_string(),
            Type::Array(t) => format!("{}[]", t.to_string_internal()),
            Type::Class(c) => {
                // Only the separators differ from Display's.
                let mut s = String::new();
                let mut cur = Some(c);
                let mut parts = Vec::new();
                while let Some(x) = cur {
                    parts.push(x);
                    cur = x.outer.as_deref();
                }
                for (i, x) in parts.iter().rev().enumerate() {
                    if i > 0 {
                        s.push('.');
                    }
                    s.push_str(x.name);
                    if !x.args.is_empty() {
                        let list: Vec<String> = x.args.iter().map(ToString::to_string).collect();
                        let _ = write!(s, "<{}>", list.join(", "));
                    }
                }
                s
            }
        }
    }
}

/// `cf` as `javap -v -p` writes it.
pub fn render(cf: &ClassFile<'_>, options: &Options) -> String {
    Javap {
        cf,
        cp: &cf.constant_pool,
        w: Writer::new(),
    }
    .write(options)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn floats_as_java_writes_them() {
        for (v, s) in [
            (0.0, "0.0"),
            (-0.0, "-0.0"),
            (1.0, "1.0"),
            (2.25, "2.25"),
            (1234567890123.0, "1.234567890123E12"),
            (1e7, "1.0E7"),
            (9_999_999.0, "9999999.0"),
            (0.001, "0.001"),
            (0.0001, "1.0E-4"),
            (4.9e-324, "4.9E-324"),
            (f64::MAX, "1.7976931348623157E308"),
            (100.0, "100.0"),
            (0.1, "0.1"),
            (-12.5, "-12.5"),
        ] {
            assert_eq!(java_double(v), s, "{v}");
        }
        assert_eq!(java_double(f64::NAN), "NaN");
        assert_eq!(java_double(f64::NEG_INFINITY), "-Infinity");
        for (v, s) in [
            (1.5f32, "1.5"),
            (3.0, "3.0"),
            (0.1, "0.1"),
            (1e10, "1.0E10"),
            (-0.0, "-0.0"),
        ] {
            assert_eq!(java_float(v), s, "{v}");
        }
        assert_eq!(java_float(f32::NAN), "NaN");
        assert_eq!(java_float(f32::INFINITY), "Infinity");
    }

    #[test]
    fn names_and_escapes() {
        assert_eq!(check_name("java/lang/Object"), "java/lang/Object");
        assert_eq!(check_name("<init>"), "\"<init>\"");
        assert_eq!(check_name("[Ljava/lang/String;"), "\"[Ljava/lang/String;\"");
        assert_eq!(check_name("module-info"), "\"module-info\"");
        assert_eq!(check_name("java.base"), "\"java.base\"");
        assert_eq!(check_name("a\"b"), "\"a\\\"b\"");
        assert_eq!(check_name(""), "\"\"");
        assert_eq!(check_name("$a/_b1"), "$a/_b1");
        assert_eq!(check_name("1a"), "\"1a\"");
        assert_eq!(escape_utf8("a\"b\\\t\u{1}'é"), "a\\\"b\\\\\\t\\u0001\\'é");
    }

    #[test]
    fn the_writer_holds_spaces_and_sets_tabs() {
        let mut w = Writer::new();
        w.indent(1);
        w.print("x: #1   ");
        w.tab();
        w.print("// c");
        w.println();
        w.print("trailing   ");
        w.println();
        w.indent(-1);
        w.print("a\nb");
        w.println();
        assert_eq!(
            w.out,
            format!("  x: #1{}// c\n  trailing\na\nb\n", " ".repeat(3 + 42 - 7))
        );
    }
}
