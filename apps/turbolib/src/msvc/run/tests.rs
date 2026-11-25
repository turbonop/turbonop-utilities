use super::*;

fn testdata(rel: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../testdata")
        .join(rel)
}

/// A fresh directory for one test.
fn scratch(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("turbolib-run-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

fn w(words: &[&str]) -> Vec<String> {
    words.iter().map(ToString::to_string).collect()
}

fn env_in(cwd: &Path) -> Env {
    Env {
        lib_options: None,
        lib_path: None,
        cwd: cwd.to_path_buf(),
    }
}

#[test]
fn list_matches_lib_exe() {
    let dir = scratch("list");
    std::fs::copy(testdata("coff/objects.lib"), dir.join("objects.lib")).unwrap();
    let o = run(
        Flavor::Lib,
        &w(&["/nologo", "/list", "objects.lib"]),
        &env_in(&dir),
    );
    let expected = std::fs::read_to_string(testdata("coff/objects.lib.list.txt")).unwrap();
    assert_eq!(
        o.stdout.replace("\r\n", "\n"),
        expected.replace("\r\n", "\n")
    );
    assert_eq!(o.exit, 0);
    // To a file, and an object input before the library is skipped.
    let o = run(
        Flavor::Lib,
        &w(&[
            "/nologo",
            "/LIST:l.txt",
            "missing-is-fine.obj",
            "objects.lib",
        ]),
        &env_in(&dir),
    );
    assert_eq!(o.exit, 1181, "{o:?}");
    std::fs::copy(testdata("coff/objects.obj"), dir.join("x.obj")).unwrap();
    let o = run(
        Flavor::Lib,
        &w(&["/nologo", "/LIST:l.txt", "x.obj", "objects.lib"]),
        &env_in(&dir),
    );
    assert_eq!(o.exit, 0);
    let listed = std::fs::read_to_string(dir.join("l.txt")).unwrap();
    assert_eq!(listed, expected.replace("\r\n", "\n"));
}

#[test]
fn extract() {
    let dir = scratch("extract");
    std::fs::copy(testdata("coff/objects.lib"), dir.join("objects.lib")).unwrap();
    // Without /OUT: the member's file name, here; any case.
    let o = run(
        Flavor::Lib,
        &w(&["/nologo", "/EXTRACT:OBJECTS.OBJ", "objects.lib"]),
        &env_in(&dir),
    );
    assert_eq!(o.exit, 0, "{o:?}");
    let original = std::fs::read(testdata("coff/objects.obj")).unwrap();
    assert_eq!(std::fs::read(dir.join("objects.obj")).unwrap(), original);
    // With /OUT, and a missing member is a warning.
    let o = run(
        Flavor::Lib,
        &w(&[
            "/nologo",
            "/EXTRACT:objects.obj",
            "/OUT:copy.obj",
            "objects.lib",
        ]),
        &env_in(&dir),
    );
    assert_eq!(o.exit, 0);
    assert_eq!(std::fs::read(dir.join("copy.obj")).unwrap(), original);
    let o = run(
        Flavor::Lib,
        &w(&[
            "/nologo",
            "/EXTRACT:nothere.obj",
            "/OUT:x.obj",
            "objects.lib",
        ]),
        &env_in(&dir),
    );
    assert_eq!(
        o.stdout,
        "LINK : warning LNK4014: cannot find member object nothere.obj\n"
    );
}

#[test]
fn banner_and_option_warnings_on_stdout() {
    let dir = scratch("banner");
    let o = run(
        Flavor::Lib,
        &w(&["/BOGUS", "/LIST", "none.lib"]),
        &env_in(&dir),
    );
    assert!(o.stdout.starts_with(&banner()), "{o:?}");
    assert!(o.stdout.contains(
        "LINK : warning LNK4044: unrecognized option '/BOGUS'; ignored\n\
         LINK : fatal error LNK1181: cannot open input file 'none.lib'\n"
    ));
    assert_eq!(o.exit, 1181);
    assert!(o.stderr.is_empty());
}

#[test]
fn response_files_and_lib_env() {
    let dir = scratch("rsp");
    std::fs::copy(testdata("coff/objects.lib"), dir.join("objects.lib")).unwrap();
    std::fs::write(dir.join("r.rsp"), "/nologo\r\n\"/LIST\"").unwrap();
    let mut env = env_in(&dir);
    env.lib_options = Some("objects.lib".into());
    let o = run(Flavor::Lib, &w(&["@r.rsp"]), &env);
    assert_eq!(o.stdout, "objects.obj\nobjects-big.obj\n", "{o:?}");
    // A missing response file: the banner (before /NOLOGO is read), then
    // LNK1104.
    let o = run(Flavor::Lib, &w(&["/nologo", "@nope.rsp"]), &env_in(&dir));
    assert_eq!(
        o.stdout,
        format!(
            "{}LINK : fatal error LNK1104: cannot open file 'nope.rsp'\n",
            banner()
        )
    );
    assert_eq!(o.exit, 1104);
}

#[test]
fn inputs_are_found_on_libpath_and_lib() {
    let dir = scratch("libpath");
    let sub = dir.join("sub");
    std::fs::create_dir_all(&sub).unwrap();
    std::fs::copy(testdata("coff/objects.lib"), sub.join("found.lib")).unwrap();
    let o = run(
        Flavor::Lib,
        &w(&["/nologo", "/LIBPATH:sub", "/LIST", "found.lib"]),
        &env_in(&dir),
    );
    assert_eq!(o.stdout, "objects.obj\nobjects-big.obj\n", "{o:?}");
    let mut env = env_in(&dir);
    env.lib_path = Some(format!("{};", sub.display()));
    let o = run(Flavor::Lib, &w(&["/nologo", "/LIST", "found.lib"]), &env);
    assert_eq!(o.stdout, "objects.obj\nobjects-big.obj\n", "{o:?}");
}

#[test]
fn no_inputs() {
    let dir = scratch("empty");
    let o = run(Flavor::Lib, &w(&["/nologo", "/OUT:e.lib"]), &env_in(&dir));
    assert_eq!(o, Outcome::default());
    assert!(!dir.join("e.lib").exists());
    let o = run(Flavor::LlvmLib, &w(&["/out:e.lib"]), &env_in(&dir));
    assert!(o.stderr.starts_with("warning: no input files"), "{o:?}");
    assert_eq!(o.exit, 0);
    let o = run(
        Flavor::LlvmLib,
        &w(&["/out:e.lib", "/ignore:emptyoutput"]),
        &env_in(&dir),
    );
    assert_eq!(o, Outcome::default());
}

#[test]
fn llvm_lib_diagnostics_on_stderr() {
    let dir = scratch("llvm");
    let o = run(
        Flavor::LlvmLib,
        &w(&["/out:x.lib", "nothere.obj"]),
        &env_in(&dir),
    );
    assert_eq!(o.stderr, "nothere.obj: no such file or directory\n");
    assert_eq!(o.exit, 1);
    assert!(o.stdout.is_empty());
}
