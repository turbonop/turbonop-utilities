//! Structural hashing of type records, and PDB comparisons of the
//! fixture programs.

use super::*;
use crate::report::Severity;
use turbonop_codeview::types::TypeRecord;

/// `LF_POINTER` to `ti` (64-bit, near).
fn pointer(ti: u32) -> (u16, Vec<u8>) {
    let mut v = ti.to_le_bytes().to_vec();
    v.extend(0x0001_000Cu32.to_le_bytes());
    (0x1002, v)
}

/// `LF_ARGLIST` of `args`.
fn arglist(args: &[u32]) -> (u16, Vec<u8>) {
    let mut v = (args.len() as u32).to_le_bytes().to_vec();
    for a in args {
        v.extend(a.to_le_bytes());
    }
    (0x1201, v)
}

/// `LF_PROCEDURE` returning `ret` taking `args` (an arglist's index).
fn procedure(ret: u32, count: u16, args: u32) -> (u16, Vec<u8>) {
    let mut v = ret.to_le_bytes().to_vec();
    v.extend([0, 0]);
    v.extend(count.to_le_bytes());
    v.extend(args.to_le_bytes());
    (0x1008, v)
}

fn hashes(records: &[(u16, Vec<u8>)]) -> types::Hashes {
    let r: Vec<TypeRecord<'_>> = records
        .iter()
        .map(|(k, d)| TypeRecord { kind: *k, data: d })
        .collect();
    types::hash(&r, None, None, false, false)
}

#[test]
fn equal_types_hash_alike_whatever_their_indices() {
    // int* (0x74 is int), (int*) -> int*; in two orders.
    let a = hashes(&[
        pointer(0x74),
        arglist(&[0x1000]),
        procedure(0x1000, 1, 0x1001),
    ]);
    let b = hashes(&[
        arglist(&[0x74]),
        pointer(0x74),
        arglist(&[0x1001]),
        procedure(0x1001, 1, 0x1002),
    ]);
    // b's first record is another type; the rest are a's.
    assert_eq!(a.of(0x1000), b.of(0x1001));
    assert_eq!(a.of(0x1001), b.of(0x1002));
    assert_eq!(a.of(0x1002), b.of(0x1003));
    assert_ne!(a.of(0x1001), b.of(0x1000));
    // Built-in types hash as their index.
    assert_eq!(a.of(0x74), 0x74);
    assert_eq!(a.forward, 0);
    // A pointer to another type differs.
    let c = hashes(&[pointer(0x75)]);
    assert_ne!(a.of(0x1000), c.of(0x1000));
}

#[test]
fn forward_and_unknown_records_are_counted() {
    let h = hashes(&[pointer(0x1001), pointer(0x74), (0x7777, vec![1, 2, 3, 4])]);
    assert_eq!(h.forward, 1);
    assert_eq!(h.unknown, vec![0x7777]);
}

fn read(name: &str) -> (Vec<u8>, Vec<u8>) {
    let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../testdata/turbolib");
    (
        std::fs::read(dir.join(format!("{name}.exe"))).unwrap(),
        std::fs::read(dir.join(format!("{name}.pdb"))).unwrap(),
    )
}

fn compare_pdbs(a: &str, b: &str) -> Report {
    let (x, y) = (read(a), read(b));
    let la = Linked::read(&x.0, &x.1, "a").unwrap();
    let lb = Linked::read(&y.0, &y.1, "b").unwrap();
    compare(&la, &lb)
}

#[test]
fn pdbs_of_equivalent_links_compare_equal() {
    let r = compare_pdbs("link-1", "link-2");
    assert!(r.passed(), "{}", r.text());
    assert!(r.checked_count("procedures") >= 10, "{}", r.text());
    // Every symbol record's fields are known.
    assert!(
        r.findings()
            .all(|(s, c, _)| s != Severity::Unchecked || !c.starts_with("symbol records of kinds")),
        "{}",
        r.text()
    );
    // GHASH's PDB holds the same types, ids, and symbols as lld's default.
    let r = compare_pdbs("lld-1", "lld-ghash");
    let failed: Vec<&str> = r
        .findings()
        .filter(|(s, _, _)| *s == Severity::Failure)
        .map(|(_, c, _)| c)
        .collect();
    assert_eq!(
        failed,
        ["the linker's own records differ (its command line, sections, ...)"],
        "{}",
        r.text()
    );
}

#[test]
fn a_broken_library_shows_in_the_pdb() {
    // shared_add's other code has other lines.
    let r = compare_pdbs("link-1", "link-broken");
    assert!(
        r.failed("line blocks only in the first PDB (or different)"),
        "{}",
        r.text()
    );
}

#[test]
fn a_source_line_in_another_copy_of_a_header_is_a_pick() {
    let sdk_a = br"C:\Program Files (x86)\Windows Kits\10\include\10.0.26100.0\um\winnt.h";
    let sdk_b = br"F:\1\s\src\ExternalAPIs\WindowsSDKInc\c\Include\10.0.28000.0\um/WINNT.H";
    assert_eq!(super::file_name(sdk_a), b"winnt.h");
    assert_eq!(super::file_name(sdk_b), b"WINNT.H");
    assert_eq!(super::file_name(b"winnt.h"), b"winnt.h");
    assert!(super::file_name(sdk_a).eq_ignore_ascii_case(super::file_name(sdk_b)));
}
