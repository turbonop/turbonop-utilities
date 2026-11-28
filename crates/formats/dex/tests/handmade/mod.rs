//! The hand-made fixtures (testdata/dex/Handmade-*.dex), from
//! [`super::writer`]: what no compiler writes. A class `Hand` whose
//! method `all` holds every opcode, 0x00 to 0xFF (the unused ones too),
//! each format's operands at their widths' limits, `goto/32` and `move/16`,
//! `const-string/jumbo`, every payload (packed and sparse switches, array
//! data of each element width), try blocks with typed and catch-all
//! handlers, and debug information with every opcode; a method whose
//! indices are all out of range; static values of every kind a field can
//! hold; annotations with every kind of value, of every visibility, on the
//! class, its fields, methods, and parameters; hidden-API flags; method
//! handles of each kind and a call site; an interface with no source
//! file.

#![allow(
    dead_code,
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    clippy::type_complexity,
    clippy::vec_init_then_push
)]

use super::writer::{Annotations, Class, Code, Contents, Ids, Method};
use turbonop_dex::value::{EncodedAnnotation, EncodedValue as V};

const HAND: &str = "LHand;";
const IFACE: &str = "LIface;";
const OBJECT: &str = "Ljava/lang/Object;";
const STRING: &str = "Ljava/lang/String;";
const MH: &str = "Ljava/lang/invoke/MethodHandle;";
const MT: &str = "Ljava/lang/invoke/MethodType;";
const CLASS: &str = "Ljava/lang/Class;";
const ANN: &str = "LAnn;";
const BOOT: &str = "LBoot;";
const CALL_SITE: &str = "Ljava/lang/invoke/CallSite;";
const LOOKUP: &str = "Ljava/lang/invoke/MethodHandles$Lookup;";

const STATICS: [(&str, &str); 13] = [
    ("sBool", "Z"),
    ("sByte", "B"),
    ("sChar", "C"),
    ("sClass", CLASS),
    ("sDouble", "D"),
    ("sFloat", "F"),
    ("sHandle", MH),
    ("sInt", "I"),
    ("sLong", "J"),
    ("sObj", OBJECT),
    ("sShort", "S"),
    ("sStr", STRING),
    ("sType", MT),
];

/// A supplementary character and a lone surrogate, in UTF-16.
const ODD: [u16; 6] = [0xD83D, 0xDE00, b' ' as u16, 0xDC00, b'!' as u16, 0];

fn ids(with_iface_only: bool) -> Ids {
    let mut i = Ids::default();
    i.ty(OBJECT);
    i.method(OBJECT, "<init>", "V", &[]);
    i.ty(IFACE);
    i.method(IFACE, "run", "V", &[]);
    if with_iface_only {
        i.freeze();
        return i;
    }
    for (name, ty) in STATICS {
        i.field(HAND, name, ty);
    }
    i.field(HAND, "iInt", "I");
    i.field(HAND, "iObj", OBJECT);
    i.method(HAND, "<init>", "V", &[]);
    i.method(HAND, "all", "V", &["I", "J"]);
    i.method(HAND, "broken", "V", &[]);
    i.method(HAND, "wide", "J", &["J", "D", "[I"]);
    i.method(HAND, "virt", "I", &["I"]);
    i.method(HAND, "nat", "V", &[]);
    i.method(MH, "invokeExact", OBJECT, &["[Ljava/lang/Object;"]);
    i.method(BOOT, "bsm", CALL_SITE, &[LOOKUP, STRING, MT]);
    i.proto("I", &["I", STRING]);
    i.ty("[I");
    i.ty("[J");
    i.ty("[[Ljava/lang/String;");
    i.ty(ANN);
    i.ty("Ljava/lang/Exception;");
    i.ty("Ljava/lang/Throwable;");
    for s in [
        "Hand.java",
        "Other.java",
        "hello \"q\"\n",
        "jumbo",
        "site",
        "a",
        "j",
        "d",
        "x",
        "y",
        "TT;",
        "Ljava/util/List<TT;>;",
    ] {
        i.string(s);
    }
    for n in [
        "b", "s", "c", "i", "f", "mt", "mh", "str", "ty", "fld", "mth", "en", "arr", "ann", "nul",
        "z", "value",
    ] {
        i.string(n);
    }
    i.string_utf16(&ODD[..5]);
    i.freeze();
    i
}

/// Every opcode, in order, then the payloads.
fn all_code(i: &Ids) -> Code {
    let string = i.s("hello \"q\"\n") as u16;
    let ty = i.t(STRING) as u16;
    let array = i.t("[I") as u16;
    let ifield = i.f(HAND, "iInt") as u16;
    let sfield = i.f(HAND, "sInt") as u16;
    let method = i.m(OBJECT, "<init>") as u16;
    let iface = i.m(IFACE, "run") as u16;
    let invoke_exact = i.m(MH, "invokeExact") as u16;
    let proto = i.p("I", &["I", STRING]) as u16;
    let void = i.p("V", &[]) as u16;
    let mut c: Vec<u16> = Vec::new();
    // Each switch and fill-array-data, to point at its payload.
    let mut fixups: Vec<(usize, usize)> = Vec::new();
    let op = |o: u8, rest: u8| u16::from(o) | (u16::from(rest) << 8);
    let nib =
        |o: u8, a: u8, b: u8| u16::from(o) | (u16::from(a & 0xF) << 8) | (u16::from(b & 0xF) << 12);
    for code in 0..=255u8 {
        use turbonop_dex::insn::{Format::*, IndexType, OPCODES};
        let info = &OPCODES[usize::from(code)];
        let index = match info.index {
            IndexType::String => string,
            IndexType::Type if code == 0x23 || code == 0x24 || code == 0x25 => array,
            IndexType::Type => ty,
            IndexType::Field if (0x52..=0x5F).contains(&code) => ifield,
            IndexType::Field => sfield,
            IndexType::Method if code == 0x72 || code == 0x78 => iface,
            IndexType::Method => method,
            IndexType::MethodAndProto => invoke_exact,
            IndexType::Proto => void,
            _ => 0,
        };
        let at = c.len();
        match info.format {
            F10x if code == 0 => c.push(0x0000),
            F10x => c.push(u16::from(code)),
            F12x => c.push(nib(code, 1, 15)),
            F11n => c.push(nib(code, 15, 0x8)),
            F11x => c.push(op(code, 255)),
            F10t => c.push(op(code, 0x80)),
            F20t => c.extend([op(code, 0), 0x7FFF]),
            F22x => c.extend([op(code, 255), 0xFFFF]),
            F21t => c.extend([op(code, 2), 0xFFFE]),
            F21s => c.extend([op(code, 3), 0x8000]),
            F21h => c.extend([op(code, 4), 0x4000]),
            F21c => c.extend([op(code, 5), index]),
            F23x => c.extend([op(code, 255), 0x80FE]),
            F22b => c.extend([op(code, 6), 0x8007]),
            F22t => c.extend([nib(code, 1, 2), 0x0003]),
            F22s => c.extend([nib(code, 3, 4), 0x7FFF]),
            F22c => c.extend([nib(code, 5, 6), index]),
            F30t => c.extend([op(code, 0), 0x0000, 0x8000]),
            F32x => c.extend([op(code, 0), 0xFFFF, 0x1234]),
            F31i if code == 0x14 => c.extend([op(code, 7), 0x0000, 0x3F80]),
            F31i => c.extend([op(code, 8), 0xFFFF, 0xFFFF]),
            F31t => {
                fixups.push((at, fixups.len()));
                c.extend([op(code, 9), 0, 0]);
            }
            F31c => c.extend([op(code, 10), index, 0]),
            F35c if code == 0x24 => c.extend([0x5000 | u16::from(code) | 0x0F00, index, 0xEDCB]),
            F35c if code == 0xFC => c.extend([u16::from(code), 0, 0]),
            F35c if code == 0x72 => c.extend([0x1000 | u16::from(code), index, 0x0009]),
            F35c => c.extend([
                u16::from(code) | (u16::from(code - 0x6D) << 12) | 0x0400,
                index,
                0x3210,
            ]),
            F3rc => c.extend([op(code, 3), index, 0x0100]),
            F45cc => c.extend([0x2000 | u16::from(code), index, 0x0001, proto]),
            F4rcc => c.extend([op(code, 2), index, 0x00FE, proto]),
            F51l => c.extend([op(code, 254), 0x2D18, 0x5444, 0x21FB, 0x4009]),
            _ => unreachable!("{}", info.name),
        }
    }
    // A second and third fill-array-data (bytes, an odd count; longs).
    for _ in 0..2 {
        fixups.push((c.len(), fixups.len()));
        c.extend([op(0x26, 11), 0, 0]);
    }
    // Small and negative operands: const/4 7, goto back, if-eqz forward.
    c.extend([nib(0x12, 0, 7), op(0x28, 0xFF), op(0x38, 0), 0x0002, 0x000E]);
    if c.len() % 2 == 1 {
        c.push(0x0000);
    }
    // The payloads: packed-switch, sparse-switch, then array data of 4,
    // 1, and 8 bytes.
    let mut payloads: Vec<Vec<u16>> = Vec::new();
    payloads.push(vec![
        0x0100, 3, 0xFFFE, 0xFFFF, 4, 0, 0xFFFC, 0xFFFF, 0x10, 0,
    ]);
    payloads.push(vec![
        0x0200, 2, 0x0000, 0x8000, 0x0005, 0x0000, 3, 0, 0xFFFD, 0xFFFF,
    ]);
    payloads.push(vec![0x0300, 4, 3, 0, 1, 0, 0xFFFF, 0xFFFF, 0x5678, 0x1234]);
    payloads.push(vec![0x0300, 1, 3, 0, 0x0201, 0x0003]);
    payloads.push(vec![0x0300, 8, 1, 0, 0x0001, 0x0002, 0x0003, 0x8004]);
    let mut starts = Vec::new();
    for p in &payloads {
        starts.push(c.len());
        c.extend_from_slice(p);
    }
    // The 31t instructions are packed-switch (0x2b), sparse-switch (0x2c),
    // fill-array-data (0x26) in opcode order: 0x26 first.
    for &(at, k) in &fixups {
        let payload = match (c[at] & 0xFF, k) {
            (0x2B, _) => starts[0],
            (0x2C, _) => starts[1],
            (_, 0) => starts[2],
            (_, 3) => starts[3],
            _ => starts[4],
        };
        let off = (payload as i64 - at as i64) as u32;
        c[at + 1] = off as u16;
        c[at + 2] = (off >> 16) as u16;
    }
    let exception = i.t("Ljava/lang/Exception;");
    let throwable = i.t("Ljava/lang/Throwable;");
    Code {
        registers: 0x1000,
        ins: 3,
        outs: 5,
        tries: vec![(0, 4, 0), (10, 2, 1), (20, 1, 2)],
        handlers: vec![
            (vec![(exception, 5), (throwable, 6)], Some(7)),
            (vec![], Some(8)),
            (vec![(exception, 9)], None),
        ],
        debug: Some(all_debug(i, c.len())),
        insns: c,
    }
}

/// Debug information with every opcode.
fn all_debug(i: &Ids, _len: usize) -> Vec<u8> {
    use turbonop_dex::debug::op;
    use turbonop_dex::reader::{encode_sleb128, encode_uleb128};
    let mut d = Vec::new();
    let p1 = |v: Option<u32>, d: &mut Vec<u8>| encode_uleb128(v.map_or(0, |v| v + 1), d);
    encode_uleb128(10, &mut d);
    encode_uleb128(2, &mut d);
    p1(Some(i.s("a")), &mut d);
    p1(None, &mut d);
    d.push(op::SET_PROLOGUE_END);
    d.push(op::FIRST_SPECIAL + 4);
    d.push(op::ADVANCE_PC);
    encode_uleb128(2, &mut d);
    d.push(op::ADVANCE_LINE);
    encode_sleb128(-3, &mut d);
    d.push(op::START_LOCAL);
    encode_uleb128(5, &mut d);
    p1(Some(i.s("x")), &mut d);
    p1(Some(i.t("I")), &mut d);
    d.push(op::FIRST_SPECIAL + 15 + 2);
    d.push(op::START_LOCAL_EXTENDED);
    encode_uleb128(6, &mut d);
    p1(Some(i.s("y")), &mut d);
    p1(Some(i.t(OBJECT)), &mut d);
    p1(Some(i.s("TT;")), &mut d);
    d.push(op::FIRST_SPECIAL + 2 * 15);
    d.push(op::END_LOCAL);
    encode_uleb128(5, &mut d);
    d.push(op::FIRST_SPECIAL + 15 + 14);
    d.push(op::RESTART_LOCAL);
    encode_uleb128(5, &mut d);
    // A local with neither name nor type, and one that replaces another.
    d.push(op::START_LOCAL);
    encode_uleb128(7, &mut d);
    p1(None, &mut d);
    p1(None, &mut d);
    d.push(op::ADVANCE_PC);
    encode_uleb128(1, &mut d);
    d.push(op::START_LOCAL_EXTENDED);
    encode_uleb128(7, &mut d);
    p1(Some(i.s("x")), &mut d);
    p1(Some(i.t("[I")), &mut d);
    p1(None, &mut d);
    d.push(op::SET_EPILOGUE_BEGIN);
    d.push(op::SET_FILE);
    p1(Some(i.s("Other.java")), &mut d);
    d.push(op::FIRST_SPECIAL + 3 * 15 + 1);
    d.push(op::ADVANCE_LINE);
    encode_sleb128(1000, &mut d);
    d.push(0xFF);
    d.push(op::SET_FILE);
    p1(None, &mut d);
    d.push(op::FIRST_SPECIAL);
    d.push(op::END_SEQUENCE);
    d
}

/// Debug information of `wide(JD[I)J`: parameters of two registers, a
/// generic one restarted.
fn wide_debug(i: &Ids) -> Vec<u8> {
    use turbonop_dex::debug::op;
    use turbonop_dex::reader::encode_uleb128;
    let mut d = Vec::new();
    encode_uleb128(1, &mut d);
    encode_uleb128(3, &mut d);
    encode_uleb128(i.s("j") + 1, &mut d);
    encode_uleb128(i.s("d") + 1, &mut d);
    encode_uleb128(0, &mut d);
    // d8's way with generic parameters: null in the header, then started
    // again at 0 with its signature (the empty range is dropped).
    d.push(op::START_LOCAL_EXTENDED);
    encode_uleb128(5, &mut d);
    encode_uleb128(i.s("x") + 1, &mut d);
    encode_uleb128(i.t("[I") + 1, &mut d);
    encode_uleb128(i.s("Ljava/util/List<TT;>;") + 1, &mut d);
    d.push(op::FIRST_SPECIAL + 15);
    d.push(op::END_SEQUENCE);
    d
}

fn ann(i: &Ids, ty: &str, elements: Vec<(&str, V)>) -> EncodedAnnotation {
    EncodedAnnotation {
        type_idx: i.t(ty),
        elements: elements.into_iter().map(|(n, v)| (i.s(n), v)).collect(),
    }
}

fn every_value(i: &Ids) -> Vec<(&'static str, V)> {
    vec![
        ("b", V::Byte(-128)),
        ("s", V::Short(-300)),
        ("c", V::Char(0xFFFF)),
        ("i", V::Int(i32::MIN)),
        ("j", V::Long(0x1234_5678_9ABC)),
        ("f", V::Float(-1.25)),
        ("d", V::Double(1e300)),
        ("mt", V::MethodType(i.p("I", &["I", STRING]))),
        ("mh", V::MethodHandle(1)),
        ("str", V::String(i.s_odd())),
        ("ty", V::Type(i.t("[[Ljava/lang/String;"))),
        ("fld", V::Field(i.f(HAND, "iObj"))),
        ("mth", V::Method(i.m(HAND, "wide"))),
        ("en", V::Enum(i.f(HAND, "sInt"))),
        (
            "arr",
            V::Array(vec![
                V::Int(1),
                V::Array(vec![]),
                V::Null,
                V::Boolean(false),
            ]),
        ),
        (
            "ann",
            V::Annotation(ann(i, ANN, vec![("value", V::Int(7))])),
        ),
        ("nul", V::Null),
        ("z", V::Boolean(true)),
    ]
}

trait Odd {
    fn s_odd(&self) -> u32;
}

impl Odd for Ids {
    fn s_odd(&self) -> u32 {
        self.s16(&ODD[..5])
    }
}

fn hand(i: &Ids, version: u32) -> Contents {
    let static_values = {
        let mut fields: Vec<(u32, &str)> =
            STATICS.iter().map(|(n, _)| (i.f(HAND, n), *n)).collect();
        fields.sort_unstable();
        fields
            .iter()
            .map(|(_, n)| match *n {
                "sBool" => V::Boolean(true),
                "sByte" => V::Byte(-5),
                "sChar" => V::Char(0xFFFF),
                "sClass" => V::Type(i.t(HAND)),
                "sDouble" => V::Double(-0.0),
                "sFloat" => V::Float(3.5),
                "sHandle" => V::Null,
                "sInt" => V::Int(-123_456),
                "sLong" => V::Long(i64::MIN),
                "sObj" => V::Null,
                "sShort" => V::Short(-2),
                "sStr" => V::String(i.s("hello \"q\"\n")),
                // (ART takes method types and handles for no static value.)
                _ => V::Type(i.t(MT)),
            })
            .collect()
    };
    let mut statics: Vec<(u32, u32)> = STATICS
        .iter()
        .enumerate()
        .map(|(k, (n, _))| (i.f(HAND, n), if k % 2 == 0 { 0x19 } else { 0x8 }))
        .collect();
    statics.sort_unstable();
    let instance = vec![
        (i.f(HAND, "iInt"), 0x2),
        (i.f(HAND, "iObj"), 0x4 | 0x40 | 0x80),
    ];
    let broken = Code {
        registers: 2,
        ins: 0,
        outs: 1,
        insns: vec![
            0x001A, 0xFFFF, // const-string v0, string@ffff
            0x001C, 0xFFFF, // const-class
            0x1052, 0xFFFF, // iget
            0x1071, 0xFFFF, 0x0000, // invoke-static {v0}
            0x10FA, 0xFFFF, 0x0000, 0xFFFF, // invoke-polymorphic
            0x00FF, 0xFFFF, // const-method-type
            0x001B, 0xFFFF, 0xFFFF, // const-string/jumbo
            0x000E,
        ],
        ..Code::default()
    };
    let virt_debug = {
        let mut d = vec![2, 1, 0];
        d.extend([turbonop_dex::debug::op::FIRST_SPECIAL, 0]);
        d
    };
    let direct = vec![
        Method {
            method_idx: i.m(HAND, "<init>"),
            access: 0x1_0001,
            code: Some(Code {
                registers: 1,
                ins: 1,
                outs: 1,
                insns: vec![0x1070, i.m(OBJECT, "<init>") as u16, 0x0000, 0x000E],
                ..Code::default()
            }),
        },
        Method {
            method_idx: i.m(HAND, "all"),
            access: 0x9,
            code: Some(all_code(i)),
        },
        Method {
            method_idx: i.m(HAND, "broken"),
            access: 0xA,
            code: Some(broken),
        },
        Method {
            method_idx: i.m(HAND, "wide"),
            access: 0x8 | 0x80,
            code: Some(Code {
                registers: 6,
                ins: 5,
                outs: 0,
                insns: vec![0x0010, 0x0000],
                debug: Some(wide_debug(i)),
                ..Code::default()
            }),
        },
    ];
    let mut direct = direct;
    direct.sort_by_key(|m| m.method_idx);
    let mut virtual_methods = vec![
        Method {
            method_idx: i.m(HAND, "virt"),
            access: 0x1 | 0x20,
            code: Some(Code {
                registers: 3,
                ins: 2,
                outs: 0,
                insns: vec![0x020F],
                debug: Some(virt_debug),
                ..Code::default()
            }),
        },
        Method {
            method_idx: i.m(HAND, "nat"),
            access: 0x1 | 0x100,
            code: None,
        },
    ];
    virtual_methods.sort_by_key(|m| m.method_idx);
    let members = statics.len() + instance.len() + direct.len() + virtual_methods.len();
    let flags: Vec<u32> = (0..members as u32)
        .map(|k| {
            if k < 16 {
                k
            } else {
                [0x10, 0x20, 0x21, 0x31, 0x40][k as usize - 16]
            }
        })
        .collect();
    let all_idx = i.m(HAND, "all");
    let annotations = Annotations {
        class: vec![
            (0, ann(i, ANN, every_value(i))),
            (2, ann(i, "Ljava/lang/Exception;", vec![])),
        ],
        fields: vec![
            (
                i.f(HAND, "sInt"),
                vec![(1, ann(i, ANN, vec![("value", V::Int(1))]))],
            ),
            (i.f(HAND, "iObj"), vec![(1, ann(i, ANN, vec![]))]),
        ],
        methods: vec![(
            all_idx,
            vec![(2, ann(i, ANN, vec![("z", V::Boolean(false))]))],
        )],
        parameters: vec![(
            all_idx,
            vec![
                Some(vec![(1, ann(i, ANN, vec![("value", V::Int(2))]))]),
                None,
            ],
        )],
    };
    let hand = Class {
        class_idx: i.t(HAND),
        access: 0x1,
        superclass: Some(i.t(OBJECT)),
        interfaces: vec![i.t(IFACE) as u16],
        source_file: Some(i.s("Hand.java")),
        static_fields: statics,
        instance_fields: instance,
        direct_methods: direct,
        virtual_methods,
        static_values,
        annotations: Some(annotations),
        hiddenapi: Some(flags),
    };
    let boot = i.m(BOOT, "bsm") as u16;
    Contents {
        version,
        classes: vec![iface(i), hand],
        method_handles: vec![
            (4, boot),
            (0, i.f(HAND, "sInt") as u16),
            (3, i.f(HAND, "iInt") as u16),
            (6, i.m(HAND, "<init>") as u16),
            (8, i.m(IFACE, "run") as u16),
            (7, i.m(HAND, "virt") as u16),
        ],
        call_sites: vec![vec![
            V::MethodHandle(0),
            V::String(i.s("site")),
            V::MethodType(i.p("V", &[])),
            V::Int(-1),
            V::Byte(-2),
            V::Short(-3),
            V::Char(65),
            V::Long(-4),
            V::Float(0.5),
            V::Double(-0.25),
            V::String(i.s("hello \"q\"\n")),
            V::Type(i.t(HAND)),
            V::MethodType(i.p("I", &["I", STRING])),
            V::MethodHandle(2),
            V::Null,
            V::Boolean(true),
        ]],
    }
}

fn iface(i: &Ids) -> Class {
    Class {
        class_idx: i.t(IFACE),
        access: 0x601,
        superclass: Some(i.t(OBJECT)),
        interfaces: vec![],
        source_file: None,
        static_fields: vec![],
        instance_fields: vec![],
        direct_methods: vec![],
        virtual_methods: vec![Method {
            method_idx: i.m(IFACE, "run"),
            access: 0x401,
            code: None,
        }],
        static_values: vec![],
        annotations: None,
        hiddenapi: None,
    }
}

/// `Handmade-<version>.dex`.
pub fn handmade(version: u32) -> Vec<u8> {
    let i = ids(false);
    super::writer::build(&i, &hand(&i, version))
}

/// `Handmade-container.dex`: version 041, two DEX files (the second an
/// interface alone).
pub fn container() -> Vec<u8> {
    let a = ids(false);
    let b = ids(true);
    let second = Contents {
        version: 41,
        classes: vec![iface(&b)],
        ..Contents::default()
    };
    super::writer::container(&[(&a, &hand(&a, 41)), (&b, &second)])
}
