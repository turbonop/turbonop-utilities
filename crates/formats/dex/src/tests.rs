//! The accessors on the fixtures (testdata/dex): what they're known to
//! hold.

use crate::{Dex, MethodHandleKind, NO_INDEX};

fn fixture(name: &str) -> Vec<u8> {
    std::fs::read(
        std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../../testdata/dex")
            .join(name),
    )
    .unwrap()
}

#[test]
fn headers_and_verification() {
    for (name, version) in [
        ("Sample-035.dex", 35),
        ("Sample-037.dex", 37),
        ("Sample-038.dex", 38),
        ("Sample-039.dex", 39),
        ("Sample-041.dex", 41),
    ] {
        let data = fixture(name);
        let all = Dex::parse_all(&data).unwrap();
        assert_eq!(all.len(), 1, "{name}");
        let dex = &all[0];
        assert_eq!(dex.version(), version);
        let v = dex.verify();
        assert!(v.checksum_ok && v.signature_ok, "{name}: {v:?}");
        assert_eq!(dex.header().file_size as usize, data.len());
        // A changed byte changes both.
        let mut bad = data.clone();
        let last = bad.len() - 1;
        bad[last] ^= 1;
        let v = Dex::parse(&bad).unwrap().verify();
        assert!(!v.checksum_ok && !v.signature_ok, "{name}");
        // Cut short, neither is right.
        let v = Dex::parse(&data[..data.len() - 1]).unwrap().verify();
        assert!(!v.checksum_ok && !v.signature_ok, "{name}");
    }
}

#[test]
fn ids_resolve() {
    let data = fixture("Sample-039.dex");
    let dex = Dex::parse(&data).unwrap();
    let names: Vec<String> = (0..dex.type_count())
        .map(|t| dex.type_name(t).unwrap())
        .collect();
    assert!(names.iter().any(|n| n == "dex.sample.Sample"));
    assert!(names.iter().any(|n| n == "int[][]"));
    let mut found = false;
    for class in dex.class_defs() {
        let d = dex.type_descriptor(class.class_idx).unwrap();
        if d == b"Ldex/sample/Sample;" {
            found = true;
            assert_ne!(class.superclass_idx, NO_INDEX);
            assert_eq!(
                dex.type_descriptor(class.superclass_idx).unwrap(),
                b"Ldex/sample/Base;"
            );
            let ifaces: Vec<_> = dex
                .interfaces(&class)
                .unwrap()
                .iter()
                .map(|t| dex.type_name(u32::from(t)).unwrap())
                .collect();
            assert_eq!(ifaces, ["java.lang.Cloneable"]);
            assert_eq!(dex.string(class.source_file_idx).unwrap(), "Sample.java");
            let cd = dex.class_data(&class).unwrap().unwrap();
            assert!(cd.static_fields.len() > 20);
            let values = dex.static_values(&class).unwrap();
            assert!(!values.is_empty() && values.len() <= cd.static_fields.len());
            for m in cd.methods() {
                let id = dex.method(m.method_idx).unwrap();
                assert_eq!(u32::from(id.class_idx), class.class_idx);
                if m.code_off != 0 {
                    let code = dex.code_item(m.code_off).unwrap();
                    assert!(code.ins_size <= code.registers_size);
                    for i in crate::insn::decode(code.insns) {
                        i.unwrap();
                    }
                }
            }
        }
    }
    assert!(found);
    // The strings with escapes and surrogates decode.
    let all: Vec<String> = (0..dex.string_count())
        .map(|i| dex.string(i).unwrap())
        .collect();
    assert!(
        all.iter()
            .any(|s| s.starts_with("\u{e9}\u{4e2d}\u{1F600}\0end"))
    );
    assert!(all.iter().any(|s| s.contains("\u{FFFD} lone")));
    // Call sites and method handles: every kind.
    assert_eq!(dex.call_site_count(), 2);
    assert_eq!(dex.call_site(0).unwrap().len(), 11);
    let kinds: std::collections::HashSet<_> = (0..dex.method_handle_count())
        .map(|i| MethodHandleKind::from_u16(dex.method_handle(i).unwrap().kind).unwrap())
        .collect();
    assert_eq!(kinds.len(), 9);
    assert!(dex.method_handle(dex.method_handle_count()).is_err());
    assert!(dex.string(dex.string_count()).is_err());
    assert!(dex.type_descriptor(dex.type_count()).is_err());
    assert!(dex.proto(dex.proto_count()).is_err());
    assert!(dex.field(dex.field_count()).is_err());
    assert!(dex.method(dex.method_count()).is_err());
    assert!(dex.class_def(dex.class_def_count()).is_err());
    assert!(dex.call_site(2).is_err());
    assert!(dex.hiddenapi().unwrap().is_none());
}

#[test]
fn sizes_add_up() {
    let data = fixture("Sample-039.dex");
    let dex = Dex::parse(&data).unwrap();
    let sizes: Vec<_> = crate::size::class_sizes(&dex)
        .into_iter()
        .collect::<turbonop_core::Result<_>>()
        .unwrap();
    assert_eq!(sizes.len(), dex.class_def_count() as usize);
    let total: u64 = sizes.iter().map(crate::size::ClassSize::total).sum();
    assert!(total > 0 && total < data.len() as u64);
    let code: u64 = sizes
        .iter()
        .flat_map(|s| &s.methods)
        .map(|m| u64::from(m.code))
        .sum();
    let in_map = dex
        .map()
        .unwrap()
        .iter()
        .find(|m| m.kind == crate::MapItemType::CodeItem)
        .unwrap();
    // The code items, all of them (and the padding between them, which
    // belongs to none).
    assert!(code <= u64::from(dex.header().data_size));
    assert!(code + 4 * u64::from(in_map.size) >= code);
    let packages = crate::size::package_totals(&dex, &sizes);
    // (d8 adds an annotation class of its own.)
    assert_eq!(packages.len(), 2);
    assert_eq!(packages[0].0, "dex.sample");
    assert_eq!(packages[1].0, "com.android.tools.r8.annotations");
    assert_eq!(packages[0].1 + packages[1].1, total);
}
