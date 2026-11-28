//! The Android SDK's tools and a JDK, where this machine has them (as
//! tools/dex/build.cmd finds them): for the tests that run dexdump and d8
//! live.

#![allow(dead_code)]

use std::path::{Path, PathBuf};
use std::process::Command;

/// The newest build-tools directory (with `dexdump` and `lib/d8.jar`).
pub fn build_tools() -> Option<PathBuf> {
    let mut roots: Vec<PathBuf> = ["ANDROID_HOME", "ANDROID_SDK_ROOT"]
        .iter()
        .filter_map(|v| std::env::var_os(v).map(PathBuf::from))
        .collect();
    if let Some(l) = std::env::var_os("LOCALAPPDATA") {
        roots.push(Path::new(&l).join("Android/Sdk"));
    }
    if let Some(p) = std::env::var_os("ProgramFiles(x86)") {
        roots.push(Path::new(&p).join("Android/android-sdk"));
    }
    if let Some(h) = std::env::var_os("HOME") {
        roots.push(Path::new(&h).join("Android/Sdk"));
        roots.push(Path::new(&h).join("Library/Android/sdk"));
    }
    roots
        .into_iter()
        .find_map(|r| newest(&r.join("build-tools")))
}

/// The newest platform's `android.jar`.
pub fn android_jar() -> Option<PathBuf> {
    let bt = build_tools()?;
    let platforms = bt.parent()?.parent()?.join("platforms");
    let mut dirs: Vec<PathBuf> = std::fs::read_dir(platforms)
        .ok()?
        .filter_map(|e| e.ok().map(|e| e.path().join("android.jar")))
        .filter(|p| p.exists())
        .collect();
    dirs.sort();
    dirs.pop()
}

fn newest(dir: &Path) -> Option<PathBuf> {
    let mut v: Vec<PathBuf> = std::fs::read_dir(dir)
        .ok()?
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| p.join("lib/d8.jar").exists())
        .collect();
    v.sort();
    v.pop()
}

/// dexdump.
pub fn dexdump() -> Option<PathBuf> {
    let bt = build_tools()?;
    ["dexdump.exe", "dexdump"]
        .iter()
        .map(|n| bt.join(n))
        .find(|p| p.exists())
}

/// A `java` (JAVA_HOME's, Android Studio's, or PATH's).
pub fn java(tool: &str) -> Option<PathBuf> {
    let exe = if cfg!(windows) {
        format!("{tool}.exe")
    } else {
        tool.to_string()
    };
    let mut homes: Vec<PathBuf> = std::env::var_os("JAVA_HOME")
        .map(PathBuf::from)
        .into_iter()
        .collect();
    if let Some(p) = std::env::var_os("ProgramFiles")
        && let Ok(rd) = std::fs::read_dir(Path::new(&p).join("Android/openjdk"))
    {
        homes.extend(rd.filter_map(|e| e.ok().map(|e| e.path())));
    }
    for h in homes {
        let p = h.join("bin").join(&exe);
        if p.exists() {
            return Some(p);
        }
    }
    Command::new(&exe)
        .arg("-version")
        .output()
        .ok()
        .filter(|o| o.status.success())
        .map(|_| PathBuf::from(exe))
}

/// `dexdump -a -d -f -h` of the file `name` in `dir` (run there, so that
/// it names the file as given).
pub fn run_dexdump(dir: &Path, name: &str) -> Option<Vec<u8>> {
    let out = Command::new(dexdump()?)
        .args(["-a", "-d", "-f", "-h", name])
        .current_dir(dir)
        .output()
        .ok()?;
    assert!(
        out.status.success(),
        "dexdump {name}: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    Some(out.stdout)
}
