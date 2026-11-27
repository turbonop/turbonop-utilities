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

/// Links `words` (a command line) in `dir`: the image's bytes, or the
/// errors' codes.
fn run(dir: &Path, words: &[&str]) -> Result<Vec<u8>, Vec<u32>> {
    let argv = words.iter().map(|w| {
        if w.starts_with('/') {
            (*w).to_owned()
        } else {
            dir.join(w).display().to_string()
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
