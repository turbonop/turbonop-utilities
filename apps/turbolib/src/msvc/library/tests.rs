use super::*;
use std::path::Path;

fn fixture(rel: &str) -> Vec<u8> {
    std::fs::read(
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../testdata/coff")
            .join(rel),
    )
    .unwrap()
}

/// The public symbols `dumpbin /linkermember:1` lists, from its output.
fn dumped(text: &str) -> Vec<String> {
    let mut names: Vec<String> = text
        .lines()
        .skip_while(|l| !l.contains("public symbols"))
        .skip(1)
        .filter_map(|l| {
            let mut w = l.split_whitespace();
            let _member = w.next()?;
            w.next().map(str::to_string)
        })
        .collect();
    names.sort();
    names
}

#[test]
fn ltcg_objects_index_as_lib_exe_does() {
    for (obj, listing) in [
        ("ltcg.obj", "ltcg.lib.linkermember.txt"),
        ("ltcg-x86.obj", "ltcg-x86.lib.linkermember.txt"),
    ] {
        let data = fixture(obj);
        let s = ltcg_symbols(&data).expect("a /GL object");
        let mut got: Vec<String> = s
            .symbols
            .iter()
            .map(|s| String::from_utf8_lossy(&s.name).into_owned())
            .collect();
        got.sort();
        let text = String::from_utf8(fixture(listing)).unwrap();
        assert_eq!(got, dumped(&text), "{obj}");
    }
}

#[test]
fn shown_names() {
    assert_eq!(
        shown(b"?f_a@@YAHXZ"),
        "\"int __cdecl f_a(void)\" (?f_a@@YAHXZ)"
    );
    assert_eq!(
        shown(b"?common_c@@3HA"),
        "\"int common_c\" (?common_c@@3HA)"
    );
    assert_eq!(shown(b"cfunc"), "cfunc");
}

#[test]
fn duplicates() {
    let names: Vec<&[u8]> = vec![b"?gy@@YAHXZ", b"?common_c@@3HA"];
    let strong = vec![names.clone(), names];
    let places = vec!["d2.obj".to_string(), "d1.obj".to_string()];
    let got: Vec<String> = duplicate_warnings(&strong, &places)
        .iter()
        .map(ToString::to_string)
        .collect();
    // As lib.exe 14.52 printed them for lib d1.obj d2.obj (archive order
    // d2.obj, d1.obj).
    assert_eq!(
        got,
        [
            "d1.obj : warning LNK4006: \"int common_c\" (?common_c@@3HA) already defined in \
             d2.obj; second definition ignored",
            "d1.obj : warning LNK4006: \"int __cdecl gy(void)\" (?gy@@YAHXZ) already defined \
             in d2.obj; second definition ignored",
        ]
    );
}

#[test]
fn strong_definitions_skip_comdats_but_noduplicates() {
    // objects.obj is /Gy: its external functions are NODUPLICATES COMDATs,
    // its inline ones ANY.
    let data = fixture("objects.obj");
    let strong = strong_definitions(&data, None);
    assert!(!strong.is_empty());
    let turbonop_coff::File::Object(o) = turbonop_coff::File::parse(&data).unwrap() else {
        unreachable!("objects.obj is a COFF object")
    };
    let defined = o
        .symbols
        .iter()
        .filter(|s| s.is_external() && s.section.id().is_some())
        .count();
    assert!(strong.len() < defined, "some definitions are ANY COMDATs");
}
