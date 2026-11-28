use super::*;

#[test]
fn options_in_any_case() {
    let mut s = Settings::default();
    for o in [
        "FOLD=Select-Any",
        "pick=largest",
        "pull=relaxed",
        "locals=merge",
        "precomp=clustered=8",
        "debug-types=Shared",
        "threads=3",
        "DIAGNOSE",
        "stats",
        "extract-whole",
        "report=ODR",
        "verify=OFF",
        "strict",
        "inject-failure=panic",
    ] {
        s.apply(o).unwrap();
    }
    assert_eq!(
        s,
        Settings {
            fold: Fold::SelectAny,
            pick: Pick::Largest,
            pull: Pull::Relaxed,
            locals: Locals::Merge,
            precomp: Precomp::Clustered(Some(8)),
            debug_types: DebugTypes::Shared,
            threads: Some(3),
            diagnose: true,
            stats: true,
            extract_whole: true,
            report_odr: true,
            verify: Verify::Off,
            strict: true,
            inject: Some(Inject::Panic),
        }
    );
    s.apply("verify").unwrap();
    assert_eq!(s.verify, Verify::Full);
    assert_eq!(Settings::default().verify, Verify::Full);
}

/// `verify`'s spellings: the full check by default and under every name it
/// had (`verify`, `verify=on`) or has (`verify=full`); `links` the lighter
/// check; `off` none. The last one given wins, the command line's over
/// `TURBOLIB_OPTIONS`'s.
#[test]
fn verify_modes() {
    for (o, want) in [
        ("verify", Verify::Full),
        ("verify=on", Verify::Full),
        ("VERIFY=ON", Verify::Full),
        ("verify=full", Verify::Full),
        ("verify=Full", Verify::Full),
        ("verify=links", Verify::Links),
        ("Verify=LINKS", Verify::Links),
        ("verify=off", Verify::Off),
    ] {
        for start in [Verify::Off, Verify::Full, Verify::Links] {
            let mut s = Settings {
                verify: start,
                ..Settings::default()
            };
            s.apply(o).unwrap();
            assert_eq!(s.verify, want, "{o} after {start:?}");
        }
    }
    let mut s = Settings::default();
    s.apply_env("--verify=links").unwrap();
    assert_eq!(s.verify, Verify::Links);
    s.apply_env("--verify=off --verify=full").unwrap();
    assert_eq!(s.verify, Verify::Full);
    s.apply_env("--verify=links").unwrap();
    s.apply("verify=on").unwrap();
    assert_eq!(s.verify, Verify::Full);
    for bad in [
        "verify=link",
        "verify=names",
        "verify=deep",
        "verify=links=1",
        "verify=",
    ] {
        assert!(Settings::default().apply(bad).is_err(), "{bad}");
    }
}

#[test]
fn bad_options() {
    let mut s = Settings::default();
    for o in [
        "fold",
        "fold=some",
        "threads=0",
        "j=x",
        "precomp=clustered=0",
        "debug-types",
        "debug-types=declarations",
        "stats=yes",
        "report",
        "report=everything",
        "verify=maybe",
        "strict=yes",
        "inject-failure",
        "inject-failure=disk",
        "nope",
    ] {
        assert!(s.apply(o).is_err(), "{o}");
    }
}

#[test]
fn environment() {
    let mut s = Settings::default();
    s.apply_env("--stats -j 4 --fold=none").unwrap();
    assert!(s.stats);
    assert_eq!(s.threads, Some(4));
    s.apply_env("-j2").unwrap();
    assert_eq!(s.threads, Some(2));
    assert!(s.apply_env("stats").is_err());
    assert!(s.apply_env("-j").is_err());
}

#[test]
fn what_is_written_so_far() {
    let s = Settings::default();
    assert_eq!(s.fold, Fold::None);
    assert!(s.check_supported().is_ok());
    let mut s = Settings::default();
    s.apply("fold=identical").unwrap();
    assert!(s.check_supported().is_ok());
    s.apply("fold=select-any").unwrap();
    assert!(s.check_supported().is_ok());
    let mut s = Settings::default();
    s.apply("locals=copy").unwrap();
    assert!(s.check_supported().is_ok());
    s.apply("locals=merge").unwrap();
    assert!(s.check_supported().is_ok());
    // Shared types: one precompiled-types member, or clusters; none is
    // still the default.
    let mut s = Settings::default();
    assert_eq!(s.precomp, Precomp::None);
    s.apply("precomp=library").unwrap();
    assert!(s.check_supported().is_ok());
    s.apply("precomp=clustered=4").unwrap();
    assert!(s.check_supported().is_ok());
    assert_eq!(s.precomp, Precomp::Clustered(Some(4)));
    s.apply("precomp=clustered").unwrap();
    assert_eq!(s.precomp, Precomp::Clustered(None));
}
