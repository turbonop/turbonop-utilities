extern crate std;

use std::format;
use std::path::PathBuf;
use std::string::String;
use std::vec::Vec;

use super::*;
use turbonop_archive::Archive;

fn fixtures() -> Vec<(String, Vec<u8>)> {
    let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../../testdata/coff/archives");
    let mut out: Vec<(String, Vec<u8>)> = std::fs::read_dir(&dir)
        .unwrap()
        .flatten()
        .map(|e| e.path())
        .filter(|p| {
            let n = p.file_name().unwrap().to_string_lossy();
            n.starts_with("def-") && n.ends_with(".lib")
        })
        .map(|p| {
            (
                p.file_name().unwrap().to_string_lossy().into_owned(),
                std::fs::read(&p).unwrap(),
            )
        })
        .collect();
    out.sort();
    out
}

fn stem(dll: &[u8]) -> &[u8] {
    &dll[..dll.iter().rposition(|&b| b == b'.').unwrap_or(dll.len())]
}

/// Every fixture's descriptor objects, made again: lib.exe's (with
/// `/Brepro`, or dated) and llvm-lib's, for each machine.
#[test]
fn the_fixtures_descriptors() {
    let mut checked = 0;
    for (file, data) in fixtures() {
        let ar = Archive::parse(&data).unwrap();
        let members: Vec<_> = ar.files().collect();
        let dll = members[0].name.to_vec();
        let llvm = file.contains("-llvm");
        let tool = if llvm { Tool::LlvmLib } else { Tool::LibExe };
        // The library's machine: the import objects' (ARM64X if there are
        // ARM64EC and ARM64 ones).
        let machines: Vec<u16> = members[3..]
            .iter()
            .map(|m| u16::from_le_bytes([m.data[6], m.data[7]]))
            .collect();
        let m = if machines.contains(&machine::ARM64EC) && machines.contains(&machine::ARM64) {
            machine::ARM64X
        } else if let Some(&m) = machines.first() {
            m
        } else {
            u16::from_le_bytes([members[0].data[0], members[0].data[1]])
        };
        let d = Descriptors {
            machine: m,
            dll: &dll,
            stem: stem(&dll),
            tool,
        };
        for (i, want) in members[..3].iter().enumerate() {
            let ts = u32::from_le_bytes(want.data[4..8].try_into().unwrap());
            let stamp = if llvm || file.contains("-dated") {
                Stamp::Time(ts)
            } else {
                Stamp::Brepro
            };
            let got = &d.all(stamp)[i];
            assert_eq!(got, &want.data, "{file}: descriptor {i}");
            checked += 1;
        }
    }
    assert!(checked >= 120, "{checked}");
}

#[test]
fn brepro_stamps_of_import_objects() {
    let mut checked = 0;
    for (file, data) in fixtures() {
        if file.contains("-llvm") || file.contains("-dated") {
            continue;
        }
        let ar = Archive::parse(&data).unwrap();
        for m in ar.files().skip(3) {
            let ts = u32::from_le_bytes(m.data[8..12].try_into().unwrap());
            assert_eq!(brepro_stamp(m.data, 8), ts, "{file}");
            checked += 1;
        }
    }
    assert!(checked >= 200, "{checked}");
}

#[test]
fn symbols() {
    let d = Descriptors {
        machine: machine::AMD64,
        dll: b"foo.bar.dll",
        stem: b"foo.bar",
        tool: Tool::LibExe,
    };
    let [a, b, c] = d.symbols();
    assert_eq!(
        (format!("{a:?}"), b, c),
        (
            format!("{:?}", b"__IMPORT_DESCRIPTOR_foo.bar".to_vec()),
            b"__NULL_IMPORT_DESCRIPTOR".to_vec(),
            b"\x7ffoo.bar_NULL_THUNK_DATA".to_vec()
        )
    );
}
