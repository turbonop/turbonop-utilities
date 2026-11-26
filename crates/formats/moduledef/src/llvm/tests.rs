extern crate std;

use std::format;
use std::path::PathBuf;
use std::vec;

use super::*;
use turbonop_archive::write::Style;

fn root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../..")
}

fn def(name: &str) -> Vec<u8> {
    std::fs::read(root().join(format!("tools/coff/archives/def/{name}.def"))).unwrap()
}

/// llvm-lib's libraries of the fixtures, written again byte for byte.
#[test]
fn the_fixtures() {
    let cases = [
        (
            "def-x64_llvm-x64-llvm.lib",
            "x64_llvm",
            machine::AMD64,
            None,
        ),
        ("def-x86-x86-llvm.lib", "x86", machine::I386, None),
        ("def-ec2-arm64ec-llvm.lib", "ec2", machine::ARM64EC, None),
        (
            "def-ec-arm64x-llvm.lib",
            "ec",
            machine::ARM64X,
            Some("native"),
        ),
    ];
    for (lib, name, m, native) in cases {
        let want = std::fs::read(root().join("testdata/coff/archives").join(lib)).unwrap();
        let d = parse(&def(name), m).unwrap();
        let (n, out) = match native {
            Some(n) => {
                let n = parse(&def(n), machine::ARM64).unwrap();
                let out = n.output_file.clone();
                (n.exports, out)
            }
            None => (Vec::new(), d.output_file.clone()),
        };
        let got = members(&out, &d.exports, m, &n, &ec_insertion)
            .unwrap()
            .write(Style::LlvmLib, Tool::LlvmLib)
            .unwrap();
        assert!(got == want, "{lib}");
    }
}

#[test]
fn grammar() {
    let d = parse(
        b"LIBRARY \"a b\" BASE = 0x10\nEXPORTS\n  f = g @3 NONAME DATA ; c\n  h == i\n  j EXPORTAS k\n  @fast@4\n  m @ 5\n  n\n  @bad\nHEAPSIZE 010, 0b11\nVERSION 2.7\n",
        machine::AMD64,
    )
    .unwrap();
    assert_eq!(d.output_file, b"a b.dll");
    assert_eq!(d.image_base, 0x10);
    assert_eq!(d.heap, (8, 3));
    assert_eq!(d.version, (2, 7));
    let e = &d.exports;
    assert_eq!(
        (
            &e[0].name[..],
            &e[0].ext_name[..],
            e[0].ordinal,
            e[0].noname,
            e[0].data
        ),
        (&b"g"[..], &b"f"[..], 3, true, true)
    );
    assert_eq!(e[1].import_name, b"i");
    assert_eq!(e[2].export_as, b"k");
    assert_eq!(e[3].name, b"@fast@4");
    assert_eq!((&e[4].name[..], e[4].ordinal), (&b"m"[..], 5));
    // "n", then "@bad", which isn't an ordinal: the next export.
    assert_eq!((&e[5].name[..], &e[6].name[..]), (&b"n"[..], &b"@bad"[..]));
    // Keywords only in capitals; others are errors at the top.
    assert!(parse(b"exports\n f\n", machine::AMD64).is_err());
    assert!(parse(b"DESCRIPTION x\n", machine::AMD64).is_err());
    // A keyword ends EXPORTS: DATA alone is then an unknown directive.
    assert_eq!(
        parse(b"EXPORTS\n DATA\n", machine::AMD64),
        Err(LlvmParseError("unknown directive: DATA".into()))
    );
    // x86's underscores.
    let d = parse(
        b"EXPORTS\n f\n _g@4\n ?h@@YAXXZ\n a=b\n c=x.y\n",
        machine::I386,
    )
    .unwrap();
    let names: Vec<(&[u8], &[u8])> = d
        .exports
        .iter()
        .map(|e| (&e.name[..], &e.ext_name[..]))
        .collect();
    assert_eq!(
        names,
        vec![
            (&b"_f"[..], &b""[..]),
            (b"_g@4", b""),
            (b"?h@@YAXXZ", b""),
            (b"_b", b"_a"),
            (b"x.y", b"_c"),
        ]
    );
}

#[test]
fn numbers() {
    assert_eq!(llvm_integer(b"0x1F", 0, u64::MAX), Some(31));
    assert_eq!(llvm_integer(b"017", 0, u64::MAX), Some(15));
    assert_eq!(llvm_integer(b"0o17", 0, u64::MAX), Some(15));
    assert_eq!(llvm_integer(b"0b101", 0, u64::MAX), Some(5));
    assert_eq!(llvm_integer(b"0", 0, u64::MAX), Some(0));
    assert_eq!(llvm_integer(b"1k", 0, u64::MAX), None);
    assert_eq!(llvm_integer(b"", 10, u64::MAX), None);
    assert_eq!(llvm_integer(b"65536", 10, 0xFFFF), None);
}

#[test]
fn paths() {
    assert_eq!(path_filename(b"C:\\x\\y.dll"), b"y.dll");
    assert_eq!(path_filename(b"x/"), b".");
    assert_eq!(path_stem(b"a.b.dll"), b"a.b");
    assert_eq!(path_stem(b".dll"), b"");
    assert!(!has_extension(b"dir.d\\x"));
}

/// The weak external of `a == b` when `b` is imported too, read back by
/// `turbonop-coff` as written.
#[test]
fn weak_externals() {
    let o = weak_external(b"b", b"a", true, machine::AMD64);
    assert_eq!(o.len(), 20 + 40 + 5 * 18 + 4 + 8 + 8);
    let parsed = turbonop_coff::Object::parse(&o).unwrap();
    assert_eq!(parsed.symbols.len(), 4);
    assert_eq!(parsed.symbols[3].name.as_bytes(), b"__imp_a");
    assert_eq!(parsed.symbols[3].storage_class, 105);
    assert_eq!(parsed.write().unwrap(), o);
}
