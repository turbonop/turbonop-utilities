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

/// The vtables program's tables (tools/vtables), with its full PDB or its
/// stripped one (public symbols only).
fn vtables_program(arch: &str, stripped: bool) -> Vec<Value> {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../testdata/vtables");
    let exe = dir.join(format!("vtables-{arch}.exe"));
    let pdb = dir.join(if stripped {
        format!("vtables-{arch}-stripped.pdb")
    } else {
        format!("vtables-{arch}.pdb")
    });
    let (code, out, err) = hexcavator(&[
        "-json",
        "-debug",
        pdb.to_str().unwrap(),
        exe.to_str().unwrap(),
    ]);
    assert_eq!(code, 0, "{err}");
    serde_json::from_str::<Value>(&out)
        .unwrap()
        .as_array()
        .unwrap()
        .clone()
}

/// With RTTI, a table no symbol names (a lambda's std::function's, which
/// a stripped PDB leaves out) is found by its locator and named for its
/// class; and every table, with its full PDB or its stripped one, has the
/// same slots (the tables before a lambda's end at its locator).
#[test]
fn tables_found_and_bounded_by_their_rtti() {
    for arch in ["x64", "arm64"] {
        let full = vtables_program(arch, false);
        let stripped = vtables_program(arch, true);
        let at = |t: &[Value]| -> Vec<(String, usize)> {
            t.iter()
                .map(|v| {
                    (
                        v["rva"].as_str().unwrap().to_string(),
                        v["slots"].as_array().unwrap().len(),
                    )
                })
                .collect()
        };
        assert_eq!(at(&full), at(&stripped), "{arch}");
        assert!(full.iter().all(|t| t["by_rtti"] == false), "{arch}");
        let lambdas: Vec<&Value> = stripped
            .iter()
            .filter(|t| t["name"].as_str().unwrap().contains("<lambda_"))
            .collect();
        assert_eq!(lambdas.len(), 3, "{arch}");
        for t in &lambdas {
            assert_eq!(t["by_rtti"], true, "{arch}");
            assert_eq!(t["sized"], false, "{arch}");
            assert!(
                t["class"]
                    .as_str()
                    .unwrap()
                    .starts_with("std::_Func_impl_no_alloc<"),
                "{arch}: {}",
                t["class"]
            );
            // _Copy, _Move, _Do_call, _Target_type, _Delete_this, and the
            // destructor's.
            assert_eq!(t["slots"].as_array().unwrap().len(), 6, "{arch}");
        }
        // A second base's table: named for that base.
        assert!(
            stripped
                .iter()
                .any(|t| t["name"] == "const Badge::`vftable'{for `Named'}"),
            "{arch}"
        );
    }
}
