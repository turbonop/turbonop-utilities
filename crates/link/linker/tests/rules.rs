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
    let mut strings = Vec::new();
    let mut records = Vec::new();
    for Sym(name, section) in syms {
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
        records.extend_from_slice(&0u32.to_le_bytes()); // value
        records.extend_from_slice(&section.to_le_bytes());
        records.extend_from_slice(&(if *section > 0 { 0x20u16 } else { 0 }).to_le_bytes());
        records.push(2); // external
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
/// (`$6`); the bound table (`$7`, zeros, then 0x104 bytes of room); the
/// address table in `.data`, each slot the address of its import's
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
    // The name table: hint/names, then 0.
    let names = at_rva(&image, int, 24);
    for (k, (hint, name)) in [(7u16, &b"f\0"[..]), (9, b"g\0")].into_iter().enumerate() {
        let entry = u32_at(names, 8 * k);
        assert_eq!(u32_at(names, 8 * k + 4), 0);
        let hn = at_rva(&image, entry, 4);
        assert_eq!(u16_at(hn, 0), hint);
        assert_eq!(&hn[2..4], name);
    }
    assert_eq!(&names[16..24], &[0; 8]);
    // The bound table: zeros, and the room after it.
    assert!(at_rva(&image, bound, 24 + 0x104).iter().all(|&b| b == 0));
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
    // main's calls reach the slots.
    let call = at_rva(&image, 0x1000, 12);
    assert_eq!(rel32(call, 0x1000, 2), iat);
    assert_eq!(rel32(call, 0x1000, 8), iat + 8);
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
        let relocated = if sec.relocations.is_empty() { 0 } else { n32(relocations_at) };
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
            groups.push((rva, len, String::from_utf8_lossy(&data[at + 8..end]).into_owned()));
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
    let calls: Vec<u8> = (0..6).flat_map(|_| [0xE8, 0, 0, 0, 0]).chain([0xC3]).collect();
    let relocations: Vec<(u32, u32, u16)> = (0..6).map(|k| (1 + 5 * k, 3 + k, 4)).collect();
    let mut symbols = vec![Symbol("main", 0, 1, 0x20, 2)];
    for n in ["a", "b", "c", "d", "e", "f"] {
        symbols.push(Symbol(n, 0, 0, 0, 2));
    }
    std::fs::write(dir.join("main.obj"), coff(&[code(&calls, &relocations, false)], &symbols)).unwrap();
    let one = |name: &str, body: &[u8], comdat: bool, symbols: &[Symbol]| {
        std::fs::write(dir.join(format!("{name}.obj")), coff(&[code(body, &[], comdat)], symbols)).unwrap();
    };
    one("a", &[0xB0, 1, 0xC3], false, &[Symbol("a", 0, 1, 0x20, 2)]);
    one("b", &[0xB0, 2, 0xC3], false, &[Symbol("b", 0, 1, 0, 2)]);
    one("c", &[0xB0, 3, 0xC3], true, &[Symbol("c", 0, 1, 0, 2)]);
    one("d", &[0xB0, 4, 0xC3], true, &[Symbol("d", 0, 1, 0x20, 2)]);
    one("e", &[0xB0, 5, 0xC3, 0xC3], false, &[Symbol("e", 0, 1, 0, 2), Symbol("s", 1, 1, 0x20, 3)]);
    one("f", &[0xB0, 6, 0xC3], false, &[Symbol("f", 0, 1, 0, 2), Symbol("ff", 0, 1, 0x20, 2)]);
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
        let mut words = vec!["/NODEFAULTLIB", "/ENTRY:main", "/SUBSYSTEM:CONSOLE", pad, "main.obj"];
        words.extend(["a.obj", "b.obj", "c.obj", "d.obj", "e.obj", "f.obj"]);
        run(&dir, &words).expect("links")
    };
    let targets = |image: &[u8], main: u32| {
        let code = at_rva(image, main, 30).to_vec();
        (0..6).map(|k| rel32(&code, main, 1 + 5 * k)).collect::<Vec<u32>>()
    };
    let plain = link("/OPT:NOREF");
    assert_eq!(targets(&plain, 0x1000), [0x1020, 0x1030, 0x1040, 0x1050, 0x1060, 0x1070]);
    // main: 16 bytes of padding, then main (31 bytes); a padded (16, then
    // aligned to 16); b, c not; d padded; e not; f padded.
    let sixteen = link("/FUNCTIONPADMIN:16");
    assert_eq!(targets(&sixteen, 0x1010), [0x1040, 0x1050, 0x1060, 0x1080, 0x1090, 0x10B0]);
    assert!(at_rva(&sixteen, 0x1000, 16).iter().all(|&b| b == 0xCC));
    assert!(at_rva(&sixteen, 0x1043, 13).iter().all(|&b| b == 0xCC));
    let groups = coff_groups(&sixteen);
    assert_eq!((groups[0].0, groups[0].1, groups[0].2.as_str()), (0x1000, 0xB3, ".text$mn"));
    // 6: main on 16 after the padding, a 6 bytes past its end, aligned.
    let six = link("/FUNCTIONPADMIN");
    assert_eq!(targets(&six, 0x1010), [0x1040, 0x1050, 0x1060, 0x1070, 0x1080, 0x1090]);
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
    let image = run(&dir, &["/NODEFAULTLIB", "/DLL", "/NOENTRY", "/EXPORT:exported", "e.obj"])
        .expect("links");
    let names: Vec<String> = coff_groups(&image).into_iter().map(|g| g.2).collect();
    assert_eq!(names, [".text", ".rdata", ".edata", ".rdata$voltmd", ".rdata$zzzdbg"]);
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
        code(&[0xE8, 0, 0, 0, 0, 0xE8, 0, 0, 0, 0, 0xC3], &[(1, 11, 4), (6, 12, 4)], true),
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
    let words = ["/NODEFAULTLIB", "/ENTRY:main", "/SUBSYSTEM:CONSOLE", order.as_str(), "o.obj"];
    let (image, warnings) = run_with_warnings(&dir, &words);
    let image = image.expect("links");
    assert_eq!(warnings, [4037]);
    // g, f, main.
    let code = at_rva(&image, 0x1020, 11).to_vec();
    assert_eq!(at_rva(&image, 0x1000, 3), [0xB0, 2, 0xC3]);
    assert_eq!(at_rva(&image, 0x1010, 3), [0xB0, 1, 0xC3]);
    assert_eq!([rel32(&code, 0x1020, 1), rel32(&code, 0x1020, 6)], [0x1010, 0x1000]);
    // g's unwind data first.
    let groups = coff_groups(&image);
    let xdata = groups.iter().find(|g| g.2 == ".xdata").expect("an .xdata group");
    assert_eq!(at_rva(&image, xdata.0 + 8, 1), [2]);
    let _ = std::fs::remove_dir_all(&dir);
}
