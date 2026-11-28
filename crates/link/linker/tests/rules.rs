//! link.exe's rules that turbolink follows, each checked on a small link of
//! objects (and libraries) made here: what link.exe 14.52 does with the same
//! inputs, found by experiment, is what each test expects.

use turbonop_global_alloc as _;

use std::path::{Path, PathBuf};

use turbonop_linker::{args, config, link};

/// A symbol of a made object: its name, and its section (1-based; 0 is
/// undefined).
struct Sym(&'static str, i16);

/// An x64 object of one `.text` section (`text`, its relocations: offset,
/// symbol index, type) and `syms`, all external.
fn object(text: &[u8], relocations: &[(u32, u32, u16)], syms: &[Sym]) -> Vec<u8> {
    let syms: Vec<Typed> = syms
        .iter()
        .map(|&Sym(name, section)| Typed {
            name,
            section,
            value: 0,
            ty: if section > 0 { 0x20 } else { 0 },
            class: 2,
        })
        .collect();
    object_typed(text, relocations, &syms)
}

/// A symbol of a made object, in full: its name, section (1-based; 0
/// undefined, -1 absolute), value, type (0x20 a function) and storage
/// class (2 external, 3 static).
#[derive(Clone, Copy)]
struct Typed {
    name: &'static str,
    section: i16,
    value: u32,
    ty: u16,
    class: u8,
}

/// An x64 object of one `.text` section (`text`, its relocations: offset,
/// symbol index, type) and `syms`.
fn object_typed(text: &[u8], relocations: &[(u32, u32, u16)], syms: &[Typed]) -> Vec<u8> {
    let mut strings = Vec::new();
    let mut records = Vec::new();
    for &Typed {
        name,
        section,
        value,
        ty,
        class,
    } in syms
    {
        let n = name.as_bytes();
        if n.len() <= 8 {
            let mut short = [0u8; 8];
            short[..n.len()].copy_from_slice(n);
            records.extend_from_slice(&short);
        } else {
            records.extend_from_slice(&[0; 4]);
            records.extend_from_slice(&(4 + n32(strings.len())).to_le_bytes());
            strings.extend_from_slice(n);
            strings.push(0);
        }
        records.extend_from_slice(&value.to_le_bytes());
        records.extend_from_slice(&section.to_le_bytes());
        records.extend_from_slice(&ty.to_le_bytes());
        records.push(class);
        records.push(0);
    }
    let text_at = 20 + 40;
    let relocations_at = text_at + text.len();
    let symbols_at = relocations_at + 10 * relocations.len();
    let mut o = Vec::new();
    o.extend_from_slice(&0x8664u16.to_le_bytes());
    o.extend_from_slice(&1u16.to_le_bytes());
    o.extend_from_slice(&0u32.to_le_bytes());
    o.extend_from_slice(&n32(symbols_at).to_le_bytes());
    o.extend_from_slice(&n32(syms.len()).to_le_bytes());
    o.extend_from_slice(&[0; 4]);
    o.extend_from_slice(b".text\0\0\0");
    for v in [
        0u32,
        0,
        n32(text.len()),
        n32(text_at),
        if relocations.is_empty() {
            0
        } else {
            n32(relocations_at)
        },
        0,
    ] {
        o.extend_from_slice(&v.to_le_bytes());
    }
    o.extend_from_slice(&u16::try_from(relocations.len()).unwrap().to_le_bytes());
    o.extend_from_slice(&0u16.to_le_bytes());
    o.extend_from_slice(&0x6050_0020u32.to_le_bytes());
    o.extend_from_slice(text);
    for &(at, symbol, kind) in relocations {
        o.extend_from_slice(&at.to_le_bytes());
        o.extend_from_slice(&symbol.to_le_bytes());
        o.extend_from_slice(&kind.to_le_bytes());
    }
    o.extend_from_slice(&records);
    o.extend_from_slice(&(4 + n32(strings.len())).to_le_bytes());
    o.extend_from_slice(&strings);
    o
}

/// An object that defines `name` (a `ret`).
fn defining(name: &'static str) -> Vec<u8> {
    object(&[0xC3], &[], &[Sym(name, 1)])
}

/// An archive of `members` (short names, each defining the names given),
/// with both linker members.
fn archive(members: &[(&str, Vec<u8>, &[&str])]) -> Vec<u8> {
    let header = |name: &str, size: usize| {
        format!("{name:<16}{:<12}{:<6}{:<6}{:<8}{size:<10}`\n", 0, 0, 0, 644).into_bytes()
    };
    let symbols: Vec<(usize, &str)> = members
        .iter()
        .enumerate()
        .flat_map(|(i, (_, _, names))| names.iter().map(move |n| (i, *n)))
        .collect();
    let names_size: usize = symbols.iter().map(|(_, n)| n.len() + 1).sum();
    let first_size = 4 + 4 * symbols.len() + names_size;
    let second_size = 4 + 4 * members.len() + 4 + 2 * symbols.len() + names_size;
    let pad = |n: usize| n + (n & 1);
    let mut at = 8 + 60 + pad(first_size) + 60 + pad(second_size);
    let mut offsets = Vec::new();
    for (_, data, _) in members {
        offsets.push(n32(at));
        at += 60 + pad(data.len());
    }
    let mut a = b"!<arch>\n".to_vec();
    a.extend(header("/", first_size));
    a.extend_from_slice(&n32(symbols.len()).to_be_bytes());
    for (i, _) in &symbols {
        a.extend_from_slice(&offsets[*i].to_be_bytes());
    }
    for (_, n) in &symbols {
        a.extend_from_slice(n.as_bytes());
        a.push(0);
    }
    if a.len() % 2 == 1 {
        a.push(b'\n');
    }
    let mut sorted = symbols.clone();
    sorted.sort_by(|x, y| x.1.cmp(y.1));
    a.extend(header("/", second_size));
    a.extend_from_slice(&n32(members.len()).to_le_bytes());
    for o in &offsets {
        a.extend_from_slice(&o.to_le_bytes());
    }
    a.extend_from_slice(&n32(sorted.len()).to_le_bytes());
    for (i, _) in &sorted {
        a.extend_from_slice(&(u16::try_from(*i).unwrap() + 1).to_le_bytes());
    }
    for (_, n) in &sorted {
        a.extend_from_slice(n.as_bytes());
        a.push(0);
    }
    if a.len() % 2 == 1 {
        a.push(b'\n');
    }
    for (name, data, _) in members {
        a.extend(header(&format!("{name}/"), data.len()));
        a.extend_from_slice(data);
        if a.len() % 2 == 1 {
            a.push(b'\n');
        }
    }
    a
}

/// A scratch directory of its own for test `name`.
fn scratch(name: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("turbolink-rules-{}-{name}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

/// The command line's name for file `name` in `dir`: a relative path (an
/// absolute one starting with `/` would be an option).
fn input(dir: &Path, name: &str) -> String {
    let cwd = std::env::current_dir().unwrap();
    let path = dir.join(name);
    // Up from the current directory to what it shares with the path, then
    // down (on Windows, another drive: the path itself, which has no `/`).
    let (mut up, mut rest) = (PathBuf::new(), path.as_path());
    for a in cwd.ancestors() {
        if let Ok(r) = path.strip_prefix(a) {
            rest = r;
            break;
        }
        up.push("..");
    }
    up.join(rest).display().to_string()
}

/// Links `words` (a command line) in `dir`: the image's bytes, or the
/// errors' codes.
fn run(dir: &Path, words: &[&str]) -> Result<Vec<u8>, Vec<u32>> {
    let argv = words.iter().map(|w| {
        if w.starts_with('/') {
            (*w).to_owned()
        } else {
            input(dir, w)
        }
    });
    let words = args::command_line(argv, None, None, &|p| std::fs::read(p)).unwrap();
    let mut diags = Vec::new();
    let mut config = config::parse(&words, &mut diags);
    let out = dir.join("out.img");
    config.out = Some(out.clone());
    let outcome = link::link(config);
    let errors: Vec<u32> = outcome
        .diagnostics
        .iter()
        .filter(|d| !d.warning)
        .map(|d| d.code)
        .collect();
    if !errors.is_empty() || !outcome.written {
        return Err(errors);
    }
    Ok(std::fs::read(out).unwrap())
}

fn n32(v: usize) -> u32 {
    u32::try_from(v).unwrap()
}

fn u16_at(d: &[u8], o: usize) -> u16 {
    u16::from_le_bytes([d[o], d[o + 1]])
}

fn u32_at(d: &[u8], o: usize) -> u32 {
    u32::from_le_bytes(d[o..o + 4].try_into().unwrap())
}

/// The image's sections: (name, virtual size, RVA, raw size, file offset).
fn sections(d: &[u8]) -> Vec<(String, u32, u32, u32, u32)> {
    let pe = u32_at(d, 0x3C) as usize;
    let n = u16_at(d, pe + 6) as usize;
    let at = pe + 24 + u16_at(d, pe + 20) as usize;
    (0..n)
        .map(|i| {
            let h = at + 40 * i;
            let name = String::from_utf8_lossy(&d[h..h + 8])
                .trim_end_matches('\0')
                .to_owned();
            (
                name,
                u32_at(d, h + 8),
                u32_at(d, h + 12),
                u32_at(d, h + 16),
                u32_at(d, h + 20),
            )
        })
        .collect()
}

/// The bytes at `rva`.
fn at_rva(d: &[u8], rva: u32, n: usize) -> &[u8] {
    let (_, _, va, _, ptr) = sections(d)
        .into_iter()
        .find(|s| s.2 <= rva && rva < s.2 + s.1)
        .expect("a section holds it");
    let o = (ptr + rva - va) as usize;
    &d[o..o + n]
}

fn optional_header(d: &[u8]) -> usize {
    u32_at(d, 0x3C) as usize + 24
}

/// `main` calls `y_$fo$` (and names `x_$fo$` and `plain`, unreferenced).
fn function_override_inputs(dir: &Path) {
    let main = object(
        &[0xE8, 0, 0, 0, 0, 0xC3],
        &[(1, 3, 4)], // REL32 to y_$fo$
        &[
            Sym("main", 1),
            Sym("plain", 0),
            Sym("x_$fo$", 0),
            Sym("y_$fo$", 0),
        ],
    );
    std::fs::write(dir.join("main.obj"), main).unwrap();
    let lib = archive(&[
        ("plain.obj", defining("plain"), &["plain"]),
        ("x.obj", defining("x_$fo$"), &["x_$fo$"]),
        ("y.obj", defining("y_$fo$"), &["y_$fo$"]),
    ]);
    std::fs::write(dir.join("t.lib"), lib).unwrap();
}

/// A name ending in `_$fo$` is never searched for, and undefined it is the
/// image base, not an error: `main`'s call reaches RVA 0, and only
/// plain.obj is loaded (its `ret` after `main`'s code, at 0x1010).
#[test]
fn a_function_overrides_name_is_the_image_base() {
    let dir = scratch("fo");
    function_override_inputs(&dir);
    let image = run(
        &dir,
        &[
            "/NODEFAULTLIB",
            "/ENTRY:main",
            "/SUBSYSTEM:CONSOLE",
            "main.obj",
            "t.lib",
        ],
    )
    .expect("links");
    let text = &sections(&image)[0];
    assert_eq!(text.0, ".text");
    assert_eq!(text.1, 0x11, "main and plain.obj's ret only");
    let call = at_rva(&image, 0x1000, 0x11);
    let target = 0x1005i64 + i64::from(i32::from_le_bytes(call[1..5].try_into().unwrap()));
    assert_eq!(target, 0, "the call reaches the image base");
    assert_eq!(call[0x10], 0xC3);
    let _ = std::fs::remove_dir_all(&dir);
}

/// Even with an /ALTERNATENAME, a `_$fo$` name stays the image base.
#[test]
fn a_function_overrides_name_ignores_its_alternate_name() {
    let dir = scratch("fo-alternate");
    function_override_inputs(&dir);
    let image = run(
        &dir,
        &[
            "/NODEFAULTLIB",
            "/ENTRY:main",
            "/SUBSYSTEM:CONSOLE",
            "/ALTERNATENAME:y_$fo$=plain",
            "main.obj",
            "t.lib",
        ],
    )
    .expect("links");
    let call = at_rva(&image, 0x1000, 5);
    let target = 0x1005i64 + i64::from(i32::from_le_bytes(call[1..5].try_into().unwrap()));
    assert_eq!(target, 0);
    let _ = std::fs::remove_dir_all(&dir);
}

/// An ADDR64 to an undefined `_$fo$` name is the image base, with a base
/// relocation; an ADDR32NB is 0.
#[test]
fn a_function_overrides_address_is_the_base() {
    let dir = scratch("fo-addr64");
    let mut text = vec![0xC3];
    text.resize(20, 0);
    let main = object(
        &text,
        &[(8, 1, 1), (16, 1, 3)], // ADDR64, ADDR32NB
        &[Sym("main", 1), Sym("y_$fo$", 0)],
    );
    std::fs::write(dir.join("main.obj"), main).unwrap();
    let image = run(
        &dir,
        &[
            "/NODEFAULTLIB",
            "/ENTRY:main",
            "/SUBSYSTEM:CONSOLE",
            "main.obj",
        ],
    )
    .expect("links");
    let words = at_rva(&image, 0x1008, 12);
    assert_eq!(
        u64::from_le_bytes(words[..8].try_into().unwrap()),
        0x1_4000_0000
    );
    assert_eq!(u32_at(words, 8), 0);
    assert!(sections(&image).iter().any(|s| s.0 == ".reloc"));
    let _ = std::fs::remove_dir_all(&dir);
}

/// Other names with `$fo` in them are ordinary: searched for and loaded.
#[test]
fn only_the_suffix_makes_a_function_override() {
    let dir = scratch("fo-others");
    let names = ["z$fo$", "w_$fo$q", "v$FO$", "u_$fo_bdd$", "t$fo"];
    let mut text = Vec::new();
    let mut relocations = Vec::new();
    for i in 0..names.len() {
        relocations.push((1 + 5 * n32(i), 1 + n32(i), 4));
        text.extend_from_slice(&[0xE8, 0, 0, 0, 0]);
    }
    text.push(0xC3);
    let mut syms = vec![Sym("main", 1)];
    syms.extend(names.iter().map(|n| Sym(n, 0)));
    std::fs::write(dir.join("main.obj"), object(&text, &relocations, &syms)).unwrap();
    let members: Vec<(String, Vec<u8>, [&str; 1])> = names
        .iter()
        .enumerate()
        .map(|(i, n)| (format!("n{i}.obj"), defining(n), [*n]))
        .collect();
    let members: Vec<(&str, Vec<u8>, &[&str])> = members
        .iter()
        .map(|(m, d, n)| (m.as_str(), d.clone(), &n[..]))
        .collect();
    std::fs::write(dir.join("t.lib"), archive(&members)).unwrap();
    let image = run(
        &dir,
        &[
            "/NODEFAULTLIB",
            "/ENTRY:main",
            "/SUBSYSTEM:CONSOLE",
            "main.obj",
            "t.lib",
        ],
    )
    .expect("links");
    // main's 26 bytes, then each member's ret on 16 bytes.
    assert_eq!(sections(&image)[0].1, 0x20 + 0x10 * 4 + 1);
    let _ = std::fs::remove_dir_all(&dir);
}

/// Without /SUBSYSTEM: main is the console's, WinMain Windows', a DLL is
/// Windows' whatever it defines (but main), and an EXE defining none of
/// them is LNK1221.
#[test]
fn the_subsystem_is_inferred_from_what_is_defined() {
    let dir = scratch("subsystem");
    for (file, names) in [
        ("main.obj", &["main"][..]),
        ("winmain.obj", &["WinMain", "foo"][..]),
        ("dllmain.obj", &["DllMain"][..]),
        ("foo.obj", &["foo"][..]),
    ] {
        let syms: Vec<Sym> = names.iter().map(|n| Sym(n, 1)).collect();
        std::fs::write(dir.join(file), object(&[0xC3], &[], &syms)).unwrap();
    }
    let subsystem = |image: Vec<u8>| u16_at(&image, optional_header(&image) + 68);
    let link = |words: &[&str]| run(&dir, words);
    assert_eq!(
        subsystem(link(&["/NODEFAULTLIB", "/ENTRY:main", "main.obj"]).unwrap()),
        3
    );
    assert_eq!(
        subsystem(link(&["/NODEFAULTLIB", "/ENTRY:foo", "winmain.obj"]).unwrap()),
        2
    );
    assert_eq!(
        subsystem(link(&["/NODEFAULTLIB", "/DLL", "/NOENTRY", "dllmain.obj"]).unwrap()),
        2
    );
    assert_eq!(
        subsystem(link(&["/NODEFAULTLIB", "/DLL", "/NOENTRY", "foo.obj"]).unwrap()),
        2
    );
    assert_eq!(
        subsystem(link(&["/NODEFAULTLIB", "/DLL", "/NOENTRY", "main.obj"]).unwrap()),
        3
    );
    assert_eq!(
        link(&["/NODEFAULTLIB", "/ENTRY:foo", "foo.obj"]),
        Err(vec![1221])
    );
    let _ = std::fs::remove_dir_all(&dir);
}

/// An image without base relocations has no .reloc, and its headers no
/// room for one (0x200 bytes: the Rich header unpadded); x64's volatile
/// metadata is there without a load configuration; an object without a
/// `@comp.id` is the Rich header's 0.
#[test]
fn a_minimal_image_is_link_exes_shape() {
    let dir = scratch("minimal");
    std::fs::write(dir.join("main.obj"), defining("main")).unwrap();
    std::fs::write(dir.join("other.obj"), defining("other")).unwrap();
    let image = run(
        &dir,
        &[
            "/NODEFAULTLIB",
            "/ENTRY:main",
            "/SUBSYSTEM:CONSOLE",
            "main.obj",
            "other.obj",
        ],
    )
    .expect("links");
    let names: Vec<String> = sections(&image).into_iter().map(|s| s.0).collect();
    assert_eq!(names, [".text", ".rdata"]);
    assert_eq!(
        u32_at(&image, optional_header(&image) + 60),
        0x200,
        "SizeOfHeaders"
    );
    assert_eq!(u32_at(&image, 0x3C), 0xA8, "two Rich entries, no padding");
    // The Rich header's entries, unmasked: (0, 2 objects), then link.exe's.
    let key = u32_at(&image, 0x84);
    let entry = |i: usize| {
        (
            u32_at(&image, 0x90 + 8 * i) ^ key,
            u32_at(&image, 0x94 + 8 * i) ^ key,
        )
    };
    assert_eq!(entry(0), (0, 2));
    assert_eq!(entry(1), (0x0102_90A5, 1));
    // The volatile metadata: 24 bytes, 0x18 then version 0x80008000.
    let rdata = &sections(&image)[1];
    let bytes = at_rva(&image, rdata.2, rdata.1 as usize);
    assert!(
        bytes
            .windows(8)
            .any(|w| w == [0x18, 0, 0, 0, 0, 0x80, 0, 0x80]),
        "no volatile metadata"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

/// An x64 import object (the short form): `symbol` from `dll`, by name, of
/// code, with `hint`.
fn import(dll: &str, symbol: &str, hint: u16) -> Vec<u8> {
    let mut names = symbol.as_bytes().to_vec();
    names.push(0);
    names.extend_from_slice(dll.as_bytes());
    names.push(0);
    let mut o = Vec::new();
    o.extend_from_slice(&0u16.to_le_bytes());
    o.extend_from_slice(&0xFFFFu16.to_le_bytes());
    o.extend_from_slice(&0u16.to_le_bytes()); // version
    o.extend_from_slice(&0x8664u16.to_le_bytes());
    o.extend_from_slice(&0u32.to_le_bytes()); // time stamp
    o.extend_from_slice(&n32(names.len()).to_le_bytes());
    o.extend_from_slice(&hint.to_le_bytes());
    o.extend_from_slice(&(1u16 << 2).to_le_bytes()); // code, by name
    o.extend_from_slice(&names);
    o
}

/// Links `words` in `dir`: the image (or the errors' codes), and the
/// warnings' codes.
fn run_with_warnings(dir: &Path, words: &[&str]) -> (Result<Vec<u8>, Vec<u32>>, Vec<u32>) {
    let argv = words.iter().map(|w| {
        if w.starts_with('/') {
            (*w).to_owned()
        } else {
            input(dir, w)
        }
    });
    let words = args::command_line(argv, None, None, &|p| std::fs::read(p)).unwrap();
    let mut diags = Vec::new();
    let mut config = config::parse(&words, &mut diags);
    let out = dir.join("out.img");
    config.out = Some(out.clone());
    let outcome = link::link(config);
    let codes = |warning: bool| -> Vec<u32> {
        outcome
            .diagnostics
            .iter()
            .filter(|d| d.warning == warning)
            .map(|d| d.code)
            .collect()
    };
    let (errors, warnings) = (codes(false), codes(true));
    if !errors.is_empty() || !outcome.written {
        return (Err(errors), warnings);
    }
    (Ok(std::fs::read(out).unwrap()), warnings)
}

/// Data directory `i`: (RVA, size).
fn directory(d: &[u8], i: usize) -> (u32, u32) {
    let at = optional_header(d) + 112 + 8 * i;
    (u32_at(d, at), u32_at(d, at + 4))
}

/// The base relocations' sites (RVAs).
fn base_relocation_sites(d: &[u8]) -> Vec<u32> {
    let (rva, size) = directory(d, 5);
    let mut sites = Vec::new();
    if size == 0 {
        return sites;
    }
    let blocks = at_rva(d, rva, size as usize);
    let mut o = 0;
    while o + 8 <= blocks.len() {
        let page = u32_at(blocks, o);
        let n = u32_at(blocks, o + 4) as usize;
        for k in (8..n).step_by(2) {
            let e = u16_at(blocks, o + k);
            if e >> 12 != 0 {
                sites.push(page + u32::from(e & 0xFFF));
            }
        }
        o += n;
    }
    sites
}

/// Where the 4-byte displacement at `at` in `code` (at `rva`) leads.
fn rel32(code: &[u8], rva: u32, at: usize) -> u32 {
    let disp = i32::from_le_bytes(code[at..at + 4].try_into().unwrap());
    let end = i64::from(rva) + i64::from(n32(at)) + 4;
    u32::try_from(end + i64::from(disp)).unwrap()
}

/// `main` calls `f` and `g` through their `__imp_` slots (as dllimport code
/// does), from `x.dll`, delay-loaded; delay.lib has a stand-in
/// `__delayLoadHelper2`.
fn delay_inputs(dir: &Path) {
    let main = object(
        &[0xFF, 0x15, 0, 0, 0, 0, 0xFF, 0x15, 0, 0, 0, 0, 0xC3],
        &[(2, 1, 4), (8, 2, 4)], // REL32 to __imp_f, __imp_g
        &[Sym("main", 1), Sym("__imp_f", 0), Sym("__imp_g", 0)],
    );
    std::fs::write(dir.join("main.obj"), main).unwrap();
    let x = archive(&[
        ("x.dll", import("x.dll", "f", 7), &["f", "__imp_f"]),
        ("x.dll", import("x.dll", "g", 9), &["g", "__imp_g"]),
    ]);
    std::fs::write(dir.join("x.lib"), x).unwrap();
    let helper = archive(&[(
        "helper.obj",
        defining("__delayLoadHelper2"),
        &["__delayLoadHelper2"],
    )]);
    std::fs::write(dir.join("delay.lib"), helper).unwrap();
}

/// Delay loading as link.exe lays it out (x64): a descriptor per DLL and a
/// null one (`.didat$2`, `$3`); the name table (`$4`) and hint/names
/// (`$6`); the bound table (`$7`, zeros, then room: 0x100 bytes and a tenth
/// of the bound tables' size, 0x102 for these 24 bytes); the address table
/// in `.data`, each slot the address of its import's
/// `__imp_load_` thunk (with a base relocation); the tail merge and the
/// thunks in `.text`, the tail merge's `.pdata` entry and unwind data.
#[test]
fn delay_loading_is_link_exes() {
    let dir = scratch("delay");
    delay_inputs(&dir);
    let (image, warnings) = run_with_warnings(
        &dir,
        &[
            "/NODEFAULTLIB",
            "/ENTRY:main",
            "/SUBSYSTEM:CONSOLE",
            "/DELAYLOAD:X.DLL",
            "main.obj",
            "x.lib",
            "delay.lib",
        ],
    );
    let image = image.expect("links");
    assert!(warnings.is_empty(), "{warnings:?}");
    let base = 0x1_4000_0000u64;
    let (desc, size) = directory(&image, 13);
    assert_eq!(size, 64, "one descriptor and the null one");
    assert_eq!(directory(&image, 1), (0, 0), "no ordinary imports");
    let d = at_rva(&image, desc, 64);
    let field = |i: usize| u32_at(d, 4 * i);
    assert_eq!(field(0), 1, "RVAs");
    assert_eq!(at_rva(&image, field(1), 6), b"x.dll\0");
    assert!(d[32..].iter().all(|&b| b == 0), "the null descriptor");
    let (handle, iat, int, bound) = (field(2), field(3), field(4), field(5));
    assert_eq!((field(6), field(7)), (0, 0), "no unload table, not bound");
    let data = sections(&image)
        .into_iter()
        .find(|s| s.0 == ".data")
        .expect(".data");
    assert!(
        data.2 <= iat && iat < data.2 + data.1,
        "the address table is in .data"
    );
    assert!(
        data.2 <= handle && handle < data.2 + data.1,
        "so is the module handle"
    );
    // The name table: hint/names, then 0; in link.exe's sort (`qsort`'s
    // order of the two imports and the table's end: g, then f).
    let names = at_rva(&image, int, 24);
    for (k, (hint, name)) in [(9u16, &b"g\0"[..]), (7, b"f\0")].into_iter().enumerate() {
        let entry = u32_at(names, 8 * k);
        assert_eq!(u32_at(names, 8 * k + 4), 0);
        let hn = at_rva(&image, entry, 4);
        assert_eq!(u16_at(hn, 0), hint);
        assert_eq!(&hn[2..4], name);
    }
    assert_eq!(&names[16..24], &[0; 8]);
    // The bound table: zeros, and the room after it, which ends .rdata.
    assert!(at_rva(&image, bound, 24 + 0x102).iter().all(|&b| b == 0));
    let rdata = sections(&image)
        .into_iter()
        .find(|s| s.0 == ".rdata")
        .expect(".rdata");
    assert_eq!(rdata.2 + rdata.1, bound + 24 + 0x102, "the room's end");
    // The address table: each slot the address of its thunk, relocated.
    let slots = at_rva(&image, iat, 24);
    let relocations = base_relocation_sites(&image);
    let mut tail_merge = None;
    for k in 0..2 {
        let at = 8 * k;
        let thunk = u64::from_le_bytes(slots[at..at + 8].try_into().unwrap()) - base;
        let thunk = u32::try_from(thunk).unwrap();
        let t = at_rva(&image, thunk, 12);
        assert_eq!(&t[..3], &[0x48, 0x8D, 0x05], "lea rax, [rip + slot]");
        assert_eq!(
            rel32(t, thunk, 3),
            iat + n32(at),
            "the thunk names its slot"
        );
        assert_eq!(t[7], 0xE9, "jmp");
        let to = rel32(t, thunk, 8);
        assert_eq!(
            *tail_merge.get_or_insert(to),
            to,
            "one tail merge for the DLL"
        );
        assert!(relocations.contains(&(iat + n32(at))), "slot {k} relocated");
    }
    assert_eq!(&slots[16..24], &[0; 8]);
    // The tail merge: link.exe's code, its descriptor and the helper.
    let tail_merge = tail_merge.unwrap();
    let code = at_rva(&image, tail_merge, 0x79);
    assert_eq!(&code[..0x36], &turbonop_linker::layout::TAIL_MERGE[..0x36]);
    assert_eq!(rel32(code, tail_merge, 0x36), desc, "the descriptor");
    let text = &sections(&image)[0];
    let helper = rel32(code, tail_merge, 0x3B);
    assert!(text.2 <= helper && helper < text.2 + text.1, "the helper");
    assert_eq!(&code[0x75..], &[0xEB, 0x00, 0xFF, 0xE0]);
    // Its .pdata entry and unwind data.
    let (pdata, psize) = directory(&image, 3);
    let entries = at_rva(&image, pdata, psize as usize);
    let entry = entries
        .chunks(12)
        .find(|e| u32_at(e, 0) == tail_merge)
        .expect("the tail merge's .pdata entry");
    assert_eq!(u32_at(entry, 4), tail_merge + 0x77);
    assert_eq!(
        at_rva(&image, u32_at(entry, 8), 8),
        &[0x01, 0x18, 0x01, 0x00, 0x18, 0xC2, 0x00, 0x00]
    );
    // main's calls reach the slots (f's second, g's first).
    let call = at_rva(&image, 0x1000, 12);
    assert_eq!(rel32(call, 0x1000, 2), iat + 8);
    assert_eq!(rel32(call, 0x1000, 8), iat);
    let _ = std::fs::remove_dir_all(&dir);
}

/// /DELAY:UNLOAD adds an unload table (a copy of the address table,
/// relocated); /DELAY:NOBIND drops the bound table and its room.
#[test]
fn delay_unload_and_nobind() {
    let dir = scratch("delay-options");
    delay_inputs(&dir);
    let link = |extra: &str| {
        run_with_warnings(
            &dir,
            &[
                "/NODEFAULTLIB",
                "/ENTRY:main",
                "/SUBSYSTEM:CONSOLE",
                "/DELAYLOAD:x.dll",
                extra,
                "main.obj",
                "x.lib",
                "delay.lib",
            ],
        )
        .0
        .expect("links")
    };
    let unload = link("/DELAY:UNLOAD");
    let (desc, _) = directory(&unload, 13);
    let d = at_rva(&unload, desc, 32);
    let (iat, table) = (u32_at(d, 12), u32_at(d, 24));
    assert_ne!(table, 0);
    assert_eq!(at_rva(&unload, iat, 24), at_rva(&unload, table, 24));
    let relocations = base_relocation_sites(&unload);
    assert!(relocations.contains(&table) && relocations.contains(&(table + 8)));
    let nobind = link("/DELAY:NOBIND");
    let (desc, _) = directory(&nobind, 13);
    assert_eq!(u32_at(at_rva(&nobind, desc, 32), 20), 0, "no bound table");
    let _ = std::fs::remove_dir_all(&dir);
}

/// An object whose `main` calls each of `names` through its `__imp_` slot,
/// in order.
fn calling(names: &[String]) -> Vec<u8> {
    let mut text = Vec::new();
    let mut relocations = Vec::new();
    let mut syms = vec![Sym("main", 1)];
    for (i, n) in names.iter().enumerate() {
        text.extend_from_slice(&[0xFF, 0x15, 0, 0, 0, 0]);
        relocations.push((n32(6 * i + 2), n32(i + 1), 4u16));
        syms.push(Sym(String::leak(format!("__imp_{n}")), 0));
    }
    text.push(0xC3);
    object(&text, &relocations, &syms)
}

/// Each delay-loaded DLL (in the directory's order): its name, its name
/// table's RVA, and the names in it, in order. Each address table slot
/// must lead to its own import's thunk, which names the slot.
fn delay_tables(image: &[u8]) -> Vec<(String, u32, Vec<String>)> {
    let (desc, size) = directory(image, 13);
    let name = |rva: u32| {
        let s = at_rva(image, rva, 64);
        String::from_utf8_lossy(&s[..s.iter().position(|&b| b == 0).unwrap()]).into_owned()
    };
    let mut out = Vec::new();
    for d in at_rva(image, desc, size as usize).chunks(32) {
        if u32_at(d, 4) == 0 {
            break;
        }
        let (iat, int) = (u32_at(d, 12), u32_at(d, 16));
        let mut names = Vec::new();
        for k in 0.. {
            let entry = u32_at(at_rva(image, int + 8 * k, 8), 0);
            if entry == 0 {
                break;
            }
            names.push(name(entry + 2));
            let slot = u64::from_le_bytes(at_rva(image, iat + 8 * k, 8).try_into().unwrap());
            let thunk = u32::try_from(slot - 0x1_4000_0000).unwrap();
            assert_eq!(rel32(at_rva(image, thunk, 12), thunk, 3), iat + 8 * k);
        }
        out.push((name(u32_at(d, 4)), int, names));
    }
    out
}

/// A delay-loaded DLL's name, address and unload tables are in link.exe's
/// sort of them, not the order the imports loaded: link.exe `qsort`s each
/// of those sections' pieces (every delay-loaded import that loaded, and
/// each DLL's table end, made right after its first import) by the name of
/// the archive member each came from (without its extension, compared
/// case-sensitively), a DLL's end after its imports. `qsort` isn't stable,
/// so a DLL's imports come out as it leaves them: loads A, B, C give C, B,
/// A; more than 8 pieces, it partitions (A to H: A, G, B, C, D, E, F, H);
/// and a DLL's order depends on the other DLLs' pieces too. The DLLs'
/// tables are in their names' order, their descriptors (and bound tables
/// and hint/names) in the order they loaded. (Each case's order is
/// link.exe 14.52's for the same inputs.)
#[test]
fn delay_tables_are_in_link_exes_sort() {
    let dir = scratch("delay-sort");
    let helper = archive(&[(
        "helper.obj",
        defining("__delayLoadHelper2"),
        &["__delayLoadHelper2"],
    )]);
    std::fs::write(dir.join("delay.lib"), helper).unwrap();
    // A library for DLL `dll` (its members named so), importing `names`.
    let library = |file: &str, dll: &str, names: &[String]| {
        let symbols: Vec<[String; 2]> = names
            .iter()
            .map(|n| [n.clone(), format!("__imp_{n}")])
            .collect();
        let symbols: Vec<Vec<&str>> = symbols
            .iter()
            .map(|s| s.iter().map(String::as_str).collect())
            .collect();
        let members: Vec<(&str, Vec<u8>, &[&str])> = names
            .iter()
            .zip(&symbols)
            .enumerate()
            .map(|(i, (n, s))| (dll, import(dll, n, u16::try_from(i).unwrap()), s.as_slice()))
            .collect();
        std::fs::write(dir.join(file), archive(&members)).unwrap();
    };
    let words = |s: &str| -> Vec<String> { s.split(' ').map(str::to_owned).collect() };
    // (Each link's inputs are new files: turbolink maps its inputs for good.)
    let links = std::cell::Cell::new(0);
    let link = |calls: &[String], libs: &[&str], extra: &[&str]| {
        let main = format!("main{}.obj", links.replace(links.get() + 1));
        std::fs::write(dir.join(&main), calling(calls)).unwrap();
        let mut w = vec!["/NODEFAULTLIB", "/ENTRY:main", "/SUBSYSTEM:CONSOLE"];
        w.extend_from_slice(extra);
        w.push(&main);
        w.extend_from_slice(libs);
        w.push("delay.lib");
        delay_tables(&run(&dir, &w).expect("links"))
    };
    // One DLL: the five orders of docs/turbolink.md, then 7, 8 and 20
    // imports.
    for (i, (loads, want)) in [
        ("C D", "D C"),
        ("C D S", "S D C"),
        ("C D X S", "S D X C"),
        ("C Q F X E G", "G Q F X E C"),
        ("C Q F A X E B I M G", "C M Q F A X E B I G"),
        ("A B C D E F G", "G B C D E F A"),
        ("A B C D E F G H", "A G B C D E F H"),
        (
            "A B C D E F G H I J K L M N O P Q R S T",
            "A S B C D E F G H I J K L M N O P Q R T",
        ),
    ]
    .into_iter()
    .enumerate()
    {
        let loads = words(loads);
        let lib = format!("x{i}.lib");
        library(&lib, "x.dll", &loads);
        let got = link(&loads, &[&lib], &["/DELAYLOAD:x.dll"]);
        assert_eq!(got.len(), 1);
        assert_eq!(got[0].2, words(want), "{loads:?}");
        let unload = link(&loads, &[&lib], &["/DELAYLOAD:x.dll", "/DELAY:UNLOAD"]);
        assert_eq!(unload[0].2, words(want), "{loads:?}, /DELAY:UNLOAD");
    }
    // Two DLLs: x's 9 imports and y's 3 sort together (13 pieces: x's
    // partition isn't what it is alone); x's tables first, by name.
    let xs = words("q0 q1 q2 q3 q4 q5 q6 q7 q8");
    let ys = words("r0 r1 r2");
    library("x.lib", "x.dll", &xs);
    library("y.lib", "y.dll", &ys);
    let both = [xs.clone(), ys.clone()].concat();
    let got = link(
        &both,
        &["x.lib", "y.lib"],
        &["/DELAYLOAD:x.dll", "/DELAYLOAD:y.dll"],
    );
    assert_eq!(got[0].2, words("q0 q8 q1 q2 q3 q4 q5 q6 q7"));
    assert_eq!(got[1].2, words("r2 r1 r0"));
    assert!(got[0].1 < got[1].1);
    // y loaded first: its descriptor first, but x's tables still before
    // y's; and names compare case-sensitively (`B` before `a`).
    let a = words("a0 a1 a2");
    let b = words("b0 b1");
    library("a.lib", "a.dll", &a);
    library("b.lib", "B.dll", &b);
    let calls = [ys.clone(), xs.clone(), a, b].concat();
    let got = link(
        &calls,
        &["y.lib", "x.lib", "a.lib", "b.lib"],
        &[
            "/DELAYLOAD:x.dll",
            "/DELAYLOAD:y.dll",
            "/DELAYLOAD:a.dll",
            "/DELAYLOAD:B.dll",
        ],
    );
    let dlls: Vec<&str> = got.iter().map(|g| g.0.as_str()).collect();
    assert_eq!(dlls, ["y.dll", "x.dll", "a.dll", "B.dll"], "the load order");
    let orders: Vec<String> = got.iter().map(|g| g.2.join(" ")).collect();
    assert_eq!(
        orders,
        [
            "r1 r0 r2",
            "q2 q1 q4 q5 q6 q7 q8 q0 q3",
            "a0 a1 a2",
            "b0 b1"
        ]
    );
    let mut by_table = got.clone();
    by_table.sort_by_key(|g| g.1);
    let dlls: Vec<&str> = by_table.iter().map(|g| g.0.as_str()).collect();
    assert_eq!(
        dlls,
        ["B.dll", "a.dll", "x.dll", "y.dll"],
        "the names' order"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

/// A /DELAYLOAD DLL nothing imports from is LNK4199 (a warning); without
/// `__delayLoadHelper2` (delayimp.lib) the link fails.
#[test]
fn delay_loading_diagnostics() {
    let dir = scratch("delay-diagnostics");
    delay_inputs(&dir);
    let base = [
        "/NODEFAULTLIB",
        "/ENTRY:main",
        "/SUBSYSTEM:CONSOLE",
        "/DELAYLOAD:x.dll",
    ];
    let (image, warnings) = run_with_warnings(
        &dir,
        &[
            &base[..],
            &["/DELAYLOAD:y.dll", "main.obj", "x.lib", "delay.lib"],
        ]
        .concat(),
    );
    assert!(image.is_ok());
    assert_eq!(warnings, [4199]);
    let (image, _) = run_with_warnings(&dir, &[&base[..], &["main.obj", "x.lib"]].concat());
    assert_eq!(image, Err(vec![2001]));
    let _ = std::fs::remove_dir_all(&dir);
}

/// An x64 object: `main` (`lea rax, [x]` for each of `names`, then `ret`),
/// and per name a COMDAT (any) 8-byte `.rdata` section, `name` its symbol,
/// holding `main`'s address: pairs of identical data, told apart by name.
fn data_pairs(names: &[&str]) -> Vec<u8> {
    let count = 1 + names.len();
    let main = n32(2 * count); // after the section symbols and their aux
    let mut strings: Vec<u8> = Vec::new();
    let mut name8 = |n: &str| -> [u8; 8] {
        let mut r = [0u8; 8];
        if n.len() <= 8 {
            r[..n.len()].copy_from_slice(n.as_bytes());
        } else {
            r[4..].copy_from_slice(&(4 + n32(strings.len())).to_le_bytes());
            strings.extend_from_slice(n.as_bytes());
            strings.push(0);
        }
        r
    };
    let mut text = Vec::new();
    let mut text_relocations = Vec::new();
    for i in 0..names.len() {
        text.extend_from_slice(&[0x48, 0x8D, 0x05, 0, 0, 0, 0]);
        text_relocations.push((n32(7 * i + 3), main + 1 + n32(i), 4u16));
    }
    text.push(0xC3);
    // (name, characteristics, contents, relocations, COMDAT selection)
    let mut sections = vec![(b".text$mn", 0x6050_0020u32, text, text_relocations, 0u8)];
    for _ in names {
        sections.push((
            b".rdata\0\0",
            0x4040_1040,
            vec![0; 8],
            vec![(0, main, 1)],
            2,
        ));
    }
    let mut records = Vec::new();
    for (k, (name, _, data, relocations, selection)) in sections.iter().enumerate() {
        records.extend_from_slice(&name8(
            std::str::from_utf8(&name[..])
                .unwrap()
                .trim_end_matches('\0'),
        ));
        records.extend_from_slice(&0u32.to_le_bytes());
        records.extend_from_slice(&i16::try_from(k + 1).unwrap().to_le_bytes());
        records.extend_from_slice(&[0, 0, 3, 1]);
        records.extend_from_slice(&n32(data.len()).to_le_bytes());
        records.extend_from_slice(&u16::try_from(relocations.len()).unwrap().to_le_bytes());
        records.extend_from_slice(&[0; 8]);
        records.push(*selection);
        records.extend_from_slice(&[0; 3]);
    }
    let symbol_names: Vec<&str> = std::iter::once("main")
        .chain(names.iter().copied())
        .collect();
    for (k, n) in symbol_names.iter().enumerate() {
        records.extend_from_slice(&name8(n));
        records.extend_from_slice(&0u32.to_le_bytes());
        records.extend_from_slice(&i16::try_from(k + 1).unwrap().to_le_bytes());
        records.extend_from_slice(&[0, 0, 2, 0]);
    }
    let headers = 20 + 40 * count;
    let mut body = Vec::new();
    let mut section_headers = Vec::new();
    for (name, chars, data, relocations, _) in &sections {
        let data_at = headers + body.len();
        body.extend_from_slice(data);
        let relocations_at = headers + body.len();
        for (at, symbol, kind) in relocations {
            body.extend_from_slice(&at.to_le_bytes());
            body.extend_from_slice(&symbol.to_le_bytes());
            body.extend_from_slice(&kind.to_le_bytes());
        }
        section_headers.extend_from_slice(&name[..]);
        for v in [0, 0, n32(data.len()), n32(data_at), n32(relocations_at), 0] {
            section_headers.extend_from_slice(&v.to_le_bytes());
        }
        section_headers.extend_from_slice(&u16::try_from(relocations.len()).unwrap().to_le_bytes());
        section_headers.extend_from_slice(&0u16.to_le_bytes());
        section_headers.extend_from_slice(&chars.to_le_bytes());
    }
    let mut o = Vec::new();
    o.extend_from_slice(&0x8664u16.to_le_bytes());
    o.extend_from_slice(&u16::try_from(count).unwrap().to_le_bytes());
    o.extend_from_slice(&0u32.to_le_bytes());
    o.extend_from_slice(&n32(headers + body.len()).to_le_bytes());
    o.extend_from_slice(&(main + n32(symbol_names.len())).to_le_bytes());
    o.extend_from_slice(&[0; 4]);
    o.extend_from_slice(&section_headers);
    o.extend_from_slice(&body);
    o.extend_from_slice(&records);
    o.extend_from_slice(&(4 + n32(strings.len())).to_le_bytes());
    o.extend_from_slice(&strings);
    o
}

/// /OPT:ICF folds identical vtables (`??_7`), and no other read-only data,
/// however identical: not vbtables (`??_8`), RTTI (`??_R`), or constants.
#[test]
fn only_vtables_fold_among_data() {
    let dir = scratch("icf-data");
    let names = [
        "??_7A@@6B@",
        "??_7B@@6B@",
        "??_8A@@7B@",
        "??_8B@@7B@",
        "??_R4A@@6B@",
        "??_R4B@@6B@",
        "?x@@3QBHB",
        "?y@@3QBHB",
    ];
    std::fs::write(dir.join("d.obj"), data_pairs(&names)).unwrap();
    let image = run(
        &dir,
        &[
            "/NODEFAULTLIB",
            "/ENTRY:main",
            "/SUBSYSTEM:CONSOLE",
            "/OPT:REF,ICF",
            "d.obj",
        ],
    )
    .expect("links");
    let code = at_rva(&image, 0x1000, 7 * names.len());
    let at: Vec<u32> = (0..names.len())
        .map(|i| rel32(code, 0x1000, 7 * i + 3))
        .collect();
    assert_eq!(at[0], at[1], "the vtables fold");
    for pair in at[2..].chunks(2) {
        assert_ne!(pair[0], pair[1], "the rest don't");
    }
    let _ = std::fs::remove_dir_all(&dir);
}

/// link.exe's manifest for `words`' options (an EXE's).
fn manifest_for(words: &[&str]) -> String {
    let args = args::command_line(words.iter().map(|w| (*w).to_owned()), None, None, &|p| {
        std::fs::read(p)
    })
    .unwrap();
    let mut diags = Vec::new();
    let config = config::parse(&args, &mut diags);
    String::from_utf8(turbonop_linker::manifest::xml(&config)).unwrap()
}

/// The default manifest's lines, as link.exe writes them (CRLF).
const MANIFEST_HEAD: &str = "<?xml version='1.0' encoding='UTF-8' standalone='yes'?>\r\n\
<assembly xmlns='urn:schemas-microsoft-com:asm.v1' manifestVersion='1.0'>\r\n";
const MANIFEST_UAC: &str = "  <trustInfo xmlns=\"urn:schemas-microsoft-com:asm.v3\">\r\n\
\x20   <security>\r\n\
\x20     <requestedPrivileges>\r\n\
\x20       <requestedExecutionLevel level='asInvoker' uiAccess='false' />\r\n\
\x20     </requestedPrivileges>\r\n\
\x20   </security>\r\n\
\x20 </trustInfo>\r\n";

/// The manifest's XML, as link.exe writes it: the UAC section (the
/// default, `/MANIFESTUAC`'s fragment, or none), then each dependency once.
#[test]
fn the_manifest_is_link_exes_xml() {
    let end = "</assembly>\r\n";
    assert_eq!(
        manifest_for(&["/MANIFEST"]),
        format!("{MANIFEST_HEAD}{MANIFEST_UAC}{end}")
    );
    assert_eq!(
        manifest_for(&["/MANIFEST", "/MANIFESTUAC"]),
        format!("{MANIFEST_HEAD}{MANIFEST_UAC}{end}")
    );
    assert_eq!(
        manifest_for(&["/MANIFEST", "/MANIFESTUAC:NO"]),
        format!("{MANIFEST_HEAD}{end}")
    );
    let admin = manifest_for(&[
        "/MANIFEST",
        "/MANIFESTUAC:level='requireAdministrator' uiAccess='true'",
    ]);
    assert!(admin.contains(
        "        <requestedExecutionLevel level='requireAdministrator' uiAccess='true' />\r\n"
    ));
    let dep = "/MANIFESTDEPENDENCY:type='win32' name='A.B' version='1.0.0.0'";
    let deps = manifest_for(&[
        "/MANIFEST",
        dep,
        "/MANIFESTDEPENDENCY:type='win32' name='C.D'",
        dep,
    ]);
    let block = |id: &str| {
        format!(
            "  <dependency>\r\n    <dependentAssembly>\r\n      <assemblyIdentity {id} />\r\n    </dependentAssembly>\r\n  </dependency>\r\n"
        )
    };
    assert_eq!(
        deps,
        format!(
            "{MANIFEST_HEAD}{MANIFEST_UAC}{}{}{end}",
            block("type='win32' name='A.B' version='1.0.0.0'"),
            block("type='win32' name='C.D'")
        ),
        "each dependency once, in order"
    );
}

/// The resources of an image: (type, name, language, data).
fn resources_of(d: &[u8]) -> Vec<(u32, u32, u32, Vec<u8>)> {
    let (base, _) = directory(d, 2);
    let mut out = Vec::new();
    let table = |o: u32| -> Vec<(u32, u32)> {
        let h = at_rva(d, base + o, 16);
        let n = u32::from(u16_at(h, 12)) + u32::from(u16_at(h, 14));
        let entries = at_rva(d, base + o + 16, 8 * n as usize);
        (0..n as usize)
            .map(|k| (u32_at(entries, 8 * k), u32_at(entries, 8 * k + 4)))
            .collect()
    };
    for (ty, o) in table(0) {
        for (name, o2) in table(o & 0x7FFF_FFFF) {
            for (lang, leaf) in table(o2 & 0x7FFF_FFFF) {
                let e = at_rva(d, base + leaf, 16);
                let (rva, size) = (u32_at(e, 0), u32_at(e, 4));
                out.push((ty, name, lang, at_rva(d, rva, size as usize).to_vec()));
            }
        }
    }
    out
}

/// `/MANIFEST:EMBED`: the manifest is the image's RT_MANIFEST (ID 1 for an
/// EXE, 2 for a DLL, or `ID=`'s), language 1033; `/MANIFEST` writes it
/// beside the image; a dependency alone asks for it beside the image too,
/// and `/MANIFEST:NO` for none; `/MANIFESTINPUT` without `/MANIFEST:EMBED`
/// is LNK1220.
#[test]
fn the_manifest_is_where_link_exe_puts_it() {
    let dir = scratch("manifest");
    std::fs::write(dir.join("main.obj"), defining("main")).unwrap();
    let base = [
        "/NODEFAULTLIB",
        "/ENTRY:main",
        "/SUBSYSTEM:CONSOLE",
        "main.obj",
    ];
    let link = |extra: &[&str]| run(&dir, &[&base[..], extra].concat());
    let xml = format!("{MANIFEST_HEAD}{MANIFEST_UAC}</assembly>\r\n").into_bytes();
    let image = link(&["/MANIFEST:EMBED"]).expect("links");
    assert_eq!(resources_of(&image), [(24, 1, 1033, xml.clone())]);
    assert!(!dir.join("out.img.manifest").exists());
    let image = link(&["/MANIFEST:EMBED,ID=5"]).expect("links");
    assert_eq!(resources_of(&image)[0].1, 5);
    let image = link(&["/MANIFEST:EMBED", "/DLL", "/NOENTRY"]).expect("links");
    assert_eq!(resources_of(&image)[0].1, 2, "a DLL's is 2");
    link(&["/MANIFEST"]).expect("links");
    assert_eq!(std::fs::read(dir.join("out.img.manifest")).unwrap(), xml);
    std::fs::remove_file(dir.join("out.img.manifest")).unwrap();
    let dep = "/MANIFESTDEPENDENCY:type='win32' name='A.B'";
    link(&[dep]).expect("links");
    assert!(
        dir.join("out.img.manifest").exists(),
        "a dependency asks for one"
    );
    std::fs::remove_file(dir.join("out.img.manifest")).unwrap();
    link(&[dep, "/MANIFEST:NO"]).expect("links");
    assert!(!dir.join("out.img.manifest").exists());
    assert_eq!(
        link(&["/MANIFEST", "/MANIFESTINPUT:x.manifest"]),
        Err(vec![1220])
    );
    let _ = std::fs::remove_dir_all(&dir);
}

/// `/MAP` writes link.exe's map (CRLF lines): the image's name, its time
/// stamp, its preferred load address, the groups (an empty `.edata` among
/// them), the publics by address (link.exe's 29 absolutes first, each
/// function marked `f`, each with its object), `__volatile_metadata`, the
/// entry point, and the statics.
#[test]
fn the_map_is_link_exes() {
    let dir = scratch("map");
    std::fs::write(dir.join("main.obj"), defining("main")).unwrap();
    std::fs::write(dir.join("other.obj"), defining("other")).unwrap();
    let map = dir.join("out.map");
    run(
        &dir,
        &[
            "/NODEFAULTLIB",
            "/ENTRY:main",
            "/SUBSYSTEM:CONSOLE",
            &format!("/MAP:{}", input(&dir, "out.map")),
            "main.obj",
            "other.obj",
        ],
    )
    .expect("links");
    let text = String::from_utf8(std::fs::read(&map).unwrap()).unwrap();
    assert!(
        text.ends_with("\r\n") && !text.replace("\r\n", "").contains('\n'),
        "CRLF lines"
    );
    let lines: Vec<&str> = text.split("\r\n").collect();
    assert_eq!(lines[0], " out");
    assert!(lines[2].starts_with(" Timestamp is "), "{}", lines[2]);
    assert_eq!(lines[4], " Preferred load address is 0000000140000000");
    assert_eq!(
        lines[6],
        " Start         Length     Name                   Class"
    );
    assert_eq!(
        lines[7],
        " 0001:00000000 00000011H .text                   CODE"
    );
    assert!(
        lines
            .iter()
            .any(|l| l.starts_with(" 0002:") && l.contains(" 00000000H .edata ")),
        "the empty .edata group"
    );
    let publics = lines
        .iter()
        .position(|l| l.contains("Publics by Value"))
        .expect("the publics");
    assert_eq!(
        lines[publics],
        "  Address         Publics by Value              Rva+Base               Lib:Object"
    );
    assert_eq!(
        lines[publics + 2],
        " 0000:00000000       __AbsoluteZero             0000000000000000     <absolute>"
    );
    let absolutes = lines.iter().filter(|l| l.ends_with("<absolute>")).count();
    assert_eq!(absolutes, 29);
    assert!(lines.contains(
        &" 0001:00000000       main                       0000000140001000 f   main.obj"
    ));
    assert!(lines.contains(
        &" 0001:00000010       other                      0000000140001010 f   other.obj"
    ));
    assert!(
        lines
            .iter()
            .any(|l| l.contains(" __volatile_metadata ") && l.ends_with("<linker-defined>")),
        "__volatile_metadata"
    );
    let entry = lines
        .iter()
        .position(|l| l.starts_with(" entry point at"))
        .expect("the entry point");
    assert_eq!(lines[entry], " entry point at        0001:00000000");
    assert_eq!(lines[entry + 2], " Static symbols");
    // And hexcavator reads it.
    let parsed = turbonop_linkmap::parse(&text);
    assert_eq!(parsed.module, "out");
    assert_eq!(parsed.load_address, Some(0x1_4000_0000));
    assert_eq!(parsed.entry_point, Some((1, 0)));
    assert!(
        parsed
            .groups
            .iter()
            .any(|g| g.name == ".text" && g.length == 0x11)
    );
    let main = parsed
        .symbols
        .iter()
        .find(|s| s.name == "main")
        .expect("main");
    assert_eq!(
        (main.va, main.function, main.is_static),
        (0x1_4000_1000, true, false)
    );
    let _ = std::fs::remove_dir_all(&dir);
}

/// A DLL's export object (`.exp`, beside its import library) is link.exe's:
/// an x64 object stamped 0xFFFFFFFF of `.edata` (the export directory, its
/// addresses left to relocations), `.debug$S` (the object's name, the
/// linker, its environment, and an `S_EXPORT` per export), and `.drectve`
/// (with a `.def` file's `LIBRARY`: `/DLL /OUT:` its DLL); its symbols
/// `@comp.id`, `@feat.00`, the two sections', the directory's parts, then
/// the exports' (undefined). Without a `.def` file, no `.drectve`; with
/// `/NOIMPLIB`, still an export object; none with `/NOEXP`.
#[test]
fn the_export_object_is_link_exes() {
    let dir = scratch("exp");
    std::fs::write(dir.join("f.obj"), defining("func")).unwrap();
    std::fs::write(
        dir.join("f.def"),
        "LIBRARY named
EXPORTS
    func
",
    )
    .unwrap();
    let def = format!("/DEF:{}", input(&dir, "f.def"));
    let link = |extra: &[&str]| {
        let mut words = vec!["/NODEFAULTLIB", "/NOENTRY", "/DLL"];
        words.extend_from_slice(extra);
        words.push("f.obj");
        run(&dir, &words).expect("links");
    };
    link(&[&def]);
    let exp = std::fs::read(dir.join("out.exp")).expect("the export object");
    assert!(dir.join("out.lib").exists(), "the import library");
    assert_eq!(u16_at(&exp, 0), 0x8664);
    assert_eq!(u16_at(&exp, 2), 3, "three sections");
    assert_eq!(u32_at(&exp, 4), 0xFFFF_FFFF, "the time stamp");
    let section = |i: usize| {
        let h = 20 + 40 * i;
        let name = String::from_utf8_lossy(&exp[h..h + 8])
            .trim_end_matches('\0')
            .to_owned();
        let (size, at) = (u32_at(&exp, h + 16) as usize, u32_at(&exp, h + 20) as usize);
        (name, u32_at(&exp, h + 36), exp[at..at + size].to_vec())
    };
    let (edata, debug, drectve) = (section(0), section(1), section(2));
    assert_eq!((edata.0.as_str(), edata.1), (".edata", 0x4000_0040));
    assert_eq!((debug.0.as_str(), debug.1), (".debug$S", 0x4210_0040));
    assert_eq!((drectve.0.as_str(), drectve.1), (".drectve", 0x200));
    // The directory: time stamp 0xFFFFFFFF, one function, one name.
    assert_eq!(u32_at(&edata.2, 4), 0xFFFF_FFFF);
    assert_eq!((u32_at(&edata.2, 20), u32_at(&edata.2, 24)), (1, 1));
    assert_eq!(&drectve.2[..3], b"\xEF\xBB\xBF", "a byte order mark");
    assert_eq!(&drectve.2[3..], b" /DLL /OUT:named.dll");
    // .debug$S: CodeView 4, then S_OBJNAME naming the object.
    assert_eq!(u32_at(&debug.2, 0), 4);
    assert!(debug.2.windows(7).any(|w| w == b"out.exp"), "S_OBJNAME");
    assert!(
        debug.2.windows(18).any(|w| w == b"Microsoft (R) LINK"),
        "S_COMPILE3"
    );
    assert!(debug.2.windows(5).any(|w| w == b"func\0"), "S_EXPORT");
    // Without a .def file: no .drectve.
    link(&["/EXPORT:func"]);
    let exp = std::fs::read(dir.join("out.exp")).expect("the export object");
    assert_eq!(u16_at(&exp, 2), 2, "two sections");
    // With /NOIMPLIB, the export object alone; with /NOEXP, none.
    for f in ["out.exp", "out.lib"] {
        std::fs::remove_file(dir.join(f)).unwrap();
    }
    link(&[&def, "/NOIMPLIB"]);
    assert!(
        dir.join("out.exp").exists() && !dir.join("out.lib").exists(),
        "/NOIMPLIB"
    );
    std::fs::remove_file(dir.join("out.exp")).unwrap();
    link(&[&def, "/NOEXP"]);
    assert!(
        !dir.join("out.exp").exists() && dir.join("out.lib").exists(),
        "/NOEXP"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

/// /Brepro, as link.exe's IMAGE::Normalize computes it: the SHA-256 of the
/// image with every time stamp (the file header's, each debug directory
/// entry's) all ones and the repro entry's length and hash zero; the hash
/// in the repro entry, its last 4 bytes the time stamps.
#[test]
fn brepro_is_link_exes_hash() {
    use sha2::Digest;
    let dir = scratch("brepro");
    std::fs::write(dir.join("main.obj"), defining("main")).unwrap();
    let image = run(
        &dir,
        &[
            "/NODEFAULTLIB",
            "/ENTRY:main",
            "/SUBSYSTEM:CONSOLE",
            "/Brepro",
            "main.obj",
        ],
    )
    .expect("links");
    let pe = u32_at(&image, 0x3C) as usize;
    let (debug, size) = directory(&image, 6);
    let entries = at_rva(&image, debug, size as usize).to_vec();
    let file_of = |rva: u32| {
        let s = sections(&image)
            .into_iter()
            .find(|s| s.2 <= rva && rva < s.2 + s.1)
            .unwrap();
        (s.4 + rva - s.2) as usize
    };
    let mut normal = image.clone();
    normal[pe + 8..pe + 12].fill(0xFF);
    let mut repro = None;
    for (i, e) in entries.chunks(28).enumerate() {
        let at = file_of(debug) + 28 * i + 4;
        normal[at..at + 4].fill(0xFF);
        if u32_at(e, 12) == 16 {
            repro = Some(u32_at(e, 24) as usize);
        }
    }
    let repro = repro.expect("a repro entry");
    assert_eq!(u32_at(&image, repro), 32);
    normal[repro..repro + 36].fill(0);
    let hash: [u8; 32] = sha2::Sha256::digest(&normal).into();
    assert_eq!(&image[repro + 4..repro + 36], &hash);
    assert_eq!(&image[pe + 8..pe + 12], &hash[28..]);
    for i in 0..entries.len() / 28 {
        let at = file_of(debug) + 28 * i + 4;
        assert_eq!(&image[at..at + 4], &hash[28..], "entry {i}");
    }
    let _ = std::fs::remove_dir_all(&dir);
}

/// A section of a made object: its name, characteristics, contents,
/// relocations (offset, symbol index, type), and for a COMDAT its
/// selection and (if associative) the 1-based section it goes with.
struct Section {
    name: &'static str,
    characteristics: u32,
    data: Vec<u8>,
    relocations: Vec<(u32, u32, u16)>,
    comdat: Option<(u8, u16)>,
}

/// Code (`.text$mn`), a COMDAT (selection any) if `comdat`.
fn code(data: &[u8], relocations: &[(u32, u32, u16)], comdat: bool) -> Section {
    Section {
        name: ".text$mn",
        characteristics: 0x6050_0020,
        data: data.to_vec(),
        relocations: relocations.to_vec(),
        comdat: comdat.then_some((2, 0)),
    }
}

/// A symbol of a made object: name, value, section (1-based; 0
/// undefined), type, storage class.
struct Symbol(&'static str, u32, i16, u16, u8);

/// An x64 object of `sections` and `symbols`. Each section has its symbol
/// (with its auxiliary record) first, so `symbols[k]` is symbol
/// `2 * sections.len() + k`.
fn coff(sections: &[Section], symbols: &[Symbol]) -> Vec<u8> {
    let mut strings = Vec::new();
    let mut name8 = |n: &str| -> [u8; 8] {
        let n = n.as_bytes();
        let mut r = [0u8; 8];
        if n.len() <= 8 {
            r[..n.len()].copy_from_slice(n);
        } else {
            r[4..].copy_from_slice(&(4 + n32(strings.len())).to_le_bytes());
            strings.extend_from_slice(n);
            strings.push(0);
        }
        r
    };
    let mut records = Vec::new();
    for (i, sec) in sections.iter().enumerate() {
        records.extend_from_slice(&name8(sec.name));
        records.extend_from_slice(&0u32.to_le_bytes());
        records.extend_from_slice(&(i16::try_from(i).unwrap() + 1).to_le_bytes());
        records.extend_from_slice(&[0, 0, 3, 1]);
        let (selection, with) = sec.comdat.unwrap_or((0, 0));
        records.extend_from_slice(&n32(sec.data.len()).to_le_bytes());
        records.extend_from_slice(&u16::try_from(sec.relocations.len()).unwrap().to_le_bytes());
        records.extend_from_slice(&[0; 6]); // line numbers, checksum
        records.extend_from_slice(&with.to_le_bytes());
        records.extend_from_slice(&[selection, 0, 0, 0]);
    }
    for Symbol(name, value, section, ty, class) in symbols {
        records.extend_from_slice(&name8(name));
        records.extend_from_slice(&value.to_le_bytes());
        records.extend_from_slice(&section.to_le_bytes());
        records.extend_from_slice(&ty.to_le_bytes());
        records.extend_from_slice(&[*class, 0]);
    }
    let mut headers = Vec::new();
    let mut body = Vec::new();
    let start = 20 + 40 * sections.len();
    for sec in sections {
        let data_at = start + body.len();
        body.extend_from_slice(&sec.data);
        let relocations_at = start + body.len();
        for &(at, symbol, kind) in &sec.relocations {
            body.extend_from_slice(&at.to_le_bytes());
            body.extend_from_slice(&symbol.to_le_bytes());
            body.extend_from_slice(&kind.to_le_bytes());
        }
        let short = sec.name.as_bytes();
        assert!(short.len() <= 8);
        let mut n = [0u8; 8];
        n[..short.len()].copy_from_slice(short);
        headers.extend_from_slice(&n);
        let relocated = if sec.relocations.is_empty() {
            0
        } else {
            n32(relocations_at)
        };
        for v in [0, 0, n32(sec.data.len()), n32(data_at), relocated, 0] {
            headers.extend_from_slice(&v.to_le_bytes());
        }
        headers.extend_from_slice(&u16::try_from(sec.relocations.len()).unwrap().to_le_bytes());
        headers.extend_from_slice(&0u16.to_le_bytes());
        let comdat = if sec.comdat.is_some() { 0x1000 } else { 0 };
        headers.extend_from_slice(&(sec.characteristics | comdat).to_le_bytes());
    }
    let mut o = Vec::new();
    o.extend_from_slice(&0x8664u16.to_le_bytes());
    o.extend_from_slice(&u16::try_from(sections.len()).unwrap().to_le_bytes());
    o.extend_from_slice(&0u32.to_le_bytes());
    o.extend_from_slice(&n32(start + body.len()).to_le_bytes());
    o.extend_from_slice(&n32(2 * sections.len() + symbols.len()).to_le_bytes());
    o.extend_from_slice(&[0; 4]);
    o.extend(headers);
    o.extend(body);
    o.extend(records);
    o.extend_from_slice(&(4 + n32(strings.len())).to_le_bytes());
    o.extend(strings);
    o
}

/// The COFF groups the image's POGO (coffgrp) debug entry lists: (RVA,
/// size, name).
fn coff_groups(image: &[u8]) -> Vec<(u32, u32, String)> {
    let (debug, size) = directory(image, 6);
    let entries = at_rva(image, debug, size as usize).to_vec();
    let mut groups = Vec::new();
    for e in entries.chunks(28) {
        if u32_at(e, 12) != 13 {
            continue;
        }
        let data = at_rva(image, u32_at(e, 20), u32_at(e, 16) as usize).to_vec();
        let mut at = 4;
        while at + 8 <= data.len() {
            let (rva, len) = (u32_at(&data, at), u32_at(&data, at + 4));
            let end = data[at + 8..].iter().position(|&b| b == 0).unwrap() + at + 8;
            groups.push((
                rva,
                len,
                String::from_utf8_lossy(&data[at + 8..end]).into_owned(),
            ));
            at = (end + 1 + 3) & !3;
        }
    }
    groups
}

/// `main` (a function) calling `a`..`f`, each code of its own object
/// (`mov al, n; ret`, or with a byte before it):
/// - a: not a COMDAT, a function;
/// - b: not a COMDAT, a symbol without a type (as an assembler's);
/// - c: a COMDAT without a type;
/// - d: a COMDAT function;
/// - e: a static function at 1, `e` (no type) at 0;
/// - f: `f` (no type) at 0 and a function `ff` at 0 too (padded).
fn padded_inputs(dir: &Path) {
    let calls: Vec<u8> = (0..6)
        .flat_map(|_| [0xE8, 0, 0, 0, 0])
        .chain([0xC3])
        .collect();
    let relocations: Vec<(u32, u32, u16)> = (0..6).map(|k| (1 + 5 * k, 3 + k, 4)).collect();
    let mut symbols = vec![Symbol("main", 0, 1, 0x20, 2)];
    for n in ["a", "b", "c", "d", "e", "f"] {
        symbols.push(Symbol(n, 0, 0, 0, 2));
    }
    std::fs::write(
        dir.join("main.obj"),
        coff(&[code(&calls, &relocations, false)], &symbols),
    )
    .unwrap();
    let one = |name: &str, body: &[u8], comdat: bool, symbols: &[Symbol]| {
        std::fs::write(
            dir.join(format!("{name}.obj")),
            coff(&[code(body, &[], comdat)], symbols),
        )
        .unwrap();
    };
    one("a", &[0xB0, 1, 0xC3], false, &[Symbol("a", 0, 1, 0x20, 2)]);
    one("b", &[0xB0, 2, 0xC3], false, &[Symbol("b", 0, 1, 0, 2)]);
    one("c", &[0xB0, 3, 0xC3], true, &[Symbol("c", 0, 1, 0, 2)]);
    one("d", &[0xB0, 4, 0xC3], true, &[Symbol("d", 0, 1, 0x20, 2)]);
    one(
        "e",
        &[0xB0, 5, 0xC3, 0xC3],
        false,
        &[Symbol("e", 0, 1, 0, 2), Symbol("s", 1, 1, 0x20, 3)],
    );
    one(
        "f",
        &[0xB0, 6, 0xC3],
        false,
        &[Symbol("f", 0, 1, 0, 2), Symbol("ff", 0, 1, 0x20, 2)],
    );
}

/// /FUNCTIONPADMIN: padding (int3) before each piece of code with a
/// function at its start (COMDAT or not: by the symbol's type), then its
/// alignment; none before code an assembler's symbols name. The group
/// starts before the padding. Without a number, x64's is 6 bytes.
#[test]
fn function_padding_is_link_exes() {
    let dir = scratch("padmin");
    padded_inputs(&dir);
    let link = |pad: &str| {
        let mut words = vec![
            "/NODEFAULTLIB",
            "/ENTRY:main",
            "/SUBSYSTEM:CONSOLE",
            pad,
            "main.obj",
        ];
        words.extend(["a.obj", "b.obj", "c.obj", "d.obj", "e.obj", "f.obj"]);
        run(&dir, &words).expect("links")
    };
    let targets = |image: &[u8], main: u32| {
        let code = at_rva(image, main, 30).to_vec();
        (0..6)
            .map(|k| rel32(&code, main, 1 + 5 * k))
            .collect::<Vec<u32>>()
    };
    let plain = link("/OPT:NOREF");
    assert_eq!(
        targets(&plain, 0x1000),
        [0x1020, 0x1030, 0x1040, 0x1050, 0x1060, 0x1070]
    );
    // main: 16 bytes of padding, then main (31 bytes); a padded (16, then
    // aligned to 16); b, c not; d padded; e not; f padded.
    let sixteen = link("/FUNCTIONPADMIN:16");
    assert_eq!(
        targets(&sixteen, 0x1010),
        [0x1040, 0x1050, 0x1060, 0x1080, 0x1090, 0x10B0]
    );
    assert!(at_rva(&sixteen, 0x1000, 16).iter().all(|&b| b == 0xCC));
    assert!(at_rva(&sixteen, 0x1043, 13).iter().all(|&b| b == 0xCC));
    let groups = coff_groups(&sixteen);
    assert_eq!(
        (groups[0].0, groups[0].1, groups[0].2.as_str()),
        (0x1000, 0xB3, ".text$mn")
    );
    // 6: main on 16 after the padding, a 6 bytes past its end, aligned.
    let six = link("/FUNCTIONPADMIN");
    assert_eq!(
        targets(&six, 0x1010),
        [0x1040, 0x1050, 0x1060, 0x1070, 0x1080, 0x1090]
    );
    let _ = std::fs::remove_dir_all(&dir);
}

/// A DLL's export directory goes after the inputs' groups and before the
/// imports'; link.exe's .rdata$voltmd and .rdata$zzzdbg then go before the
/// first group that sorts after them: a DLL without imports or unwind
/// data has .rdata, .edata, .rdata$voltmd, .rdata$zzzdbg.
#[test]
fn the_export_directory_is_where_link_exe_puts_it() {
    let dir = scratch("edata");
    std::fs::write(dir.join("e.obj"), defining("exported")).unwrap();
    let image = run(
        &dir,
        &[
            "/NODEFAULTLIB",
            "/DLL",
            "/NOENTRY",
            "/EXPORT:exported",
            "e.obj",
        ],
    )
    .expect("links");
    let names: Vec<String> = coff_groups(&image).into_iter().map(|g| g.2).collect();
    assert_eq!(
        names,
        [
            ".text",
            ".rdata",
            ".edata",
            ".rdata$voltmd",
            ".rdata$zzzdbg"
        ]
    );
    // With unwind data (.xdata), .edata is after it, and link.exe's own
    // groups before it.
    let xdata = Section {
        name: ".xdata",
        characteristics: 0x4030_0040,
        data: vec![1, 4, 1, 0, 4, 0x42, 0, 0],
        relocations: Vec::new(),
        comdat: None,
    };
    let object = coff(
        &[code(&[0xC3], &[], false), xdata],
        &[Symbol("exported", 0, 1, 0x20, 2)],
    );
    std::fs::write(dir.join("u.obj"), object).unwrap();
    let image = run(
        &dir,
        &[
            "/NODEFAULTLIB",
            "/DLL",
            "/NOENTRY",
            "/EXPORT:exported",
            "u.obj",
        ],
    )
    .expect("links");
    let names: Vec<String> = coff_groups(&image).into_iter().map(|g| g.2).collect();
    assert_eq!(
        names,
        [
            ".text$mn",
            ".rdata",
            ".rdata$voltmd",
            ".rdata$zzzdbg",
            ".xdata",
            ".edata"
        ]
    );
    let _ = std::fs::remove_dir_all(&dir);
}

/// `main` calls `f` and `g`, COMDATs, each with unwind data (`.xdata`,
/// associative): the order main, f, g.
fn ordered_inputs(dir: &Path) {
    let xdata = |n: u8, with: u16| Section {
        name: ".xdata",
        characteristics: 0x4030_0040,
        data: vec![1, 4, 1, 0, 4, 0x42, 0, 0, n],
        relocations: Vec::new(),
        comdat: Some((5, with)),
    };
    let sections = [
        code(
            &[0xE8, 0, 0, 0, 0, 0xE8, 0, 0, 0, 0, 0xC3],
            &[(1, 11, 4), (6, 12, 4)],
            true,
        ),
        code(&[0xB0, 1, 0xC3], &[], true),
        xdata(1, 2),
        code(&[0xB0, 2, 0xC3], &[], true),
        xdata(2, 4),
    ];
    let symbols = [
        Symbol("main", 0, 1, 0x20, 2),
        Symbol("f", 0, 2, 0x20, 2),
        Symbol("g", 0, 4, 0x20, 2),
    ];
    std::fs::write(dir.join("o.obj"), coff(&sections, &symbols)).unwrap();
}

/// /ORDER:@file: the functions it names go first in their group, in its
/// order, the rest after them as they were; their associative sections
/// (unwind data) follow them. A name nothing defines is LNK4037.
#[test]
fn order_is_link_exes() {
    let dir = scratch("order");
    ordered_inputs(&dir);
    std::fs::write(dir.join("order.txt"), "g\nnosuchfunction\nf\n").unwrap();
    let order = format!("/ORDER:@{}", input(&dir, "order.txt"));
    let words = [
        "/NODEFAULTLIB",
        "/ENTRY:main",
        "/SUBSYSTEM:CONSOLE",
        order.as_str(),
        "o.obj",
    ];
    let (image, warnings) = run_with_warnings(&dir, &words);
    let image = image.expect("links");
    assert_eq!(warnings, [4037]);
    // g, f, main.
    let code = at_rva(&image, 0x1020, 11).to_vec();
    assert_eq!(at_rva(&image, 0x1000, 3), [0xB0, 2, 0xC3]);
    assert_eq!(at_rva(&image, 0x1010, 3), [0xB0, 1, 0xC3]);
    assert_eq!(
        [rel32(&code, 0x1020, 1), rel32(&code, 0x1020, 6)],
        [0x1010, 0x1000]
    );
    // g's unwind data first.
    let groups = coff_groups(&image);
    let xdata = groups
        .iter()
        .find(|g| g.2 == ".xdata")
        .expect("an .xdata group");
    assert_eq!(at_rva(&image, xdata.0 + 8, 1), [2]);
    let _ = std::fs::remove_dir_all(&dir);
}

/// An archive's members: each header and its data.
fn archive_members(d: &[u8]) -> Vec<(&[u8], &[u8])> {
    assert_eq!(&d[..8], b"!<arch>\n");
    let mut out = Vec::new();
    let mut at = 8;
    while at < d.len() {
        let h = &d[at..at + 60];
        let size: usize = std::str::from_utf8(&h[48..58])
            .unwrap()
            .trim()
            .parse()
            .unwrap();
        out.push((h, &d[at + 60..at + 60 + size]));
        at += 60 + size + (size & 1);
    }
    out
}

/// A DLL's import library, as link.exe 14.52 writes it: every member's
/// date -1; the descriptors (each with a `.debug$S` of `S_OBJNAME_ST` and
/// `S_COMPILE2_ST`, and an `@comp.id` for product 0x101), then the short
/// imports in the order of their names (bytes: `B` < `_` < `a`); the first
/// linker member's symbols in the order the exports were given, each
/// member's sorted, and the second linker member's members in that order
/// too; each member's time stamp the first 4 bytes of its SHA-256 (with the
/// stamp 0xFFFFFFFF) with the top two bits set; an export given an ordinal
/// imported by it. And the image's ordinals: the unnumbered exports, by
/// name, take the free ones from the smallest given up.
#[test]
fn the_import_library_is_link_exes() {
    use sha2::Digest;
    let dir = scratch("implib");
    let names = ["zeta", "alpha", "Beta", "_under", "impl1", "impl2", "impl4"];
    let syms: Vec<Sym> = names.iter().map(|&n| Sym(n, 1)).collect();
    std::fs::write(dir.join("f.obj"), object(&[0xC3], &[], &syms)).unwrap();
    let image = run(
        &dir,
        &[
            "/NODEFAULTLIB",
            "/NOENTRY",
            "/DLL",
            "/EXPORT:zeta",
            "/EXPORT:x=impl2,@7",
            "/EXPORT:alpha",
            "/EXPORT:Beta",
            "/EXPORT:impl1,DATA",
            "/EXPORT:impl4,@4,NONAME",
            "/EXPORT:_under",
            "f.obj",
        ],
    )
    .expect("links");
    let lib = std::fs::read(dir.join("out.lib")).expect("the import library");
    let members = archive_members(&lib);
    assert_eq!(members.len(), 2 + 3 + 7);
    for (h, _) in &members {
        assert_eq!(&h[16..48], b"-1                      0       ", "the date");
    }
    let stamp = |data: &[u8], at: usize| {
        let mut d = data.to_vec();
        d[at..at + 4].fill(0xFF);
        let h = sha2::Sha256::digest(&d);
        u32::from_le_bytes([h[0], h[1], h[2], h[3]]) | 0xC000_0000
    };
    let section_names = |o: &[u8]| -> Vec<String> {
        (0..usize::from(u16_at(o, 2)))
            .map(|i| {
                String::from_utf8_lossy(&o[20 + 40 * i..28 + 40 * i])
                    .trim_end_matches('\0')
                    .to_owned()
            })
            .collect()
    };
    // The descriptors' objects.
    let wanted: [&[&str]; 3] = [
        &[".debug$S", ".idata$2", ".idata$6"],
        &[".debug$S", ".idata$3"],
        &[".debug$S", ".idata$5", ".idata$4"],
    ];
    let mut want_debug = 2u32.to_le_bytes().to_vec();
    want_debug.extend_from_slice(&[2 + 4 + 1 + 7, 0, 9, 0, 0, 0, 0, 0, 7]);
    want_debug.extend_from_slice(b"out.img");
    want_debug.extend_from_slice(&[39, 0, 0x13, 0x10, 7, 0, 0, 0, 0xD0, 0]);
    want_debug.extend_from_slice(&[0, 0, 0, 0, 0, 0, 14, 0, 52, 0, 0xA5, 0x90, 18]);
    want_debug.extend_from_slice(b"Microsoft (R) LINK");
    for (k, want) in wanted.iter().enumerate() {
        let (h, o) = members[2 + k];
        assert_eq!(&h[..16], b"out.img/        ");
        assert_eq!(section_names(o), *want);
        assert_eq!(u32_at(o, 4), stamp(o, 4), "the time stamp");
        // .debug$S: on 1, discardable, read-only.
        assert_eq!(u32_at(o, 20 + 36), 0x4210_0040);
        let at = u32_at(o, 20 + 20) as usize;
        assert_eq!(&o[at..at + u32_at(o, 20 + 16) as usize], want_debug);
        // The first symbol: @comp.id, absolute.
        let symbols = u32_at(o, 8) as usize;
        assert_eq!(&o[symbols..symbols + 8], b"@comp.id");
        assert_eq!(u32_at(o, symbols + 8), 0x0101_90A5);
        assert_eq!(u16_at(o, symbols + 12), 0xFFFF);
    }
    // The descriptor's .idata$6 has as its relocations' pointer .idata$2's
    // (it has none).
    let d = members[2].1;
    assert_eq!(u16_at(d, 20 + 40 + 32), 3);
    assert_eq!(u32_at(d, 20 + 80 + 24), u32_at(d, 20 + 40 + 24));
    // The imports, by name: (name, hint or ordinal, type: code 0, data 1,
    // and the name type (by name 1, by ordinal 0) << 2).
    let imports: Vec<(&[u8], u16, u16)> = members[5..]
        .iter()
        .map(|(_, o)| {
            assert_eq!(u32_at(o, 8), stamp(o, 8), "the time stamp");
            let name = o[20..].split(|&b| b == 0).next().unwrap();
            (name, u16_at(o, 16), u16_at(o, 18))
        })
        .collect();
    assert_eq!(
        imports,
        [
            (&b"Beta"[..], 0, 4),
            (b"_under", 1, 4),
            (b"alpha", 2, 4),
            (b"impl1", 3, 5),
            (b"impl4", 4, 0),
            (b"x", 7, 0),
            (b"zeta", 5, 4),
        ]
    );
    // Each import member's offset in the file.
    let mut offsets = std::collections::HashMap::new();
    let mut at = 8u32;
    for (_, o) in &members {
        if o.starts_with(&[0, 0, 0xFF, 0xFF]) {
            let name = o[20..].split(|&b| b == 0).next().unwrap();
            offsets.insert(name.to_vec(), at);
        }
        at += 60 + n32(o.len() + (o.len() & 1));
    }
    // The first linker member: symbols as the exports were given, each
    // member's sorted ("Beta" < "__imp_Beta" < "__imp_alpha" < "alpha"),
    // and the offsets of the members where they are.
    let first = members[0].1;
    let count = u32::from_be_bytes(first[..4].try_into().unwrap()) as usize;
    let strings: Vec<&[u8]> = first[4 + 4 * count..]
        .split(|&b| b == 0)
        .take(count)
        .collect();
    assert_eq!(
        strings,
        [
            &b"__IMPORT_DESCRIPTOR_out"[..],
            b"__NULL_IMPORT_DESCRIPTOR",
            b"\x7fout_NULL_THUNK_DATA",
            b"__imp_zeta",
            b"zeta",
            b"__imp_x",
            b"x",
            b"__imp_alpha",
            b"alpha",
            b"Beta",
            b"__imp_Beta",
            b"__imp_impl1",
            b"__imp_impl4",
            b"impl4",
            b"__imp__under",
            b"_under",
        ]
    );
    let be = |i: usize| u32::from_be_bytes(first[4 + 4 * i..8 + 4 * i].try_into().unwrap());
    assert_eq!(be(3), offsets[&b"zeta"[..]]);
    assert_eq!(be(9), offsets[&b"Beta"[..]]);
    // The second linker member's members: in that order too.
    let second = members[1].1;
    assert_eq!(u32_at(second, 0), 10);
    let table: Vec<u32> = (3..10).map(|i| u32_at(second, 4 + 4 * i)).collect();
    let given = ["zeta", "x", "alpha", "Beta", "impl1", "impl4", "_under"];
    let want: Vec<u32> = given.iter().map(|n| offsets[n.as_bytes()]).collect();
    assert_eq!(table, want);
    // The image's ordinals: 4 and 7 given; Beta, _under, alpha, impl1,
    // zeta (by name) take 5, 6, 8, 9, 10.
    let (edata, _) = directory(&image, 0);
    let header = at_rva(&image, edata, 40);
    assert_eq!(u32_at(header, 16), 4, "the base");
    assert_eq!(u32_at(header, 20), 7, "the functions");
    let (names_at, ordinals_at) = (u32_at(header, 32), u32_at(header, 36));
    let mut by_name = Vec::new();
    for k in 0..u32_at(header, 24) {
        let name_rva = u32_at(at_rva(&image, names_at + 4 * k, 4), 0);
        let name = at_rva(&image, name_rva, 8)
            .split(|&b| b == 0)
            .next()
            .unwrap();
        let ordinal = u16_at(at_rva(&image, ordinals_at + 2 * k, 2), 0) + 4;
        by_name.push((String::from_utf8_lossy(name).into_owned(), ordinal));
    }
    let want: Vec<(String, u16)> = [
        ("Beta", 5),
        ("_under", 6),
        ("alpha", 8),
        ("impl1", 9),
        ("x", 7),
        ("zeta", 10),
    ]
    .iter()
    .map(|&(n, o)| (n.to_owned(), o))
    .collect();
    assert_eq!(by_name, want);
    let _ = std::fs::remove_dir_all(&dir);
}

/// The export directory's strings, as link.exe lays them out: after the
/// DLL's name, by export in the order of their names (not what `EXPORTAS`
/// exports them as), each's name then its forwarder's; the name pointers
/// sorted by exported name. And a name exported twice is kept once, the
/// first (the command line's before an object's directive), by its name:
/// `EXPORTAS` aside.
#[test]
fn the_export_directorys_strings_are_link_exes() {
    let dir = scratch("edata-strings");
    // (link.exe wants the forwarded function's name defined, Target.)
    let syms = [Sym("alpha", 1), Sym("beta", 1), Sym("Target", 1)];
    std::fs::write(dir.join("f.obj"), object(&[0xC3], &[], &syms)).unwrap();
    std::fs::write(
        dir.join("f.def"),
        "LIBRARY out.img\nEXPORTS\n    fwd = other.Target\n",
    )
    .unwrap();
    let def = format!("/DEF:{}", input(&dir, "f.def"));
    let image = run(
        &dir,
        &[
            "/NODEFAULTLIB",
            "/NOENTRY",
            "/DLL",
            "/EXPORT:beta",
            &def,
            "/EXPORT:alpha,EXPORTAS,zz",
            "/EXPORT:alpha",
            "f.obj",
        ],
    )
    .expect("links");
    let (edata, _) = directory(&image, 0);
    let header = at_rva(&image, edata, 40);
    assert_eq!(u32_at(header, 20), 3, "three functions");
    let names = u32_at(header, 24);
    assert_eq!(names, 3);
    let want = b"out.img\0zz\0beta\0fwd\0other.Target\0";
    let strings = at_rva(&image, u32_at(header, 12), want.len());
    assert_eq!(strings, want);
    // The names sorted, and their ordinals: by name, alpha (zz), beta,
    // fwd.
    let (names_at, ordinals_at) = (u32_at(header, 32), u32_at(header, 36));
    let mut sorted = Vec::new();
    for k in 0..names {
        let rva = u32_at(at_rva(&image, names_at + 4 * k, 4), 0);
        let name = at_rva(&image, rva, 4).split(|&b| b == 0).next().unwrap();
        let ordinal = u16_at(at_rva(&image, ordinals_at + 2 * k, 2), 0) + 1;
        sorted.push((String::from_utf8_lossy(name).into_owned(), ordinal));
    }
    let want: Vec<(String, u16)> = [("beta", 2), ("fwd", 3), ("zz", 1)]
        .iter()
        .map(|&(n, o)| (n.to_owned(), o))
        .collect();
    assert_eq!(sorted, want);
    let _ = std::fs::remove_dir_all(&dir);
}

/// `/GUARD:CF` with an object not compiled with `/guard:cf` (no
/// `@feat.00`): its functions that a `REL32` or `ADDR64` relocation refers
/// to are targets, calls included, an import's thunk too, as their
/// definitions are typed; not those `ADDR32NB` or `REL32_1` refer to, nor
/// one defined untyped. In an object compiled with it (`@feat.00` 0x800),
/// a call makes no target (a call to it from one compiled without does).
/// (As link.exe 14.52 builds the table.)
#[test]
fn guard_cf_takes_the_targets_of_objects_compiled_without_it() {
    let dir = scratch("guard-nocf");
    let t = |name, value, ty| Typed {
        name,
        section: 1,
        value,
        ty,
        class: 2,
    };
    let syms = [
        t("main", 0, 0x20),
        t("fA", 0x40, 0x20),
        t("fB", 0x50, 0),
        t("fC", 0x60, 0x20),
        t("fD", 0x70, 0x20),
        t("fE", 0x80, 0x20),
        Typed {
            class: 3,
            ..t("fF", 0x90, 0x20)
        },
        Typed {
            section: 0,
            ..t("puts", 0, 0)
        },
        Typed {
            section: 0,
            ..t("__guard_fids_table", 0, 0)
        },
        Typed {
            section: 0,
            ..t("__guard_fids_count", 0, 0)
        },
        Typed {
            section: 0,
            ..t("gcaller", 0, 0x20)
        },
    ];
    // main calls fA, fB, puts, fF, gcaller, and fD (REL32_1); then data:
    // fC's ADDR32NB, the table's address and count, fE's address.
    let mut text = vec![0; 0xA0];
    for at in [0, 5, 10, 15, 20, 26] {
        text[at] = 0xE8;
    }
    text[25] = 0x90;
    text[31] = 0xC3;
    for at in [0x40, 0x50, 0x60, 0x70, 0x80, 0x90] {
        text[at] = 0xC3;
    }
    let (r32, r32_1, nb32, a64) = (4, 5, 3, 1);
    let relocations = [
        (1, 1, r32),
        (6, 2, r32),
        (11, 7, r32),
        (16, 6, r32),
        (21, 4, r32_1),
        (27, 10, r32),
        (0x20, 3, nb32),
        (0x28, 8, a64),
        (0x30, 9, a64),
        (0x38, 5, a64),
    ];
    std::fs::write(
        dir.join("main.obj"),
        object_typed(&text, &relocations, &syms),
    )
    .unwrap();
    // g.obj, compiled with /guard:cf: gcaller calls gx.
    let g = [
        Typed {
            name: "@feat.00",
            section: -1,
            value: 0x800,
            ty: 0,
            class: 3,
        },
        t("gx", 0, 0x20),
        t("gcaller", 0x10, 0x20),
    ];
    let mut gtext = vec![0; 0x20];
    gtext[0] = 0xC3;
    gtext[0x10] = 0xE8;
    gtext[0x15] = 0xC3;
    std::fs::write(
        dir.join("g.obj"),
        object_typed(&gtext, &[(0x11, 1, r32)], &g),
    )
    .unwrap();
    std::fs::write(
        dir.join("x.lib"),
        archive(&[("x.dll", import("x.dll", "puts", 0), &["puts", "__imp_puts"])]),
    )
    .unwrap();
    let image = run(
        &dir,
        &[
            "/NODEFAULTLIB",
            "/ENTRY:main",
            "/SUBSYSTEM:CONSOLE",
            "/GUARD:CF",
            "main.obj",
            "g.obj",
            "x.lib",
        ],
    )
    .expect("links");
    let text_section = &sections(&image)[0];
    assert_eq!(text_section.0, ".text");
    let base = 0x1_4000_0000u64;
    let code = at_rva(&image, text_section.2, 0xA0).to_vec();
    let rva = text_section.2;
    let at = |o: usize| u64::from_le_bytes(code[o..o + 8].try_into().unwrap());
    let table = u32::try_from(at(0x28) - base).unwrap();
    let count = usize::try_from(at(0x30)).unwrap();
    let entries: Vec<u32> = (0..count)
        .map(|i| u32_at(at_rva(&image, table + n32(4 * i), 4), 0))
        .collect();
    let target = |o| rel32(&code, rva, o);
    let g_text = target(27) - 0x10;
    let gx = rel32(at_rva(&image, g_text, 0x20), g_text, 0x11);
    let mut expected = vec![
        rva,        // main, the entry point
        target(1),  // fA
        target(11), // puts's thunk
        target(16), // fF, static
        rva + 0x80, // fE, by its address
        target(27), // gcaller, in a /guard:cf object, called from main
    ];
    expected.sort_unstable();
    assert_eq!(entries, expected);
    for (what, not) in [
        ("fB, untyped", target(6)),
        ("fC, by ADDR32NB", rva + 0x60),
        ("fD, by REL32_1", rva + 0x70),
        ("gx, called from a /guard:cf object", gx),
    ] {
        assert!(!entries.contains(&not), "{what}");
    }
    let _ = std::fs::remove_dir_all(&dir);
}

/// The Rich header lists the command line's objects' tools as link.exe
/// walks them, from the last object to the first, each tool new to it put
/// at the front (so a tool used first and last is listed after one used
/// only between); then the linker.
#[test]
fn the_rich_header_lists_the_command_lines_tools_in_order() {
    let dir = scratch("rich-order");
    let made_by = |name: &'static str, comp_id: u32| {
        let syms = [
            Typed {
                name: "@comp.id",
                section: -1,
                value: comp_id,
                ty: 0,
                class: 3,
            },
            Typed {
                name,
                section: 1,
                value: 0,
                ty: 0x20,
                class: 2,
            },
        ];
        object_typed(&[0xC3], &[], &syms)
    };
    std::fs::write(dir.join("a.obj"), made_by("main", 0x0104_0001)).unwrap();
    std::fs::write(dir.join("b.obj"), made_by("b", 0x0105_0002)).unwrap();
    std::fs::write(dir.join("c.obj"), made_by("c", 0x0104_0001)).unwrap();
    for (order, expected) in [
        (
            ["a.obj", "b.obj", "c.obj"],
            [(0x0105_0002, 1), (0x0104_0001, 2)],
        ),
        (
            ["a.obj", "c.obj", "b.obj"],
            [(0x0104_0001, 2), (0x0105_0002, 1)],
        ),
    ] {
        let mut words = vec!["/NODEFAULTLIB", "/ENTRY:main", "/SUBSYSTEM:CONSOLE"];
        words.extend(order);
        let image = run(&dir, &words).expect("links");
        let key = u32_at(&image, 0x84);
        let entry = |i: usize| {
            (
                u32_at(&image, 0x90 + 8 * i) ^ key,
                u32_at(&image, 0x94 + 8 * i) ^ key,
            )
        };
        assert_eq!([entry(0), entry(1)], expected, "{order:?}");
        assert_eq!(entry(2), (0x0102_90A5, 1));
    }
    let _ = std::fs::remove_dir_all(&dir);
}

/// Links `words` in `dir` to `dir/name` (its import library, with exports,
/// `dir/` the name's stem `.lib`): the image's bytes, or the errors' codes.
fn run_as(dir: &Path, words: &[&str], name: &str) -> Result<Vec<u8>, Vec<u32>> {
    let argv = words.iter().map(|w| {
        if w.starts_with('/') {
            (*w).to_owned()
        } else {
            input(dir, w)
        }
    });
    let words = args::command_line(argv, None, None, &|p| std::fs::read(p)).unwrap();
    let mut diags = Vec::new();
    let mut config = config::parse(&words, &mut diags);
    let out = dir.join(name);
    config.out = Some(out.clone());
    let outcome = link::link(config);
    if outcome.diagnostics.iter().any(|d| !d.warning) || !outcome.written {
        return Err(outcome.diagnostics.iter().map(|d| d.code).collect());
    }
    Ok(std::fs::read(out).unwrap())
}

/// An x64 object: `main` (a non-COMDAT `.text$mn`) calls each of `live`
/// through its `__imp_` slot; `dead` (a COMDAT, any, that nothing calls)
/// calls each of `dead`. The names are 2 bytes (`__imp_xy`: 8).
fn live_and_dead(live: &[&str], dead: &[&str]) -> Vec<u8> {
    let calls = |names: &[&str], first: u32| {
        let mut text = Vec::new();
        let mut relocations = Vec::new();
        for (i, _) in names.iter().enumerate() {
            text.extend_from_slice(&[0xFF, 0x15, 0, 0, 0, 0]);
            relocations.push((n32(6 * i + 2), first + n32(i)));
        }
        text.push(0xC3);
        (text, relocations)
    };
    // Symbols: the COMDAT's section symbol and its aux, main, dead, then
    // the live imports and the dead ones.
    let (text1, rel1) = calls(live, 4);
    let (text2, rel2) = calls(dead, 4 + n32(live.len()));
    let at1 = 20 + 80;
    let rel1_at = at1 + text1.len();
    let at2 = rel1_at + 10 * rel1.len();
    let rel2_at = at2 + text2.len();
    let symbols_at = rel2_at + 10 * rel2.len();
    let count = 4 + live.len() + dead.len();
    let mut o = Vec::new();
    for v in [0x8664u16, 2] {
        o.extend_from_slice(&v.to_le_bytes());
    }
    for v in [0u32, n32(symbols_at), n32(count)] {
        o.extend_from_slice(&v.to_le_bytes());
    }
    o.extend_from_slice(&[0; 4]);
    let header = |o: &mut Vec<u8>, size: usize, raw: usize, rel: usize, n: usize, chars: u32| {
        o.extend_from_slice(b".text$mn");
        for v in [0, 0, n32(size), n32(raw), n32(rel), 0] {
            o.extend_from_slice(&v.to_le_bytes());
        }
        o.extend_from_slice(&u16::try_from(n).unwrap().to_le_bytes());
        o.extend_from_slice(&0u16.to_le_bytes());
        o.extend_from_slice(&chars.to_le_bytes());
    };
    header(&mut o, text1.len(), at1, rel1_at, rel1.len(), 0x6050_0020);
    header(&mut o, text2.len(), at2, rel2_at, rel2.len(), 0x6050_1020);
    for (text, rel) in [(&text1, &rel1), (&text2, &rel2)] {
        o.extend_from_slice(text);
        for &(at, symbol) in rel {
            o.extend_from_slice(&at.to_le_bytes());
            o.extend_from_slice(&symbol.to_le_bytes());
            o.extend_from_slice(&4u16.to_le_bytes()); // REL32
        }
    }
    let symbol = |o: &mut Vec<u8>, name: &str, section: i16, ty: u16, class: u8, aux: u8| {
        let mut short = [0u8; 8];
        short[..name.len()].copy_from_slice(name.as_bytes());
        o.extend_from_slice(&short);
        o.extend_from_slice(&0u32.to_le_bytes());
        o.extend_from_slice(&section.to_le_bytes());
        o.extend_from_slice(&ty.to_le_bytes());
        o.push(class);
        o.push(aux);
    };
    symbol(&mut o, ".text$mn", 2, 0, 3, 1);
    o.extend_from_slice(&n32(text2.len()).to_le_bytes());
    o.extend_from_slice(&u16::try_from(rel2.len()).unwrap().to_le_bytes());
    o.extend_from_slice(&[0; 2 + 4 + 2]); // line numbers, checksum, number
    o.extend_from_slice(&[2, 0, 0, 0]); // selection: any
    symbol(&mut o, "main", 1, 0x20, 2, 0);
    symbol(&mut o, "dead", 2, 0x20, 2, 0);
    for n in live.iter().chain(dead) {
        symbol(&mut o, &format!("__imp_{n}"), 0, 0, 2, 0);
    }
    o.extend_from_slice(&4u32.to_le_bytes());
    o
}

/// Each DLL's imports (by name), in the import directory's order.
fn import_names(image: &[u8]) -> Vec<(String, Vec<String>)> {
    let (rva, size) = directory(image, 1);
    let name = |rva: u32| {
        let s = at_rva(image, rva, 64);
        String::from_utf8_lossy(&s[..s.iter().position(|&b| b == 0).unwrap()]).into_owned()
    };
    let mut out = Vec::new();
    for d in at_rva(image, rva, size as usize).chunks(20) {
        if u32_at(d, 12) == 0 {
            break;
        }
        let mut names = Vec::new();
        for k in 0.. {
            let entry = u32_at(at_rva(image, u32_at(d, 0) + 8 * k, 8), 0);
            if entry == 0 {
                break;
            }
            names.push(name(entry + 2));
        }
        out.push((name(u32_at(d, 12)), names));
    }
    out
}

/// The import lookup tables and the IAT are link.exe's `qsort` of every
/// import that loaded and each DLL's null thunk, by DLL name (each import
/// library's DLL's, without the extension). That includes a DLL none of
/// whose imports /OPT:REF keeps: its pieces are sorted too, then dropped
/// with it, and their places count. Here a.dll's three imports (only `dead`
/// calls them) and its null thunk come first, so b.dll's six come out of an
/// 11-piece partition (B0 to B5, in order), not the 7-piece selection sort
/// they'd get alone (B1, B2, B3, B4, B5, B0). (link.exe 14.52's orders for
/// the same inputs.)
#[test]
fn a_dll_with_nothing_live_counts_in_the_import_sort() {
    let dir = scratch("import-sort-dead");
    let exports = |dll: &str, names: &[&'static str]| {
        let syms: Vec<Sym> = names.iter().map(|&n| Sym(n, 1)).collect();
        let obj = format!("{dll}.obj");
        std::fs::write(dir.join(&obj), object(&[0xC3], &[], &syms)).unwrap();
        let mut w = vec!["/NODEFAULTLIB", "/NOENTRY", "/DLL"];
        let e: Vec<String> = names.iter().map(|n| format!("/EXPORT:{n}")).collect();
        w.extend(e.iter().map(String::as_str));
        w.push(&obj);
        run_as(&dir, &w, &format!("{dll}.dll")).expect("the DLL links");
    };
    let a = ["A0", "A1", "A2"];
    let b = ["B0", "B1", "B2", "B3", "B4", "B5"];
    exports("a", &a);
    exports("b", &b);
    std::fs::write(dir.join("main.obj"), live_and_dead(&b, &a)).unwrap();
    let link = |libs: &[&str], out: &str| {
        let mut w = vec![
            "/NODEFAULTLIB",
            "/ENTRY:main",
            "/SUBSYSTEM:CONSOLE",
            "/OPT:REF",
            "main.obj",
        ];
        w.extend_from_slice(libs);
        import_names(&run_as(&dir, &w, out).expect("links"))
    };
    let both = link(&["a.lib", "b.lib"], "both.exe");
    assert_eq!(both.len(), 1, "a.dll has nothing live: {both:?}");
    assert_eq!(both[0].0, "b.dll");
    assert_eq!(both[0].1, b);
    // Without a.lib (and so without its pieces), b.dll's alone.
    std::fs::write(dir.join("live.obj"), live_and_dead(&b, &[])).unwrap();
    let w = [
        "/NODEFAULTLIB",
        "/ENTRY:main",
        "/SUBSYSTEM:CONSOLE",
        "/OPT:REF",
        "live.obj",
        "b.lib",
    ];
    let alone = import_names(&run_as(&dir, &w, "alone.exe").expect("links"));
    assert_eq!(alone[0].1, ["B1", "B2", "B3", "B4", "B5", "B0"]);
    let _ = std::fs::remove_dir_all(&dir);
}
