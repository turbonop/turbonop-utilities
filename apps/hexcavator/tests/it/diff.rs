//! `hexcavator diff`: two images or two PDBs compared, to the byte and by
//! what they say.

use std::path::{Path, PathBuf};
use std::process::Command;

fn testdata(rel: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../testdata")
        .join(rel)
}

/// Runs `hexcavator diff` with `args`: its exit code and output.
fn diff(args: &[&std::ffi::OsStr]) -> (i32, String, String) {
    let out = Command::new(env!("CARGO_BIN_EXE_hexcavator"))
        .arg("diff")
        .args(args)
        .output()
        .unwrap();
    (
        out.status.code().unwrap_or(-1),
        String::from_utf8_lossy(&out.stdout).into_owned(),
        String::from_utf8_lossy(&out.stderr).into_owned(),
    )
}

fn scratch(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("hexcavator-diff-{}-{name}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

#[test]
fn an_image_is_the_same_as_itself() {
    let exe = testdata("linkopts/msvc-strs-gf.exe");
    let (code, out, err) = diff(&[exe.as_os_str(), exe.as_os_str()]);
    assert_eq!(code, 0, "{out}{err}");
    assert!(out.contains("images: the same"), "{out}");
    let (code, out, _) = diff(&["-json".as_ref(), exe.as_os_str(), exe.as_os_str()]);
    assert_eq!(code, 0);
    let v: serde_json::Value = serde_json::from_str(&out).unwrap();
    assert_eq!(v["identical"], true);
    assert_eq!(v["kind"], "image");
}

#[test]
fn a_changed_byte_is_found_and_attributed() {
    let exe = testdata("linkopts/msvc-strs-gf.exe");
    let dir = scratch("byte");
    let copy = dir.join("changed.exe");
    let mut data = std::fs::read(&exe).unwrap();
    // The optional header's MajorImageVersion: e_lfanew + 4 + 20 + 44.
    let pe = u32::from_le_bytes(data[0x3C..0x40].try_into().unwrap()) as usize;
    data[pe + 4 + 20 + 44] ^= 0x7;
    // And a byte of the first section's contents.
    let sections = pe + 4 + 20 + usize::from(u16::from_le_bytes([data[pe + 20], data[pe + 21]]));
    let raw = u32::from_le_bytes(data[sections + 20..sections + 24].try_into().unwrap()) as usize;
    data[raw + 3] ^= 0xFF;
    std::fs::write(&copy, &data).unwrap();
    let (code, out, err) = diff(&[exe.as_os_str(), copy.as_os_str()]);
    assert_eq!(code, 1, "{out}{err}");
    assert!(out.contains("Bytes: 2 differences"), "{out}");
    assert!(out.contains("optional header: MajorImageVersion"), "{out}");
    assert!(
        out.contains("optional header MajorImageVersion: 0x"),
        "{out}"
    );
    assert!(out.contains(".text contents differ"), "{out}");
    // -only limits the parts.
    let (code, out, _) = diff(&[
        "-only".as_ref(),
        "headers".as_ref(),
        exe.as_os_str(),
        copy.as_os_str(),
    ]);
    assert_eq!(code, 1);
    assert!(out.contains("Headers: 1 difference"), "{out}");
    assert!(!out.contains("Bytes"), "{out}");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn string_pooling_differs() {
    // The same program linked with /GF (strings pooled) and without.
    let (a, b) = (
        testdata("linkopts/msvc-strs-gf.exe"),
        testdata("linkopts/msvc-strs-nogf.exe"),
    );
    let (code, out, err) = diff(&[a.as_os_str(), b.as_os_str()]);
    assert_eq!(code, 1, "{out}{err}");
    assert!(out.contains("sizes: 2560 | 3072 bytes"), "{out}");
    assert!(out.contains("optional header AddressOfEntryPoint"), "{out}");
    assert!(!out.contains("panicked"), "{err}");
}

#[test]
fn errors_and_mixed_kinds() {
    let exe = testdata("linkopts/msvc-strs-gf.exe");
    let (code, _, err) = diff(&[exe.as_os_str()]);
    assert_eq!(code, 2, "{err}");
    let (code, _, err) = diff(&[exe.as_os_str(), "no-such-file.exe".as_ref()]);
    assert_eq!(code, 2, "{err}");
    let pdb = testdata("sizebench/bin/SizeBenchV2.AnalysisEngine.Tests.Dllx64Managed.pdb");
    let (code, _, err) = diff(&[exe.as_os_str(), pdb.as_os_str()]);
    assert_eq!(code, 2, "{err}");
    assert!(err.contains("like with like"), "{err}");
}

#[test]
fn a_pdb_is_the_same_as_itself() {
    let pdb = testdata("sizebench/bin/SizeBenchV2.AnalysisEngine.Tests.Dllx64Managed.pdb");
    let (code, out, err) = diff(&[pdb.as_os_str(), pdb.as_os_str()]);
    assert_eq!(code, 0, "{out}{err}");
    assert!(out.contains("PDBs (logically): the same"), "{out}");
}

#[test]
fn two_builds_of_one_program() {
    // diff.c, built twice with changes (testdata/diff): their PDBs and
    // their images, by the PDBs' names.
    let (a, b) = (testdata("diff/diff-v1.pdb"), testdata("diff/diff-v2.pdb"));
    let (code, out, err) = diff(&[a.as_os_str(), b.as_os_str()]);
    assert_eq!(code, 1, "{out}{err}");
    assert!(out.contains("Modules: 2 differences"), "{out}");
    assert!(
        out.contains(r"module only in a: R:\target\fixtures\diff-v1\diff.obj"),
        "{out}"
    );
    assert!(out.contains("Types (TPI): "), "{out}");
    let (code, out, err) = diff(&["-json".as_ref(), a.as_os_str(), b.as_os_str()]);
    assert_eq!(code, 1, "{err}");
    let v: serde_json::Value = serde_json::from_str(&out).unwrap();
    assert_eq!(v["kind"], "pdb");
    assert_eq!(v["identical"], false);
    let (a, b) = (testdata("diff/diff-v1.exe"), testdata("diff/diff-v2.exe"));
    let (code, out, err) = diff(&["-n".as_ref(), "0".as_ref(), a.as_os_str(), b.as_os_str()]);
    assert_eq!(code, 1, "{out}{err}");
    assert!(out.contains("names from: the PDB | the PDB"), "{out}");
    assert!(
        out.contains("AddressOfEntryPoint: 0x1370 | 0x1400 (both mainCRTStartup"),
        "{out}"
    );
    assert!(out.contains("main(int, char**): contents differ"), "{out}");
    assert!(out.contains("g_message: size 0x3 | 0x2f"), "{out}");
    // What only refers to __ImageBase (the headers) is not a difference.
    assert!(
        !out.contains("__scrt_is_nonwritable_in_current_image(const void*): contents differ"),
        "{out}"
    );
    // Each PDB is the same as itself.
    let pdb = testdata("diff/diff-v1.pdb");
    let (code, out, _) = diff(&[pdb.as_os_str(), pdb.as_os_str()]);
    assert_eq!(code, 0, "{out}");
}

/// ARM64's builds of diff.cpp (testdata/diff): what moved is told from
/// what changed by the instructions, decoded (branches, ADRP pairs, and
/// what each loads in full), and 64-bit pointers in data by their
/// targets. What differs is what diff.cpp changed, as on x64.
#[test]
fn two_arm64_builds_by_their_instructions() {
    let (a, b) = (
        testdata("diff/diff-arm64-v1.exe"),
        testdata("diff/diff-arm64-v2.exe"),
    );
    let (code, out, err) = diff(&["-n".as_ref(), "0".as_ref(), a.as_os_str(), b.as_os_str()]);
    assert_eq!(code, 1, "{out}{err}");
    let summary = out
        .lines()
        .find(|l| l.contains("names in both"))
        .unwrap_or_default();
    assert!(summary.contains(" 4 differ;"), "{summary}");
    for changed in [
        "main(int, char**): contents differ",
        "g_lookup: size 0x80 | 0x180",
        "g_message: size 0x3 | 0x2f",
        "transform(int): size 0x18 | 0x84",
    ] {
        assert!(out.contains(changed), "{changed}: {out}");
    }
    // Moved, not changed: code (branches, ADRP pairs, import thunks) and
    // data (pointers to functions).
    for moved in [
        "_RTC_Initialize",
        "mainCRTStartup",
        "_cexit",
        "pre_c_initializer",
    ] {
        assert!(
            !out.contains(&format!("{moved}: contents differ")),
            "{moved}: {out}"
        );
    }
}

#[test]
fn coff_groups_in_dump_and_diff() {
    // link.exe's COFF groups (the POGO debug entry): how it laid the image
    // out, group by group.
    let v1 = testdata("diff/diff-v1.exe");
    let out = Command::new(env!("CARGO_BIN_EXE_hexcavator"))
        .args(["dump", "-coffgroups"])
        .arg(&v1)
        .output()
        .unwrap();
    assert!(out.status.success());
    let text = String::from_utf8_lossy(&out.stdout);
    assert!(text.contains("COFF Groups (31 groups)"), "{text}");
    assert!(
        text.contains("00001000      C80 .text     .text$mn"),
        "{text}"
    );
    assert!(
        text.contains("00002000      158 .rdata    .idata$5"),
        "{text}"
    );
    let v2 = testdata("diff/diff-v2.exe");
    let (code, out, err) = diff(&[
        "-only".as_ref(),
        "coffgroups".as_ref(),
        v1.as_os_str(),
        v2.as_os_str(),
    ]);
    assert_eq!(code, 1, "{out}{err}");
    assert!(
        out.contains(".text$mn: .text+0x0, 0xc80 bytes | .text+0x0, 0xd10 bytes"),
        "{out}"
    );
    // A part there isn't is an error, not "the same".
    let (code, _, err) = diff(&[
        "-only".as_ref(),
        "bogus".as_ref(),
        v1.as_os_str(),
        v2.as_os_str(),
    ]);
    assert_eq!(code, 2, "{err}");
    assert!(err.contains("no part 'bogus'"), "{err}");
}
