//! `hexcavator ltcg` on LTCG (/GL) objects (testdata/coff: tools/coff/ltcg.cpp
//! and ltcg.c, see tools/coff/build.cmd), against what the tools show of
//! them: dumpbin's header, lib.exe's symbol index, and the directives of
//! the same source compiled without /GL.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::process::Command;

fn coff() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../testdata/coff")
}

fn hexcavator(args: &[&str]) -> String {
    let out = Command::new(env!("CARGO_BIN_EXE_hexcavator"))
        .args(args)
        .current_dir(coff())
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "{args:?}: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8(out.stdout).unwrap()
}

/// The symbols `hexcavator ltcg` lists: (kind, storage, size, name).
fn symbols(file: &str, option: &str) -> Vec<(String, String, String, String)> {
    let text = hexcavator(&["ltcg", option, file]);
    let mut out = Vec::new();
    for line in text.lines() {
        let t: Vec<&str> = line.split_whitespace().collect();
        if t.len() < 4 || !(t[1] == "function" || t[1] == "data") {
            continue;
        }
        // (The name: the last column, before any undecorated form.)
        let name = line[70..].split(" (").next().unwrap().to_string();
        let size = if t[1] == "data" {
            t[3].to_string()
        } else {
            String::new()
        };
        out.push((t[1].to_string(), t[2].to_string(), size, name));
    }
    assert!(!out.is_empty(), "{text}");
    out
}

/// lib.exe's symbol index of a library (dumpbin /linkermember:2).
fn index(file: &str) -> BTreeSet<String> {
    let text = std::fs::read_to_string(coff().join(file)).unwrap();
    let list = text.split("public symbols").nth(1).unwrap();
    list.lines()
        .map(str::trim)
        .skip_while(|l| l.is_empty())
        .take_while(|l| !l.is_empty())
        .map(|l| l.split_whitespace().nth(1).unwrap().to_string())
        .collect()
}

#[test]
fn dump_headers_as_dumpbin() {
    for name in ["ltcg.obj", "ltcg-x86.obj"] {
        let ours = hexcavator(&["dump", "-headers", name]);
        let theirs = std::fs::read_to_string(coff().join(format!("{name}.headers.txt"))).unwrap();
        let a: Vec<&str> = theirs.trim().lines().map(str::trim_end).collect();
        let b: Vec<&str> = ours.trim().lines().map(str::trim_end).collect();
        assert_eq!(a, b, "{name}");
    }
}

/// Every public symbol lib.exe indexes is one hexcavator finds defined (lib.exe
/// leaves out inline functions nothing uses, which hexcavator lists).
#[test]
fn public_definitions_hold_libs_index() {
    let ours: BTreeSet<String> = symbols("ltcg.obj", "-publics")
        .into_iter()
        .map(|s| s.3)
        .collect();
    let theirs = index("ltcg.lib.linkermember.txt");
    assert!(theirs.len() > 20);
    for name in &theirs {
        assert!(ours.contains(name), "{name} is not among {ours:?}");
    }
    // x86's C names: lib.exe decorates them (_name, _name@N, @name@N);
    // the IL holds them as the compiler wrote them.
    let ours: BTreeSet<String> = symbols("ltcg-x86.obj", "-publics")
        .into_iter()
        .map(|s| s.3)
        .collect();
    let theirs = index("ltcg-x86.lib.linkermember.txt");
    let undecorated: BTreeSet<String> = theirs
        .iter()
        .map(|n| n.trim_start_matches(['_', '@']).to_string())
        .collect();
    assert_eq!(
        undecorated,
        [
            "cdecl_fn",
            "fast_fn@8",
            "std_fn@8",
            "tentative",
            "tentative_array"
        ]
        .map(String::from)
        .into()
    );
    assert_eq!(ours, undecorated);
}

#[test]
fn kinds_storage_and_sizes() {
    let all = symbols("ltcg.obj", "-symbols");
    let find = |name: &str| {
        all.iter()
            .find(|s| s.3 == name)
            .unwrap_or_else(|| panic!("{name}"))
            .clone()
    };
    let has = |name: &str, kind: &str, storage: &str, size: &str| {
        assert_eq!(
            find(name),
            (kind.into(), storage.into(), size.into(), name.into()),
            "{name}"
        );
    };
    has("?counter@@3HA", "data", "defined", "4");
    has("?hidden@@3PAHA", "data", "static", "A0");
    has("?elsewhere@@3HA", "data", "declared", "4");
    has("?exported_value@@3HA", "data", "dllexport", "4");
    has("?banner@@3QBDB", "data", "static", "F");
    has("ltcg_kept", "data", "defined", "4");
    has("exported", "function", "dllexport", "");
    has("?imported@@YAHH@Z", "function", "dllimport", "");
    has("?helper@@YAHH@Z", "function", "static", "");
    has("?callee_pops@@YAHHH@Z", "function", "defined", "");
    has("?fail@@YAXPEBD@Z", "function", "declared", "");
    // String literals' sizes are in their names (??_C@_0<length>@).
    for (_, _, size, name) in all.iter().filter(|s| s.3.starts_with("??_C@_0")) {
        // (A digit is 1 to 10; else hexadecimal digits A-P, then @.)
        let rest = &name["??_C@_0".len()..];
        let len = match rest.as_bytes()[0] {
            d @ b'0'..=b'9' => u64::from(d - b'0') + 1,
            _ => rest
                .split('@')
                .next()
                .unwrap()
                .bytes()
                .fold(0, |v, b| v * 16 + u64::from(b - b'A')),
        };
        assert_eq!(u64::from_str_radix(size, 16).unwrap(), len, "{name}");
    }
    let x86 = symbols("ltcg-x86.obj", "-symbols");
    assert!(x86.contains(&(
        "data".into(),
        "common".into(),
        "190".into(),
        "tentative_array".into()
    )));
}

/// The directives are those the source gives (the ones the compiler adds
/// itself, its libraries and exports, are the back end's).
#[test]
fn directives_are_the_sources() {
    let text = hexcavator(&["ltcg", "-directives", "ltcg.obj"]);
    let ours: Vec<String> = text
        .lines()
        .filter(|l| l.starts_with("    /"))
        .map(|l| l.trim().replace('"', ""))
        .collect();
    let nogl = std::fs::read_to_string(coff().join("ltcg.directives.txt")).unwrap();
    let theirs: Vec<&str> = nogl
        .lines()
        .map(str::trim)
        .filter(|l| l.starts_with('/'))
        .collect();
    assert_eq!(ours.len(), 4, "{text}");
    for d in &ours {
        assert!(theirs.contains(&d.as_str()), "{d} not in {theirs:?}");
    }
}

#[test]
fn sources_options_and_types() {
    let text = hexcavator(&["ltcg", "-all", "ltcg.obj"]);
    assert!(
        text.contains("    R:\\tools\\coff\\ltcg.cpp\n      SHA-256 "),
        "{text}"
    );
    assert!(text.contains("\\ucrt\\stdio.h\n      SHA-256 "));
    assert!(text.contains("  Back end's command line (15 arguments)\n    -typedil\n"));
    assert!(text.contains("    -FoR:\\testdata\\coff\\ltcg.obj\n"));
    assert!(text.contains("    .cil$db "));
    assert!(text.contains("LF_STRUCTURE Square size 16\n"), "{text}");
    assert!(text.contains("LF_CLASS") || text.contains("LF_STRUCTURE Shape size 8\n"));
    assert!(text.contains("LF_BUILDINFO"));
    // A library of them: each member.
    let lib = hexcavator(&["ltcg", "-headers", "ltcg.lib"]);
    assert!(lib.starts_with("LTCG object ltcg.lib(ltcg.obj)\n"), "{lib}");
    let info = hexcavator(&["info", "ltcg.obj"]);
    assert!(info.contains("Visual C++ LTCG object"), "{info}");
}

#[test]
fn not_ltcg() {
    let out = Command::new(env!("CARGO_BIN_EXE_hexcavator"))
        .args(["ltcg", "objects.obj"])
        .current_dir(coff())
        .output()
        .unwrap();
    assert!(!out.status.success());
    assert!(String::from_utf8_lossy(&out.stderr).contains("not an LTCG object"));
}

/// What each function refers to, as `tools/coff/ltcg.cpp` has it:
/// `exported` calls (directly and virtually), reads variables, and takes
/// its string literals' addresses; `helper` uses `hidden`.
#[test]
fn references() {
    let text = hexcavator(&["ltcg", "-refs", "ltcg.obj"]);
    let of = |function: &str| -> Vec<String> {
        let mut lines = text.lines().skip_while(|l| l.trim() != function).skip(1);
        let mut out = Vec::new();
        for l in lines.by_ref() {
            if !l.starts_with("      ") {
                break;
            }
            out.push(l.trim().to_string());
        }
        out
    };
    let e = of("exported");
    for want in [
        "calls printf",
        "uses ?counter@@3HA",
        "uses ?banner@@3QBDB",
        "uses ?elsewhere@@3HA",
        "calls ?imported@@YAHH@Z",
        "calls virtually ?area@Shape@@UEBAHXZ",
        "calls ??0Square@@QEAA@H@Z",
        "calls ?helper@@YAHH@Z",
        "calls ?fail@@YAXPEBD@Z",
        "calls ?callee_pops@@YAHHH@Z",
        "calls ??$twice@H@@YAHH@Z",
        "takes the address of ??_C@_08JKGPMEKK@negative@",
    ] {
        assert!(e.iter().any(|l| l == want), "{want}: {e:#?}");
    }
    assert_eq!(of("?helper@@YAHH@Z"), ["uses ?hidden@@3PAHA"]);
    assert!(of("??$twice@H@@YAHH@Z").is_empty());
    assert_eq!(of("??0Square@@QEAA@H@Z").len(), 3, "{text}");
}

/// Each function's IL: the bodies follow one another in `.cil$ex` after
/// its header, so their sizes add up to the rest of it.
#[test]
fn function_bodies_tile_the_il() {
    let data = std::fs::read(coff().join("ltcg.obj")).unwrap();
    let obj = turbonop_pe::ltcg::LtcgObject::parse(&data).unwrap();
    let g = obj.globals().unwrap();
    let il = obj.stream(b".cil$ex").unwrap();
    let bodies: Vec<_> = g.symbols.iter().filter_map(|s| s.il).collect();
    assert_eq!(bodies.len(), 52);
    let sizes: u64 = g
        .il_sizes(il.len())
        .into_iter()
        .flatten()
        .map(u64::from)
        .sum();
    let first = bodies.iter().map(|b| b.il_offset).min().unwrap();
    assert_eq!(sizes, (il.len() as u64) - u64::from(first));
    // (In the records' order; and each function's parameters, `this` too.)
    let offsets: Vec<u32> = bodies.iter().map(|b| b.il_offset).collect();
    assert!(offsets.is_sorted());
    let params = |name: &[u8]| {
        g.symbols
            .iter()
            .find(|s| s.name == name)
            .and_then(|s| s.il)
            .unwrap()
            .params
    };
    assert_eq!(params(b"??0Square@@QEAA@H@Z"), 2);
    assert_eq!(params(b"?callee_pops@@YAHHH@Z"), 2);
    assert_eq!(params(b"??1Shape@@UEAA@XZ"), 1);
}

/// testdata/ltcg-il: tools/ltcg-il/records.cpp and records.c (see its
/// build.cmd), for x64, ARM64, and x86.
fn il_fixtures() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../testdata/ltcg-il")
}

const IL_OBJECTS: [&str; 4] = [
    "records.obj",
    "records-a64.obj",
    "records-c.obj",
    "records-x86.obj",
];

fn il_hexcavator(args: &[&str]) -> String {
    let out = Command::new(env!("CARGO_BIN_EXE_hexcavator"))
        .args(args)
        .current_dir(il_fixtures())
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "{args:?}: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8(out.stdout).unwrap()
}

/// Every record of every section is walked, `.cil$ex`'s units agree with
/// `.cil$sy`'s streams, and each function's record names its unit and its
/// stream.
#[test]
fn il_sections_walked_whole() {
    for name in IL_OBJECTS
        .into_iter()
        .chain(["../coff/ltcg.obj", "../coff/ltcg-x86.obj"])
    {
        let data = std::fs::read(il_fixtures().join(name)).unwrap();
        let obj = turbonop_pe::ltcg::LtcgObject::parse(&data).unwrap();
        let g = obj.globals().unwrap();
        assert_eq!(g.walked, Ok(()), "{name}");
        let locals = obj.locals().unwrap();
        assert_eq!(locals.walked, Ok(()), "{name}");
        assert_eq!(obj.initializers().unwrap().walked, Ok(()), "{name}");
        let units = obj.il_units();
        assert!(units.agrees_with(&locals), "{name}");
        let mut bodies = 0;
        for s in &g.symbols {
            if let Some(il) = s.il {
                let k = units.index_of(il.il_offset).unwrap();
                assert_eq!(locals.functions[k].offset, il.symbols_offset as usize);
                bodies += 1;
            }
        }
        // Functions nothing refers to keep their IL, but no record (the
        // C++ sources have some).
        assert!(units.len() >= bodies, "{name}");
        if name.ends_with("records.obj") || name.ends_with("a64.obj") || name.ends_with("/ltcg.obj") {
            assert!(units.len() > bodies, "{name}");
        }
        // The spans tile the table after its header.
        let mut at = 30;
        for &(start, end, _) in &g.spans {
            assert_eq!(start, at, "{name}");
            at = end;
        }
        assert_eq!(at, obj.stream(b".cil$gl").unwrap().len());
    }
}

/// The public definitions are exactly what lib.exe lists (inline functions
/// only if something emitted refers to them), for each architecture.
#[test]
fn il_publics_are_libs_index() {
    for name in IL_OBJECTS {
        let text = il_hexcavator(&["ltcg", "-publics", name]);
        let ours: BTreeSet<String> = text
            .lines()
            .filter(|l| l.contains("  function  ") || l.contains("  data  "))
            .map(|l| l[70..].split(" (").next().unwrap().to_string())
            .collect();
        let index =
            std::fs::read_to_string(il_fixtures().join(name.replace(".obj", ".linkermember.txt")))
                .unwrap();
        let list = index.split("public symbols").nth(1).unwrap();
        let theirs: BTreeSet<String> = list
            .lines()
            .map(str::trim)
            .skip_while(|l| l.is_empty())
            .take_while(|l| !l.is_empty())
            .map(|l| l.split_whitespace().nth(1).unwrap().to_string())
            // (x86 C names: lib.exe decorates them; the IL doesn't.)
            .map(|n| {
                if name.contains("x86") && !n.starts_with('?') {
                    n.trim_start_matches(['_', '@']).to_string()
                } else {
                    n
                }
            })
            .collect();
        assert!(theirs.len() > 5, "{name}");
        assert_eq!(ours, theirs, "{name}");
    }
    let text = il_hexcavator(&["ltcg", "-symbols", "records.obj"]);
    assert!(!text.contains("unused_"), "{text}");
}

/// The other records: sections, namespaces, named types, constants, and
/// attributes with what they apply to.
#[test]
fn il_records() {
    let text = il_hexcavator(&["ltcg", "-records", "records.obj"]);
    for want in [
        ".mine               flags 01  characteristics C0000040",
        ".CRT$XCU     DATA",
        "\n    geometry\n",
        "  geometry::Square\n",
        "70000        ?limit@@3HB",
        "[[deprecated(\"use checked\")]]  ?old_way@@YAHH@Z",
        "[[nodiscard]]  ?checked@@YAHH@Z",
        "[[noreturn]]  ?fail@@YAXPEBD@Z",
    ] {
        assert!(text.contains(want), "{want}: {text}");
    }
}

/// A function's scopes and locals, and the variables' initializers.
#[test]
fn il_locals_and_initializers() {
    let text = il_hexcavator(&["ltcg", "-locals", "records.obj"]);
    let at = text.find("?locals@@YAHHH@Z, IL at").unwrap();
    let f: Vec<&str> = text[at..]
        .lines()
        .skip(1)
        .take_while(|l| l.starts_with("      "))
        .map(str::trim)
        .collect();
    for want in [
        "label done",
        "parameter a (4 bytes, type 0x74, scope 1)",
        "parameter b (4 bytes, type 0x74, scope 1)",
        "local sum (4 bytes, type 0x74, scope 2)",
        "static ?calls@?1??locals@@YAHHH@Z@4HA (4 bytes, global 677)",
    ] {
        assert!(f.contains(&want), "{want}: {f:#?}");
    }
    assert!(f.iter().any(|l| l.starts_with("local inner ")), "{f:#?}");
    let text = il_hexcavator(&["ltcg", "-init", "records.obj"]);
    for want in [
        "    ?counter@@3HA (4 bytes)\n      integer 4: 0x5\n",
        "    ?hidden@@3PAHA (160 bytes)\n      integer 4: 0x1\n      integer 4: 0x2\n      integer 4: 0x3\n      zeros 148\n",
        "    ?pointer_to@@3PEAHEA (8 bytes)\n      address 8: ?counter@@3HA + 0x0\n",
        "    ?big_value@@3_JA (8 bytes)\n      integer 8: 0x123456789ab\n",
        "    ?ratio@@3NA (8 bytes)\n      float 8: 000000000000f83f\n",
        "    ?pair@@3UPair@@A (16 bytes)\n      integer 1: 0x1\n      zeros 3\n      integer 4: 0x2\n      integer 8: 0x3\n",
        "    ?limit@@3HB (4 bytes)\n      integer 4: 0x11170\n",
        "address 8: ??__Eglobal@@YAXXZ + 0x0",
    ] {
        assert!(text.contains(want), "{want}: {text}");
    }
}

/// Damaged objects: every section cut short or a byte changed, read
/// without a panic.
#[test]
fn il_hostile() {
    let data = std::fs::read(il_fixtures().join("records.obj")).unwrap();
    let obj = turbonop_pe::ltcg::LtcgObject::parse(&data).unwrap();
    for name in [&b".cil$gl"[..], b".cil$sy", b".cil$in", b".cil$ex"] {
        let s = obj.stream(name).unwrap();
        let mut m = s.to_vec();
        for i in (0..s.len()).step_by(3) {
            for v in [0x00, 0x80, 0xFF] {
                m[i] = v;
                let _ = turbonop_pe::ltcg::Globals::parse(&m).map(|g| g.lib_listed());
                let _ = turbonop_pe::ltcg::Locals::parse(&m);
                let _ = turbonop_pe::ltcg::Initializers::parse(&m);
                let _ = turbonop_pe::ltcg::IlUnits::find(&m);
            }
            m[i] = s[i];
            let _ = turbonop_pe::ltcg::Globals::parse(&s[..i]);
            let _ = turbonop_pe::ltcg::Locals::parse(&s[..i]);
            let _ = turbonop_pe::ltcg::Initializers::parse(&s[..i]);
        }
    }
    for n in (0..data.len()).step_by(97) {
        if let Ok(o) = turbonop_pe::ltcg::LtcgObject::parse(&data[..n]) {
            let _ = o.globals();
            let _ = o.locals();
            let _ = o.initializers();
        }
    }
}
