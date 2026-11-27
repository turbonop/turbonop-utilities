//! Programs compiled with the installed Visual C++ tools, linked by
//! turbolink, and run: their output and exit code must be what the source
//! says. Without Visual Studio (found with `vswhere`), nothing is checked.

use turbonop_global_alloc as _;

use std::path::{Path, PathBuf};
use std::process::Command;

/// `vcvars64.bat`, if Visual Studio's C++ tools are installed.
fn vcvars() -> Option<PathBuf> {
    if !cfg!(windows) {
        return None;
    }
    let pf86 = std::env::var_os("ProgramFiles(x86)")?;
    let vswhere = Path::new(&pf86).join(r"Microsoft Visual Studio\Installer\vswhere.exe");
    let out = Command::new(vswhere)
        .args([
            "-latest",
            "-prerelease",
            "-products",
            "*",
            "-requires",
            "Microsoft.VisualStudio.Component.VC.Tools.x86.x64",
            "-property",
            "installationPath",
        ])
        .output()
        .ok()?;
    let root = String::from_utf8_lossy(&out.stdout).trim().to_owned();
    let bat = Path::new(&root).join(r"VC\Auxiliary\Build\vcvars64.bat");
    bat.is_file().then_some(bat)
}

/// A fresh directory for one test's files.
fn scratch(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("turbolink-{}-{name}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("a scratch directory");
    dir
}

/// Runs `commands` (joined with `&&`) after `vcvars64.bat`, in `dir`.
fn in_vs(vcvars: &Path, dir: &Path, commands: &[String]) {
    let script = dir.join("run.cmd");
    let body = format!(
        "@echo off\r\ncall \"{}\" >nul || exit /b 1\r\ncd /d \"{}\" || exit /b 1\r\n{}\r\n",
        vcvars.display(),
        dir.display(),
        commands.join(" || exit /b 1\r\n")
    );
    std::fs::write(&script, body).expect("the script");
    let out = Command::new("cmd")
        .arg("/c")
        .arg(&script)
        .output()
        .expect("cmd runs");
    assert!(
        out.status.success(),
        "{}\n{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
}

/// Compiles `source` (in `tests/programs`) with `cl_flags`, links it with
/// turbolink, runs it with `args`; returns its output and exit code.
fn build_and_run(test: &str, source: &str, cl_flags: &str, args: &[&str]) -> Option<(String, i32)> {
    let Some(vcvars) = vcvars() else {
        eprintln!("no Visual C++ tools: {test} not checked");
        return None;
    };
    let dir = scratch(test);
    let src = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/programs")
        .join(source);
    let turbolink = env!("CARGO_BIN_EXE_turbolink");
    in_vs(
        &vcvars,
        &dir,
        &[
            format!(
                "cl /nologo /c {cl_flags} /Fo{test}.obj \"{}\" >nul",
                src.display()
            ),
            format!("\"{turbolink}\" /nologo /OUT:{test}.exe {test}.obj"),
        ],
    );
    let out = Command::new(dir.join(format!("{test}.exe")))
        .args(args)
        .output()
        .expect("the program runs");
    let _ = std::fs::remove_dir_all(&dir);
    Some((
        String::from_utf8_lossy(&out.stdout).replace("\r\n", "\n"),
        out.status.code().unwrap_or(-1),
    ))
}

fn hello(test: &str, flags: &str) {
    if let Some((out, code)) = build_and_run(test, "hello.c", flags, &["a", "b"]) {
        assert!(out.starts_with("hello from "), "{out}");
        assert!(out.ends_with(", 3 args\n"), "{out}");
        assert_eq!(code, 3);
    }
}

#[test]
fn c_with_the_dynamic_crt() {
    hello("c_md", "/O2 /MD");
}

#[test]
fn c_with_the_static_crt() {
    hello("c_mt", "/O2 /MT");
}

#[test]
fn c_unoptimized_with_the_debug_crt() {
    hello("c_mdd", "/Od /MDd");
}

fn cpp(test: &str, flags: &str) {
    if let Some((out, code)) = build_and_run(test, "cpp.cpp", flags, &[]) {
        assert_eq!(out, "caught: boom 14.000000\ntable two=2 tls=42\n");
        assert_eq!(code, 14);
    }
}

#[test]
fn cpp_exceptions_and_tls_with_the_dynamic_crt() {
    cpp("cpp_md", "/EHsc /O2 /MD");
}

#[test]
fn cpp_exceptions_and_tls_with_the_static_crt() {
    cpp("cpp_mt", "/EHsc /O2 /MT");
}

#[test]
fn cpp_unoptimized_with_the_static_debug_crt() {
    cpp("cpp_mtd", "/EHsc /Od /MTd");
}

/// Runs `exe` (in `dir`), returning its output.
fn run(dir: &Path, exe: &str) -> String {
    let out = Command::new(dir.join(exe))
        .current_dir(dir)
        .output()
        .expect("the program runs");
    assert!(out.status.success(), "{exe}: {:?}", out.status);
    String::from_utf8_lossy(&out.stdout).replace("\r\n", "\n")
}

/// A DLL linked by turbolink (exports from `__declspec(dllexport)` and a
/// `.def` file: renamed, by ordinal only), used through turbolink's import
/// library by a program linked by turbolink and by one linked by link.exe;
/// and link.exe's DLL's import library used by turbolink.
#[test]
fn dll_with_exports_and_its_import_library() {
    let Some(vcvars) = vcvars() else {
        eprintln!("no Visual C++ tools: not checked");
        return;
    };
    let dir = scratch("dll");
    let src = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/programs");
    for f in ["dll.c", "dll.def", "usedll.c"] {
        std::fs::copy(src.join(f), dir.join(f)).expect("a copy");
    }
    let tl = env!("CARGO_BIN_EXE_turbolink");
    in_vs(
        &vcvars,
        &dir,
        &[
            "cl /nologo /c /O2 /MD dll.c usedll.c >nul".to_owned(),
            format!("\"{tl}\" /nologo /DLL /DEF:dll.def /OUT:dll.dll dll.obj"),
            format!("\"{tl}\" /nologo /OUT:use-tl.exe usedll.obj dll.lib"),
            "link /nologo /OUT:use-link.exe usedll.obj dll.lib".to_owned(),
        ],
    );
    let want = "add=5 counter=41 times=42 hidden=7\n";
    assert_eq!(run(&dir, "use-tl.exe"), want);
    assert_eq!(run(&dir, "use-link.exe"), want);
    // link.exe's DLL and import library, used by turbolink.
    in_vs(
        &vcvars,
        &dir,
        &[
            "link /nologo /DLL /DEF:dll.def /OUT:dll.dll dll.obj".to_owned(),
            format!("\"{tl}\" /nologo /OUT:use-tl2.exe usedll.obj dll.lib"),
        ],
    );
    assert_eq!(run(&dir, "use-tl2.exe"), want);
    let _ = std::fs::remove_dir_all(&dir);
}

/// `image` with its time stamps zeroed: the file header's and each debug
/// directory entry's (they differ between two links run apart).
fn without_time_stamps(mut image: Vec<u8>) -> Vec<u8> {
    let u32_at = |d: &[u8], o: usize| u32::from_le_bytes(d[o..o + 4].try_into().unwrap());
    let pe = u32_at(&image, 0x3C) as usize;
    image[pe + 8..pe + 12].fill(0);
    let optional = pe + 24;
    let sections = u16::from_le_bytes([image[pe + 6], image[pe + 7]]) as usize;
    let headers = optional + u16::from_le_bytes([image[pe + 20], image[pe + 21]]) as usize;
    let (rva, size) = (
        u32_at(&image, optional + 112 + 48),
        u32_at(&image, optional + 112 + 52),
    );
    for s in 0..sections {
        let h = headers + 40 * s;
        let (va, raw, ptr) = (
            u32_at(&image, h + 12),
            u32_at(&image, h + 16),
            u32_at(&image, h + 20),
        );
        if rva >= va && rva < va + raw {
            let at = (ptr + rva - va) as usize;
            for e in 0..(size / 28) as usize {
                image[at + 28 * e + 4..at + 28 * e + 8].fill(0);
            }
        }
    }
    image
}

/// A C program with the dynamic CRT: turbolink's image is link.exe's, byte
/// for byte (time stamps aside): its Rich header, sections, groups, import
/// tables (in qsort's order), and debug directory.
#[test]
fn c_with_the_dynamic_crt_is_link_exes_image() {
    let Some(vcvars) = vcvars() else {
        eprintln!("no Visual C++ tools: not checked");
        return;
    };
    let dir = scratch("same_md");
    let src = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/programs/hello.c");
    let tl = env!("CARGO_BIN_EXE_turbolink");
    in_vs(
        &vcvars,
        &dir,
        &[
            format!(
                "cl /nologo /c /O2 /MD /Fohello.obj \"{}\" >nul",
                src.display()
            ),
            "link /nologo /OUT:link.exe hello.obj".to_owned(),
            format!("\"{tl}\" /nologo /OUT:turbolink.exe hello.obj"),
        ],
    );
    let theirs = without_time_stamps(std::fs::read(dir.join("link.exe")).unwrap());
    let ours = without_time_stamps(std::fs::read(dir.join("turbolink.exe")).unwrap());
    assert_eq!(theirs.len(), ours.len());
    let differ: Vec<usize> = (0..ours.len())
        .filter(|&i| ours[i] != theirs[i])
        .take(16)
        .collect();
    assert!(differ.is_empty(), "bytes differ at {differ:x?}");
    let _ = std::fs::remove_dir_all(&dir);
}

/// A DLL with exports from `__declspec(dllexport)` and a `.def` file:
/// turbolink's image is link.exe's, byte for byte (time stamps aside).
#[test]
fn dll_is_link_exes_image() {
    let Some(vcvars) = vcvars() else {
        eprintln!("no Visual C++ tools: not checked");
        return;
    };
    let dir = scratch("same_dll");
    let src = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/programs");
    for f in ["dll.c", "dll.def"] {
        std::fs::copy(src.join(f), dir.join(f)).expect("a copy");
    }
    let tl = env!("CARGO_BIN_EXE_turbolink");
    in_vs(
        &vcvars,
        &dir,
        &[
            "cl /nologo /c /O2 /MD dll.c >nul".to_owned(),
            "link /nologo /DLL /DEF:dll.def /NOIMPLIB /OUT:link.dll dll.obj".to_owned(),
            format!("\"{tl}\" /nologo /DLL /DEF:dll.def /NOIMPLIB /OUT:turbolink.dll dll.obj"),
        ],
    );
    let theirs = without_time_stamps(std::fs::read(dir.join("link.dll")).unwrap());
    let ours = without_time_stamps(std::fs::read(dir.join("turbolink.dll")).unwrap());
    assert_eq!(theirs.len(), ours.len());
    let differ: Vec<usize> = (0..ours.len())
        .filter(|&i| ours[i] != theirs[i])
        .take(16)
        .collect();
    assert!(differ.is_empty(), "bytes differ at {differ:x?}");
    let _ = std::fs::remove_dir_all(&dir);
}
