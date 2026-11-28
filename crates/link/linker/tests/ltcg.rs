//! LTCG's stand-in object: what the first resolution sees of a `/GL`
//! object (its definitions, COMDATs as the IL's are, its references, its
//! directives).

use turbonop_global_alloc as _;

use turbonop_linker::c2::{IlObject, Public};
use turbonop_linker::object::{Object, Selection, SymbolKind};

fn public(name: &str, kind: u32, comdat: Option<u8>) -> Public {
    Public {
        name: name.as_bytes().to_vec(),
        kind,
        handle: 0,
        comdat,
    }
}

#[test]
fn a_stand_in_defines_and_refers_as_the_il_does() {
    let il = IlObject {
        publics: vec![
            public("main", 2, None),
            public("printf", 2, Some(2)),
            public("_OptionsStorage_long_name", 4, Some(2)),
            public("square", 8, None),
            public("__imp___stdio_common_vfprintf", 1, None),
        ],
        directives: b"/DEFAULTLIB:\"MSVCRT\" /DEFAULTLIB:\"OLDNAMES\"".to_vec(),
    };
    let bytes: &'static [u8] = Box::leak(turbonop_linker::ltcg::stand_in(&il).into_boxed_slice());
    let object = Object::parse(bytes).expect("a valid COFF object");
    let find = |n: &str| {
        object
            .symbols
            .iter()
            .find(|s| s.name == n.as_bytes())
            .unwrap_or_else(|| panic!("{n}"))
    };
    // Definitions, each in a section of its own; a COMDAT as the IL's is.
    let section_of = |n: &str| match find(n).kind {
        SymbolKind::Defined { section, .. } => section as usize,
        k => panic!("{n}: {k:?}"),
    };
    assert!(object.sections[section_of("main")].comdat.is_none());
    for n in ["printf", "_OptionsStorage_long_name"] {
        assert!(matches!(
            object.sections[section_of(n)].comdat,
            Some((Selection::Any, _))
        ));
    }
    assert_ne!(section_of("main"), section_of("printf"));
    // A reference, undefined; a static, not there.
    assert!(matches!(
        find("__imp___stdio_common_vfprintf").kind,
        SymbolKind::Undefined
    ));
    assert!(object.symbols.iter().all(|s| s.name != b"square"));
    // The directives.
    assert_eq!(object.directives.len(), 2);
}
