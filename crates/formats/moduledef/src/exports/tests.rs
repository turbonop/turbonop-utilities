extern crate std;

use std::string::String;
use std::vec;
use std::vec::Vec;

use super::*;
use crate::parse;

fn s(b: &[u8]) -> String {
    String::from_utf8_lossy(b).into_owned()
}

/// x86 exports found among definitions, as lib.exe imports them
/// (measured on `tools/coff/archives/decorated.c` and more).
#[test]
fn found_in_objects() {
    let def = parse(b"EXPORTS\n Func\n Cd\n Fa\n Data1 DATA\n Ren=Func\n Ren2=_Func@8\n _Cd\n Cd2=Cd\n Func@8\n Hidden\n").unwrap();
    let defs = Definitions::new(vec![
        &b"_Func@8"[..],
        b"_Cd",
        b"@Fa@4",
        b"_Data1",
        b"?Hidden@@YGHH@Z",
    ]);
    let given: Vec<_> = def
        .exports()
        .into_iter()
        .map(|e| (e, Origin::Definition))
        .collect();
    let (r, holes) = resolve_with_holes(&given, machine::I386, Some(&defs)).unwrap();
    let got: Vec<(String, ImportNameType, String)> = r
        .iter()
        .map(|r| (s(&r.symbol), r.name_type, s(&r.internal)))
        .collect();
    use ImportNameType::*;
    let want = [
        ("_Func@8", Undecorate, "_Func@8"),
        ("_Cd", NoPrefix, "_Cd"),
        ("@Fa@4", Undecorate, "@Fa@4"),
        ("_Data1", NoPrefix, "_Data1"),
        ("_Ren@8", Undecorate, "_Func@8"),
        ("Ren2", Name, "_Func@8"),
        ("_Cd2", NoPrefix, "_Cd"),
        ("Func@8", Name, "Func@8"),
        ("?Hidden@@YGHH@Z", Undecorate, "?Hidden@@YGHH@Z"),
    ];
    let want: Vec<(String, ImportNameType, String)> = want
        .iter()
        .map(|&(a, t, b)| (a.into(), t, b.into()))
        .collect();
    assert_eq!(got, want);
    // `_Cd` is `Cd`'s symbol: one export, and an empty place in the index.
    assert_eq!(holes, [6]);
}

#[test]
fn unresolved() {
    let def = parse(b"EXPORTS\n Missing\n Over\n Fwd=x.y\n").unwrap();
    let defs = Definitions::new(vec![&b"?Over@@YAHD@Z"[..], b"?Over@@YAHH@Z"]);
    let given: Vec<_> = def
        .exports()
        .into_iter()
        .map(|e| (e, Origin::Definition))
        .collect();
    let e = resolve(&given, machine::AMD64, Some(&defs)).unwrap_err();
    assert_eq!(
        e,
        [
            Unresolved::Missing(Cow::Borrowed(&b"Missing"[..])),
            Unresolved::Ambiguous(b"Over", vec![&b"?Over@@YAHD@Z"[..], b"?Over@@YAHH@Z"]),
            Unresolved::Missing(Cow::Borrowed(&b"y"[..])),
        ]
    );
}

/// Directives are symbols: on x86 `_f` is `NOPREFIX`, `_f@4` by name.
#[test]
fn directives() {
    let e = |n: &'static [u8]| Export {
        name: n,
        internal: None,
        ordinal: None,
        noname: false,
        kind: crate::ExportKind::Code,
        private: false,
        export_as: None,
        line: 0,
    };
    let given = [
        (e(b"_f"), Origin::Directive),
        (e(b"_g@4"), Origin::Directive),
    ];
    let r = resolve(&given, machine::I386, None).unwrap();
    assert_eq!(
        (s(&r[0].name), r[0].name_type),
        ("f".into(), ImportNameType::NoPrefix)
    );
    assert_eq!(
        (s(&r[1].name), r[1].name_type),
        ("_g@4".into(), ImportNameType::Name)
    );
}
