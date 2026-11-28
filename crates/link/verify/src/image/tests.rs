//! `compare` on the programs `tools/turbolib/verify/build.cmd` links: the
//! same inputs linked twice, in other orders, with lld-link's GHASH, and
//! against a broken library; and images changed here.

use super::*;
use crate::report::Severity;

fn fixture(name: &str) -> Vec<u8> {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../../testdata/turbolib")
        .join(name);
    std::fs::read(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display()))
}

type Files = (Vec<u8>, Vec<u8>);

fn program(name: &str) -> Files {
    (
        fixture(&format!("{name}.exe")),
        fixture(&format!("{name}.pdb")),
    )
}

fn run(a: &Files, b: &Files) -> Report {
    compare(
        Program {
            image: &a.0,
            pdb: &a.1,
        },
        Program {
            image: &b.0,
            pdb: &b.1,
        },
    )
    .unwrap()
}

/// The failing categories.
fn failures(r: &Report) -> Vec<&str> {
    r.findings()
        .filter(|(s, _, _)| *s == Severity::Failure)
        .map(|(_, c, _)| c)
        .collect()
}

#[test]
fn two_links_of_the_same_inputs_are_equivalent() {
    let a = program("link-1");
    let b = program("link-2");
    let r = run(&a, &b);
    assert!(r.passed(), "{}", r.text());
    // Everything was looked at.
    for what in [
        "functions and data (section contributions) compared",
        "PC-relative operands matched by name",
        "absolute addresses (base relocations) matched by name",
        "imports",
        "exports",
        "C++ initializers (.CRT$XC*)",
        "CFG function table entries",
        "unwind entries",
        "types (TPI) records",
        "ids (IPI) records",
        "publics",
        "globals",
        "procedures",
        "line blocks",
        "inlinees' lines",
    ] {
        assert!(r.checked_count(what) > 0, "{what}:\n{}", r.text());
    }
    // And a program with itself.
    let r = run(&a, &a);
    assert!(r.passed(), "{}", r.text());
    let r = run(&program("lld-1"), &program("lld-1"));
    assert!(r.passed(), "{}", r.text());
}

#[test]
fn other_layouts_are_equivalent_but_for_the_command_line() {
    // /ORDER moves functions; GHASH builds the PDB another way. Only the
    // linker's own records (its command line) differ.
    for (x, y) in [
        ("link-1", "link-order"),
        ("lld-1", "lld-order"),
        ("lld-1", "lld-ghash"),
    ] {
        let r = run(&program(x), &program(y));
        assert_eq!(
            failures(&r),
            ["the linker's own records differ (its command line, sections, ...)"],
            "{x} and {y}:\n{}",
            r.text()
        );
    }
    let r = run(&program("link-1"), &program("link-order"));
    assert!(
        r.finding(Severity::Expected, "sections of another size")
            .is_some()
    );
}

#[test]
fn a_broken_library_is_caught() {
    let r = run(&program("link-1"), &program("link-broken"));
    let f = failures(&r);
    for want in [
        "functions or data whose bytes differ",
        "line blocks only in the first PDB (or different)",
    ] {
        assert!(f.contains(&want), "{want}:\n{}", r.text());
    }
    let bytes = r
        .finding(Severity::Failure, "functions or data whose bytes differ")
        .unwrap();
    assert!(
        bytes
            .examples
            .iter()
            .any(|e| e.starts_with("?shared_add@@YAHHH@Z")),
        "{}",
        r.text()
    );
}

#[test]
fn a_pdb_of_another_link_is_caught() {
    let a = program("link-1");
    let other = (a.0.clone(), fixture("link-order.pdb"));
    let r = run(&a, &other);
    assert!(
        r.failed("programs whose PDB doesn't match the image"),
        "{}",
        r.text()
    );
}

/// The RVA of public `name` in a program.
fn public(p: &Files, name: &[u8]) -> u32 {
    let l = Linked::read(&p.0, &p.1, "the program").unwrap();
    l.labels.iter().find(|x| *x.name == *name).unwrap().rva
}

/// The file offset of `rva` in a program's image.
fn offset(p: &Files, rva: u32) -> usize {
    let pe = turbonop_pe::PeFile::parse_image(&p.0).unwrap();
    pe.rva_to_offset(turbonop_core::Rva(rva)).unwrap() as usize
}

#[test]
fn changed_code_is_caught() {
    let a = program("link-1");
    // A byte of shared_add's code.
    let at = offset(&a, public(&a, b"?shared_add@@YAHHH@Z"));
    let mut image = a.0.clone();
    image[at] ^= 0x01;
    let r = run(&a, &(image, a.1.clone()));
    assert!(
        r.failed("functions or data whose bytes differ"),
        "{}",
        r.text()
    );
    // entry's reference to fixture_export, sent to check_icall instead.
    let entry = public(&a, b"entry");
    let target = public(&a, b"fixture_export");
    let other = public(&a, b"check_icall");
    let start = offset(&a, entry);
    let found = (start..start + 0x60).find(|&at| {
        let d = i32::from_le_bytes(a.0[at..at + 4].try_into().unwrap());
        let next = i64::from(entry) + (at - start) as i64 + 4;
        next + i64::from(d) == i64::from(target)
    });
    let at = found.expect("a reference to fixture_export");
    let d = (i64::from(other) - (i64::from(entry) + (at - start) as i64 + 4)) as i32;
    let mut image = a.0.clone();
    image[at..at + 4].copy_from_slice(&d.to_le_bytes());
    let r = run(&a, &(image, a.1.clone()));
    assert!(
        r.failed("code whose operands name other functions or data"),
        "{}",
        r.text()
    );
}

#[test]
fn not_images_are_errors() {
    let a = program("link-1");
    let bad = compare(
        Program {
            image: b"MZ nope",
            pdb: &a.1,
        },
        Program {
            image: &a.0,
            pdb: &a.1,
        },
    );
    assert!(bad.is_err());
    let bad = compare(
        Program {
            image: &a.0,
            pdb: b"nope",
        },
        Program {
            image: &a.0,
            pdb: &a.1,
        },
    );
    assert!(bad.is_err());
}

mod equiv;
