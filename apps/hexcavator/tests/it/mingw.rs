//! PE images with DWARF, as MinGW (and clang's windows-gnu targets) build
//! them (testdata/mingw, from tools/mingw): read as every other image with
//! its debug information is, at the image's relative addresses (DWARF's
//! are virtual), from the image itself or from the debug file its
//! .gnu_debuglink names.

use std::path::{Path, PathBuf};
use std::process::Command;

fn mingw(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../testdata/mingw")
        .join(name)
}

fn hexcavator(args: &[&str]) -> String {
    let out = Command::new(env!("CARGO_BIN_EXE_hexcavator"))
        .args(args)
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "{args:?}: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8(out.stdout).unwrap()
}

#[test]
fn symbols_at_the_images_addresses() {
    for (image, used, unused) in [
        ("program.exe", 0x1000, 0x1010),
        ("program-arm64.exe", 0x1000, 0x100C),
    ] {
        let path = mingw(image);
        let out = hexcavator(&[
            "query",
            "-json",
            path.to_str().unwrap(),
            "kind=function select name, rva, compiland",
        ]);
        let v: serde_json::Value = serde_json::from_str(&out).unwrap();
        let rva = |name: &str| {
            v["rows"]
                .as_array()
                .unwrap()
                .iter()
                .find(|r| r["name"] == name)
                .unwrap_or_else(|| panic!("{image}: {v}"))["rva"]
                .as_u64()
        };
        assert_eq!(rva("used(int)"), Some(used), "{image}");
        assert_eq!(rva("unused(int)"), Some(unused), "{image}");
        assert!(
            v["rows"][0]["compiland"]
                .as_str()
                .unwrap()
                .ends_with("program.c")
        );
    }
}

/// The stripped image's DWARF, from the debug file .gnu_debuglink names.
#[test]
fn separate_debug_file() {
    let info = hexcavator(&[
        "info",
        "-json",
        mingw("program-stripped.exe").to_str().unwrap(),
    ]);
    let v: serde_json::Value = serde_json::from_str(&info).unwrap();
    assert!(
        v[0]["debug"]["path"]
            .as_str()
            .unwrap()
            .ends_with("program.debug"),
        "{v}"
    );
    assert_eq!(v[0]["debug"]["kind"], "dwarf");
    // (A PDB's annotations aren't DWARF's.)
    let tools: Vec<&str> = v[0]["tools"]
        .as_array()
        .unwrap()
        .iter()
        .map(|t| t.as_str().unwrap())
        .collect();
    assert!(
        tools.contains(&"why") && !tools.contains(&"annotations"),
        "{tools:?}"
    );
    let out = hexcavator(&[
        "symbolize",
        mingw("program-stripped.exe").to_str().unwrap(),
        "0x140001010",
    ]);
    assert!(
        out.contains("unused") && out.contains("program.c:9"),
        "{out}"
    );
}

/// Its link judged as MinGW's linkers link: sections of their own and
/// --gc-sections, not COMDATs and /OPT:REF.
#[test]
fn in_its_linkers_terms() {
    let out = hexcavator(&[
        "xref",
        "-why",
        "unused",
        mingw("program.exe").to_str().unwrap(),
    ]);
    assert!(
        out.contains("--gc-sections") && !out.contains("/Gy"),
        "{out}"
    );
    let out = hexcavator(&["recommend", "-json", mingw("program.exe").to_str().unwrap()]);
    let v: serde_json::Value = serde_json::from_str(&out).unwrap();
    assert_eq!(v["toolchain"], "gnu");
    assert_eq!(v["debug"], true);
}
