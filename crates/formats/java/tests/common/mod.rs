//! What the tests share: the fixtures, javap's output, and the machine's
//! JDK (when there is one).

#![allow(dead_code)]

use std::path::{Path, PathBuf};

/// A fixture's path (testdata/java/`name`).
pub fn testdata(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../../testdata/java")
        .join(name)
}

/// javap's output without what it says of the file rather than the class
/// (its path, modification time, size, and checksum), with LF line ends.
pub fn javap_body(text: &str) -> String {
    let text = text.replace("\r\n", "\n");
    let mut out = String::new();
    for l in text.lines() {
        if l.starts_with("Classfile ")
            || l.starts_with("  Last modified ")
            || l.starts_with("  SHA-256 checksum ")
            || l.starts_with("  MD5 checksum ")
        {
            continue;
        }
        out.push_str(l);
        out.push('\n');
    }
    out
}

/// Where `ours` first differs from `theirs`: the line, both texts, and
/// some context.
pub fn first_difference(ours: &str, theirs: &str) -> Option<String> {
    let a: Vec<&str> = ours.lines().collect();
    let b: Vec<&str> = theirs.lines().collect();
    for i in 0..a.len().max(b.len()) {
        let (x, y) = (a.get(i), b.get(i));
        if x != y {
            let from = i.saturating_sub(3);
            let context: Vec<&str> = b[from.min(b.len())..i.min(b.len())].to_vec();
            return Some(format!(
                "line {}:\n  ours:   {:?}\n  javap's: {:?}\n  after:\n    {}",
                i + 1,
                x,
                y,
                context.join("\n    ")
            ));
        }
    }
    None
}

/// A JDK's home: `JAVA_HOME`'s, the one `javac` on the `PATH` is in, or one
/// in the usual places.
pub fn java_home() -> Option<PathBuf> {
    let is_jdk = |p: &Path| p.join("lib").is_dir() && p.join("release").is_file();
    if let Some(h) = std::env::var_os("JAVA_HOME").map(PathBuf::from)
        && is_jdk(&h)
    {
        return Some(h);
    }
    if let Some(path) = std::env::var_os("PATH") {
        for dir in std::env::split_paths(&path) {
            for exe in ["javac", "javac.exe"] {
                if dir.join(exe).is_file()
                    && let Some(home) = dir.parent()
                    && is_jdk(home)
                {
                    return Some(home.to_path_buf());
                }
            }
        }
    }
    for root in [
        "C:/Program Files/Android/openjdk",
        "C:/Program Files/Java",
        "C:/Program Files/Eclipse Adoptium",
        "C:/Program Files/Microsoft",
        "/usr/lib/jvm",
        "/Library/Java/JavaVirtualMachines",
    ] {
        let Ok(dirs) = std::fs::read_dir(root) else {
            continue;
        };
        let mut found: Vec<PathBuf> = dirs
            .flatten()
            .flat_map(|e| [e.path(), e.path().join("Contents/Home")])
            .filter(|p| is_jdk(p))
            .collect();
        found.sort();
        if let Some(h) = found.pop() {
            return Some(h);
        }
    }
    None
}

/// The JDK's feature version (`JAVA_VERSION="21.0.8"` in its `release`).
pub fn jdk_version(home: &Path) -> Option<u32> {
    let text = std::fs::read_to_string(home.join("release")).ok()?;
    let line = text.lines().find(|l| l.starts_with("JAVA_VERSION="))?;
    let v = line.trim_start_matches("JAVA_VERSION=").trim_matches('"');
    let first = v.split(['.', '_', '-', '+']).next()?;
    match first.parse().ok()? {
        // Java 8 and before: 1.8.0_x.
        1 => v.split('.').nth(1)?.parse().ok(),
        n => Some(n),
    }
}
