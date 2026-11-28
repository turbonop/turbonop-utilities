//! `verify=links` through the `lib` personality: every spelling of
//! `verify` writes the same folded library (the check never changes what
//! is written), a failed lighter check still falls back, and a broken
//! precompiled-types reference fails both depths.

use super::*;
use turbonop_verify::library::{self, Depth, pch};

const FOLD: &str = "--fold=identical --precomp=library";

#[test]
fn every_verify_mode_writes_the_same_library() {
    let dir = loose_dir("verify-modes");
    let o = lib_in(&dir, "default.lib", FOLD);
    assert_eq!(o.exit, 0, "{o:?}");
    let default = std::fs::read(dir.join("default.lib")).unwrap();
    for mode in [
        "--verify",
        "--verify=on",
        "--verify=full",
        "--verify=links",
        "--verify=LINKS",
        "--verify=off",
    ] {
        let o = lib_in(&dir, "x.lib", &format!("{FOLD} {mode} --strict"));
        assert_eq!(o.exit, 0, "{mode}: {o:?}");
        assert!(!o.stdout.contains("warning"), "{mode}: {o:?}");
        assert_eq!(std::fs::read(dir.join("x.lib")).unwrap(), default, "{mode}");
    }
    // On the command line, over TURBOLIB_OPTIONS.
    let mut env = env_in(&dir);
    env.turbolib_options = Some(format!("{FOLD} --verify=off"));
    let o = run(
        Flavor::Lib,
        &w(&[
            "/nologo",
            "/Brepro",
            "/TURBOLIB:verify=links",
            "/TURBOLIB:stats",
            "/OUT:cl.lib",
            "a.obj",
            "b.obj",
            "c.obj",
        ]),
        &env,
    );
    assert_eq!(o.exit, 0, "{o:?}");
    assert!(o.stdout.contains("turbolib: self-check "), "{o:?}");
    assert_eq!(std::fs::read(dir.join("cl.lib")).unwrap(), default);
    // verify=off runs no check.
    let o = lib_in(&dir, "off.lib", &format!("{FOLD} --verify=off --stats"));
    assert_eq!(o.exit, 0, "{o:?}");
    assert!(!o.stdout.contains("turbolib: self-check "), "{o:?}");
    // A value it doesn't know.
    let o = run(
        Flavor::Lib,
        &w(&["/nologo", "/TURBOLIB:verify=names", "a.obj"]),
        &env_in(&dir),
    );
    assert_eq!(o.exit, 1117, "{o:?}");
    // A failed lighter check falls back as the full one does.
    let o = lib_in(&dir, "plain.lib", "--fold=none");
    assert_eq!(o.exit, 0, "{o:?}");
    let plain = std::fs::read(dir.join("plain.lib")).unwrap();
    let o = lib_in(
        &dir,
        "inj.lib",
        &format!("{FOLD} --verify=links --inject-failure=verify"),
    );
    assert_eq!(o.exit, 0, "{o:?}");
    assert!(
        o.stdout
            .contains("folding inj.lib failed, so it was written unfolded: the self-check failed: injected failure"),
        "{o:?}"
    );
    assert_eq!(std::fs::read(dir.join("inj.lib")).unwrap(), plain);
    assert!(temps_left(&dir).is_empty(), "{:?}", temps_left(&dir));
    let _ = std::fs::remove_dir_all(&dir);
}

/// Where in `lib` the `.debug$T` of the member named `name` starts.
fn debug_t_at(lib: &[u8], name: &str) -> usize {
    let archive = turbonop_archive::Archive::parse(lib).unwrap();
    let m = archive
        .files()
        .find(|m| text(&m.name).eq_ignore_ascii_case(name))
        .unwrap();
    let o = turbonop_coff::Object::parse(m.data).unwrap();
    let t = o
        .sections
        .iter()
        .find(|s| s.name.as_bytes() == b".debug$T")
        .unwrap();
    // (Offsets within one buffer: no unsafe needed.)
    t.bytes().as_ptr() as usize - lib.as_ptr() as usize
}

#[test]
fn a_broken_precompiled_types_reference_fails_both_depths() {
    let dir = loose_dir("verify-pch");
    let o = lib_in(&dir, "plain.lib", "--fold=none");
    assert_eq!(o.exit, 0, "{o:?}");
    let o = lib_in(&dir, "folded.lib", FOLD);
    assert_eq!(o.exit, 0, "{o:?}");
    let plain = std::fs::read(dir.join("plain.lib")).unwrap();
    let folded = std::fs::read(dir.join("folded.lib")).unwrap();
    let check = |lib: &[u8], depth: Depth| {
        library::verify(
            &plain,
            lib,
            &library::Options {
                depth,
                ..library::Options::default()
            },
        )
        .unwrap()
    };
    for depth in [Depth::Full, Depth::Links] {
        let r = check(&folded, depth);
        assert!(r.passed(), "{depth:?}: {}", r.text());
        assert_eq!(
            // (a.obj, b.obj, c.obj, and two synthesized members.)
            r.checked_count("users of turbolib's precompiled-types members"),
            5,
            "{depth:?}: {}",
            r.text()
        );
    }
    let at = debug_t_at(&folded, "a.obj");
    let user = pch::use_of(&{
        let archive = turbonop_archive::Archive::parse(&folded).unwrap();
        let m = archive.files().find(|m| text(&m.name) == "a.obj").unwrap();
        m.data.to_vec()
    })
    .expect("a.obj shares the library's types");
    assert!(pch::names_turbolib_pch(&user.path), "{user:?}");
    // The signature of a.obj's LF_PRECOMP (after the C13 signature, the
    // record's length and kind, its start index and count).
    let mut signature = folded.clone();
    signature[at + 16] ^= 1;
    // The path it names.
    let mut path = folded.clone();
    let end = at + 20 + user.path.len() - 1;
    path[end] ^= 0x01;
    // Its type count.
    let mut count = folded.clone();
    count[at + 12] ^= 1;
    for depth in [Depth::Full, Depth::Links] {
        for (lib, want) in [
            (
                &signature,
                "users whose LF_PRECOMP doesn't match their precompiled-types member",
            ),
            (
                &count,
                "users whose LF_PRECOMP doesn't match their precompiled-types member",
            ),
            (
                &path,
                "users naming a precompiled-types member the library doesn't have (by its S_OBJNAME path)",
            ),
        ] {
            let r = check(lib, depth);
            assert!(r.failed(want), "{depth:?}: {want}: {}", r.text());
        }
    }
    let _ = std::fs::remove_dir_all(&dir);
}
