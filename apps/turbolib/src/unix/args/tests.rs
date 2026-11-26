use super::*;

fn words(s: &str) -> Vec<String> {
    s.split_whitespace().map(String::from).collect()
}

#[test]
fn flavors() {
    assert_eq!(flavor("ar"), Some(Flavor::GnuAr));
    assert_eq!(flavor("/usr/bin/x86_64-linux-gnu-ar"), Some(Flavor::GnuAr));
    assert_eq!(flavor("ranlib"), Some(Flavor::GnuRanlib));
    assert_eq!(flavor("aarch64-linux-gnu-ranlib"), Some(Flavor::GnuRanlib));
    assert_eq!(flavor("llvm-ar"), Some(Flavor::LlvmAr));
    assert_eq!(flavor("llvm-ar-21"), Some(Flavor::LlvmAr));
    assert_eq!(flavor(r"C:\llvm\bin\LLVM-AR.EXE"), Some(Flavor::LlvmAr));
    assert_eq!(flavor("llvm-ranlib"), Some(Flavor::LlvmRanlib));
    assert_eq!(flavor("lib"), None);
    assert_eq!(flavor("llvm-lib"), None);
    assert_eq!(flavor("turbolib"), None);
    assert_eq!(flavor("radar"), None);
}

#[test]
fn gnu_old_style_and_dashes() {
    let c = parse(Flavor::GnuAr, words("rcs x.a a.o b.o")).unwrap();
    assert_eq!(c.op, Some(Op::Replace));
    assert!(c.create);
    assert_eq!(c.symtab, Some(true));
    assert_eq!(c.archives, ["x.a"]);
    assert_eq!(c.files, ["a.o", "b.o"]);
    // Options anywhere, as getopt permutes them.
    let c = parse(Flavor::GnuAr, words("-r x.a -v a.o -c")).unwrap();
    assert!(c.verbose && c.create);
    assert_eq!(c.files, ["a.o"]);
    // `--` ends the options.
    let c = parse(Flavor::GnuAr, words("r x.a -- -v")).unwrap();
    assert_eq!(c.files, ["-v"]);
    // q with s is r.
    let c = parse(Flavor::GnuAr, words("qs x.a a.o")).unwrap();
    assert_eq!(c.op, Some(Op::Replace));
    // s alone, or with t, is ranlib.
    assert_eq!(
        parse(Flavor::GnuAr, words("s x.a")).unwrap().op,
        Some(Op::SymbolTable)
    );
    assert_eq!(
        parse(Flavor::GnuAr, words("ts x.a")).unwrap().op,
        Some(Op::SymbolTable)
    );
    assert!(parse(Flavor::GnuAr, words("rt x.a")).is_err());
    assert!(parse(Flavor::GnuAr, words("rz x.a")).is_err());
}

#[test]
fn gnu_positions_and_counts() {
    let c = parse(Flavor::GnuAr, words("rb b.o x.a d.o")).unwrap();
    assert_eq!(c.position, Some(Position::Before("b.o".into())));
    assert_eq!(c.archives, ["x.a"]);
    let c = parse(Flavor::GnuAr, words("dN 2 x.a a.o")).unwrap();
    assert_eq!(c.count, Some(2));
    assert!(parse(Flavor::GnuAr, words("rN 2 x.a a.o")).is_err());
    assert!(parse(Flavor::GnuAr, words("dN 0 x.a a.o")).is_err());
    // A long option first, then old-style letters (PR 13256).
    let c = parse(Flavor::GnuAr, words("--plugin p.so rcs x.a a.o")).unwrap();
    assert_eq!(c.op, Some(Op::Replace));
    assert_eq!(c.files, ["a.o"]);
    let c = parse(Flavor::GnuAr, words("rc --record-libdeps -lfoo x.a a.o")).unwrap();
    assert_eq!(c.libdeps.as_deref(), Some("-lfoo"));
    let c = parse(Flavor::GnuAr, words("--thin rc x.a a.o")).unwrap();
    assert!(c.thin);
}

#[test]
fn llvm_letters() {
    let c = parse(Flavor::LlvmAr, words("rcs x.a a.o")).unwrap();
    assert_eq!(
        (c.op, c.create, c.symtab),
        (Some(Op::Replace), true, Some(true))
    );
    // `-` words add letters; positional words after the first are files.
    let c = parse(Flavor::LlvmAr, words("-r -c x.a a.o")).unwrap();
    assert_eq!(c.archives, ["x.a"]);
    // a, b, i, and N take their arguments when met.
    let c = parse(Flavor::LlvmAr, words("ma b.o x.a d.o")).unwrap();
    assert_eq!(c.position, Some(Position::After("b.o".into())));
    let c = parse(Flavor::LlvmAr, words("dN 2 x.a a.o")).unwrap();
    assert_eq!(c.count, Some(2));
    assert!(parse(Flavor::LlvmAr, words("dN two x.a a.o")).is_err());
    // s with files is refused; s alone is ranlib.
    assert!(parse(Flavor::LlvmAr, words("s x.a a.o")).is_err());
    assert_eq!(
        parse(Flavor::LlvmAr, words("s x.a")).unwrap().op,
        Some(Op::SymbolTable)
    );
    assert!(parse(Flavor::LlvmAr, words("rq x.a")).is_err());
    assert!(parse(Flavor::LlvmAr, words("ro x.a")).is_err());
    assert!(parse(Flavor::LlvmAr, words("rL x.a")).is_err());
    let c = parse(Flavor::LlvmAr, words("rcs --format=darwin x.a a.o")).unwrap();
    assert_eq!(c.format.as_deref(), Some("darwin"));
    assert!(parse(Flavor::LlvmAr, words("rcs --format=elf x.a a.o")).is_err());
    let c = parse(Flavor::LlvmAr, words("--thin rcs x.a a.o")).unwrap();
    assert!(c.thin && c.full_path);
    let c = parse(Flavor::LlvmAr, words("-M")).unwrap();
    assert!(c.mri);
    assert!(parse(Flavor::LlvmAr, words("-M rcs x.a")).is_err());
}

#[test]
fn ranlibs() {
    let c = parse(Flavor::GnuRanlib, words("-D x.a y.a")).unwrap();
    assert_eq!(c.op, Some(Op::SymbolTable));
    assert_eq!(c.archives, ["x.a", "y.a"]);
    assert_eq!(c.deterministic, Some(true));
    let c = parse(Flavor::GnuRanlib, words("-t x.a")).unwrap();
    assert!(c.touch);
    let c = parse(Flavor::LlvmRanlib, words("-U x.a")).unwrap();
    assert_eq!(c.deterministic, Some(false));
    assert!(parse(Flavor::LlvmRanlib, words("-t x.a")).is_err());
    assert!(parse(Flavor::LlvmRanlib, words("-D")).is_err());
}

#[test]
fn turbolib_options() {
    let c = parse(
        Flavor::GnuAr,
        words("rcs --fold=identical x.a -j 4 a.o --stats"),
    )
    .unwrap();
    assert_eq!(c.turbolib, ["fold=identical", "threads=4", "stats"]);
    assert_eq!(c.files, ["a.o"]);
    let c = parse(Flavor::LlvmRanlib, words("--pull=relaxed -j2 x.a")).unwrap();
    assert_eq!(c.turbolib, ["pull=relaxed", "threads=2"]);
    // After `--`, a file.
    let c = parse(Flavor::GnuAr, words("rc x.a -- --stats")).unwrap();
    assert_eq!(c.files, ["--stats"]);
}

#[test]
fn response_files() {
    let mut read = |p: &str| -> std::io::Result<Vec<u8>> {
        match p {
            "a.rsp" => Ok(b"rcs x.a 'with space.o' \"q.o\" @b.rsp".to_vec()),
            "b.rsp" => Ok(b"c\\ d.o".to_vec()),
            _ => Err(std::io::ErrorKind::NotFound.into()),
        }
    };
    let w = expand(&words("@a.rsp e.o @missing.rsp"), &mut read);
    assert_eq!(
        w,
        [
            "rcs",
            "x.a",
            "with space.o",
            "q.o",
            "c d.o",
            "e.o",
            "@missing.rsp"
        ]
    );
}
