//! The machine's JDK, when there is one: its JARs, JMODs (an archive after
//! a four-byte header), and source archive, every entry read and its CRC
//! checked.

use turbonop_global_alloc as _;

use std::path::{Path, PathBuf};

/// A JDK's home: `JAVA_HOME`'s, the one `javac` on the `PATH` is in, or one
/// in the usual places.
fn java_home() -> Option<PathBuf> {
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

fn archives(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for e in entries.flatten() {
        let p = e.path();
        if p.is_dir() {
            archives(&p, out);
        } else if p
            .extension()
            .is_some_and(|x| x == "jar" || x == "jmod" || x == "zip")
        {
            out.push(p);
        }
    }
}

#[test]
fn every_jdk_archive_reads() {
    let Some(home) = java_home() else {
        eprintln!("no JDK: skipped");
        return;
    };
    let mut files = Vec::new();
    archives(&home, &mut files);
    files.sort();
    let mut entries = 0;
    for p in &files {
        let data = std::fs::read(p).unwrap();
        let a =
            turbonop_zip::Archive::parse(&data).unwrap_or_else(|e| panic!("{}: {e}", p.display()));
        if p.extension().is_some_and(|x| x == "jmod") {
            assert_eq!(a.base(), 4, "{}", p.display());
        }
        for (e, r) in a.entries().iter().zip(a.read_all()) {
            let d = r.unwrap_or_else(|err| panic!("{}: {}: {err}", p.display(), e.name()));
            assert_eq!(d.len() as u64, e.uncompressed_size);
            let x = a.extent(e).unwrap();
            assert!(x.whole().end <= a.central_directory().start);
        }
        entries += a.len();
    }
    eprintln!("{} archives, {entries} entries", files.len());
}
