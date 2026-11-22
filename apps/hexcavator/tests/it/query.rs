//! `hexc query` end to end, over the cross-reference test program
//! (testdata/xref).

use std::path::{Path, PathBuf};
use std::process::Command;

fn fixture() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../testdata/xref/xref-x64.exe")
}

fn hexc(args: &[&str]) -> (i32, String, String) {
    let out = Command::new(env!("CARGO_BIN_EXE_hexc"))
        .arg("query")
        .args(args)
        .output()
        .unwrap();
    (
        out.status.code().unwrap_or(-1),
        String::from_utf8(out.stdout).unwrap(),
        String::from_utf8(out.stderr).unwrap(),
    )
}

fn query(q: &str, extra: &[&str]) -> String {
    let image = fixture();
    let mut args = extra.to_vec();
    args.push(image.to_str().unwrap());
    args.push(q);
    let (code, out, err) = hexc(&args);
    assert_eq!(code, 0, "{q}: {err}");
    out
}

#[test]
fn filters_and_selects() {
    assert_eq!(
        query(
            "name~'^(inner|outer)\\(' select name, kind, section sort by name",
            &[]
        ),
        "name        kind      section\n\
         inner(int)  function  .text\n\
         outer(int)  function  .text\n\
         (2 of 2 matching rows)\n"
    );
    // The CSV and JSON forms.
    let csv = query("name='op_add(int)' select name, compiland", &["-csv"]);
    assert!(
        csv.starts_with("name,compiland\nop_add(int),") && csv.ends_with("main.obj\n"),
        "{csv}"
    );
    assert_eq!(csv.lines().count(), 2);
    let v: serde_json::Value = serde_json::from_str(&query(
        "kind=function not folded name~'^Ghost::' sort by name",
        &["-json"],
    ))
    .unwrap();
    assert_eq!(v["matched"], 4);
    let names: Vec<&str> = v["rows"]
        .as_array()
        .unwrap()
        .iter()
        .map(|r| r["name"].as_str().unwrap())
        .collect();
    assert_eq!(
        names,
        [
            "Ghost::Ghost()",
            "Ghost::`scalar deleting destructor'(unsigned int)",
            "Ghost::area() const",
            "Ghost::name() const"
        ]
    );
    assert_eq!(v["rows"][0]["folded"], false);
    assert!(v["rows"][0]["file"].as_u64().unwrap() > 0);
}

#[test]
fn grouping() {
    let v: serde_json::Value = serde_json::from_str(&query(
        "kind=function not folded compiland~'\\\\(main|dead)\\.obj$' group by compiland sort by count desc",
        &["-json"],
    ))
    .unwrap();
    let rows = v["rows"].as_array().unwrap();
    assert_eq!(rows.len(), 2);
    assert!(
        rows[0]["compiland"].as_str().unwrap().ends_with("main.obj"),
        "{v}"
    );
    assert!(
        rows[1]["compiland"].as_str().unwrap().ends_with("dead.obj"),
        "{v}"
    );
    // dead.obj: dead_entry, dead_helper, live_in_dead_object, and Ghost's
    // constructor, scalar deleting destructor, and two virtual functions.
    assert_eq!(rows[1]["count"], 7, "{v}");
    let sum: u64 = rows.iter().map(|r| r["count"].as_u64().unwrap()).sum();
    assert_eq!(v["matched"].as_u64().unwrap(), sum);
    for r in rows {
        assert!(r["file"].as_u64().unwrap() > 0 && r["vm"].as_u64().is_some());
    }
    let out = query("section=.text group by section", &[]);
    let lines: Vec<&str> = out.lines().collect();
    assert_eq!(lines.len(), 3, "{out}");
    assert_eq!(
        lines[0].split_whitespace().collect::<Vec<_>>(),
        ["section", "count", "file", "vm"]
    );
    assert!(lines[1].starts_with(".text "), "{out}");
}

#[test]
fn errors_and_help() {
    let image = fixture();
    let image = image.to_str().unwrap();
    let (code, _, err) = hexc(&[image, "size>1k"]);
    assert_eq!(code, 2);
    assert!(err.contains("no column 'size'"), "{err}");
    assert!(err.contains("\n  size>1k\n  ^\n"), "{err}");
    let (code, _, err) = hexc(&[image, "file>big"]);
    assert_eq!(code, 2);
    assert!(err.contains("\n  file>big\n       ^\n"), "{err}");
    let (code, _, err) = hexc(&[image]);
    assert_eq!(code, 2);
    assert!(err.contains("no query given"), "{err}");
    let (code, _, err) = hexc(&[image, "kind=function", "extra"]);
    assert_eq!(code, 2);
    assert!(err.contains("too many arguments"), "{err}");
    let (code, _, err) = hexc(&[image, "sort by nope"]);
    assert_eq!(code, 2);
    assert!(err.contains("can't sort by 'nope'"), "{err}");
    // A stripped image with no debug file to be found.
    let elf = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../testdata/elf/program-stripped");
    let (code, _, err) = hexc(&[elf.to_str().unwrap(), "kind=function"]);
    assert_eq!(code, 1);
    assert!(err.contains("no matching debug information found"), "{err}");
    let (code, out, _) = hexc(&["-columns"]);
    assert_eq!(code, 0);
    assert!(
        out.contains("  name       the symbol's name, demangled\n"),
        "{out}"
    );
    assert_eq!(out.lines().count(), 13);
}

/// ELF and Mach-O images, with DWARF: in the image, or given.
#[test]
fn elf_and_macho_with_dwarf() {
    let data = |p: &str| {
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../testdata")
            .join(p)
            .to_str()
            .unwrap()
            .to_string()
    };
    let elf = data("elf/dwarf5-x86_64-linux-gnu");
    let (code, out, err) = hexc(&[&elf, "kind=function sort by name", "-csv"]);
    assert_eq!(code, 0, "{err}");
    let names: Vec<&str> = out
        .lines()
        .skip(1)
        .map(|l| l.split(',').next().unwrap())
        .collect();
    assert_eq!(
        names,
        [
            "_start()",
            "compute(int)",
            "optional_hook()",
            "second_len()",
            "second_sum(int)"
        ]
    );
    // Each in its compile unit and source file.
    assert!(
        out.lines()
            .any(|l| l.starts_with("compute(int),") && l.contains("program.c"))
    );
    assert!(
        out.lines()
            .any(|l| l.starts_with("second_sum(int),") && l.contains("second.c"))
    );
    // A dSYM's DWARF, given.
    let image = data("macho/program-arm64");
    let dwarf = data("macho/program-arm64.dwarf");
    let (code, out, err) = hexc(&[&image, "kind=function", "-pdb", &dwarf, "-csv"]);
    assert_eq!(code, 0, "{err}");
    assert!(out.lines().any(|l| l.starts_with("compute(int),")), "{out}");
    // Not another image's.
    let other = data("macho/program-x86_64.dwarf");
    let (code, _, err) = hexc(&[&image, "kind=function", "-pdb", &other]);
    assert_eq!(code, 1);
    assert!(err.contains("does not match"), "{err}");
}

/// With DWARF, the image's own symbols name what the DWARF doesn't
/// describe, as publics (as a PDB's publics do): an ELF image's typeinfo
/// (which the compiler emits without DWARF), and a WebAssembly module's
/// function wasm-ld made.
#[test]
fn image_symbols_beside_dwarf() {
    let testdata = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../testdata");
    for (image, name) in [
        ("types/types-x86_64-linux-gnu", "typeinfo for Derived"),
        ("wasm/startup.wasm", "__wasm_call_ctors"),
    ] {
        let path = testdata.join(image);
        let (code, out, err) = hexc(&[
            "-json",
            path.to_str().unwrap(),
            "kind=public select name, compiland, file",
        ]);
        assert_eq!(code, 0, "{image}: {err}");
        let v: serde_json::Value = serde_json::from_str(&out).unwrap();
        let row = v["rows"]
            .as_array()
            .unwrap()
            .iter()
            .find(|r| r["name"] == name)
            .unwrap_or_else(|| panic!("{image}: {v}"));
        assert!(row["file"].as_u64().unwrap() > 0, "{image}: {row}");
    }
}

/// A WebAssembly module's data: each variable in its segment, with its
/// compile unit; and `hexc data` reading one at its address in linear
/// memory, and `hexc recommend` finding everything reached (code's
/// constant addresses and offsets are references to data).
#[test]
fn wasm_data() {
    let image = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../testdata/wasm/data.wasm");
    // (Each command: its options, the image, what follows it.)
    let run = |args: &[&str], after: &[&str]| {
        let out = Command::new(env!("CARGO_BIN_EXE_hexc"))
            .args(args)
            .arg(&image)
            .args(after)
            .output()
            .unwrap();
        assert!(
            out.status.success(),
            "{}",
            String::from_utf8_lossy(&out.stderr)
        );
        String::from_utf8(out.stdout).unwrap()
    };
    let rows: serde_json::Value =
        serde_json::from_str(&run(&["query", "-json"], &["kind=data"])).unwrap();
    let row = |name: &str| {
        rows["rows"]
            .as_array()
            .unwrap()
            .iter()
            .find(|r| r["name"] == name)
            .unwrap_or_else(|| panic!("{name}: {rows}"))
            .clone()
    };
    for (name, section) in [
        ("primes", ".rodata"),
        ("twice_b", ".rodata"),
        ("counter", ".data"),
    ] {
        let r = row(name);
        assert_eq!(r["section"], section, "{r}");
        assert!(r["compiland"].as_str().unwrap().ends_with("data.c"), "{r}");
    }
    let text = run(&["data"], &["counter"]);
    assert!(
        text.starts_with("counter  (4 bytes at 0x100bc, .data)"),
        "{text}"
    );
    assert!(text.contains("int = 42"), "{text}");
    let text = run(&["recommend"], &[]);
    assert!(
        text.contains("the link did collect unreferenced functions"),
        "{text}"
    );
    assert!(!text.contains("--no-gc-sections"), "{text}");
}

/// A WebAssembly module's data where code puts it: with shared memory,
/// its passive segments where `__wasm_init_memory` copies them, and the
/// `.bss` it zeroes; as a position-independent module, its segment and
/// `.bss` from its `__memory_base` (as 0), and its variables' addresses,
/// DWARF's, from the same base. Each variable reads as it is.
#[test]
fn wasm_data_placed_by_code() {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../testdata/wasm");
    let data = |module: &str, name: &str| {
        let out = Command::new(env!("CARGO_BIN_EXE_hexc"))
            .arg("data")
            .arg(dir.join(module))
            .arg(name)
            .output()
            .unwrap();
        assert!(
            out.status.success(),
            "{module} {name}: {}",
            String::from_utf8_lossy(&out.stderr)
        );
        String::from_utf8(out.stdout).unwrap()
    };
    // (As wasm-ld's map for the shared module says.)
    for (module, name, head, value) in [
        (
            "data-shared.wasm",
            "primes",
            "primes  (32 bytes at 0x10000, .rodata)",
            "[7] = 19",
        ),
        (
            "data-shared.wasm",
            "counter",
            "counter  (4 bytes at 0x100bc, .data)",
            "int = 42",
        ),
        (
            "data-shared.wasm",
            "scratch",
            "scratch  (64 bytes at 0x100c0, .bss)",
            "[15] = 0",
        ),
        (
            "data-pic.wasm",
            "primes",
            "primes  (32 bytes at 0x0, .data)",
            "[7] = 19",
        ),
        (
            "data-pic.wasm",
            "counter",
            "counter  (4 bytes at 0xbc, .data)",
            "int = 42",
        ),
        (
            "data-pic.wasm",
            "scratch",
            "scratch  (64 bytes at 0xc0, .bss)",
            "[15] = 0",
        ),
    ] {
        let text = data(module, name);
        assert!(text.starts_with(head), "{module} {name}: {text}");
        assert!(text.contains(value), "{module} {name}: {text}");
    }
}

/// An ELF object's functions and data where a linker would lay its
/// sections out (an object's addresses are all its sections' offsets):
/// each at its own address, in its section, with its source line; and a
/// datum read there.
#[test]
fn object_symbols_laid_out() {
    let object =
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../../testdata/elf/program-x86_64-linux-gnu.o");
    let run = |args: &[&str]| {
        let out = Command::new(env!("CARGO_BIN_EXE_hexc"))
            .args(&args[..1])
            .arg(&object)
            .args(&args[1..])
            .output()
            .unwrap();
        assert!(
            out.status.success(),
            "{}",
            String::from_utf8_lossy(&out.stderr)
        );
        String::from_utf8(out.stdout).unwrap()
    };
    let rows: serde_json::Value =
        serde_json::from_str(&run(&["query", "-json", "kind=function or kind=data"])).unwrap();
    let rows = rows["rows"].as_array().unwrap();
    let row = |name: &str| {
        rows.iter()
            .find(|r| r["name"] == name)
            .unwrap_or_else(|| panic!("{name}: {rows:?}"))
    };
    for (name, section) in [
        ("compute(int)", ".text"),
        ("_start()", ".text"),
        ("counter", ".data"),
        ("buffer", ".bss"),
    ] {
        assert_eq!(row(name)["section"], section, "{name}");
    }
    assert_eq!(row("compute(int)")["line"], 15);
    // (No two at one address.)
    let mut at: Vec<u64> = rows.iter().filter_map(|r| r["rva"].as_u64()).collect();
    let n = at.len();
    at.sort_unstable();
    at.dedup();
    assert_eq!(at.len(), n, "{rows:?}");
    let text = run(&["data", "counter"]);
    assert!(
        text.contains(".data)") && text.contains("int = 3"),
        "{text}"
    );
}

/// Runs `hexc <command> <testdata/path> <args>`, which must succeed.
fn run_on(command: &str, path: &str, args: &[&str]) -> String {
    let file = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../testdata")
        .join(path);
    let out = Command::new(env!("CARGO_BIN_EXE_hexc"))
        .arg(command)
        .arg(&file)
        .args(args)
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "{command} {path}: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8(out.stdout).unwrap()
}

/// `hexc query -json <path> <q>`'s rows.
fn rows_of(path: &str, q: &str) -> Vec<serde_json::Value> {
    let rows: serde_json::Value =
        serde_json::from_str(&run_on("query", path, &["-json", q])).unwrap();
    rows["rows"].as_array().unwrap().clone()
}

/// A COFF object's CodeView is its debug information (read as the PDB a
/// linker would write of it): its functions with their source lines, a
/// function's static, vtables as large as their publics say, the
/// functions inlined at an address, and data read as its type.
#[test]
fn coff_object_codeview() {
    let rows = rows_of("coff/objects.obj", "kind!=public");
    let row = |name: &str| {
        rows.iter()
            .find(|r| r["name"] == name)
            .unwrap_or_else(|| panic!("{name}: {rows:?}"))
    };
    for (name, line) in [
        ("exported(int)", 26),
        ("use_statics(int)", 22),
        ("Square::area() const", 17),
        ("twice<int>(int)", 8),
    ] {
        assert_eq!(row(name)["line"], line, "{name}");
        assert_eq!(row(name)["source"], r"R:\tools\coff\objects.cpp", "{name}");
        assert!(
            row(name)["section"]
                .as_str()
                .unwrap()
                .starts_with(".text$mn"),
            "{name}"
        );
    }
    assert_eq!(
        row("calls (in int use_statics(int n))")["section"],
        ".bss (2)"
    );
    assert_eq!(row("Square::`vftable'")["vm"], 16);
    assert_eq!(row("counter")["section"], ".bss");
    // (Its publics name what CodeView doesn't: RTTI.)
    let publics = rows_of("coff/objects.obj", "kind=public");
    assert!(
        publics
            .iter()
            .any(|r| r["name"] == "Square::`RTTI Base Class Array'"),
        "{publics:?}"
    );
    // exported's code at 0x28 is use_statics', inlined.
    let at = row("exported(int)")["rva"].as_u64().unwrap() + 0x28;
    let text = run_on("symbolize", "coff/objects.obj", &[&format!("{at:#x}")]);
    assert!(
        text.contains(r"use_statics    R:\tools\coff\objects.cpp:23  (inlined)")
            && text.contains(r"exported+0x28  R:\tools\coff\objects.cpp:29"),
        "{text}"
    );
    let text = run_on("data", "coff/objects.obj", &["table"]);
    assert!(text.contains("[2] = 3  (int)"), "{text}");
    let text = run_on("types", "coff/objects.obj", &[]);
    assert!(text.contains("Square") && text.contains("Shape"), "{text}");
    // /Zi's (its types in objects-zi.pdb, its type server, found beside
    // it: where it says it is, R:, isn't), the same functions, by type.
    let names = |path: &str| -> Vec<(String, String)> {
        let mut v: Vec<(String, String)> = rows_of(path, "kind=function")
            .iter()
            .map(|r| {
                (
                    r["name"].as_str().unwrap().to_string(),
                    r["line"].to_string(),
                )
            })
            .collect();
        v.sort();
        v
    };
    assert_eq!(names("coff/objects-zi.obj"), names("coff/objects.obj"));
    let text = run_on("types", "coff/objects-zi.obj", &[]);
    assert!(text.contains("Square") && text.contains("Shape"), "{text}");
    // clang's, too.
    let rows = rows_of("mingw/weak.obj", "kind=function");
    assert_eq!(rows.len(), 1, "{rows:?}");
    assert_eq!(rows[0]["line"], 12);
}

/// A MinGW object's DWARF, relocated as a linker would: each function in
/// its section with its source line, and data read where it is.
#[test]
fn mingw_object_dwarf() {
    for object in ["mingw/program.o", "mingw/program-arm64.o"] {
        let rows = rows_of(object, "kind=function or kind=data");
        let row = |name: &str| {
            rows.iter()
                .find(|r| r["name"] == name)
                .unwrap_or_else(|| panic!("{object} {name}: {rows:?}"))
        };
        for (name, section) in [
            ("used(int)", ".text$used"),
            ("unused(int)", ".text$unused"),
            ("start()", ".text$start"),
            ("table", ".data$table"),
            ("message", ".rdata$message"),
        ] {
            assert_eq!(row(name)["section"], section, "{object} {name}");
        }
        assert_eq!(row("start()")["line"], 15, "{object}");
        assert_eq!(row("unused(int)")["source"], r"R:\tools\mingw\program.c");
        let text = run_on("data", object, &["table"]);
        assert!(text.contains("[3] = 4"), "{object}: {text}");
        let at = row("unused(int)")["rva"].as_u64().unwrap();
        let text = run_on("symbolize", object, &[&format!("{at:#x}")]);
        assert!(
            text.contains(r"unused  R:\tools\mingw\program.c:9"),
            "{object}: {text}"
        );
    }
}

/// A WebAssembly object's data segments, each where the object's own
/// layout puts it (named as its `linking` section names them), and its
/// data read there.
#[test]
fn wasm_object_data_placed() {
    let rows = rows_of("wasm/data.o", "kind=function or kind=data");
    let row = |name: &str| {
        rows.iter()
            .find(|r| r["name"] == name)
            .unwrap_or_else(|| panic!("{name}: {rows:?}"))
    };
    for (name, section) in [
        ("primes", ".rodata.primes"),
        ("counter", ".data.counter"),
        ("scratch", ".bss.scratch"),
        ("lookup(int)", "Code"),
    ] {
        assert_eq!(row(name)["section"], section, "{name}");
    }
    let text = run_on("data", "wasm/data.o", &["counter"]);
    assert!(text.contains("int = 42"), "{text}");
    let text = run_on("data", "wasm/data.o", &["primes"]);
    assert!(text.contains("[7] = 19"), "{text}");
    let text = run_on("symbolize", "wasm/data.o", &["0x10"]);
    assert!(
        text.contains(r"lookup+0xe  R:\tools\wasm\data.c:20"),
        "{text}"
    );
}

/// A WebAssembly module's `.bss` without shared memory (in no segment):
/// where its DWARF's variables past the segments are, as wasm-ld's map
/// says (`.bss` at 0x100c0, 0x40 bytes), zeroed.
#[test]
fn wasm_bss_from_debug_information() {
    let rows = rows_of("wasm/data.wasm", "kind=data");
    let scratch = rows.iter().find(|r| r["name"] == "scratch").unwrap();
    assert_eq!(scratch["section"], ".bss");
    let text = run_on("data", "wasm/data.wasm", &["scratch"]);
    assert!(
        text.starts_with("scratch  (64 bytes at 0x100c0, .bss)"),
        "{text}"
    );
    assert!(text.contains("[15] = 0  (int)"), "{text}");
    let text = run_on("size", "wasm/data.wasm", &[]);
    assert!(
        text.lines()
            .any(|l| l.split_whitespace().collect::<Vec<_>>() == [".bss", "0", "64", "0"]),
        "{text}"
    );
}
