//! turbonop-dex against dexdump live, where this machine has the Android
//! SDK and a JDK (skipped, saying so, where it hasn't): what's too big to
//! commit, built by javac and d8 into the tests' scratch directory.
//!
//! - A version 041 container of two DEX files (more method references
//!   than one file holds), which only d8 at large sizes writes.
//! - (Ignored by default, for its minutes: `--ignored`.) d8's own jar,
//!   d8-ed: thousands of classes of real code, every one dumped by
//!   dexdump and by us, on every core.

use turbonop_global_alloc as _;

mod dump;
mod sdk;
mod walk;

use std::path::{Path, PathBuf};
use std::process::Command;

fn crlf_to_lf(b: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(b.len());
    for (i, &c) in b.iter().enumerate() {
        if !(c == b'\r' && b.get(i + 1) == Some(&b'\n')) {
            out.push(c);
        }
    }
    out
}

struct Tools {
    java: PathBuf,
    javac: PathBuf,
    d8: PathBuf,
}

fn tools() -> Option<Tools> {
    let t = Tools {
        java: sdk::java("java")?,
        javac: sdk::java("javac")?,
        d8: sdk::build_tools()?.join("lib/d8.jar"),
    };
    sdk::dexdump()?;
    Some(t)
}

fn run(cmd: &mut Command) {
    let out = cmd.output().unwrap();
    assert!(
        out.status.success(),
        "{cmd:?}: {}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
}

fn scratch(name: &str) -> PathBuf {
    let dir = Path::new(env!("CARGO_TARGET_TMPDIR"))
        .join("dex-live")
        .join(name);
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

/// Our dump of `dir/name` against dexdump's, and every reader over it
/// without an error.
fn check(dir: &Path, name: &str) -> Result<(), String> {
    let data = std::fs::read(dir.join(name)).unwrap();
    let mut e = walk::Errors::default();
    walk::file(&data, &mut e);
    if !e.0.is_empty() {
        return Err(format!("{name}: {:?}", &e.0[..e.0.len().min(10)]));
    }
    let theirs = crlf_to_lf(&sdk::run_dexdump(dir, name).unwrap());
    let ours = dump::dump(name, &data);
    match dump::first_difference(&ours, &theirs) {
        Some(d) => Err(format!("{name} differs from dexdump at {d}")),
        None => Ok(()),
    }
}

#[test]
fn d8_container_of_two_files_matches_dexdump() {
    let Some(t) = tools() else {
        eprintln!("no Android SDK and JDK: skipped");
        return;
    };
    let dir = scratch("container");
    // 9 classes of 7500 methods: more than a DEX file's 65536 method
    // references, so d8 writes two files into the container.
    let mut sources = Vec::new();
    for c in 0..9 {
        let mut s = format!("public class C{c} {{\n");
        for m in 0..7500 {
            s.push_str(&format!(
                "  static int m{m}(int x) {{ return x * {m} + {c}; }}\n"
            ));
        }
        s.push_str("}\n");
        let p = dir.join(format!("C{c}.java"));
        std::fs::write(&p, s).unwrap();
        sources.push(p);
    }
    run(Command::new(&t.javac)
        .arg("-d")
        .arg(dir.join("classes"))
        .args(&sources));
    let classes: Vec<PathBuf> = (0..9)
        .map(|c| dir.join(format!("classes/C{c}.class")))
        .collect();
    let out = dir.join("out");
    std::fs::create_dir_all(&out).unwrap();
    run(Command::new(&t.java)
        .arg("-Dcom.android.tools.r8.dexContainerExperiment=true")
        .arg("-cp")
        .arg(&t.d8)
        .arg("com.android.tools.r8.D8")
        .args(["--min-api", "36", "--output"])
        .arg(&out)
        .args(&classes));
    let data = std::fs::read(out.join("classes.dex")).unwrap();
    let all = turbonop_dex::Dex::parse_all(&data).unwrap();
    assert_eq!(all.len(), 2);
    assert!(
        all.iter()
            .all(|d| d.verify().checksum_ok && d.verify().signature_ok)
    );
    assert!(all[1].offset() > 0);
    check(&out, "classes.dex").unwrap();
}

#[test]
#[ignore = "minutes: d8 and dexdump over thousands of classes"]
fn d8_jar_dexed_matches_dexdump() {
    let Some(t) = tools() else {
        eprintln!("no Android SDK and JDK: skipped");
        return;
    };
    let dir = scratch("d8-jar");
    let java_home = t.java.parent().unwrap().parent().unwrap();
    run(Command::new(&t.java)
        .arg("-cp")
        .arg(&t.d8)
        .arg("com.android.tools.r8.D8")
        .args(["--min-api", "26", "--lib"])
        .arg(java_home)
        .arg("--output")
        .arg(&dir)
        .arg(&t.d8));
    let mut names: Vec<String> = std::fs::read_dir(&dir)
        .unwrap()
        .filter_map(|e| e.ok()?.file_name().into_string().ok())
        .filter(|n| n.ends_with(".dex"))
        .collect();
    names.sort();
    assert!(!names.is_empty());
    let failures: Vec<String> = turbonop_par::map(&names, 1, |n| check(&dir, n).err())
        .into_iter()
        .flatten()
        .collect();
    assert!(failures.is_empty(), "{}", failures.join("\n\n"));
    eprintln!("{} DEX files match", names.len());
}
