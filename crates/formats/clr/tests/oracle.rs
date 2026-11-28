//! turbonop-clr against System.Reflection.Metadata: the fixtures
//! (testdata/clr, built by tools/clr/build.cmd) dumped as tools/clr/ClrDump.cs
//! dumps them, line for line against its committed output; the CIL opcode
//! table against System.Reflection.Emit's; and, where there's a `dotnet` to
//! run the oracle, the machine's framework assemblies against it live.

#![allow(clippy::cast_possible_truncation)]

use turbonop_global_alloc as _;

mod dump;
mod walk;

use walk::framework_dir;

use std::path::{Path, PathBuf};
use std::process::Command;

fn testdata(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../../testdata/clr")
        .join(name)
}

fn check(name: &str) {
    let data = std::fs::read(testdata(name)).unwrap();
    let theirs = std::fs::read_to_string(testdata(&format!("{name}.dump.txt")))
        .unwrap()
        .replace("\r\n", "\n");
    let ours = dump::dump(&data);
    if let Some(d) = dump::first_difference(&ours, &theirs) {
        panic!("{name} differs from the oracle at {d}");
    }
}

#[test]
fn sample_dll_matches_the_oracle() {
    check("Sample.dll");
}

#[test]
fn sample_pdb_matches_the_oracle() {
    check("Sample.pdb");
}

#[test]
fn netmodule_matches_the_oracle() {
    check("Sample.Extra.netmodule");
}

#[test]
fn hello_exe_matches_the_oracle() {
    check("Hello.exe");
}

#[test]
fn opcodes_match_reflection_emit() {
    let theirs = std::fs::read_to_string(testdata("opcodes.txt")).unwrap();
    let theirs: Vec<String> = theirs.lines().map(str::to_string).collect();
    // (Reflection.Emit has no `no.`, 0xFE19.)
    let ours: Vec<String> = turbonop_clr::il::OPCODES
        .iter()
        .filter(|o| o.value != 0xFE19)
        .map(|o| {
            format!(
                "0x{:04X} {} {}",
                o.value,
                o.name,
                o.operand.reflection_name()
            )
        })
        .collect();
    assert_eq!(ours, theirs);
}

/// The oracle's dump of `path`, if it runs here.
fn oracle(path: &Path) -> Option<String> {
    let out = Command::new("dotnet")
        .arg(testdata("oracle/ClrDump.dll"))
        .arg(path)
        .output()
        .ok()?;
    if !out.status.success() {
        eprintln!(
            "the oracle didn't run: {}",
            String::from_utf8_lossy(&out.stderr)
        );
        return None;
    }
    Some(String::from_utf8(out.stdout).unwrap())
}

fn compare_with_oracle(path: &Path) -> Result<(), String> {
    let Some(theirs) = oracle(path) else {
        return Ok(());
    };
    let data = std::fs::read(path).unwrap();
    let ours = dump::dump(&data);
    match dump::first_difference(&ours, &theirs) {
        Some(d) => Err(format!("{} differs from the oracle at {d}", path.display())),
        None => Ok(()),
    }
}

#[test]
fn framework_assemblies_match_the_oracle() {
    let Some(dir) = framework_dir() else {
        eprintln!("no dotnet: skipped");
        return;
    };
    // A facade of forwarders, generics-heavy collections and LINQ,
    // interop, and a few resources.
    let names = [
        "System.Runtime.dll",
        "System.Collections.dll",
        "System.Linq.dll",
        "System.Console.dll",
        "System.Private.Uri.dll",
        "System.Runtime.InteropServices.dll",
    ];
    let mut failures = Vec::new();
    for n in names {
        let p = dir.join(n);
        if p.exists()
            && let Err(e) = compare_with_oracle(&p)
        {
            failures.push(e);
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

/// Every framework assembly against the oracle (minutes: run by hand,
/// `cargo test -p turbonop-clr --test oracle -- --ignored`).
#[test]
#[ignore]
fn every_framework_assembly_matches_the_oracle() {
    let Some(dir) = framework_dir() else {
        return;
    };
    let mut files: Vec<PathBuf> = std::fs::read_dir(&dir)
        .unwrap()
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| p.extension().is_some_and(|e| e == "dll"))
        .filter(|p| {
            std::fs::read(p)
                .ok()
                .and_then(|d| {
                    turbonop_clr::CliImage::parse_bytes(&d)
                        .ok()
                        .flatten()
                        .map(|_| ())
                })
                .is_some()
        })
        .collect();
    files.sort();
    let failures: Vec<String> = turbonop_par::map(&files, 1, |p| compare_with_oracle(p).err())
        .into_iter()
        .flatten()
        .collect();
    assert!(
        failures.is_empty(),
        "{} failed:\n{}",
        failures.len(),
        failures.join("\n")
    );
}
