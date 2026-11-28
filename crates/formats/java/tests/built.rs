//! Classes built here, a byte at a time, holding what javac doesn't write:
//! `jsr` and `ret`, `goto_w` and `jsr_w`, `swap`, `nop`, `breakpoint`, a
//! dynamic constant and a method type, the Synthetic attribute, a
//! SourceDebugExtension, a CharacterRangeTable, SourceID and
//! CompilationID, invisible parameter annotations, an unknown attribute,
//! old class versions, and damaged attributes. Each is read back; and,
//! where there's a JDK 21, written out and compared with its javap.

#![allow(clippy::cast_possible_truncation)]

use turbonop_global_alloc as _;

mod common;

use std::collections::HashMap;
use std::process::Command;

use common::{first_difference, java_home, javap_body, jdk_version};
use turbonop_java::attributes::AttributeValue;
use turbonop_java::bytecode::{self, Operand};
use turbonop_java::{ClassFile, Constant};

/// A class file writer: a constant pool that shares its entries, and the
/// parts appended as bytes.
#[derive(Default)]
struct Builder {
    pool: Vec<u8>,
    count: u16,
    utf8s: HashMap<String, u16>,
}

impl Builder {
    fn new() -> Self {
        Self {
            count: 1,
            ..Self::default()
        }
    }

    fn add(&mut self, bytes: &[u8], slots: u16) -> u16 {
        let i = self.count;
        self.pool.extend_from_slice(bytes);
        self.count += slots;
        i
    }

    fn utf8(&mut self, s: &str) -> u16 {
        if let Some(&i) = self.utf8s.get(s) {
            return i;
        }
        let mut b = vec![1];
        b.extend_from_slice(&(s.len() as u16).to_be_bytes());
        b.extend_from_slice(s.as_bytes());
        let i = self.add(&b, 1);
        self.utf8s.insert(s.to_string(), i);
        i
    }

    fn u16_entry(&mut self, tag: u8, v: u16) -> u16 {
        let mut b = vec![tag];
        b.extend_from_slice(&v.to_be_bytes());
        self.add(&b, 1)
    }

    fn class(&mut self, name: &str) -> u16 {
        let n = self.utf8(name);
        self.u16_entry(7, n)
    }

    fn pair(&mut self, tag: u8, a: u16, b: u16) -> u16 {
        let mut v = vec![tag];
        v.extend_from_slice(&a.to_be_bytes());
        v.extend_from_slice(&b.to_be_bytes());
        self.add(&v, 1)
    }

    fn name_and_type(&mut self, name: &str, desc: &str) -> u16 {
        let (n, d) = (self.utf8(name), self.utf8(desc));
        self.pair(12, n, d)
    }

    fn method_ref(&mut self, class: &str, name: &str, desc: &str) -> u16 {
        let c = self.class(class);
        let nt = self.name_and_type(name, desc);
        self.pair(10, c, nt)
    }

    /// An attribute: name, length, data.
    fn attr(&mut self, name: &str, data: &[u8]) -> Vec<u8> {
        let mut v = self.utf8(name).to_be_bytes().to_vec();
        v.extend_from_slice(&(data.len() as u32).to_be_bytes());
        v.extend_from_slice(data);
        v
    }

    /// A Code attribute.
    fn code(&mut self, stack: u16, locals: u16, code: &[u8], attrs: &[Vec<u8>]) -> Vec<u8> {
        let mut d = stack.to_be_bytes().to_vec();
        d.extend_from_slice(&locals.to_be_bytes());
        d.extend_from_slice(&(code.len() as u32).to_be_bytes());
        d.extend_from_slice(code);
        d.extend_from_slice(&[0, 0]);
        d.extend_from_slice(&(attrs.len() as u16).to_be_bytes());
        for a in attrs {
            d.extend_from_slice(a);
        }
        self.attr("Code", &d)
    }

    fn member(&mut self, flags: u16, name: &str, desc: &str, attrs: &[Vec<u8>]) -> Vec<u8> {
        let mut v = flags.to_be_bytes().to_vec();
        v.extend_from_slice(&self.utf8(name).to_be_bytes());
        v.extend_from_slice(&self.utf8(desc).to_be_bytes());
        v.extend_from_slice(&(attrs.len() as u16).to_be_bytes());
        for a in attrs {
            v.extend_from_slice(a);
        }
        v
    }

    #[allow(clippy::too_many_arguments)]
    fn finish(
        self,
        minor: u16,
        major: u16,
        flags: u16,
        this: u16,
        sup: u16,
        fields: &[Vec<u8>],
        methods: &[Vec<u8>],
        attrs: &[Vec<u8>],
    ) -> Vec<u8> {
        let mut v = 0xCAFE_BABEu32.to_be_bytes().to_vec();
        v.extend_from_slice(&minor.to_be_bytes());
        v.extend_from_slice(&major.to_be_bytes());
        v.extend_from_slice(&self.count.to_be_bytes());
        v.extend_from_slice(&self.pool);
        v.extend_from_slice(&flags.to_be_bytes());
        v.extend_from_slice(&this.to_be_bytes());
        v.extend_from_slice(&sup.to_be_bytes());
        v.extend_from_slice(&[0, 0]);
        for list in [fields, methods, attrs] {
            v.extend_from_slice(&(list.len() as u16).to_be_bytes());
            for m in list {
                v.extend_from_slice(m);
            }
        }
        v
    }
}

/// An old class (Java 1.2's version) with subroutines, wide jumps, and
/// javac's rarer attributes.
fn old_class() -> Vec<u8> {
    let mut b = Builder::new();
    let this = b.class("built/Old");
    let object = b.class("java/lang/Object");
    let init = b.method_ref("java/lang/Object", "<init>", "()V");
    let source = b.utf8("Old.java");
    // jsr to a subroutine that stores its return address and rets; swap,
    // nop, a goto_w over a breakpoint, a jsr_w.
    let mut code = vec![
        0x2A, // 0 aload_0
        0xB7, // 1 invokespecial
    ];
    code.extend_from_slice(&init.to_be_bytes());
    code.extend_from_slice(&[
        0x00, // 4 nop
        0xA8, 0, 13, // 5 jsr 18
        0x04, 0x05, // 8 iconst_1, iconst_2
        0x5F, // 10 swap
        0x58, // 11 pop2
        0xC8, 0, 0, 0, 6,    // 12 goto_w 18 ... (to 18 - 12 = 6)
        0xCA, // 17 breakpoint (never reached)
        0xB1, // 18 return... but 18 is the subroutine: see below
    ]);
    // 18 is the subroutine: astore_1, ret 1; then 21: jsr_w, return.
    code.truncate(18);
    code.extend_from_slice(&[
        0x4C, // 18 astore_1
        0xA9, 1, // 19 ret 1
        0xC9, 0xFF, 0xFF, 0xFF, 0xFC, // 21 jsr_w 17
        0xB1, // 26 return
    ]);
    let crt = {
        let mut d = 1u16.to_be_bytes().to_vec();
        d.extend_from_slice(&[0, 0, 0, 4]);
        d.extend_from_slice(&((3u32 << 10) | 5).to_be_bytes());
        d.extend_from_slice(&((3u32 << 10) | 20).to_be_bytes());
        d.extend_from_slice(&0x0041u16.to_be_bytes());
        b.attr("CharacterRangeTable", &d)
    };
    let code_attr = b.code(2, 2, &code, &[crt]);
    let synthetic = b.attr("Synthetic", &[]);
    let ctor = b.member(0x0001, "<init>", "()V", &[code_attr, synthetic]);
    // An abstract method with invisible parameter annotations.
    let ann = b.utf8("Lbuilt/Ann;");
    let pa = {
        let mut d = vec![2, 0, 1];
        d.extend_from_slice(&ann.to_be_bytes());
        d.extend_from_slice(&[0, 0, 0, 0]);
        b.attr("RuntimeInvisibleParameterAnnotations", &d)
    };
    let unknown = b.attr("Vendor", &(0u8..20).collect::<Vec<u8>>());
    let abs = b.member(0x0401, "two", "(IJ)V", &[pa, unknown]);
    let sde = b.attr("SourceDebugExtension", b"SMAP\nOld.jsp\nJSP\n*S JSP\n*E\n");
    let sf = {
        let s = source.to_be_bytes();
        b.attr("SourceFile", &s)
    };
    let sid = {
        let i = b.utf8("source-id");
        b.attr("SourceID", &i.to_be_bytes())
    };
    let cid = {
        let i = b.utf8("compilation-id");
        b.attr("CompilationID", &i.to_be_bytes())
    };
    b.finish(
        3,
        46,
        0x0421,
        this,
        object,
        &[],
        &[ctor, abs],
        &[sf, sde, sid, cid],
    )
}

/// A modern class with a dynamic constant, a method type, and a method
/// handle loaded by `ldc`.
fn dynamic_class() -> Vec<u8> {
    let mut b = Builder::new();
    let this = b.class("built/Dyn");
    let object = b.class("java/lang/Object");
    let bsm_ref = b.method_ref(
        "java/lang/invoke/ConstantBootstraps",
        "nullConstant",
        "(Ljava/lang/invoke/MethodHandles$Lookup;Ljava/lang/String;Ljava/lang/Class;)Ljava/lang/Object;",
    );
    let mh = {
        let mut v = vec![15, 6];
        v.extend_from_slice(&bsm_ref.to_be_bytes());
        b.add(&v, 1)
    };
    let nt = b.name_and_type("_", "Ljava/lang/Object;");
    let condy = b.pair(17, 0, nt);
    let mt = {
        let d = b.utf8("(I)V");
        b.u16_entry(16, d)
    };
    let big = b.add(&[3, 0x7F, 0xFF, 0xFF, 0xFF], 1);
    let code = [
        0x12,
        condy as u8, // ldc condy
        0x57,        // pop
        0x12,
        mt as u8, // ldc MethodType
        0x57,
        0x12,
        mh as u8, // ldc MethodHandle
        0x57,
        0x12,
        big as u8,
        0x57,
        0xB1,
    ];
    let code_attr = b.code(1, 0, &code, &[]);
    let m = b.member(0x0009, "run", "()V", &[code_attr]);
    // The reserved opcodes and an undefined one (javap names none of them).
    let odd = b.code(0, 0, &[0xCA, 0xFE, 0xFF, 0xCB, 0xB1], &[]);
    let odd = b.member(0x0009, "odd", "()V", &[odd]);
    let bsms = {
        let mut d = 1u16.to_be_bytes().to_vec();
        d.extend_from_slice(&mh.to_be_bytes());
        d.extend_from_slice(&[0, 0]);
        b.attr("BootstrapMethods", &d)
    };
    let inner = {
        let lookup = b.class("java/lang/invoke/MethodHandles$Lookup");
        let outer = b.class("java/lang/invoke/MethodHandles");
        let name = b.utf8("Lookup");
        let mut d = 1u16.to_be_bytes().to_vec();
        for v in [lookup, outer, name, 0x0019] {
            d.extend_from_slice(&v.to_be_bytes());
        }
        b.attr("InnerClasses", &d)
    };
    b.finish(0, 65, 0x0021, this, object, &[], &[m, odd], &[bsms, inner])
}

#[test]
fn old_class_reads() {
    let data = old_class();
    let cf = ClassFile::parse(&data).unwrap();
    assert!(cf.malformed_attributes().is_empty());
    assert_eq!(cf.java_version().as_deref(), Some("1.2"));
    assert_eq!(cf.source_file().as_deref(), Some("Old.java"));
    let ctor = cf.method("<init>", "()V").unwrap();
    assert!(
        ctor.attributes
            .iter()
            .any(|a| a.value == AttributeValue::Synthetic)
    );
    let code = ctor.code().unwrap();
    let ins: Vec<_> = bytecode::decode(code.code)
        .collect::<Result<_, _>>()
        .unwrap();
    let names: Vec<String> = ins.iter().map(bytecode::Instruction::mnemonic).collect();
    assert_eq!(
        names,
        [
            "aload_0",
            "invokespecial",
            "nop",
            "jsr",
            "iconst_1",
            "iconst_2",
            "swap",
            "pop2",
            "goto_w",
            "breakpoint",
            "astore_1",
            "ret",
            "jsr_w",
            "return"
        ]
    );
    assert_eq!(ins[3].target(), Some(18));
    assert_eq!(ins[8].target(), Some(18));
    assert_eq!(ins[11].operand, Operand::Local(1));
    assert_eq!(ins[12].target(), Some(17));
    assert!(matches!(
        &code.attributes[0].value,
        AttributeValue::CharacterRangeTable(v) if v[0].flags == 0x41
    ));
    let two = cf.method("two", "(IJ)V").unwrap();
    assert!(matches!(
        &two.attributes[0].value,
        AttributeValue::RuntimeInvisibleParameterAnnotations(p) if p.len() == 2 && p[0].len() == 1
    ));
    assert_eq!(two.attributes[1].value, AttributeValue::Unknown);
    assert!(cf.attributes.iter().any(|a| matches!(
        a.value,
        AttributeValue::SourceDebugExtension(d) if d.starts_with(b"SMAP")
    )));
}

#[test]
fn dynamic_constants_read() {
    let data = dynamic_class();
    let cf = ClassFile::parse(&data).unwrap();
    assert!(cf.malformed_attributes().is_empty());
    let tags: Vec<&str> = cf
        .constant_pool
        .entries()
        .iter()
        .map(Constant::tag_name)
        .collect();
    for t in ["Dynamic", "MethodType", "MethodHandle"] {
        assert!(tags.contains(&t), "{t}");
    }
    let text = turbonop_java::javap::render(&cf, &Default::default());
    assert!(
        text.contains("// Dynamic #0:_:Ljava/lang/Object;"),
        "{text}"
    );
    assert!(text.contains("// MethodType (I)V"), "{text}");
    assert!(text.contains("// int 2147483647"), "{text}");
}

#[test]
fn damaged_attributes_are_kept_with_their_errors() {
    let mut b = Builder::new();
    let this = b.class("built/Bad");
    let object = b.class("java/lang/Object");
    let attrs = vec![
        b.attr("SourceFile", &[0]),
        b.attr("Signature", &[0, 1, 2]),
        b.attr("InnerClasses", &[0, 9]),
        b.attr("StackMapTable", &[0, 1, 200]),
        b.attr(
            "RuntimeVisibleAnnotations",
            &[0, 1, 0, 1, 0, 1, 0, 2, b'x', 0, 0],
        ),
    ];
    let data = b.finish(0, 52, 0x0021, this, object, &[], &[], &attrs);
    let cf = ClassFile::parse(&data).unwrap();
    assert_eq!(cf.malformed_attributes().len(), 5);
    // javap writes them as bytes, with the reason.
    let text = turbonop_java::javap::render(&cf, &Default::default());
    assert!(text.contains("SourceFile: length = 0x1 ("), "{text}");
}

#[test]
fn built_classes_match_javap() {
    let Some(home) = java_home() else {
        eprintln!("no JDK: skipped");
        return;
    };
    let javap = home
        .join("bin")
        .join(if cfg!(windows) { "javap.exe" } else { "javap" });
    if jdk_version(&home) != Some(21) || !javap.is_file() {
        eprintln!("no JDK 21 javap: skipped");
        return;
    }
    let dir = std::env::temp_dir().join(format!("turbonop-java-built-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    for (name, data) in [("Old.class", old_class()), ("Dyn.class", dynamic_class())] {
        let path = dir.join(name);
        std::fs::write(&path, &data).unwrap();
        let out = Command::new(&javap)
            .args([
                "-J-Dstdout.encoding=UTF-8",
                "-J-Duser.language=en",
                "-v",
                "-p",
            ])
            .arg(&path)
            .output()
            .unwrap();
        assert!(
            out.status.success(),
            "{}",
            String::from_utf8_lossy(&out.stderr)
        );
        let theirs = javap_body(&String::from_utf8(out.stdout).unwrap());
        let cf = ClassFile::parse(&data).unwrap();
        let ours = turbonop_java::javap::render(&cf, &Default::default());
        if let Some(d) = first_difference(&ours, &theirs) {
            let _ = std::fs::remove_dir_all(&dir);
            panic!("{name}: {d}");
        }
    }
    let _ = std::fs::remove_dir_all(&dir);
}
