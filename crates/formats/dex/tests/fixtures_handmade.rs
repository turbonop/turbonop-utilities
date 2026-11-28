//! The hand-made fixtures (testdata/dex/Handmade-*.dex, written by
//! tests/handmade with tests/writer: every opcode, payload, debug opcode,
//! value kind, hidden-API flags, a container of two files) against
//! dexdump's committed dumps of them, byte for byte; and the writer
//! still writes them as committed.
//!
//! `TURBONOP_DEX_BLESS=1` writes the fixtures again and, with dexdump
//! (the Android SDK's build-tools), their dumps.

use turbonop_global_alloc as _;

mod dump;
mod handmade;
mod sdk;
mod writer;

use std::path::PathBuf;

fn testdata() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../../testdata/dex")
}

fn crlf_to_lf(b: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(b.len());
    for (i, &c) in b.iter().enumerate() {
        if !(c == b'\r' && b.get(i + 1) == Some(&b'\n')) {
            out.push(c);
        }
    }
    out
}

fn check(name: &str, data: &[u8]) {
    let dir = testdata();
    if std::env::var_os("TURBONOP_DEX_BLESS").is_some() {
        std::fs::write(dir.join(name), data).unwrap();
        if let Some(d) = sdk::run_dexdump(&dir, name) {
            std::fs::write(dir.join(format!("{name}.txt")), d).unwrap();
        }
    }
    let committed = std::fs::read(dir.join(name)).unwrap();
    assert!(committed == data, "{name}: the writer no longer writes it as committed (bless it)");
    let theirs = crlf_to_lf(&std::fs::read(dir.join(format!("{name}.txt"))).unwrap());
    let ours = dump::dump(name, data);
    if let Some(d) = dump::first_difference(&ours, &theirs) {
        panic!("{name} differs from dexdump at {d}");
    }
    // And it verifies.
    for dex in turbonop_dex::Dex::parse_all(data).unwrap() {
        let v = dex.verify();
        assert!(v.checksum_ok && v.signature_ok, "{name}");
    }
}

#[test]
fn handmade_039_matches_dexdump() {
    check("Handmade-039.dex", &handmade::handmade(39));
}

#[test]
fn handmade_040_matches_dexdump() {
    check("Handmade-040.dex", &handmade::handmade(40));
}

#[test]
fn handmade_container_matches_dexdump() {
    check("Handmade-container.dex", &handmade::container());
}
