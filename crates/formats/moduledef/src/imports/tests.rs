extern crate std;

use std::string::String;
use std::vec::Vec;

use super::*;
use crate::parse;

const DEF: &str = "LIBRARY lib.dll\nEXPORTS\n  zeta\n  Alpha @3\n  hidden @4 NONAME DATA\n  private PRIVATE\n  as EXPORTAS aaa\n  ?f@@YAXXZ\n  c CONSTANT\n";

fn s(b: &[u8]) -> String {
    String::from_utf8_lossy(b).into_owned()
}

/// An import: its symbol, type, name type, ordinal or hint, and third name.
type Row = (String, ImportType, ImportNameType, u16, Option<String>);

fn plan(m: u16) -> Vec<Row> {
    let def = parse(DEF.as_bytes()).unwrap();
    ImportLibrary::lib_exe(&def, m, b"unused")
        .imports
        .iter()
        .map(|i| {
            (
                s(&i.symbol),
                i.kind,
                i.name_type,
                i.ordinal_or_hint,
                i.export_name.as_deref().map(s),
            )
        })
        .collect()
}

fn i(sym: &str, kind: ImportType, t: ImportNameType, n: u16, third: Option<&str>) -> Row {
    (sym.into(), kind, t, n, third.map(String::from))
}

#[test]
fn x64() {
    use ImportNameType::*;
    use ImportType::*;
    // Named, sorted by the names exported: ?f@@YAXXZ, Alpha, aaa (as), c,
    // private, zeta.
    assert_eq!(
        plan(machine::AMD64),
        [
            i("zeta", Code, Name, 5, None),
            i("Alpha", Code, Ordinal, 3, None),
            i("hidden", Data, Ordinal, 4, None),
            i("as", Code, ExportAs, 2, Some("aaa")),
            i("?f@@YAXXZ", Code, Name, 0, None),
            i("c", Const, Name, 3, None),
        ]
    );
    assert_eq!(plan(machine::ARM64), plan(machine::AMD64));
}

#[test]
fn x86() {
    use ImportNameType::*;
    use ImportType::*;
    assert_eq!(
        plan(machine::I386),
        [
            i("_zeta", Code, NoPrefix, 5, None),
            i("_Alpha", Code, Ordinal, 3, None),
            i("_hidden", Data, Ordinal, 4, None),
            i("_as", Code, ExportAs, 2, Some("aaa")),
            i("?f@@YAXXZ", Code, Name, 0, None),
            i("_c", Const, NoPrefix, 3, None),
        ]
    );
}

#[test]
fn arm64ec() {
    use ImportNameType::*;
    use ImportType::*;
    assert_eq!(
        plan(machine::ARM64EC),
        [
            i("#zeta", Code, ExportAs, 0, Some("zeta")),
            i("#Alpha", Code, Ordinal, 3, Some("Alpha")),
            i("hidden", Data, Ordinal, 4, None),
            i("#as", Code, ExportAs, 0, Some("aaa")),
            i("?f@@$$hYAXXZ", Code, ExportAs, 0, Some("?f@@YAXXZ")),
            i("c", Const, Name, 0, None),
        ]
    );
}

#[test]
fn the_library() {
    let def = parse(DEF.as_bytes()).unwrap();
    let lib = ImportLibrary::lib_exe(&def, machine::AMD64, b"unused");
    assert_eq!(lib.dll, b"lib.dll");
    assert_eq!(lib.stem(), b"lib");
    let [a, b, c] = lib.descriptor_symbols();
    assert_eq!(
        (&a[..], &b[..], &c[..]),
        (
            &b"__IMPORT_DESCRIPTOR_lib"[..],
            &b"__NULL_IMPORT_DESCRIPTOR"[..],
            &b"\x7flib_NULL_THUNK_DATA"[..]
        )
    );
    // The archive's order: by symbol.
    let order: Vec<String> = lib
        .archive_order()
        .into_iter()
        .map(|k| s(&lib.imports[k].symbol))
        .collect();
    assert_eq!(order, ["?f@@YAXXZ", "Alpha", "as", "c", "hidden", "zeta"]);
    // Each one's short import object.
    let o = lib.imports[3].object(machine::AMD64, &lib.dll, 7);
    assert_eq!(
        (o.symbol, o.dll, o.export_name, o.time_date_stamp),
        (&b"as"[..], &b"lib.dll"[..], Some(&b"aaa"[..]), 7)
    );
    assert_eq!(o.to_vec().unwrap()[20..], *b"as\0lib.dll\0aaa\0");
    // Without a name, the .def file's.
    let def = parse(b"EXPORTS\n  A\n").unwrap();
    let lib = ImportLibrary::lib_exe(&def, machine::AMD64, b"file");
    assert_eq!((&lib.dll[..], lib.stem()), (&b"file.dll"[..], &b"file"[..]));
}
