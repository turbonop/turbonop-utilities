//! The import libraries lib.exe made of tools/coff/archives/def/*.def
//! (testdata/coff/archives/def-*.lib, from tools/coff/archives.cmd)
//! against what this crate reads of the same files and plans of them:
//! the DLL's name, the descriptor objects' symbols, and every import (its
//! symbol, type, name type, hint or ordinal, and third name), in both the
//! `.def` file's order (the second linker member's) and the archive's.

use turbonop_global_alloc as _;

use std::path::{Path, PathBuf};

use turbonop_archive::Archive;
use turbonop_moduledef::imports::ImportLibrary;
use turbonop_pe::consts::machine;
use turbonop_pe::import_object::ImportObject;

fn root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../..")
}

fn show(b: &[u8]) -> String {
    String::from_utf8_lossy(b).into_owned()
}

/// `def-<name>[-<machine>].lib`: the `.def` file's name and the machine,
/// or `None` for those this doesn't plan (objects given too, ARM64X,
/// llvm-lib's).
fn case(file: &str) -> Option<(String, u16)> {
    let stem = file.strip_prefix("def-")?.strip_suffix(".lib")?;
    let mut parts = stem.split('-');
    let name = parts.next()?.to_owned();
    let rest: Vec<&str> = parts.collect();
    let m = match rest.as_slice() {
        [] | ["x64"] | ["x64", "dated"] => machine::AMD64,
        ["x86"] => machine::I386,
        ["arm64"] => machine::ARM64,
        ["arm64ec"] => machine::ARM64EC,
        _ => return None,
    };
    Some((name, m))
}

fn check(lib: &Path, def_name: &str, m: u16) -> usize {
    let file = lib.file_name().unwrap().to_string_lossy().into_owned();
    let text = std::fs::read(
        root()
            .join("tools/coff/archives/def")
            .join(format!("{def_name}.def")),
    )
    .unwrap();
    let def = turbonop_moduledef::parse(&text).unwrap_or_else(|e| panic!("{file}: {e}"));
    let stem = def_name.as_bytes();
    let plan = ImportLibrary::lib_exe(&def, m, stem);

    let data = std::fs::read(lib).unwrap();
    let ar = Archive::parse(&data).unwrap();
    let files: Vec<_> = ar.files().collect();
    assert!(
        files.iter().all(|f| *f.name == plan.dll),
        "{file}: DLL {}",
        show(&plan.dll)
    );
    // The descriptor objects first, in both orders.
    let first = ar.first_linker_member().unwrap();
    let descriptors: Vec<&[u8]> = first.iter().take(3).map(|s| s.name).collect();
    let want = plan.descriptor_symbols();
    let want: Vec<&[u8]> = want.iter().map(Vec::as_slice).collect();
    assert_eq!(descriptors, want, "{file}");
    for f in &files[..3] {
        assert!(!ImportObject::is_import_object(f.data), "{file}");
    }
    fn imports<'a>(members: Vec<&turbonop_archive::Member<'a>>) -> Vec<ImportObject<'a>> {
        members
            .into_iter()
            .map(|f| ImportObject::parse(f.data).unwrap())
            .collect()
    }
    // The archive's order: sorted by symbol.
    let archived = imports(files[3..].iter().collect());
    // The second linker member's: the .def file's.
    let (list, _) = ar.second_linker_member().unwrap();
    let listed = imports(
        list[3..]
            .iter()
            .map(|&o| files.iter().find(|f| f.header_offset == o).unwrap())
            .collect(),
    );
    let order = plan.archive_order();
    for (got, want) in [
        (&listed, (0..plan.imports.len()).collect::<Vec<_>>()),
        (&archived, order),
    ] {
        assert_eq!(got.len(), want.len(), "{file}: import count");
        for (g, &w) in got.iter().zip(&want) {
            let w = &plan.imports[w];
            let what = format!("{file}: {}", show(&w.symbol));
            assert_eq!((g.machine, g.symbol), (m, &*w.symbol), "{what}");
            assert_eq!((g.kind, g.name_type), (w.kind, w.name_type), "{what}");
            assert_eq!(g.export_name, w.export_name.as_deref(), "{what}");
            // (lib.exe's ARM64EC hints of imports by name aren't
            // reproduced.)
            let by_name = g.name_type != turbonop_pe::import_object::ImportNameType::Ordinal;
            if !(m == machine::ARM64EC && by_name) {
                assert_eq!(g.ordinal_or_hint, w.ordinal_or_hint, "{what}: hint");
            }
            // The object the plan describes, written: lib.exe's but its
            // time stamp (and those hints).
            let mut mine = w.object(m, &plan.dll, g.time_date_stamp);
            mine.ordinal_or_hint = g.ordinal_or_hint;
            assert_eq!(mine.to_vec().unwrap(), g.to_vec().unwrap(), "{what}");
        }
    }
    plan.imports.len()
}

#[test]
fn import_libraries_as_lib_exe_makes_them() {
    let dir = root().join("testdata/coff/archives");
    let mut libs: Vec<PathBuf> = std::fs::read_dir(&dir)
        .unwrap()
        .flatten()
        .map(|e| e.path())
        .collect();
    libs.sort();
    let (mut checked, mut imports) = (0, 0);
    for lib in libs {
        let name = lib.file_name().unwrap().to_string_lossy().into_owned();
        if let Some((def, m)) = case(&name) {
            imports += check(&lib, &def, m);
            checked += 1;
        }
    }
    assert!(checked >= 30, "{checked} libraries");
    assert!(imports >= 150, "{imports} imports");
}

/// What each module statement makes the DLL's name, and its stem.
#[test]
fn dll_names() {
    let plan = |text: &'static str, m: u16| {
        let def = turbonop_moduledef::parse(text.as_bytes()).unwrap();
        let p = ImportLibrary::lib_exe(&def, m, b"out");
        (show(&p.dll), show(p.stem()))
    };
    let p = |a: &str, b: &str| (a.to_owned(), b.to_owned());
    assert_eq!(
        plan("LIBRARY foo.bar.baz\n", machine::AMD64),
        p("foo.bar.baz", "foo.bar")
    );
    assert_eq!(plan("NAME prog\n", machine::AMD64), p("prog.exe", "prog"));
    assert_eq!(plan("", machine::AMD64), p("out.dll", "out"));
    assert_eq!(plan("LIBRARY .dll\n", machine::AMD64), p(".dll", ""));
}

/// lib.exe's ARM64EC hints of imports by name aren't reproduced (they are
/// 0, or bytes of memory lib.exe didn't set: the same `.def` file gave 0
/// and 26982 in two runs), so each is taken from the library.
fn take_ec_hints(plan: &mut [turbonop_moduledef::imports::Import<'_>], lib: &[ImportObject<'_>]) {
    use turbonop_pe::import_object::ImportNameType;
    for i in plan.iter_mut() {
        if i.name_type != ImportNameType::Ordinal
            && let Some(g) = lib.iter().find(|g| g.symbol == &*i.symbol)
        {
            i.ordinal_or_hint = g.ordinal_or_hint;
        }
    }
}

/// Every fixture lib.exe made of a `.def` file alone, written again whole,
/// byte for byte: the archive, the descriptor objects, and the import
/// objects with their /Brepro stamps (or, for the dated one, its time).
#[test]
fn whole_libraries_as_lib_exe_writes_them() {
    use turbonop_archive::write::{Style, Tool};
    use turbonop_moduledef::descriptors::Stamp;
    let dir = root().join("testdata/coff/archives");
    let def = |name: &str| {
        std::fs::read(root().join(format!("tools/coff/archives/def/{name}.def"))).unwrap()
    };
    let mut libs: Vec<PathBuf> = std::fs::read_dir(&dir)
        .unwrap()
        .flatten()
        .map(|e| e.path())
        .collect();
    libs.sort();
    let mut checked = 0;
    for lib in libs {
        let file = lib.file_name().unwrap().to_string_lossy().into_owned();
        let data = std::fs::read(&lib).unwrap();
        let (text, native) = match case(&file) {
            Some((name, m)) => ((def(&name), name, m), None),
            None if file == "def-ec-arm64x.lib" => (
                (def("ec"), "ec".to_owned(), machine::ARM64X),
                Some(def("native")),
            ),
            None => continue,
        };
        let (text, name, m) = text;
        let parsed = turbonop_moduledef::parse(&text).unwrap();
        let native_parsed = native
            .as_ref()
            .map(|n| turbonop_moduledef::parse(n).unwrap());
        let mut plan = match &native_parsed {
            Some(n) => ImportLibrary::lib_exe_arm64x(&parsed, n, name.as_bytes()),
            None => ImportLibrary::lib_exe(&parsed, m, name.as_bytes()),
        };
        let ar = Archive::parse(&data).unwrap();
        let objects: Vec<ImportObject<'_>> = ar
            .files()
            .filter(|f| ImportObject::is_import_object(f.data))
            .map(|f| ImportObject::parse(f.data).unwrap())
            .collect();
        let ec: Vec<ImportObject<'_>> = objects
            .iter()
            .filter(|o| o.machine == machine::ARM64EC)
            .copied()
            .collect();
        take_ec_hints(&mut plan.imports, &ec);
        let dated = file.contains("-dated");
        let (stamp, style) = if dated {
            let t = objects[0].time_date_stamp;
            (Stamp::Time(t), Style::LibExe { time: u64::from(t) })
        } else {
            (Stamp::Brepro, Style::LibExeBrepro)
        };
        let mine = plan.members(stamp).write(style, Tool::LibExe).unwrap();
        assert!(mine == data, "{file}: the library differs");
        checked += 1;
    }
    assert!(checked >= 35, "{checked} libraries");
}
