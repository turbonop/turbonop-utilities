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
            verify: false,
            strict: true,
            inject: Some(Inject::Panic),
        }
    );
    s.apply("verify").unwrap();
    assert!(s.verify);
    assert!(Settings::default().verify);
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
