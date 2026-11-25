use super::*;

fn w(words: &[&str]) -> Vec<String> {
    words.iter().map(ToString::to_string).collect()
}

fn lib(words: &[&str]) -> (Options, Vec<String>) {
    let (o, d) = parse(Flavor::Lib, &w(words)).unwrap();
    (o, d.iter().map(|d| d.render(Flavor::Lib)).collect())
}

fn lib_fatal(words: &[&str]) -> String {
    parse(Flavor::Lib, &w(words))
        .unwrap_err()
        .render(Flavor::Lib)
}

#[test]
fn lib_basics_any_case_either_prefix_last_wins() {
    let (o, d) = lib(&[
        "-nologo",
        "/out:first.lib",
        "/OUT:second.lib",
        "a.obj",
        "/Brepro",
        "b.lib",
    ]);
    assert!(d.is_empty());
    assert!(o.nologo && o.brepro);
    assert_eq!(o.out.as_deref(), Some("second.lib"));
    assert_eq!(o.inputs, ["a.obj", "b.lib"]);
    assert!(o.index);
}

#[test]
fn lib_unknown_and_flag_values_are_warnings() {
    let (o, d) = lib(&["/BOGUS", "/OUT=eq.lib", "/NOLOGO:x", "a.obj"]);
    assert!(o.nologo);
    assert_eq!(o.out, None);
    assert_eq!(
        d,
        [
            "LINK : warning LNK4044: unrecognized option '/BOGUS'; ignored",
            "LINK : warning LNK4044: unrecognized option '/OUT=eq.lib'; ignored",
            "LINK : warning LNK4056: extra arguments ignored for option '/NOLOGO:x'",
        ]
    );
}

#[test]
fn lib_missing_values_are_fatal() {
    for opt in ["/MACHINE", "/OUT", "/OUT:", "/NAME", "/EXTRACT", "/IGNORE"] {
        assert_eq!(
            lib_fatal(&[opt]),
            format!("LINK : fatal error LNK1146: no argument specified with option '{opt}'")
        );
    }
    // Measured: /LIBPATH alone is fine.
    assert!(lib(&["/LIBPATH"]).1.is_empty());
}

#[test]
fn lib_bad_values() {
    assert_eq!(
        lib_fatal(&["/SUBSYSTEM:bogus"]),
        "LINK : fatal error LNK1117: syntax error in option 'SUBSYSTEM:bogus'"
    );
    assert_eq!(
        lib_fatal(&["/ERRORREPORT:bogus"]),
        "LINK : fatal error LNK1117: syntax error in option 'ERRORREPORT:bogus'"
    );
    assert_eq!(
        lib_fatal(&["/VERBOSE:1"]),
        "LINK : fatal error LNK1117: syntax error in option 'VERBOSE:1'"
    );
    assert_eq!(
        lib_fatal(&["/WX:abc"]),
        "LINK : fatal error LNK1147: invalid number specified with option '/WX:abc'"
    );
    assert_eq!(
        lib_fatal(&["/IGNORE:abc"]),
        "LINK : fatal error LNK1147: invalid number specified with option '/IGNORE:abc'"
    );
    assert!(lib_fatal(&["/MACHINE:arm"]).contains("LNK1423"));
    let (o, d) = lib(&["/MACHINE:BOGUS"]);
    assert_eq!(o.machine, None);
    assert_eq!(
        d,
        [
            "LINK : warning LNK4012: invalid value 'BOGUS', must be 'ARM64, ARM64X, EBC, X64, \
          or X86'; option ignored"
        ]
    );
}

#[test]
fn lib_values() {
    let (o, d) = lib(&[
        "/MACHINE:arm64ec",
        "/SUBSYSTEM:WINDOWS,6.02",
        "/WX:4006,4221",
        "/IGNORE:4042",
        "/DEF",
        "/LIST:out.txt",
        "/NODEFAULTLIB",
        "/NODEFAULTLIB:libcmt.lib",
        "/EXTRACT:sub\\a.obj",
        "/REMOVE:x.obj",
        "/LTCG:foo",
        "/TURBOLIB:fold=none",
    ]);
    assert!(d.is_empty(), "{d:?}");
    assert_eq!(o.machine, Some(Machine::Arm64Ec));
    assert_eq!(o.subsystem.as_deref(), Some("WINDOWS,6.02"));
    assert_eq!(o.warnings.errors, [4006, 4221]);
    assert_eq!(o.warnings.ignored, [4042]);
    assert_eq!(o.def, Some(None));
    assert_eq!(o.list, Some(Some("out.txt".into())));
    assert_eq!(o.nodefaultlib, [None, Some("libcmt.lib".into())]);
    assert_eq!(o.extract, ["sub\\a.obj"]);
    assert_eq!(o.remove, ["x.obj"]);
    assert!(o.ltcg);
    assert_eq!(o.turbolib, ["fold=none"]);
    let (o, _) = lib(&["/WX", "/WX:NO"]);
    assert!(!o.warnings.all_errors);
}

#[test]
fn subsystems() {
    assert!(valid_subsystem("console"));
    assert!(valid_subsystem("NATIVE,10"));
    assert!(valid_subsystem("EFI_ROM,1.5"));
    assert!(!valid_subsystem("CONSOLE,"));
    assert!(!valid_subsystem("CONSOLE,1."));
    assert!(!valid_subsystem("GUI"));
}

#[test]
fn llvm_lib_unknown_slash_words_are_inputs() {
    let (o, d) = parse(
        Flavor::LlvmLib,
        &w(&[
            "/BOGUS",
            "-bogus",
            "/OUT:x.lib",
            "/llvmlibthin",
            "/llvmlibindex:no",
        ]),
    )
    .unwrap();
    assert_eq!(o.inputs, ["/BOGUS"]);
    assert_eq!(o.out.as_deref(), Some("x.lib"));
    assert!(o.thin && !o.index);
    assert_eq!(
        d.iter()
            .map(|d| d.render(Flavor::LlvmLib))
            .collect::<Vec<_>>(),
        ["ignoring unknown argument: -bogus"]
    );
}

#[test]
fn llvm_lib_machines_and_ignores() {
    let (o, _) = parse(
        Flavor::LlvmLib,
        &w(&[
            "/machine:ARM",
            "/ignore:emptyoutput",
            "/ignore:4006",
            "/def:x.def",
        ]),
    )
    .unwrap();
    assert_eq!(o.machine, Some(Machine::Arm));
    assert_eq!(o.warnings.ignored, [EMPTY_OUTPUT, 4006]);
    assert_eq!(o.def, Some(Some("x.def".into())));
    let e = parse(Flavor::LlvmLib, &w(&["/machine:vax"])).unwrap_err();
    assert_eq!(e.render(Flavor::LlvmLib), "unknown /machine: arg vax");
    // A flag spelled with a value isn't the flag: /list:x is an input.
    let (o, _) = parse(Flavor::LlvmLib, &w(&["/list:x"])).unwrap();
    assert_eq!(o.list, None);
    assert_eq!(o.inputs, ["/list:x"]);
}
