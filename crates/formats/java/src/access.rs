//! Access flags: classes', fields', methods', nested classes', parameters',
//! and a module's (JVMS 4.1, 4.5, 4.6, 4.7.6, 4.7.24, 4.7.25), with their
//! names (`ACC_PUBLIC`) and the modifiers Java writes (`public`).

/// `ACC_PUBLIC`.
pub const PUBLIC: u16 = 0x0001;
/// `ACC_PRIVATE`.
pub const PRIVATE: u16 = 0x0002;
/// `ACC_PROTECTED`.
pub const PROTECTED: u16 = 0x0004;
/// `ACC_STATIC`.
pub const STATIC: u16 = 0x0008;
/// `ACC_FINAL`.
pub const FINAL: u16 = 0x0010;
/// `ACC_SUPER` (classes), the same bit as `ACC_SYNCHRONIZED` (methods)
/// and `ACC_TRANSITIVE` (requires), and `ACC_OPEN` is 0x0020 (modules).
pub const SUPER: u16 = 0x0020;
/// `ACC_SYNCHRONIZED`.
pub const SYNCHRONIZED: u16 = 0x0020;
/// `ACC_VOLATILE` (fields), the same bit as `ACC_BRIDGE` (methods).
pub const VOLATILE: u16 = 0x0040;
/// `ACC_BRIDGE`.
pub const BRIDGE: u16 = 0x0040;
/// `ACC_TRANSIENT` (fields), the same bit as `ACC_VARARGS` (methods).
pub const TRANSIENT: u16 = 0x0080;
/// `ACC_VARARGS`.
pub const VARARGS: u16 = 0x0080;
/// `ACC_NATIVE`.
pub const NATIVE: u16 = 0x0100;
/// `ACC_INTERFACE`.
pub const INTERFACE: u16 = 0x0200;
/// `ACC_ABSTRACT`.
pub const ABSTRACT: u16 = 0x0400;
/// `ACC_STRICT`.
pub const STRICT: u16 = 0x0800;
/// `ACC_SYNTHETIC`.
pub const SYNTHETIC: u16 = 0x1000;
/// `ACC_ANNOTATION`.
pub const ANNOTATION: u16 = 0x2000;
/// `ACC_ENUM`.
pub const ENUM: u16 = 0x4000;
/// `ACC_MODULE` (classes), the same bit as `ACC_MANDATED` (parameters,
/// requires, exports, opens).
pub const MODULE: u16 = 0x8000;
/// `ACC_MANDATED`.
pub const MANDATED: u16 = 0x8000;
/// `ACC_OPEN` (a module).
pub const OPEN: u16 = 0x0020;
/// `ACC_TRANSITIVE` (requires).
pub const TRANSITIVE: u16 = 0x0020;
/// `ACC_STATIC_PHASE` (requires).
pub const STATIC_PHASE: u16 = 0x0040;

/// What the flags belong to: the bits mean different things in each.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum FlagKind {
    /// A class or interface.
    Class,
    /// A field.
    Field,
    /// A method.
    Method,
    /// A nested class (InnerClasses).
    InnerClass,
    /// A method parameter (MethodParameters).
    Parameter,
    /// A module (Module).
    Module,
    /// A `requires`.
    Requires,
    /// An `exports` or `opens`.
    Exports,
}

/// The flags of each kind, in javap's order, with their names.
fn table(kind: FlagKind) -> &'static [(u16, &'static str)] {
    match kind {
        FlagKind::Class => &[
            (PUBLIC, "ACC_PUBLIC"),
            (FINAL, "ACC_FINAL"),
            (SUPER, "ACC_SUPER"),
            (INTERFACE, "ACC_INTERFACE"),
            (ABSTRACT, "ACC_ABSTRACT"),
            (SYNTHETIC, "ACC_SYNTHETIC"),
            (ANNOTATION, "ACC_ANNOTATION"),
            (ENUM, "ACC_ENUM"),
            (MODULE, "ACC_MODULE"),
        ],
        FlagKind::Field => &[
            (PUBLIC, "ACC_PUBLIC"),
            (PRIVATE, "ACC_PRIVATE"),
            (PROTECTED, "ACC_PROTECTED"),
            (STATIC, "ACC_STATIC"),
            (FINAL, "ACC_FINAL"),
            (VOLATILE, "ACC_VOLATILE"),
            (TRANSIENT, "ACC_TRANSIENT"),
            (SYNTHETIC, "ACC_SYNTHETIC"),
            (ENUM, "ACC_ENUM"),
        ],
        FlagKind::Method => &[
            (PUBLIC, "ACC_PUBLIC"),
            (PRIVATE, "ACC_PRIVATE"),
            (PROTECTED, "ACC_PROTECTED"),
            (STATIC, "ACC_STATIC"),
            (FINAL, "ACC_FINAL"),
            (SYNCHRONIZED, "ACC_SYNCHRONIZED"),
            (BRIDGE, "ACC_BRIDGE"),
            (VARARGS, "ACC_VARARGS"),
            (NATIVE, "ACC_NATIVE"),
            (ABSTRACT, "ACC_ABSTRACT"),
            (STRICT, "ACC_STRICT"),
            (SYNTHETIC, "ACC_SYNTHETIC"),
        ],
        FlagKind::InnerClass => &[
            (PUBLIC, "ACC_PUBLIC"),
            (PRIVATE, "ACC_PRIVATE"),
            (PROTECTED, "ACC_PROTECTED"),
            (STATIC, "ACC_STATIC"),
            (FINAL, "ACC_FINAL"),
            (INTERFACE, "ACC_INTERFACE"),
            (ABSTRACT, "ACC_ABSTRACT"),
            (SYNTHETIC, "ACC_SYNTHETIC"),
            (ANNOTATION, "ACC_ANNOTATION"),
            (ENUM, "ACC_ENUM"),
        ],
        FlagKind::Parameter => &[
            (FINAL, "ACC_FINAL"),
            (SYNTHETIC, "ACC_SYNTHETIC"),
            (MANDATED, "ACC_MANDATED"),
        ],
        FlagKind::Module => &[
            (OPEN, "ACC_OPEN"),
            (SYNTHETIC, "ACC_SYNTHETIC"),
            (MANDATED, "ACC_MANDATED"),
        ],
        FlagKind::Requires => &[
            (TRANSITIVE, "ACC_TRANSITIVE"),
            (STATIC_PHASE, "ACC_STATIC_PHASE"),
            (SYNTHETIC, "ACC_SYNTHETIC"),
            (MANDATED, "ACC_MANDATED"),
        ],
        FlagKind::Exports => &[(SYNTHETIC, "ACC_SYNTHETIC"), (MANDATED, "ACC_MANDATED")],
    }
}

/// The names of `flags`' bits (`ACC_PUBLIC`, ...), in javap's order; bits
/// that mean nothing for `kind` are `0x...`, highest first.
pub fn flag_names(flags: u16, kind: FlagKind) -> Vec<String> {
    let mut out = Vec::new();
    let mut rest = flags;
    for &(bit, name) in table(kind) {
        if rest & bit != 0 {
            out.push(name.to_string());
            rest &= !bit;
        }
    }
    while rest != 0 {
        let bit = 1u16 << (15 - rest.leading_zeros());
        out.push(format!("0x{bit:x}"));
        rest &= !bit;
    }
    out
}

/// The Java modifiers of `flags` as javap writes them before a
/// declaration (`public static final`), each followed by a space.
pub fn modifiers(flags: u16, kind: FlagKind) -> String {
    let list: &[(u16, &str)] = match kind {
        FlagKind::Class | FlagKind::InnerClass => &[
            (PUBLIC, "public"),
            (PRIVATE, "private"),
            (PROTECTED, "protected"),
            (STATIC, "static"),
            (FINAL, "final"),
            (ABSTRACT, "abstract"),
        ],
        FlagKind::Field => &[
            (PUBLIC, "public"),
            (PRIVATE, "private"),
            (PROTECTED, "protected"),
            (STATIC, "static"),
            (FINAL, "final"),
            (VOLATILE, "volatile"),
            (TRANSIENT, "transient"),
        ],
        FlagKind::Method => &[
            (PUBLIC, "public"),
            (PRIVATE, "private"),
            (PROTECTED, "protected"),
            (STATIC, "static"),
            (FINAL, "final"),
            (SYNCHRONIZED, "synchronized"),
            (NATIVE, "native"),
            (ABSTRACT, "abstract"),
            (STRICT, "strictfp"),
        ],
        FlagKind::Parameter => &[
            (FINAL, "final"),
            (SYNTHETIC, "synthetic"),
            (MANDATED, "mandated"),
        ],
        FlagKind::Module | FlagKind::Requires | FlagKind::Exports => &[],
    };
    let mut s = String::new();
    for &(bit, name) in list {
        if flags & bit != 0 {
            s.push_str(name);
            s.push(' ');
        }
    }
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_and_modifiers() {
        assert_eq!(
            flag_names(0x0421, FlagKind::Class),
            ["ACC_PUBLIC", "ACC_SUPER", "ACC_ABSTRACT"]
        );
        // The same bits, a method's.
        assert_eq!(
            flag_names(0x00A1, FlagKind::Method),
            ["ACC_PUBLIC", "ACC_SYNCHRONIZED", "ACC_VARARGS"]
        );
        assert_eq!(
            flag_names(0x00C0, FlagKind::Field),
            ["ACC_VOLATILE", "ACC_TRANSIENT"]
        );
        // Bits that mean nothing here, highest first.
        assert_eq!(
            flag_names(0x8101, FlagKind::Field),
            ["ACC_PUBLIC", "0x8000", "0x100"]
        );
        assert!(flag_names(0, FlagKind::Class).is_empty());
        assert_eq!(
            flag_names(0x8060, FlagKind::Requires),
            ["ACC_TRANSITIVE", "ACC_STATIC_PHASE", "ACC_MANDATED"]
        );
        assert_eq!(
            flag_names(0x8010, FlagKind::Parameter),
            ["ACC_FINAL", "ACC_MANDATED"]
        );
        assert_eq!(modifiers(0x0019, FlagKind::Field), "public static final ");
        assert_eq!(
            modifiers(0x0129, FlagKind::Method),
            "public static synchronized native "
        );
        assert_eq!(modifiers(0x0411, FlagKind::Class), "public final abstract ");
        assert_eq!(modifiers(0x0020, FlagKind::Module), "");
    }
}
