//! Damaged classes and JARs: the fixtures with bytes changed and cut
//! short, read through every reader and the javap writer. Any result is
//! fine but a panic (or a hang).

#![allow(clippy::cast_possible_truncation)]

use turbonop_global_alloc as _;

mod common;

use common::testdata;
use turbonop_java::attributes::AttributeValue;
use turbonop_java::types::{parse_any_signature, parse_field_descriptor, parse_method_descriptor};
use turbonop_java::{ClassFile, Jar, bytecode};

/// xorshift64*: deterministic, so a failure reproduces.
struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 >> 12;
        self.0 ^= self.0 << 25;
        self.0 ^= self.0 >> 27;
        self.0.wrapping_mul(0x2545_F491_4F6C_DD1D)
    }

    fn below(&mut self, n: usize) -> usize {
        (self.next() % n as u64) as usize
    }
}

fn walk_class(data: &[u8]) {
    let Ok(cf) = ClassFile::parse(data) else {
        return;
    };
    let cp = &cf.constant_pool;
    let _ = (
        cf.name(),
        cf.super_name(),
        cf.interface_names(),
        cf.package(),
    );
    let _ = (
        cf.source_file(),
        cf.malformed_attributes(),
        cf.java_version(),
    );
    for m in cf.methods.iter().chain(&cf.fields) {
        if let (Ok(n), Ok(d)) = (m.name(cp), m.descriptor(cp)) {
            let _ = (n, parse_method_descriptor(&d), parse_field_descriptor(&d));
        }
        for a in &m.attributes {
            match &a.value {
                AttributeValue::Signature(s) => {
                    if let Ok(s) = cp.utf8(*s) {
                        let _ = parse_any_signature(&s);
                    }
                }
                AttributeValue::Code(c) => {
                    let _ = bytecode::decode(c.code).count();
                }
                _ => {}
            }
        }
    }
    for i in 0..cp.count() {
        let i = i as u16;
        let _ = (
            cp.utf8(i),
            cp.class_name(i),
            cp.member_ref(i),
            cp.name_and_type(i),
        );
    }
    let _ = turbonop_java::javap::render(&cf, &Default::default());
}

fn fixture_classes() -> Vec<Vec<u8>> {
    [
        "main/fixture/Everything.class",
        "main/fixture/Annotated.class",
        "main/fixture/Info.class",
        "main/module-info.class",
        "main/fixture/Point.class",
        "old/legacy/Legacy.class",
        "main/fixture/Shape.class",
    ]
    .iter()
    .map(|n| std::fs::read(testdata(n)).unwrap())
    .collect()
}

#[test]
fn mutated_classes_never_panic() {
    for data in fixture_classes() {
        let seeds: Vec<u64> = (0..3000).collect();
        turbonop_par::map(&seeds, 16, |&seed| {
            let mut rng = Rng(seed.wrapping_mul(0x9E37_79B9_7F4A_7C15) | 1);
            let mut d = data.clone();
            for _ in 0..1 + rng.below(4) {
                let at = rng.below(d.len());
                d[at] = match rng.below(5) {
                    0 => 0,
                    1 => 0xFF,
                    2 => d[at] ^ (1 << rng.below(8)),
                    3 => d[at].wrapping_add(1),
                    _ => rng.next() as u8,
                };
            }
            walk_class(&d);
        });
    }
}

#[test]
fn truncated_classes_never_panic() {
    for data in fixture_classes() {
        let lens: Vec<usize> = (0..data.len()).collect();
        turbonop_par::map(&lens, 64, |&n| walk_class(&data[..n]));
    }
}

#[test]
fn every_byte_of_a_small_class() {
    // Each byte of a whole class, each of a few values: every field, count,
    // index, and length damaged in turn.
    let data = std::fs::read(testdata("main/fixture/Hello.class")).unwrap();
    let cases: Vec<(usize, u8)> = (0..data.len())
        .flat_map(|i| [0u8, 1, 0x7F, 0x80, 0xFF].map(|v| (i, v)))
        .collect();
    turbonop_par::map(&cases, 64, |&(at, v)| {
        let mut d = data.clone();
        d[at] = v;
        walk_class(&d);
    });
}

#[test]
fn mutated_jars_never_panic() {
    let data = std::fs::read(testdata("mr.jar")).unwrap();
    let seeds: Vec<u64> = (0..1500).collect();
    turbonop_par::map(&seeds, 16, |&seed| {
        let mut rng = Rng(seed | 1);
        let mut d = data.clone();
        for _ in 0..1 + rng.below(3) {
            let at = rng.below(d.len());
            d[at] = rng.next() as u8;
        }
        if let Ok(jar) = Jar::parse(&d) {
            let _ = (jar.releases(), jar.signature_files(), jar.package_sizes());
            let _ = jar.classes_for_release(21);
            for c in jar.classes() {
                if let Ok(b) = jar.archive().read(&jar.archive().entries()[c.index]) {
                    walk_class(&b);
                }
            }
        }
    });
}
