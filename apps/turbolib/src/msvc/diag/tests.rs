use super::*;

#[test]
fn lib_wording() {
    // As lib.exe 14.52 prints them.
    assert_eq!(
        Diag::warning(4044, "unrecognized option '/BOGUS'; ignored").render(Flavor::Lib),
        "LINK : warning LNK4044: unrecognized option '/BOGUS'; ignored"
    );
    assert_eq!(
        Diag::fatal(1181, "cannot open input file 'nothere.obj'").render(Flavor::Lib),
        "LINK : fatal error LNK1181: cannot open input file 'nothere.obj'"
    );
    assert_eq!(
        Diag::warning(4042, "object specified more than once; extras ignored")
            .on("a.obj")
            .render(Flavor::Lib),
        "a.obj : warning LNK4042: object specified more than once; extras ignored"
    );
}

#[test]
fn wx_stops_with_the_warnings_number() {
    let policy = WarningPolicy {
        all_errors: true,
        ..Default::default()
    };
    let mut r = Reporter::new(Flavor::Lib, policy);
    assert!(r.report(Diag::warning(4042, "x").on("a.obj")).is_err());
    assert_eq!(r.exit, 4042);
    assert!(!r.write_output);
    assert_eq!(
        r.lines[1],
        "LINK : error LNK1218: warning treated as error; no output file generated"
    );
}

#[test]
fn wx_for_some_warnings_and_ignore() {
    let policy = WarningPolicy {
        all_errors: false,
        errors: vec![4006],
        ignored: vec![4042],
    };
    let mut r = Reporter::new(Flavor::Lib, policy);
    assert!(r.report(Diag::warning(4042, "silenced")).is_ok());
    assert!(r.lines.is_empty());
    assert!(r.report(Diag::warning(4044, "shown")).is_ok());
    assert_eq!(r.exit, 0);
    assert!(r.report(Diag::warning(4006, "made an error")).is_err());
    assert_eq!(r.exit, 4006);
}

#[test]
fn errors_and_fatal_errors() {
    let mut r = Reporter::new(Flavor::Lib, WarningPolicy::default());
    assert!(r.report(Diag::fatal(1112, "machine")).is_err());
    assert_eq!(r.exit, 1112);
    let mut r = Reporter::new(Flavor::LlvmLib, WarningPolicy::default());
    assert!(
        r.report(Diag::llvm(Kind::Fatal, "no such file or directory").on("x.obj"))
            .is_err()
    );
    assert_eq!(r.exit, 1);
    assert_eq!(r.lines, ["x.obj: no such file or directory"]);
}
