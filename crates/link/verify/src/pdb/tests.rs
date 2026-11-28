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

/// `compare_symbols` of the Debug fixture's programs (testdata/turbolib/
/// equiv: the original and the folded), the second's reduction changed by
/// `change`.
fn symbols_changed(change: impl FnOnce(&mut Reduced)) -> Report {
    let (x, y) = (read("equiv/link-original"), read("equiv/link-folded"));
    let la = Linked::read(&x.0, &x.1, "a").unwrap();
    let lb = Linked::read(&y.0, &y.1, "b").unwrap();
    let ra = reduce(&la).unwrap();
    let mut rb = reduce(&lb).unwrap();
    change(&mut rb);
    let mut rep = Report::new();
    compare_symbols(&la, &lb, &ra, &rb, &mut rep);
    rep
}

#[test]
fn module_records_dropped_or_duplicated_are_caught() {
    let r = symbols_changed(|_| {});
    assert!(r.passed(), "{}", r.text());
    // A module of an original member, with records of its own. (A
    // synthesized member's module is only in the second program: its own
    // records, S_OBJNAME and S_COMPILE3, have nothing to compare with.)
    let own = |r: &Reduced, synthesized: bool| {
        r.modules
            .iter()
            .position(|m| m.1 == synthesized && m.2.len() >= 2)
            .expect("a module with records")
    };
    {
        let synthesized = false;
        // A record dropped.
        let r = symbols_changed(|r| {
            let i = own(r, synthesized);
            let m = &mut r.modules[i];
            m.2.pop();
            m.3.pop();
            m.4.pop();
        });
        assert!(!r.passed(), "dropped ({synthesized}):\n{}", r.text());
        // A record twice.
        let r = symbols_changed(|r| {
            let i = own(r, synthesized);
            let m = &mut r.modules[i];
            let (h, d, t) = (m.2[0], m.3[0].clone(), m.4[0]);
            m.2.push(h);
            m.3.push(d);
            m.4.push(t);
        });
        assert!(!r.passed(), "duplicated ({synthesized}):\n{}", r.text());
    }
    // A module twice, and a module dropped.
    let r = symbols_changed(|r| {
        let i = own(r, false);
        let m = r.modules[i].clone();
        r.modules.push(m);
    });
    assert!(!r.passed(), "a module twice:\n{}", r.text());
    let r = symbols_changed(|r| {
        let i = own(r, false);
        r.modules.remove(i);
    });
    assert!(!r.passed(), "a module dropped:\n{}", r.text());
}

#[test]
fn one_line_blocks_pair_whatever_their_order() {
    // The first PDB: a function's one line not at its unit's end, and one
    // at an end; the second: one at an end, and one not. Pairing the first's
    // end with the second's end first would leave the other two unpaired
    // (a line not at an end pairs only with an end); the order of the
    // blocks follows the process's hash keys.
    let block = |name: &[u8], h: u128, eof: bool| -> LineBlock {
        (name.to_vec(), h, Some((0x33, eof)), show(name))
    };
    for flip in [false, true] {
        // (Blocks are taken in the order of their hashes.)
        let (hx, hy) = if flip { (2, 1) } else { (1, 2) };
        let ra = Reduced {
            lines: vec![block(b"x", hx, false), block(b"y", hy, true)],
            ..Reduced::default()
        };
        let rb = Reduced {
            lines: vec![block(b"p", 3, true), block(b"q", 4, false)],
            ..Reduced::default()
        };
        let mut rep = Report::new();
        lines_compared(&ra, &rb, &mut rep);
        assert!(rep.passed(), "{flip}:\n{}", rep.text());
    }
}
