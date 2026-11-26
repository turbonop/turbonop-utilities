use super::*;
use turbonop_archive::unix::Contents;

/// A fresh directory with text files and a small ELF object.
fn fixture(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("turbolib-ar-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(dir.join("sub")).unwrap();
    std::fs::write(dir.join("notes.txt"), b"not an object\n").unwrap();
    std::fs::write(dir.join("odd.txt"), b"odd").unwrap();
    std::fs::write(dir.join("sub/odd.txt"), b"ODD").unwrap();
    std::fs::write(dir.join("a_long_file_name.txt"), b"long").unwrap();
    std::fs::write(dir.join("f.o"), object()).unwrap();
    dir
}

/// An ELF object defining `f` and `g`.
fn object() -> Vec<u8> {
    let mut b = vec![0u8; 64];
    b[..4].copy_from_slice(b"\x7fELF");
    b[4] = 2;
    b[5] = 1;
    b[6] = 1;
    b[16] = 1;
    b[18] = 62;
    b[20] = 1;
    // Sections: null, .text, .symtab, .strtab, .shstrtab.
    let text = b.len();
    b.extend_from_slice(&[0xC3; 8]);
    let symtab = b.len();
    let sym = |b: &mut Vec<u8>, name: u32, info: u8, shndx: u16| {
        b.extend_from_slice(&name.to_le_bytes());
        b.push(info);
        b.push(0);
        b.extend_from_slice(&shndx.to_le_bytes());
        b.extend_from_slice(&[0; 16]);
    };
    sym(&mut b, 0, 0, 0);
    sym(&mut b, 1, 0x12, 1);
    sym(&mut b, 3, 0x22, 1);
    sym(&mut b, 5, 0x10, 0);
    let strtab = b.len();
    b.extend_from_slice(b"\0f\0g\0u\0");
    let shstrtab = b.len();
    b.extend_from_slice(b"\0.text\0.symtab\0.strtab\0.shstrtab\0");
    while !b.len().is_multiple_of(8) {
        b.push(0);
    }
    let shoff = b.len();
    let sh = |b: &mut Vec<u8>,
              name: u32,
              kind: u32,
              off: usize,
              size: usize,
              link: u32,
              info: u32,
              ent: u64| {
        b.extend_from_slice(&name.to_le_bytes());
        b.extend_from_slice(&kind.to_le_bytes());
        b.extend_from_slice(&0u64.to_le_bytes());
        b.extend_from_slice(&0u64.to_le_bytes());
        b.extend_from_slice(&(off as u64).to_le_bytes());
        b.extend_from_slice(&(size as u64).to_le_bytes());
        b.extend_from_slice(&link.to_le_bytes());
        b.extend_from_slice(&info.to_le_bytes());
        b.extend_from_slice(&1u64.to_le_bytes());
        b.extend_from_slice(&ent.to_le_bytes());
    };
    sh(&mut b, 0, 0, 0, 0, 0, 0, 0);
    sh(&mut b, 1, 1, text, 8, 0, 0, 0);
    sh(&mut b, 7, 2, symtab, 96, 3, 1, 24);
    sh(&mut b, 15, 3, strtab, 7, 0, 0, 0);
    sh(&mut b, 23, 3, shstrtab, 33, 0, 0, 0);
    b[40..48].copy_from_slice(&(shoff as u64).to_le_bytes());
    b[52..54].copy_from_slice(&64u16.to_le_bytes());
    b[58..60].copy_from_slice(&64u16.to_le_bytes());
    b[60..62].copy_from_slice(&5u16.to_le_bytes());
    b[62..64].copy_from_slice(&4u16.to_le_bytes());
    b
}

fn ar(flavor: Flavor, dir: &Path, line: &str) -> Outcome {
    let o = ar_quiet(flavor, dir, line);
    if o.exit != 0 {
        std::eprintln!("{line}: {}", o.stderr);
    }
    o
}

fn ar_quiet(flavor: Flavor, dir: &Path, line: &str) -> Outcome {
    let env = Env {
        cwd: dir.to_path_buf(),
        tz: Some("UTC0".into()),
        ..Env::default()
    };
    let words: Vec<String> = line.split_whitespace().map(String::from).collect();
    let name = match flavor {
        Flavor::GnuAr => "ar",
        Flavor::GnuRanlib => "ranlib",
        Flavor::LlvmAr => "llvm-ar",
        Flavor::LlvmRanlib => "llvm-ranlib",
    };
    run(flavor, name, &words, &env)
}

fn names(path: &Path) -> Vec<String> {
    let data = std::fs::read(path).unwrap();
    let a = Archive::parse(&data).unwrap();
    a.files()
        .map(|m| String::from_utf8_lossy(&m.name).into_owned())
        .collect()
}

#[test]
fn create_list_and_index() {
    let dir = fixture("create");
    let o = ar(
        Flavor::GnuAr,
        &dir,
        "rcs x.a f.o notes.txt odd.txt a_long_file_name.txt",
    );
    assert_eq!((o.exit, o.stderr.as_str()), (0, ""));
    let data = std::fs::read(dir.join("x.a")).unwrap();
    let a = Archive::parse(&data).unwrap();
    let symbols: Vec<&[u8]> = a.symbols().iter().map(|s| s.name).collect();
    assert_eq!(symbols, [&b"f"[..], b"g"]);
    let c = Contents::read(&a).unwrap();
    assert_eq!(c.write(Archiver::GnuAr).unwrap(), data);
    let t = ar(Flavor::GnuAr, &dir, "t x.a");
    assert_eq!(
        String::from_utf8(t.stdout).unwrap(),
        "f.o\nnotes.txt\nodd.txt\na_long_file_name.txt\n"
    );
    let tv = ar(Flavor::GnuAr, &dir, "tv x.a odd.txt");
    assert_eq!(
        String::from_utf8(tv.stdout).unwrap(),
        "rw-r--r-- 0/0      3 Jan  1 00:00 1970 odd.txt\n"
    );
    // Creating it said so (without `c`).
    let o = ar(Flavor::GnuAr, &dir, "r y.a odd.txt");
    assert_eq!(o.stderr, "ar: creating y.a\n");
    let o = ar(Flavor::LlvmAr, &dir, "r z.a odd.txt");
    assert_eq!(o.stderr, "llvm-ar: warning: creating z.a\n");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn replace_delete_move() {
    let dir = fixture("edit");
    assert_eq!(
        ar(Flavor::GnuAr, &dir, "rc x.a odd.txt notes.txt f.o").exit,
        0
    );
    // r replaces in place; a new name goes last.
    assert_eq!(
        ar(
            Flavor::GnuAr,
            &dir,
            "r x.a sub/odd.txt a_long_file_name.txt"
        )
        .exit,
        0
    );
    assert_eq!(
        names(&dir.join("x.a")),
        ["odd.txt", "notes.txt", "f.o", "a_long_file_name.txt"]
    );
    let data = std::fs::read(dir.join("x.a")).unwrap();
    let a = Archive::parse(&data).unwrap();
    assert_eq!(a.files().next().unwrap().data, b"ODD");
    // m to the end; b before another.
    assert_eq!(ar(Flavor::GnuAr, &dir, "m x.a odd.txt").exit, 0);
    assert_eq!(
        names(&dir.join("x.a")),
        ["notes.txt", "f.o", "a_long_file_name.txt", "odd.txt"]
    );
    assert_eq!(ar(Flavor::GnuAr, &dir, "mb notes.txt x.a odd.txt").exit, 0);
    assert_eq!(
        names(&dir.join("x.a")),
        ["odd.txt", "notes.txt", "f.o", "a_long_file_name.txt"]
    );
    // d.
    let o = ar(Flavor::GnuAr, &dir, "dv x.a notes.txt nothing.txt");
    assert_eq!(
        String::from_utf8(o.stdout).unwrap(),
        "d - notes.txt\nNo member named `nothing.txt'\n"
    );
    assert_eq!(
        names(&dir.join("x.a")),
        ["odd.txt", "f.o", "a_long_file_name.txt"]
    );
    // q appends, the same name again.
    assert_eq!(ar(Flavor::GnuAr, &dir, "q x.a odd.txt").exit, 0);
    assert_eq!(
        names(&dir.join("x.a")),
        ["odd.txt", "f.o", "a_long_file_name.txt", "odd.txt"]
    );
    // dN 2: the second.
    assert_eq!(ar(Flavor::GnuAr, &dir, "dN 2 x.a odd.txt").exit, 0);
    assert_eq!(
        names(&dir.join("x.a")),
        ["odd.txt", "f.o", "a_long_file_name.txt"]
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn extract_and_print() {
    let dir = fixture("extract");
    assert_eq!(ar(Flavor::LlvmAr, &dir, "rc x.a odd.txt notes.txt").exit, 0);
    std::fs::remove_file(dir.join("odd.txt")).unwrap();
    let o = ar(Flavor::LlvmAr, &dir, "xv x.a odd.txt");
    assert_eq!(String::from_utf8(o.stdout).unwrap(), "x - odd.txt\n");
    assert_eq!(std::fs::read(dir.join("odd.txt")).unwrap(), b"odd");
    let p = ar(Flavor::GnuAr, &dir, "pv x.a notes.txt");
    assert_eq!(p.stdout, b"\n<notes.txt>\n\nnot an object\n");
    let missing = ar(Flavor::LlvmAr, &dir, "p x.a nothing.txt");
    assert_eq!(missing.exit, 1);
    assert_eq!(
        missing.stderr,
        "llvm-ar: error: 'nothing.txt' was not found\n"
    );
    let missing = ar(Flavor::GnuAr, &dir, "p x.a nothing.txt");
    assert_eq!(
        (missing.exit, missing.stderr.as_str()),
        (0, "no entry nothing.txt in archive\n")
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn ranlib_and_tables() {
    let dir = fixture("ranlib");
    assert_eq!(ar(Flavor::GnuAr, &dir, "qcS x.a f.o odd.txt").exit, 0);
    let before = std::fs::read(dir.join("x.a")).unwrap();
    assert!(Archive::parse(&before).unwrap().symbols().is_empty());
    assert_eq!(ar(Flavor::GnuRanlib, &dir, "x.a").exit, 0);
    let after = std::fs::read(dir.join("x.a")).unwrap();
    assert_eq!(Archive::parse(&after).unwrap().symbols().len(), 2);
    // llvm-ranlib leaves an archive with a table as it is.
    assert_eq!(ar(Flavor::LlvmRanlib, &dir, "x.a").exit, 0);
    assert_eq!(std::fs::read(dir.join("x.a")).unwrap(), after);
    // llvm-ar's BSD form.
    assert_eq!(ar(Flavor::LlvmAr, &dir, "rcs --format=bsd b.a f.o").exit, 0);
    let b = std::fs::read(dir.join("b.a")).unwrap();
    assert_eq!(&b[8..13], b"#1/12");
    // A thin archive names its files.
    assert_eq!(ar(Flavor::GnuAr, &dir, "rcT sub/t.a f.o odd.txt").exit, 0);
    let t = ar(Flavor::GnuAr, &dir, "t sub/t.a");
    assert_eq!(
        String::from_utf8(t.stdout).unwrap(),
        "sub/../f.o\nsub/../odd.txt\n"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn folding_is_refused_for_now() {
    let dir = fixture("fold");
    let o = ar(Flavor::GnuAr, &dir, "rcs --fold=identical x.a f.o");
    assert_eq!(o.exit, 1);
    assert!(o.stderr.contains("isn't written yet"), "{}", o.stderr);
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn relative_paths() {
    let cwd = Path::new("/w");
    assert_eq!(relative_to_archive(cwd, "lib/x.a", "a.o"), "../a.o");
    assert_eq!(relative_to_archive(cwd, "x.a", "sub/a.o"), "sub/a.o");
    assert_eq!(relative_to_archive(cwd, "lib/x.a", "lib/../lib/a.o"), "a.o");
    assert_eq!(relative_to_archive(cwd, "a/b/x.a", "a/c/d.o"), "../c/d.o");
}

#[test]
fn calendar_dates() {
    assert_eq!(calendar(0), "Jan  1 00:00 1970");
    assert_eq!(calendar(1_795_682_044), "Nov 26 08:34 2026");
    assert_eq!(calendar(-1), "Dec 31 23:59 1969");
}
