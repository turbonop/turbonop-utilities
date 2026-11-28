//! turbonop-java against javap: every fixture class (testdata/java, built
//! by tools/java/build.cmd) written as `javap -v -p` writes it, line for
//! line against JDK 21's committed output; and, where there's a JDK 21,
//! the JDK's own classes against its javap, live.

use turbonop_global_alloc as _;

mod common;

use std::path::{Path, PathBuf};

use common::{first_difference, javap_body, testdata};

fn class_files(dir: &Path, out: &mut Vec<PathBuf>) {
    for e in std::fs::read_dir(dir).unwrap().flatten() {
        let p = e.path();
        if p.is_dir() {
            class_files(&p, out);
        } else if p.extension().is_some_and(|x| x == "class") {
            out.push(p);
        }
    }
}

#[test]
fn fixtures_match_javap() {
    let mut files = Vec::new();
    class_files(&testdata(""), &mut files);
    files.sort();
    assert!(files.len() > 40, "{}", files.len());
    let mut failures = Vec::new();
    for p in &files {
        let data = std::fs::read(p).unwrap();
        let cf = turbonop_java::ClassFile::parse(&data).unwrap();
        assert!(cf.malformed_attributes().is_empty(), "{}", p.display());
        let ours = turbonop_java::javap::render(&cf, &Default::default());
        let mut txt = p.clone().into_os_string();
        txt.push(".javap.txt");
        let theirs = javap_body(&std::fs::read_to_string(&txt).unwrap());
        if let Some(d) = first_difference(&ours, &theirs) {
            failures.push(format!("{}: {d}", p.display()));
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n\n"));
}

#[test]
fn modular_jar_module_info_matches_javap() {
    let data = std::fs::read(testdata("fixtures.jar")).unwrap();
    let jar = turbonop_java::Jar::parse(&data).unwrap();
    let bytes = jar
        .archive()
        .read_by_name("module-info.class")
        .unwrap()
        .unwrap();
    let cf = turbonop_java::ClassFile::parse(&bytes).unwrap();
    let ours = turbonop_java::javap::render(&cf, &Default::default());
    let theirs = javap_body(
        &std::fs::read_to_string(testdata("fixtures.jar.module-info.javap.txt")).unwrap(),
    );
    if let Some(d) = first_difference(&ours, &theirs) {
        panic!("module-info.class differs: {d}");
    }
}
