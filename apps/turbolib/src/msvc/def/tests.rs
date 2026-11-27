use super::super::run::{Env, run};
use super::*;
use std::path::{Path, PathBuf};

fn root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn scratch(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("turbolib-def-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

/// An import library with each ARM64EC import by name's hint and stamp
/// zeroed (lib.exe's hints there are memory it didn't set: see
/// `turbonop_moduledef::imports`).
fn masked(lib: &[u8]) -> Vec<Vec<u8>> {
    let ar = turbonop_archive::Archive::parse(lib).unwrap();
    let mut out = vec![lib[..8].to_vec()];
    for m in ar.members() {
        let mut d = m.data.to_vec();
        let ec_by_name = d.len() > 20
            && d[..4] == [0, 0, 0xFF, 0xFF]
            && u16::from_le_bytes([d[6], d[7]]) == inputs::ARM64EC
            && (u16::from_le_bytes([d[18], d[19]]) >> 2) & 7 != 0;
        if ec_by_name {
            d[8..12].fill(0);
            d[16..18].fill(0);
        }
        let h = usize::try_from(m.header_offset).unwrap();
        out.push(lib[h..h + 60].to_vec());
        out.push(d);
    }
    out
}

/// A `.debug$S`'s subsections and records, with what names this machine
/// left out: `S_ENVBLOCK` (the directory, and the linker's path, which is
/// turbolib's own) and the string table (the `.def` file's full path).
fn codeview(data: &[u8]) -> Vec<(u32, Vec<u8>)> {
    let mut out = Vec::new();
    let mut p = 4;
    while p + 8 <= data.len() {
        let kind = u32::from_le_bytes(data[p..p + 4].try_into().unwrap());
        let len = u32::from_le_bytes(data[p + 4..p + 8].try_into().unwrap()) as usize;
        let body = &data[p + 8..p + 8 + len];
        match kind {
            0xF1 => {
                let mut q = 0;
                while q + 4 <= body.len() {
                    let rl = usize::from(u16::from_le_bytes([body[q], body[q + 1]]));
                    let rk = u32::from(u16::from_le_bytes([body[q + 2], body[q + 3]]));
                    if rk != 0x113D {
                        out.push((rk, body[q + 4..q + 2 + rl].to_vec()));
                    }
                    q += 2 + rl;
                }
            }
            0xF3 => out.push((kind, Vec::new())),
            _ => out.push((kind, body.to_vec())),
        }
        p = (p + 8 + len + 3) & !3;
    }
    out
}

/// An export file as values, but for what [`codeview`] leaves out.
fn exp_values(b: &[u8]) -> String {
    let o = turbonop_coff::Object::parse(b).unwrap();
    let mut s = format!(
        "{:x} {:x} {:x}\n",
        o.machine, o.time_date_stamp, o.characteristics
    );
    for sec in &o.sections {
        let data = if sec.name.as_bytes() == b".debug$S" {
            format!("{:?}", codeview(sec.bytes()))
        } else {
            format!("{:?}", sec.bytes())
        };
        s += &format!(
            "{:?} {:x} {data} {:?}\n",
            String::from_utf8_lossy(sec.name.as_bytes()),
            sec.characteristics,
            sec.relocations
        );
    }
    for sym in &o.symbols {
        s += &format!(
            "{:?} {:x} {:?} {:x} {}\n",
            String::from_utf8_lossy(sym.name.as_bytes()),
            sym.value,
            sym.section,
            sym.kind,
            sym.storage_class
        );
    }
    s
}

/// Every line of `tools/coff/imports/cases.txt` run as turbolib, against
/// what lib.exe 14.52 and llvm-lib wrote (`tools/coff/imports.cmd`): the
/// libraries byte for byte (but ARM64EC hints), and the export files but
/// for this machine's paths (layouts are checked by the fixtures' sizes
/// of everything else).
#[test]
fn the_fixtures() {
    let src = root().join("tools/coff/imports");
    let fixtures = root().join("testdata/coff/imports");
    let cases = std::fs::read_to_string(src.join("cases.txt")).unwrap();
    let mut checked = 0;
    for line in cases.lines().filter(|l| !l.trim().is_empty()) {
        let mut parts = line.splitn(3, '|');
        let (name, tool, args) = (
            parts.next().unwrap(),
            parts.next().unwrap(),
            parts.next().unwrap(),
        );
        let dir = scratch(name);
        for f in std::fs::read_dir(&src).unwrap().flatten() {
            if f.path().extension().is_some_and(|e| e == "def") {
                std::fs::copy(f.path(), dir.join(f.file_name())).unwrap();
            }
        }
        std::fs::copy(
            root().join("tools/coff/archives/def/decorated.def"),
            dir.join("decorated.def"),
        )
        .unwrap();
        for o in ["decorated.obj", "exports.obj"] {
            std::fs::copy(fixtures.join(o), dir.join(o)).unwrap();
        }
        let flavor = if tool == "lib" {
            Flavor::Lib
        } else {
            Flavor::LlvmLib
        };
        let words: Vec<String> = args.split_whitespace().map(str::to_string).collect();
        let env = Env {
            lib_options: None,
            lib_path: None,
            turbolib_options: None,
            cwd: dir.clone(),
        };
        let o = run(flavor, &words, &env);
        assert_eq!(o.exit, 0, "{name}: {}{}", o.stdout, o.stderr);
        for ext in ["lib", "exp", "a64n.exp"] {
            let want = fixtures.join(format!("{name}.{ext}"));
            let got = dir.join(format!("{name}.{ext}"));
            assert_eq!(want.exists(), got.exists(), "{name}.{ext}");
            if !want.exists() {
                continue;
            }
            let (want, got) = (std::fs::read(want).unwrap(), std::fs::read(got).unwrap());
            if ext == "lib" {
                assert!(masked(&want) == masked(&got), "{name}.lib differs");
            } else {
                assert_eq!(exp_values(&want), exp_values(&got), "{name}.{ext}");
            }
            checked += 1;
        }
        let _ = std::fs::remove_dir_all(&dir);
    }
    assert_eq!(checked, 29);
}

#[test]
fn names() {
    assert_eq!(with_extension("o", "exp"), "o.exp");
    assert_eq!(with_extension("o.x.y", "exp"), "o.x.exp");
    assert_eq!(with_extension("a.b\\o", "exp"), "a.b\\o.exp");
    assert_eq!(stem("sub\\x.def"), "x");
    assert_eq!(
        full_path(Path::new("C:\\a\\.\\b\\..\\c/d.def")),
        "C:\\a\\c\\d.def"
    );
}
