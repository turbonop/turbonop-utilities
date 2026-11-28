//! The machine's framework assemblies (when there's a .NET runtime): every
//! one read through every reader with no errors, and what's known about
//! them (ReadyToRun, strong names, resources) as expected.

#![allow(clippy::cast_possible_truncation)]

use turbonop_global_alloc as _;

mod walk;

use std::path::PathBuf;

fn assemblies() -> Vec<PathBuf> {
    let Some(dir) = walk::framework_dir() else {
        return Vec::new();
    };
    let mut files: Vec<PathBuf> = std::fs::read_dir(dir)
        .unwrap()
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| p.extension().is_some_and(|e| e == "dll"))
        .collect();
    files.sort();
    files
}

#[test]
fn every_framework_assembly_reads_without_errors() {
    let files = assemblies();
    if files.is_empty() {
        eprintln!("no .NET runtime: skipped");
        return;
    }
    // Attributes name enums of other assemblies (whose underlying types
    // they don't say): the framework's are CoreLib's, through facades.
    let corelib_data = files
        .iter()
        .find(|p| p.ends_with("System.Private.CoreLib.dll"))
        .map(|p| std::fs::read(p).unwrap())
        .unwrap();
    let corelib = turbonop_clr::CliImage::parse_bytes(&corelib_data)
        .unwrap()
        .unwrap();
    let results = turbonop_par::map(&files, 1, |p| {
        let data = std::fs::read(p).unwrap();
        let Ok(Some(cli)) = turbonop_clr::CliImage::parse_bytes(&data) else {
            // (Native DLLs live there too.)
            return (false, None);
        };
        let md = cli.metadata();
        let mut enums = turbonop_clr::attributes::EnumTypes::of(md);
        for (_, a) in md.tables().iter::<turbonop_clr::tables::AssemblyRefRow>() {
            enums.add_assembly(corelib.metadata(), &md.string(a.name));
        }
        let mut e = walk::Errors::default();
        walk::image_with(&cli, enums, &mut e);
        let r2r = cli.ready_to_run().ok().flatten().is_some();
        (
            r2r,
            (!e.0.is_empty()).then(|| format!("{}: {:?}", p.display(), e.0)),
        )
    });
    let failures: Vec<&String> = results.iter().filter_map(|r| r.1.as_ref()).collect();
    assert!(failures.is_empty(), "{failures:#?}");
    // The shared framework is compiled ahead of time.
    assert!(results.iter().filter(|r| r.0).count() > 50);
}

#[test]
fn corelib() {
    let files = assemblies();
    let Some(p) = files
        .iter()
        .find(|p| p.ends_with("System.Private.CoreLib.dll"))
    else {
        return;
    };
    let data = std::fs::read(p).unwrap();
    let cli = turbonop_clr::CliImage::parse_bytes(&data).unwrap().unwrap();
    let md = cli.metadata();
    assert_eq!(
        md.assembly_name().as_deref(),
        Some("System.Private.CoreLib")
    );
    assert_eq!(md.version(), "v4.0.30319");
    // System.Object, which nothing extends.
    let object = md.find_type("System", "Object").unwrap();
    let row: turbonop_clr::tables::TypeDefRow = md.row(object).unwrap();
    assert!(row.extends.is_null());
    // ReadyToRun, with a compiler identifier.
    let r2r = cli.ready_to_run().unwrap().unwrap();
    assert!(r2r.major_version >= 5, "{r2r:?}");
    assert!(r2r.section(102).is_some());
    let id = cli.compiler_identifier().unwrap();
    assert!(id.contains("Crossgen2") || id.contains("crossgen"), "{id}");
    // Room for a strong-name signature (a 1024-bit RSA one's, 128 bytes).
    let sig = cli.strong_name_signature().unwrap().unwrap();
    assert_eq!(sig.data.len(), 128);
    // Its resources: the exception messages.
    let resources = cli.embedded_resources();
    let strings = resources
        .iter()
        .flatten()
        .find(|r| turbonop_clr::resources::is_resources(r.data))
        .unwrap();
    let set = turbonop_clr::resources::parse_resources(strings.data).unwrap();
    assert!(set.entries.len() > 1000);
    assert!(
        set.entries
            .iter()
            .all(|e| e.type_name == "ResourceTypeCode.String")
    );
}
