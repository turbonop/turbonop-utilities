//! Types and members as Java writes them (`java.lang.String[]`,
//! `int max(int, int)`), and access flags as Java and as dexdump write
//! them.

/// What access flags belong to (their bits mean different things).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum AccessFor {
    /// A class.
    Class,
    /// A method.
    Method,
    /// A field.
    Field,
}

/// The access flags.
pub mod acc {
    /// `public`.
    pub const PUBLIC: u32 = 0x1;
    /// `private`.
    pub const PRIVATE: u32 = 0x2;
    /// `protected`.
    pub const PROTECTED: u32 = 0x4;
    /// `static`.
    pub const STATIC: u32 = 0x8;
    /// `final`.
    pub const FINAL: u32 = 0x10;
    /// A method's `synchronized`.
    pub const SYNCHRONIZED: u32 = 0x20;
    /// A field's `volatile`.
    pub const VOLATILE: u32 = 0x40;
    /// A method's bridge.
    pub const BRIDGE: u32 = 0x40;
    /// A field's `transient`.
    pub const TRANSIENT: u32 = 0x80;
    /// A method's varargs.
    pub const VARARGS: u32 = 0x80;
    /// `native`.
    pub const NATIVE: u32 = 0x100;
    /// An interface.
    pub const INTERFACE: u32 = 0x200;
    /// `abstract`.
    pub const ABSTRACT: u32 = 0x400;
    /// `strictfp`.
    pub const STRICT: u32 = 0x800;
    /// Not in the source.
    pub const SYNTHETIC: u32 = 0x1000;
    /// An annotation type.
    pub const ANNOTATION: u32 = 0x2000;
    /// An enum, or an enum's constant.
    pub const ENUM: u32 = 0x4000;
    /// A constructor (`<init>`, `<clinit>`).
    pub const CONSTRUCTOR: u32 = 0x1_0000;
    /// Declared `synchronized` (native or with no monitor of its own).
    pub const DECLARED_SYNCHRONIZED: u32 = 0x2_0000;
}

/// The flags' names as dexdump writes them (`PUBLIC FINAL SYNTHETIC`):
/// the first 18 bits, `?` for those that mean nothing for `what`.
pub fn access_flags_text(flags: u32, what: AccessFor) -> String {
    const CLASS: [&str; 18] = [
        "PUBLIC",
        "PRIVATE",
        "PROTECTED",
        "STATIC",
        "FINAL",
        "?",
        "?",
        "?",
        "?",
        "INTERFACE",
        "ABSTRACT",
        "?",
        "SYNTHETIC",
        "ANNOTATION",
        "ENUM",
        "?",
        "VERIFIED",
        "OPTIMIZED",
    ];
    const METHOD: [&str; 18] = [
        "PUBLIC",
        "PRIVATE",
        "PROTECTED",
        "STATIC",
        "FINAL",
        "SYNCHRONIZED",
        "BRIDGE",
        "VARARGS",
        "NATIVE",
        "?",
        "ABSTRACT",
        "STRICT",
        "SYNTHETIC",
        "?",
        "?",
        "MIRANDA",
        "CONSTRUCTOR",
        "DECLARED_SYNCHRONIZED",
    ];
    const FIELD: [&str; 18] = [
        "PUBLIC",
        "PRIVATE",
        "PROTECTED",
        "STATIC",
        "FINAL",
        "?",
        "VOLATILE",
        "TRANSIENT",
        "?",
        "?",
        "?",
        "?",
        "SYNTHETIC",
        "?",
        "ENUM",
        "?",
        "?",
        "?",
    ];
    let table = match what {
        AccessFor::Class => &CLASS,
        AccessFor::Method => &METHOD,
        AccessFor::Field => &FIELD,
    };
    let mut out = String::new();
    for (bit, name) in table.iter().enumerate() {
        if flags & (1 << bit) != 0 {
            if !out.is_empty() {
                out.push(' ');
            }
            out.push_str(name);
        }
    }
    out
}

/// The flags as Java's modifiers, in the order Java writes them (`public
/// static final`).
pub fn java_modifiers(flags: u32, what: AccessFor) -> String {
    let mut m = Vec::new();
    for (bit, name) in [
        (acc::PUBLIC, "public"),
        (acc::PROTECTED, "protected"),
        (acc::PRIVATE, "private"),
    ] {
        if flags & bit != 0 {
            m.push(name);
        }
    }
    if flags & acc::ABSTRACT != 0 && !(what == AccessFor::Class && flags & acc::INTERFACE != 0) {
        m.push("abstract");
    }
    if flags & acc::STATIC != 0 {
        m.push("static");
    }
    if flags & acc::FINAL != 0 {
        m.push("final");
    }
    match what {
        AccessFor::Method => {
            if flags & (acc::SYNCHRONIZED | acc::DECLARED_SYNCHRONIZED) != 0 {
                m.push("synchronized");
            }
            if flags & acc::NATIVE != 0 {
                m.push("native");
            }
            if flags & acc::STRICT != 0 {
                m.push("strictfp");
            }
        }
        AccessFor::Field => {
            if flags & acc::TRANSIENT != 0 {
                m.push("transient");
            }
            if flags & acc::VOLATILE != 0 {
                m.push("volatile");
            }
        }
        AccessFor::Class => {}
    }
    m.join(" ")
}

/// A primitive descriptor's Java name.
fn primitive(c: u8) -> Option<&'static str> {
    Some(match c {
        b'V' => "void",
        b'Z' => "boolean",
        b'B' => "byte",
        b'S' => "short",
        b'C' => "char",
        b'I' => "int",
        b'J' => "long",
        b'F' => "float",
        b'D' => "double",
        _ => return None,
    })
}

/// A type descriptor as Java writes the type: `I` is `int`,
/// `[Ljava/lang/String;` `java.lang.String[]`, `Ljava/util/Map$Entry;`
/// `java.util.Map$Entry` (a binary name: whether `$` is nesting, only the
/// class's `InnerClass` annotation says). A descriptor that isn't one is
/// itself.
pub fn descriptor_to_java(descriptor: &str) -> String {
    let dims = descriptor.bytes().take_while(|&c| c == b'[').count();
    let rest = &descriptor[dims..];
    let base = match rest.as_bytes() {
        [c] => match primitive(*c) {
            Some(p) => p.to_string(),
            None => return descriptor.to_string(),
        },
        [b'L', inner @ .., b';'] if !inner.is_empty() => {
            String::from_utf8_lossy(inner).replace('/', ".")
        }
        _ => return descriptor.to_string(),
    };
    let mut out = base;
    for _ in 0..dims {
        out.push_str("[]");
    }
    out
}

/// A class's package (`java.lang` for `Ljava/lang/String;`; empty for the
/// default package, arrays, and primitives).
pub fn package_of(descriptor: &str) -> String {
    match descriptor
        .strip_prefix('L')
        .and_then(|d| d.strip_suffix(';'))
    {
        Some(name) => name
            .rsplit_once('/')
            .map_or_else(String::new, |(p, _)| p.replace('/', ".")),
        None => String::new(),
    }
}

/// A method descriptor's (`(I[Ljava/lang/String;)V`) parameter and return
/// types' descriptors, if it is one.
pub fn split_method_descriptor(desc: &str) -> Option<(Vec<&str>, &str)> {
    let inner = desc.strip_prefix('(')?;
    let (params, ret) = inner.split_once(')')?;
    let mut out = Vec::new();
    let b = params.as_bytes();
    let mut i = 0;
    while i < b.len() {
        let start = i;
        while i < b.len() && b[i] == b'[' {
            i += 1;
        }
        match b.get(i)? {
            b'L' => {
                i += params[i..].find(';')? + 1;
            }
            c if primitive(*c).is_some() && *c != b'V' => i += 1,
            _ => return None,
        }
        out.push(&params[start..i]);
    }
    Some((out, ret))
}

/// A method as Java declares it: `int max(int, int)`.
pub fn java_method(name: &str, params: &[String], ret: &str) -> String {
    format!("{ret} {name}({})", params.join(", "))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn descriptors() {
        for (d, j) in [
            ("I", "int"),
            ("V", "void"),
            ("[[J", "long[][]"),
            ("Ljava/lang/String;", "java.lang.String"),
            ("[Ljava/util/Map$Entry;", "java.util.Map$Entry[]"),
            ("LNoPackage;", "NoPackage"),
            ("X", "X"),
            ("L;", "L;"),
            ("[", "["),
            ("", ""),
        ] {
            assert_eq!(descriptor_to_java(d), j, "{d}");
        }
        assert_eq!(package_of("Ljava/lang/String;"), "java.lang");
        assert_eq!(package_of("LTop;"), "");
        assert_eq!(package_of("[I"), "");
        assert_eq!(
            split_method_descriptor("(I[Ljava/lang/String;J[[Z)V"),
            Some((vec!["I", "[Ljava/lang/String;", "J", "[[Z"], "V"))
        );
        assert_eq!(split_method_descriptor("()V"), Some((vec![], "V")));
        assert_eq!(split_method_descriptor("(V)V"), None);
        assert_eq!(split_method_descriptor("(Lx)V"), None);
        assert_eq!(split_method_descriptor("I"), None);
        assert_eq!(
            java_method("max", &["int".into(), "int".into()], "int"),
            "int max(int, int)"
        );
    }

    #[test]
    fn access_flags() {
        assert_eq!(
            access_flags_text(0x1011, AccessFor::Class),
            "PUBLIC FINAL SYNTHETIC"
        );
        assert_eq!(
            access_flags_text(0x1_1001, AccessFor::Method),
            "PUBLIC SYNTHETIC CONSTRUCTOR"
        );
        assert_eq!(access_flags_text(0x20, AccessFor::Field), "?");
        assert_eq!(access_flags_text(0x4_0000, AccessFor::Field), "");
        assert_eq!(
            java_modifiers(0x19, AccessFor::Field),
            "public static final"
        );
        assert_eq!(java_modifiers(0x601, AccessFor::Class), "public");
        assert_eq!(
            java_modifiers(0x2_0102, AccessFor::Method),
            "private synchronized native"
        );
        assert_eq!(java_modifiers(0xC0, AccessFor::Field), "transient volatile");
    }
}
