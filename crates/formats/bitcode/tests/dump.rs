//! The dump of every fixture in testdata/bitcode against llvm-bcanalyzer's
//! (written by tools/bitcode/oracle.py): byte for byte, records, blobs,
//! and per-block summary; IR, and other bitstreams (clang's serialized
//! diagnostics, optimization remarks) with their BLOCKINFO names. And the
//! other options: the summary with its record histograms, numeric IDs,
//! BLOCKINFO's own records, and binary blobs.

use turbonop_global_alloc as _;

use std::path::PathBuf;

use turbonop_bitcode::dump::{DumpOptions, analyze};

fn testdata() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../../testdata/bitcode")
}

/// What each oracle file's options are, by its suffix (see oracle.py).
fn options(file: &str) -> Option<(&str, DumpOptions)> {
    let dump = DumpOptions {
        histogram: false,
        ..DumpOptions::default()
    };
    if let Some(input) = file.strip_suffix(".dump.txt") {
        return Some((input, dump));
    }
    if let Some(input) = file.strip_suffix(".stats.txt") {
        return Some((
            input,
            DumpOptions {
                records: false,
                ..DumpOptions::default()
            },
        ));
    }
    if let Some(input) = file.strip_suffix(".nonsymbolic.txt") {
        return Some((
            input,
            DumpOptions {
                symbolic: false,
                ..dump
            },
        ));
    }
    if let Some(input) = file.strip_suffix(".blockinfo.txt") {
        return Some((
            input,
            DumpOptions {
                blockinfo: true,
                binary_blobs: true,
                // (warn.dia's has the histograms too.)
                histogram: input == "warn.dia",
                ..dump
            },
        ));
    }
    None
}

#[test]
fn dumps_match_llvm_bcanalyzer() {
    let mut checked = 0;
    for e in std::fs::read_dir(testdata()).unwrap() {
        let path = e.unwrap().path();
        let name = path.file_name().unwrap().to_str().unwrap().to_string();
        let Some((input, opts)) = options(&name) else {
            continue;
        };
        let want = std::fs::read(&path).unwrap();
        let data = std::fs::read(testdata().join(input)).unwrap();
        let mut have = Vec::new();
        analyze(&data, &opts, Some(input), &mut have).unwrap();
        if have != want {
            let have = String::from_utf8_lossy(&have);
            let want = String::from_utf8_lossy(&want);
            for (i, (h, w)) in have.lines().zip(want.lines()).enumerate() {
                assert_eq!(h, w, "{name}: line {}", i + 1);
            }
            assert_eq!(
                have.lines().count(),
                want.lines().count(),
                "{name}: line count"
            );
            panic!("{name}: differs (line ends?)");
        }
        checked += 1;
    }
    assert!(checked >= 13, "{checked} dumps");
}
