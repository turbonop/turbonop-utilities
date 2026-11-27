//! The dump of every fixture in testdata/bitcode against llvm-bcanalyzer's
//! (`--dump --disable-histogram`, written by tools/bitcode/oracle.py):
//! byte for byte, records, blobs, and per-block summary.

use turbonop_global_alloc as _;

use std::path::PathBuf;

use turbonop_bitcode::dump::{DumpOptions, analyze};

fn testdata() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../../testdata/bitcode")
}

#[test]
fn dumps_match_llvm_bcanalyzer() {
    let mut checked = 0;
    for e in std::fs::read_dir(testdata()).unwrap() {
        let path = e.unwrap().path();
        let name = path.file_name().unwrap().to_str().unwrap().to_string();
        let Some(input) = name.strip_suffix(".dump.txt") else {
            continue;
        };
        let want = std::fs::read(&path).unwrap();
        let data = std::fs::read(testdata().join(input)).unwrap();
        let opts = DumpOptions {
            histogram: false,
            ..DumpOptions::default()
        };
        let mut have = Vec::new();
        analyze(&data, &opts, Some(input), &mut have).unwrap();
        if have != want {
            let have = String::from_utf8_lossy(&have);
            let want = String::from_utf8_lossy(&want);
            for (i, (h, w)) in have.lines().zip(want.lines()).enumerate() {
                assert_eq!(h, w, "{input}: line {}", i + 1);
            }
            assert_eq!(
                have.lines().count(),
                want.lines().count(),
                "{input}: line count"
            );
            panic!("{input}: differs (line ends?)");
        }
        checked += 1;
    }
    assert!(checked >= 7, "{checked} dumps");
}
