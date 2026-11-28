//! turbonop-dex against dexdump: the fixtures (testdata/dex, built by
//! tools/dex/build.cmd with javac and d8) dumped as `dexdump -a -d -f -h`
//! dumps them, byte for byte against its committed output.

use turbonop_global_alloc as _;

mod dump;

use std::path::PathBuf;

fn testdata(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../../testdata/dex")
        .join(name)
}

/// (dexdump.exe writes CRLF.)
fn crlf_to_lf(b: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(b.len());
    for (i, &c) in b.iter().enumerate() {
        if !(c == b'\r' && b.get(i + 1) == Some(&b'\n')) {
            out.push(c);
        }
    }
    out
}

fn check(name: &str) {
    let data = std::fs::read(testdata(name)).unwrap();
    let theirs = crlf_to_lf(&std::fs::read(testdata(&format!("{name}.txt"))).unwrap());
    let ours = dump::dump(name, &data);
    if let Some(d) = dump::first_difference(&ours, &theirs) {
        panic!("{name} differs from dexdump at {d}");
    }
}

#[test]
fn dex_035_matches_dexdump() {
    check("Sample-035.dex");
}

#[test]
fn dex_037_matches_dexdump() {
    check("Sample-037.dex");
}

#[test]
fn dex_038_matches_dexdump() {
    check("Sample-038.dex");
}

#[test]
fn dex_039_matches_dexdump() {
    check("Sample-039.dex");
}

#[test]
fn dex_041_matches_dexdump() {
    check("Sample-041.dex");
}
