//! `hexcavator vtables` end to end: the xref program's classes (tools/xref),
//! for x64 and ARM64.

use std::path::{Path, PathBuf};
use std::process::Command;

use serde_json::Value;

fn fixture(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../testdata/xref")
        .join(name)
}

fn hexcavator(args: &[&str]) -> (i32, String, String) {
    let out = Command::new(env!("CARGO_BIN_EXE_hexcavator"))
        .arg("vtables")
        .args(args)
        .output()
        .unwrap();
    (
        out.status.code().unwrap_or(-1),
        String::from_utf8(out.stdout).unwrap(),
        String::from_utf8(out.stderr).unwrap(),
    )
}

fn tables(image: &str, filter: &str) -> Vec<Value> {
    let exe = fixture(image);
    let (code, out, err) = hexcavator(&["-json", exe.to_str().unwrap(), filter]);
    assert_eq!(code, 0, "{err}");
    serde_json::from_str::<Value>(&out)
        .unwrap()
        .as_array()
        .unwrap()
        .clone()
}

/// The names of a table's slots, in order.
fn slots(t: &Value) -> Vec<String> {
    t["slots"]
        .as_array()
        .unwrap()
        .iter()
        .map(|s| s["name"].as_str().unwrap_or("?").to_string())
        .collect()
}

#[test]
fn each_slot_by_the_function_it_points_to() {
    for image in ["xref-x64.exe", "xref-arm64.exe"] {
        let t = tables(image, "Square::");
        assert_eq!(t.len(), 1, "{image}");
        let sq = &t[0];
        assert_eq!(sq["class"], "Square");
        assert_eq!(sq["section"], ".rdata");
        assert_eq!(sq["sized"], true);
        assert_eq!(
            slots(sq),
            [
                "Square::`scalar deleting destructor'(unsigned int)",
                "Square::area() const",
                "Square::name() const",
            ],
            "{image}"
        );
        assert_eq!(sq["slots"][2]["offset"], 16);
    }
}

#[test]
fn inherited_and_pure_slots() {
    let t = tables("xref-x64.exe", "Circle::");
    // Circle doesn't override name(): its slot is Shape's.
    assert_eq!(
        slots(&t[0]),
        [
            "Circle::`scalar deleting destructor'(unsigned int)",
            "Circle::area() const",
            "Shape::name() const",
        ]
    );
    assert_eq!(t[0]["slots"][2]["own"], 0);
    let shape = tables("xref-x64.exe", "Shape::");
    assert_eq!(slots(&shape[0])[1], "_purecall");
}

#[test]
fn every_table_listed() {
    let exe = fixture("xref-x64.exe");
    let (code, out, err) = hexcavator(&[exe.to_str().unwrap()]);
    assert_eq!(code, 0, "{err}");
    for class in [
        "Tag",
        "Shape",
        "Square",
        "Circle",
        "Ghost",
        "std::exception",
    ] {
        assert!(
            out.lines()
                .any(|l| l.starts_with(&format!("{class}::`vftable'"))),
            "{class}: {out}"
        );
    }
    assert!(out.trim_end().ends_with("(9 tables)"), "{out}");
}

#[test]
fn usage_errors() {
    let (code, _, err) = hexcavator(&[]);
    assert_eq!(code, 2);
    assert!(err.contains("usage: hexcavator vtables"), "{err}");
}
