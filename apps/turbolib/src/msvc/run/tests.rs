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
        turbolib_options: None,
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

#[test]
fn turbolib_options() {
    let dir = scratch("turbolib-options");
    let o = run(
        Flavor::Lib,
        &w(&["/nologo", "/TURBOLIB:fold=some", "a.obj"]),
        &env_in(&dir),
    );
    assert_eq!(o.exit, 1117);
    assert!(
        o.stdout
            .starts_with("LINK : fatal error LNK1117: syntax error in option 'TURBOLIB:fold=some'")
    );
    let o = run(
        Flavor::Lib,
        &w(&["/nologo", "/TURBOLIB:precomp=clustered=4", "a.obj"]),
        &env_in(&dir),
    );
    // Accepted: what fails is the missing input.
    assert_eq!(o.exit, 1181, "{o:?}");
    assert!(!o.stdout.contains("TURBOLIB"), "{o:?}");
    let mut env = env_in(&dir);
    env.turbolib_options = Some("stats".into());
    let o = run(Flavor::LlvmLib, &w(&["a.obj"]), &env);
    assert_eq!(o.stderr, "TURBOLIB_OPTIONS: stats: options start with --\n");
}

#[test]
fn folding_writes_a_library_with_synthesized_members_last() {
    let dir = scratch("fold");
    std::fs::copy(testdata("coff/objects.obj"), dir.join("a.obj")).unwrap();
    std::fs::copy(testdata("coff/objects-big.obj"), dir.join("b.obj")).unwrap();
    let o = run(
        Flavor::Lib,
        &w(&[
            "/nologo",
            "/TURBOLIB:fold=identical",
            "/TURBOLIB:stats",
            "/OUT:f.lib",
            "a.obj",
            "b.obj",
        ]),
        &env_in(&dir),
    );
    assert_eq!(o.exit, 0, "{o:?}");
    assert!(o.stdout.contains("groups folded"), "{o:?}");
    let data = std::fs::read(dir.join("f.lib")).unwrap();
    let archive = turbonop_archive::Archive::parse(&data).unwrap();
    let files: Vec<_> = archive.files().collect();
    let names: Vec<String> = files.iter().map(|m| text(&m.name)).collect();
    // lib.exe's order is b.obj, a.obj; under --pull=preserve the
    // synthesized member comes last, indexed for its anchor alone.
    assert_eq!(names.len(), 3, "{names:?}");
    assert_eq!(&names[..2], ["b.obj", "a.obj"]);
    assert!(names[2].starts_with("turbolib"), "{names:?}");
    let last = files[2].header_offset;
    let listed: Vec<&[u8]> = archive
        .symbols()
        .iter()
        .filter(|s| s.member == last)
        .map(|s| s.name)
        .collect();
    assert_eq!(listed.len(), 1, "{listed:?}");
    assert!(listed[0].starts_with(b"__turbolib$anchor$"));
}

/// Phase 4's modes, through the `lib` personality: each folds the
/// fixtures of tools/turbolib/loose, says what it did, and makes a library
/// `turbolib verify` passes with the same mode, and fails with the
/// identical, locals-keeping one where the mode did more.
#[test]
fn less_conservative_modes_verify() {
    use turbonop_verify::library::{self, Fold as VFold, Pull as VPull};
    let dir = scratch("loose");
    for m in ["a", "b", "c"] {
        std::fs::copy(
            testdata(&format!("coff/loose/msvc-od-{m}.obj")),
            dir.join(format!("{m}.obj")),
        )
        .unwrap();
    }
    let lib = |out: &str, options: &str| {
        let mut env = env_in(&dir);
        env.turbolib_options = Some(options.to_string());
        let o = run(
            Flavor::Lib,
            &w(&[
                "/nologo",
                "/Brepro",
                &format!("/OUT:{out}"),
                "a.obj",
                "b.obj",
                "c.obj",
            ]),
            &env,
        );
        assert_eq!(o.exit, 0, "{o:?}");
        (o, std::fs::read(dir.join(out)).unwrap())
    };
    let (_, original) = lib("original.lib", "--fold=none");
    // Folded without preserve's conditions, refers() (which calls ext_one
    // in a and ext_two in b and c) moves, and verify under preserve finds
    // a synthesized member that refers to a name a member anchoring it
    // didn't.
    let (_, relaxed) = lib("relaxed.lib", "--fold=select-any --pull=relaxed");
    let report = library::verify(
        &original,
        &relaxed,
        &library::Options {
            pull: VPull::Preserve,
            fold: VFold::SelectAny,
            locals: false,
        },
    )
    .unwrap();
    assert!(
        report
            .text()
            .contains("names a synthesized member refers to that a member anchoring it didn't"),
        "{}",
        report.text()
    );
    // --report odr alone changes nothing.
    let (o, same) = lib("odr.lib", "--fold=none --report=odr");
    assert!(o.stdout.contains("turbolib: odr: ?variant@@YAHXZ"), "{o:?}");
    assert_eq!(same, original);
    for (options, fold, locals, pull, says) in [
        (
            "--fold=select-any --stats",
            VFold::SelectAny,
            false,
            VPull::Preserve,
            "groups folded whose copies differ",
        ),
        (
            "--fold=select-any --pick=largest --pull=relaxed --stats",
            VFold::SelectAny,
            false,
            VPull::Relaxed,
            "groups folded whose copies differ",
        ),
        (
            "--fold=identical --locals=copy --stats",
            VFold::Identical,
            true,
            VPull::Preserve,
            "local units moved with what reaches them",
        ),
        (
            "--fold=select-any --locals=merge --pull=relaxed --stats",
            VFold::SelectAny,
            true,
            VPull::Relaxed,
            "pieces copied",
        ),
        (
            "--fold=identical --report=odr",
            VFold::Identical,
            false,
            VPull::Preserve,
            "turbolib: odr: ?variant@@YAHXZ: 2 definitions",
        ),
    ] {
        let (o, folded) = lib("folded.lib", options);
        assert!(o.stdout.contains(says), "{options}: {o:?}");
        let check = |fold, locals| {
            let options = library::Options { pull, fold, locals };
            library::verify(&original, &folded, &options).unwrap()
        };
        let report = check(fold, locals);
        assert!(report.passed(), "{options}: {}", report.text());
        if fold == VFold::SelectAny || locals {
            assert!(
                !check(VFold::Identical, false).passed(),
                "{options}: verify in identical, locals=keep mode should fail"
            );
        }
    }
}

/// The fixtures `fallback_*` fold: tools/turbolib/loose's three objects.
fn loose_dir(name: &str) -> PathBuf {
    let dir = scratch(name);
    for m in ["a", "b", "c"] {
        std::fs::copy(
            testdata(&format!("coff/loose/msvc-od-{m}.obj")),
            dir.join(format!("{m}.obj")),
        )
        .unwrap();
    }
    dir
}

fn lib_in(dir: &Path, out: &str, options: &str) -> Outcome {
    let mut env = env_in(dir);
    env.turbolib_options = Some(options.to_string());
    run(
        Flavor::Lib,
        &w(&[
            "/nologo",
            "/Brepro",
            &format!("/OUT:{out}"),
            "a.obj",
            "b.obj",
            "c.obj",
        ]),
        &env,
    )
}

/// The files a test left beside its outputs.
fn temps_left(dir: &Path) -> Vec<String> {
    std::fs::read_dir(dir)
        .unwrap()
        .filter_map(Result::ok)
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .filter(|n| n.contains(".tmp"))
        .collect()
}

/// Whatever goes wrong in folding (a panic on a worker thread, a library
/// that can't be laid out, a self-check that fails), the library is still
/// written, unfolded, byte for byte as without folding, with one warning
/// line; `strict` makes it an error that writes nothing. No file is left
/// beside the output.
#[test]
fn a_failed_fold_writes_the_unfolded_library() {
    let dir = loose_dir("fallback");
    let o = lib_in(&dir, "plain.lib", "--fold=none");
    assert_eq!(o.exit, 0, "{o:?}");
    let plain = std::fs::read(dir.join("plain.lib")).unwrap();
    // Folding does change it, and passes its own check.
    let o = lib_in(&dir, "folded.lib", "--fold=identical --precomp=library");
    assert_eq!(o.exit, 0, "{o:?}");
    assert!(!o.stdout.contains("warning"), "{o:?}");
    assert_ne!(std::fs::read(dir.join("folded.lib")).unwrap(), plain);
    for (inject, says) in [
        ("panic", "panic: injected panic at "),
        ("verify", "the self-check failed: injected failure"),
        ("layout", "injected layout failure"),
    ] {
        let options = format!("--fold=identical --precomp=library --inject-failure={inject}");
        let o = lib_in(&dir, "x.lib", &options);
        assert_eq!(o.exit, 0, "{inject}: {o:?}");
        let warnings: Vec<&str> = o.stdout.lines().filter(|l| l.contains("warning")).collect();
        assert_eq!(warnings.len(), 1, "{inject}: {o:?}");
        assert!(
            warnings[0].starts_with(
                "turbolib: warning: folding x.lib failed, so it was written unfolded: "
            ),
            "{inject}: {o:?}"
        );
        assert!(warnings[0].contains(says), "{inject}: {o:?}");
        assert_eq!(std::fs::read(dir.join("x.lib")).unwrap(), plain, "{inject}");
        // /WX doesn't make it an error: it is turbolib's, not lib.exe's.
        let mut env = env_in(&dir);
        env.turbolib_options = Some(options.clone());
        let o = run(
            Flavor::Lib,
            &w(&[
                "/nologo",
                "/Brepro",
                "/WX",
                "/OUT:wx.lib",
                "a.obj",
                "b.obj",
                "c.obj",
            ]),
            &env,
        );
        assert_eq!(o.exit, 0, "{inject}: {o:?}");
        assert_eq!(
            std::fs::read(dir.join("wx.lib")).unwrap(),
            plain,
            "{inject}"
        );
        // Strict: an error, and nothing written.
        let _ = std::fs::remove_file(dir.join("y.lib"));
        let o = lib_in(&dir, "y.lib", &format!("{options} --strict"));
        assert_eq!(o.exit, 1000, "{inject}: {o:?}");
        assert!(
            o.stdout
                .contains("fatal error LNK1000: turbolib: folding failed: "),
            "{o:?}"
        );
        assert!(!dir.join("y.lib").exists(), "{inject}");
        // llvm-lib's flavor too.
        let mut env = env_in(&dir);
        env.turbolib_options = Some(options);
        let o = run(
            Flavor::LlvmLib,
            &w(&["/OUT:llvm.lib", "a.obj", "b.obj", "c.obj"]),
            &env,
        );
        assert_eq!(o.exit, 0, "{inject}: {o:?}");
        assert!(
            o.stderr
                .contains("turbolib: warning: folding llvm.lib failed"),
            "{o:?}"
        );
    }
    assert!(temps_left(&dir).is_empty(), "{:?}", temps_left(&dir));
    let _ = std::fs::remove_dir_all(&dir);
}

/// An output another process holds open for a moment is replaced once it
/// lets go (Windows: a sharing violation is retried); a read-only output
/// isn't replaced, and the error says why.
#[cfg(windows)]
#[test]
fn an_output_held_open_for_a_moment_is_replaced() {
    use std::os::windows::fs::OpenOptionsExt;
    let dir = loose_dir("held");
    let o = lib_in(&dir, "held.lib", "--fold=identical");
    assert_eq!(o.exit, 0, "{o:?}");
    // Open with no sharing at all, as a scanner might.
    let held = std::fs::OpenOptions::new()
        .read(true)
        .share_mode(0)
        .open(dir.join("held.lib"))
        .unwrap();
    let release = std::thread::spawn(move || {
        std::thread::sleep(std::time::Duration::from_millis(400));
        drop(held);
    });
    let o = lib_in(&dir, "held.lib", "--fold=none");
    release.join().unwrap();
    assert_eq!(o.exit, 0, "{o:?}");
    let o2 = lib_in(&dir, "plain.lib", "--fold=none");
    assert_eq!(o2.exit, 0, "{o2:?}");
    assert_eq!(
        std::fs::read(dir.join("held.lib")).unwrap(),
        std::fs::read(dir.join("plain.lib")).unwrap()
    );
    // Read-only: refused at once, as lib.exe refuses it.
    let ro = dir.join("plain.lib");
    let mut p = std::fs::metadata(&ro).unwrap().permissions();
    p.set_readonly(true);
    std::fs::set_permissions(&ro, p.clone()).unwrap();
    // (Without folding, which under a loaded test run can itself take
    // seconds: the time measured is the rename's.)
    let start = std::time::Instant::now();
    let o = lib_in(&dir, "plain.lib", "--fold=none");
    assert!(
        start.elapsed() < crate::output::RETRY,
        "{:?}",
        start.elapsed()
    );
    assert_eq!(o.exit, 1104, "{o:?}");
    assert!(o.stdout.contains("cannot open file 'plain.lib'"), "{o:?}");
    #[allow(clippy::permissions_set_readonly_false)]
    p.set_readonly(false);
    std::fs::set_permissions(&ro, p).unwrap();
    assert!(temps_left(&dir).is_empty(), "{:?}", temps_left(&dir));
    let _ = std::fs::remove_dir_all(&dir);
}

/// Paths past MAX_PATH and outside ASCII, an output that is one of the
/// inputs, and a response file in UTF-8 and UTF-16 naming them.
#[test]
fn long_and_non_ascii_paths_and_the_output_as_an_input() {
    let base = loose_dir("paths");
    let mut deep = base.clone();
    for i in 0..6 {
        deep = deep.join(format!("ünïcødé-директория-{i}-{}", "x".repeat(30)));
    }
    std::fs::create_dir_all(&deep).unwrap();
    assert!(deep.as_os_str().len() > 300, "{}", deep.display());
    for m in ["a", "b", "c"] {
        std::fs::copy(
            base.join(format!("{m}.obj")),
            deep.join(format!("{m}-é.obj")),
        )
        .unwrap();
    }
    let rsp = "/nologo /Brepro /OUT:\"out ü.lib\" a-é.obj b-é.obj\r\n";
    std::fs::write(deep.join("utf8.rsp"), rsp).unwrap();
    let mut utf16 = vec![0xFF, 0xFE];
    for u in rsp.encode_utf16() {
        utf16.extend_from_slice(&u.to_le_bytes());
    }
    std::fs::write(deep.join("utf16.rsp"), utf16).unwrap();
    let env = |options: &str| {
        let mut e = env_in(&deep);
        e.turbolib_options = Some(options.to_string());
        e
    };
    let folding = "--fold=identical --precomp=library --strict";
    let o = run(Flavor::Lib, &w(&["@utf8.rsp"]), &env(folding));
    assert_eq!(o.exit, 0, "{o:?}");
    let first = std::fs::read(deep.join("out ü.lib")).unwrap();
    let o = run(Flavor::Lib, &w(&["@utf16.rsp"]), &env(folding));
    assert_eq!(o.exit, 0, "{o:?}");
    assert_eq!(std::fs::read(deep.join("out ü.lib")).unwrap(), first);
    // The output as an input: `lib x.lib c.obj` adds c.obj to x.lib, here
    // a library made without folding.
    let o = run(Flavor::Lib, &w(&["@utf8.rsp"]), &env("--fold=none"));
    assert_eq!(o.exit, 0, "{o:?}");
    let o = run(
        Flavor::Lib,
        &w(&["/nologo", "/Brepro", "out ü.lib", "c-é.obj"]),
        &env(folding),
    );
    assert_eq!(o.exit, 0, "{o:?}");
    let data = std::fs::read(deep.join("out ü.lib")).unwrap();
    let archive = turbonop_archive::Archive::parse(&data).unwrap();
    let names: Vec<String> = archive.files().map(|m| text(&m.name)).collect();
    assert!(names.iter().any(|n| n == "c-é.obj"), "{names:?}");
    assert!(names.iter().any(|n| n.starts_with("turbolib")), "{names:?}");
    assert!(temps_left(&deep).is_empty(), "{:?}", temps_left(&deep));
    let _ = std::fs::remove_dir_all(&base);
}

/// A library folded before, folded again with more members, and again
/// alone (as a build that makes libraries of libraries does): each fold is
/// kept (no fallback, under `strict`), passes `turbolib verify` against
/// lib.exe's library of the same inputs, has no two members of one name
/// (the new synthesized members skip the input's names), and has nothing
/// two members define (lib.exe's LNK4006): the aliases' default is the
/// linkers' `__ImageBase`, which no member defines and the index doesn't
/// list.
#[test]
fn a_folded_library_archived_again() {
    use turbonop_verify::library;
    let dir = loose_dir("again");
    let o = lib_in(&dir, "x.lib", "--fold=identical --precomp=library");
    assert_eq!(o.exit, 0, "{o:?}");
    let mut env = env_in(&dir);
    env.turbolib_options = Some("--fold=identical --precomp=library --strict".into());
    for (out, inputs) in [
        ("y.lib", &["x.lib", "c.obj"][..]),
        ("z.lib", &["y.lib"][..]),
    ] {
        let out_arg = format!("/OUT:{out}");
        let args = [&["/nologo", "/Brepro", &out_arg][..], inputs].concat();
        let o = run(Flavor::Lib, &w(&args), &env);
        assert_eq!(o.exit, 0, "{out}: {o:?}");
        assert!(!o.stdout.contains("LNK4006"), "{out}: {o:?}");
        let plain = format!("plain-{out}");
        let plain_arg = format!("/OUT:{plain}");
        let args = [&["/nologo", "/Brepro", &plain_arg][..], inputs].concat();
        let o = run(Flavor::Lib, &w(&args), &env_in(&dir));
        assert_eq!(o.exit, 0, "{o:?}");
        let folded = std::fs::read(dir.join(out)).unwrap();
        let original = std::fs::read(dir.join(&plain)).unwrap();
        if out == "y.lib" {
            assert_ne!(folded, original, "{out} folded");
        }
        let report = library::verify(&original, &folded, &library::Options::default()).unwrap();
        assert!(report.passed(), "{out}: {}", report.text());
        let archive = turbonop_archive::Archive::parse(&folded).unwrap();
        let mut names: Vec<String> = archive
            .files()
            .map(|m| text(&m.name).to_ascii_lowercase())
            .collect();
        let count = names.len();
        names.sort();
        names.dedup();
        assert_eq!(
            names.len(),
            count,
            "{out}: two members of one name: {names:?}"
        );
        assert_new_alias_form(&folded, out);
    }
    let _ = std::fs::remove_dir_all(&dir);
}

/// Each external record of `name` in the COFF members of `library`:
/// (member, defined).
fn records_of(library: &[u8], name: &[u8]) -> Vec<(String, bool)> {
    let archive = turbonop_archive::Archive::parse(library).unwrap();
    let mut out = Vec::new();
    for m in archive.files() {
        let Ok(turbonop_coff::File::Object(o)) = turbonop_coff::File::parse(m.data) else {
            continue;
        };
        for s in &o.symbols {
            if s.is_external() && s.name.as_bytes() == name {
                out.push((
                    text(&m.name),
                    s.section != turbonop_coff::SectionNumber::Undefined,
                ));
            }
        }
    }
    out
}

/// What lib.exe would warn LNK4006 about archiving `library` again: names
/// two members define strongly (`library::strong_definitions`, whose
/// exemption for the old alias default doesn't matter: it mustn't be
/// there).
fn duplicate_strong_definitions(library: &[u8]) -> Vec<String> {
    let archive = turbonop_archive::Archive::parse(library).unwrap();
    let members: Vec<_> = archive.files().collect();
    let strong: Vec<Vec<&[u8]>> = members
        .iter()
        .map(|m| crate::msvc::library::strong_definitions(m.data, None))
        .collect();
    let places: Vec<String> = members.iter().map(|m| text(&m.name)).collect();
    crate::msvc::library::duplicate_warnings(&strong, &places)
        .iter()
        .map(|d| format!("{d:?}"))
        .collect()
}

/// A folded library in the new form: no record of `__turbolib$alias`
/// anywhere, `__ImageBase` neither defined nor indexed, each alias's
/// default `__ImageBase`, and nothing lib.exe would warn LNK4006 about.
fn assert_new_alias_form(library: &[u8], what: &str) {
    let legacy = turbonop_fold::rewrite::LEGACY_ALIAS_DEFAULT;
    let default = turbonop_fold::rewrite::alias_default(0x8664);
    assert_eq!(records_of(library, legacy), [], "{what}: the old default");
    let defaults = records_of(library, default);
    assert!(!defaults.is_empty(), "{what}: no aliases");
    assert!(
        defaults.iter().all(|(_, defined)| !defined),
        "{what}: {defaults:?}"
    );
    let archive = turbonop_archive::Archive::parse(library).unwrap();
    assert!(
        archive
            .symbols()
            .iter()
            .all(|s| s.name != legacy && s.name != default),
        "{what}: the alias default is never indexed"
    );
    for m in archive.files() {
        let Ok(turbonop_coff::File::Object(o)) = turbonop_coff::File::parse(m.data) else {
            continue;
        };
        for s in &o.symbols {
            if let turbonop_coff::Aux::WeakExternal(w) = &s.aux
                && w.search == turbonop_coff::weak_search::ALIAS
            {
                assert_eq!(o.symbols[w.tag.index()].name.as_bytes(), default, "{what}");
            }
        }
    }
    let dups = duplicate_strong_definitions(library);
    assert!(dups.is_empty(), "{what}: {dups:?}");
}

/// `library` (turbolib's, in the new form) in the form an earlier turbolib
/// wrote, its members written to `dir`: each alias's default
/// `__turbolib$alias`, which each synthesized member defines as the
/// absolute 0. The member names, in order.
fn legacy_members(library: &[u8], dir: &Path) -> Vec<String> {
    use turbonop_coff::{Name, SectionNumber, Symbol, storage_class};
    let legacy = turbonop_fold::rewrite::LEGACY_ALIAS_DEFAULT;
    let default = turbonop_fold::rewrite::alias_default(0x8664);
    let archive = turbonop_archive::Archive::parse(library).unwrap();
    let mut names = Vec::new();
    for m in archive.files() {
        let name = text(&m.name);
        let data = match turbonop_coff::File::parse(m.data) {
            Ok(turbonop_coff::File::Object(o)) => {
                let mut o = o.into_owned();
                for s in &mut o.symbols {
                    if s.is_external() && s.name.as_bytes() == default {
                        s.name = Name::new(legacy.to_vec());
                    }
                }
                if name.starts_with("turbolib") {
                    o.push_symbol(Symbol::new(
                        Name::new(legacy.to_vec()),
                        0,
                        SectionNumber::Absolute,
                        0,
                        storage_class::EXTERNAL,
                    ));
                }
                o.write().unwrap()
            }
            _ => m.data.to_vec(),
        };
        std::fs::write(dir.join(&name), data).unwrap();
        names.push(name);
    }
    names
}

/// Archives `members` (files in `dir`) unfolded, as `out`.
fn archive_unfolded(dir: &Path, out: &str, members: &[String]) {
    let mut env = env_in(dir);
    env.turbolib_options = Some("--fold=none".into());
    let out_arg = format!("/OUT:{out}");
    let args: Vec<&str> = ["/nologo", "/Brepro", out_arg.as_str()]
        .into_iter()
        .chain(members.iter().map(String::as_str))
        .collect();
    let o = run(Flavor::Lib, &w(&args), &env);
    assert_eq!(o.exit, 0, "{o:?}");
}

/// The fixtures folded (into more than one synthesized member, so that
/// lib.exe archiving the old form again warned LNK4006) have nothing two
/// members define; in the old form, each synthesized member defined
/// `__turbolib$alias`.
#[test]
fn a_folded_library_defines_nothing_twice() {
    let dir = loose_dir("defines-once");
    let o = lib_in(&dir, "x.lib", "--fold=identical --strict");
    assert_eq!(o.exit, 0, "{o:?}");
    let folded = std::fs::read(dir.join("x.lib")).unwrap();
    let archive = turbonop_archive::Archive::parse(&folded).unwrap();
    let synthesized = archive
        .files()
        .filter(|m| text(&m.name).starts_with("turbolib"))
        .count();
    assert!(synthesized > 1, "{synthesized} synthesized members");
    assert_new_alias_form(&folded, "x.lib");
    // The old form, as lib.exe would see it archiving it again.
    let old = dir.join("old");
    std::fs::create_dir_all(&old).unwrap();
    let names = legacy_members(&folded, &old);
    archive_unfolded(&old, "old.lib", &names);
    let old_lib = std::fs::read(old.join("old.lib")).unwrap();
    let legacy = turbonop_fold::rewrite::LEGACY_ALIAS_DEFAULT;
    let defined = records_of(&old_lib, legacy)
        .into_iter()
        .filter(|(_, d)| *d)
        .count();
    assert_eq!(defined, synthesized);
    let _ = std::fs::remove_dir_all(&dir);
}

/// A library an earlier turbolib folded (each alias's default the absolute
/// `__turbolib$alias` every synthesized member defined) folded again, alone
/// and with more objects: the fold is kept (under `strict`), passes
/// `turbolib verify` against the unfolded library of the same inputs, and
/// is in the new form throughout.
#[test]
fn a_library_folded_in_the_old_form_folds_again() {
    use turbonop_verify::library;
    let dir = loose_dir("old-form");
    let mut env = env_in(&dir);
    env.turbolib_options = Some("--fold=identical --precomp=library --strict".into());
    let o = run(
        Flavor::Lib,
        &w(&["/nologo", "/Brepro", "/OUT:x.lib", "a.obj", "b.obj"]),
        &env,
    );
    assert_eq!(o.exit, 0, "{o:?}");
    let folded = std::fs::read(dir.join("x.lib")).unwrap();
    let old = dir.join("old");
    std::fs::create_dir_all(&old).unwrap();
    let names = legacy_members(&folded, &old);
    archive_unfolded(&old, "old.lib", &names);
    std::fs::copy(dir.join("c.obj"), old.join("more.obj")).unwrap();
    let mut env = env_in(&old);
    env.turbolib_options = Some("--fold=identical --precomp=library --strict".into());
    for (out, inputs) in [
        ("alone.lib", &["old.lib"][..]),
        ("more.lib", &["old.lib", "more.obj"][..]),
    ] {
        let out_arg = format!("/OUT:{out}");
        let args = [&["/nologo", "/Brepro", &out_arg][..], inputs].concat();
        let o = run(Flavor::Lib, &w(&args), &env);
        assert_eq!(o.exit, 0, "{out}: {o:?}");
        assert!(!o.stdout.contains("LNK4006"), "{out}: {o:?}");
        let plain = format!("plain-{out}");
        let inputs: Vec<String> = inputs.iter().map(ToString::to_string).collect();
        archive_unfolded(&old, &plain, &inputs);
        let folded = std::fs::read(old.join(out)).unwrap();
        let original = std::fs::read(old.join(&plain)).unwrap();
        let report = library::verify(&original, &folded, &library::Options::default()).unwrap();
        assert!(report.passed(), "{out}: {}", report.text());
        assert_new_alias_form(&folded, out);
    }
    let _ = std::fs::remove_dir_all(&dir);
}

/// An output that can't be written (here, its directory doesn't exist; a
/// full disk takes the same path): lib.exe's LNK1104, then the system's
/// reason, folded or not, and nothing left behind.
#[test]
fn an_output_that_cant_be_written() {
    let dir = loose_dir("unwritable");
    for options in ["--fold=none", "--fold=identical --precomp=library"] {
        let o = lib_in(&dir, "missing/x.lib", options);
        assert_eq!(o.exit, 1104, "{options}: {o:?}");
        assert!(
            o.stdout
                .contains("fatal error LNK1104: cannot open file 'missing/x.lib'"),
            "{o:?}"
        );
        assert!(o.stdout.contains("turbolib: missing/x.lib: "), "{o:?}");
    }
    assert!(temps_left(&dir).is_empty(), "{:?}", temps_left(&dir));
    let _ = std::fs::remove_dir_all(&dir);
}

/// One library and nothing else: copied byte for byte, as lib.exe copies
/// it (its dates and tables kept, /Brepro or not), unless turbolib folds;
/// a /REMOVE makes it write the library again.
#[test]
fn one_library_alone_is_copied() {
    let dir = loose_dir("copy");
    let o = lib_in(&dir, "x.lib", "--fold=none");
    assert_eq!(o.exit, 0, "{o:?}");
    // Not as turbolib would write it: a date in each member header.
    let mut original = std::fs::read(dir.join("x.lib")).unwrap();
    let archive = turbonop_archive::Archive::parse(&original).unwrap();
    let at = usize::try_from(archive.files().last().unwrap().header_offset).unwrap() + 16;
    original[at..at + 12].copy_from_slice(b"1234567890  ");
    std::fs::write(dir.join("in.lib"), &original).unwrap();
    for words in [
        &["/nologo", "/OUT:y.lib", "in.lib"][..],
        &["/nologo", "/Brepro", "/MACHINE:X64", "/OUT:y.lib", "in.lib"][..],
    ] {
        let o = run(Flavor::Lib, &w(words), &env_in(&dir));
        assert_eq!(o.exit, 0, "{o:?}");
        assert_eq!(o.stdout, "", "{o:?}");
        assert_eq!(std::fs::read(dir.join("y.lib")).unwrap(), original);
    }
    let o = run(
        Flavor::Lib,
        &w(&[
            "/nologo",
            "/Brepro",
            "/REMOVE:none.obj",
            "/OUT:z.lib",
            "in.lib",
        ]),
        &env_in(&dir),
    );
    assert_eq!(o.exit, 0, "{o:?}");
    assert_ne!(std::fs::read(dir.join("z.lib")).unwrap(), original);
    // Folding: folded, not copied.
    let mut env = env_in(&dir);
    env.turbolib_options = Some("--fold=identical".into());
    let o = run(
        Flavor::Lib,
        &w(&["/nologo", "/Brepro", "/OUT:f.lib", "in.lib"]),
        &env,
    );
    assert_eq!(o.exit, 0, "{o:?}");
    assert_ne!(std::fs::read(dir.join("f.lib")).unwrap(), original);
    let _ = std::fs::remove_dir_all(&dir);
}
