//! The fixture JARs (testdata/java, built by tools/java/build.cmd): the
//! manifest, the modular JAR's module-info, signature files, the
//! multi-release layout, and the classes' sizes.

use turbonop_global_alloc as _;

mod common;

use common::testdata;
use turbonop_java::attributes::AttributeValue;
use turbonop_java::jar::SignatureFileKind;
use turbonop_java::{AttributesExt, ClassFile, Jar};

#[test]
fn fixtures_jar() {
    let data = std::fs::read(testdata("fixtures.jar")).unwrap();
    let jar = Jar::parse(&data).unwrap();
    let m = jar.manifest().unwrap();
    assert_eq!(jar.main_class(), Some("fixture.Hello"));
    assert_eq!(m.version(), Some("1.0"));
    assert!(m.created_by().unwrap().starts_with("21."));
    // A continued line, joined.
    assert_eq!(
        m.get("X-Long-Attribute"),
        Some(
            "a value long enough that the jar tool continues it on the next line, and the line \
             after that, as manifests wrap at 72 bytes"
        )
    );
    // The package's section, and the signer's digests.
    let internal = m.entry("fixture/internal/").unwrap();
    assert_eq!(internal.get("Sealed"), Some("true"));
    assert!(internal.get("SHA-384-Digest").is_some());
    assert!(
        m.entry("fixture/Everything.class")
            .unwrap()
            .get("SHA-384-Digest")
            .is_some()
    );
    assert!(!jar.is_multi_release());
    // Signed.
    assert!(jar.is_signed());
    let sig = jar.signature_files();
    let kinds: Vec<SignatureFileKind> = sig.iter().map(|s| s.kind).collect();
    assert!(kinds.contains(&SignatureFileKind::SignatureFile));
    assert!(kinds.contains(&SignatureFileKind::SignatureBlock));
    let sf = sig
        .iter()
        .find(|s| s.kind == SignatureFileKind::SignatureFile)
        .unwrap();
    let sf = jar.signature_file(&sf.path).unwrap().unwrap();
    assert!(sf.get("SHA-384-Digest-Manifest").is_some());
    // Every class reads (on every core), and matches its name.
    let classes = jar.read_classes();
    assert_eq!(classes.len(), 36);
    for (c, r) in &classes {
        let s = r.as_ref().unwrap();
        if c.class_name != "module-info" {
            assert_eq!(s.name, c.class_name);
        }
        assert_eq!(s.major_version, 65);
        assert_eq!(s.malformed_attributes, 0);
        assert!(c.overhead > 30 + 46);
    }
    let everything = classes
        .iter()
        .find(|(c, _)| c.class_name == "fixture/Everything")
        .unwrap();
    let s = everything.1.as_ref().unwrap();
    assert_eq!(s.methods.len(), 21);
    assert!(
        s.methods
            .iter()
            .any(|(n, d, size)| n == "switches" && d.starts_with("(I") && *size > 500)
    );
    // Sizes by package.
    let p = jar.package_sizes();
    let names: Vec<&str> = p.iter().map(|p| p.package.as_str()).collect();
    assert_eq!(names, ["", "fixture", "fixture/internal"]);
    assert_eq!(p[1].classes, 34);
    let total: u64 = p.iter().map(|p| p.uncompressed).sum();
    assert_eq!(
        total,
        classes.iter().map(|c| c.0.uncompressed_size).sum::<u64>()
    );
    // The jar tool's additions to module-info: its packages and main class.
    let mi = jar
        .archive()
        .read_by_name("module-info.class")
        .unwrap()
        .unwrap();
    let cf = ClassFile::parse(&mi).unwrap();
    assert!(cf.is_module());
    let packages = cf
        .attributes
        .find_map(|v| match v {
            AttributeValue::ModulePackages(p) => Some(p.len()),
            _ => None,
        })
        .unwrap();
    assert_eq!(packages, 2);
    let main = cf
        .attributes
        .find_map(|v| match v {
            AttributeValue::ModuleMainClass(c) => Some(*c),
            _ => None,
        })
        .unwrap();
    assert_eq!(cf.constant_pool.class_name(main).unwrap(), "fixture/Hello");
}

#[test]
fn multi_release_jar() {
    let data = std::fs::read(testdata("mr.jar")).unwrap();
    let jar = Jar::parse(&data).unwrap();
    assert!(jar.is_multi_release());
    assert_eq!(jar.releases(), [17]);
    assert!(!jar.is_signed());
    let classes = jar.classes();
    let versioned: Vec<&str> = classes
        .iter()
        .filter(|c| c.release == Some(17))
        .map(|c| c.class_name.as_str())
        .collect();
    assert_eq!(versioned, ["mr/Version$1R", "mr/Version"]);
    // Java 11 loads the base Version; Java 17, its own.
    let for11 = jar.classes_for_release(11);
    let v = for11.iter().find(|c| c.class_name == "mr/Version").unwrap();
    assert_eq!(v.release, None);
    assert!(!for11.iter().any(|c| c.class_name == "mr/Version$1R"));
    let for17 = jar.classes_for_release(17);
    let v = for17.iter().find(|c| c.class_name == "mr/Version").unwrap();
    assert_eq!(v.release, Some(17));
    assert_eq!(for17.len(), 5);
    let summaries = jar.read_classes();
    let majors: Vec<(String, u16)> = summaries
        .iter()
        .map(|(c, s)| (c.path.clone(), s.as_ref().unwrap().major_version))
        .collect();
    assert!(majors.contains(&("mr/Version.class".into(), 52)));
    assert!(majors.contains(&("META-INF/versions/17/mr/Version.class".into(), 61)));
    assert!(majors.contains(&("legacy/Legacy.class".into(), 52)));
}

#[test]
fn not_a_jar() {
    assert!(Jar::parse(b"not a zip").is_err());
    // A ZIP with no manifest.
    let data = std::fs::read(
        std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../../../testdata/zip/stored.zip"),
    )
    .unwrap();
    let jar = Jar::parse(&data).unwrap();
    assert!(jar.manifest().is_none());
    assert!(jar.manifest_error().is_none());
    assert!(jar.classes().is_empty());
    assert_eq!(jar.main_class(), None);
}
