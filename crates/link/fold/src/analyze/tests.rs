use super::*;

fn fixture(name: &str) -> Vec<u8> {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../../testdata/coff")
        .join(name);
    std::fs::read(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display()))
}

fn analyze(name: &str) -> Library {
    let data = fixture(name);
    let archive = turbonop_archive::Archive::parse(&data).unwrap();
    let members: Vec<(&[u8], &[u8])> = archive
        .files()
        .map(|m| (&*m.name.to_vec().leak(), m.data))
        .collect();
    library(name, data.len() as u64, &members)
}

#[test]
fn the_same_object_twice_folds_completely() {
    // objects.lib: objects.obj and objects-big.obj, one source compiled
    // twice.
    let l = analyze("objects.lib");
    let s = &l.summary;
    assert_eq!(s.members.coff, 2);
    let c = &s.comdats;
    assert!(c.names > 0);
    assert_eq!(c.duplicated_names, c.names);
    assert_eq!(c.units, 2 * c.names);
    assert_eq!(c.differing_names, 0);
    // Its two functions with external linkage (`/Gy` COMDATs, NODUPLICATES)
    // are strong definitions: never folded.
    assert_eq!(c.strong_duplicates, 2);
    // Identical folding saves the second copy of every other unit: the
    // second member's, objects.obj (objects-big.obj is first).
    let facts: Vec<_> = {
        let data = fixture("objects.obj");
        crate::member::read(&data).units
    };
    let expected: u64 = facts
        .iter()
        .filter(|u| u.selection != comdat_select::NODUPLICATES)
        .map(|u| u.bytes.total())
        .sum();
    assert_eq!(c.saved[0].total(), expected);
    // Nothing reaches locals but the strong definitions, so keeping or
    // copying locals saves the same.
    assert_eq!(c.saved[2].total(), expected);
    // Select-any saves as much as identical when the copies are identical.
    assert_eq!(c.saved[3].total(), c.saved[2].total());
    // The types: nearly every record is shared.
    let t = &s.types;
    assert_eq!(t.members, 2);
    // Every record is in both members but the two that hold each one's
    // command line, and the function ids, which stay in each member.
    let ids = facts_ids();
    assert_eq!(t.distinct_records, t.shared_records + 4 + ids, "{t:?}");
    assert!(t.unshareable_duplicate_bytes > 0);
    assert!(t.saved > 0 && t.after < t.bytes);
    assert_eq!(s.saved, c.saved[1].total() + t.saved);
}

#[test]
fn import_and_ltcg_libraries_save_nothing() {
    for name in ["imports.lib", "ltcg.lib"] {
        let l = analyze(name);
        assert_eq!(l.summary.saved, 0, "{name}");
        assert_eq!(l.summary.comdats.units, 0, "{name}");
    }
    assert!(
        analyze("ltcg.lib")
            .diagnostics
            .iter()
            .any(|d| d.contains("LTCG"))
    );
}

#[test]
fn summaries_add() {
    let a = analyze("objects.lib").summary;
    let mut total = Summary::default();
    total.add(&a);
    total.add(&a);
    assert_eq!(total.libraries, 2);
    assert_eq!(total.saved, 2 * a.saved);
    assert_eq!(
        total.comdats.saved[0].total(),
        2 * a.comdats.saved[0].total()
    );
}

/// The distinct LF_FUNC_ID and LF_MFUNC_ID records of objects.obj.
fn facts_ids() -> u64 {
    let data = fixture("objects.obj");
    match crate::member::read(&data).types {
        Types::Records { records, .. } => {
            let mut ids: Vec<u128> = records
                .iter()
                .filter(|r| !r.shareable)
                .map(|r| r.hash)
                .collect();
            ids.sort_unstable();
            ids.dedup();
            ids.len() as u64
        }
        other => panic!("{other:?}"),
    }
}
