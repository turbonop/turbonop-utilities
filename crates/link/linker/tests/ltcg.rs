//! LTCG's stand-in object: what resolution sees of a `/GL` object (its
//! definitions, COMDATs as the IL's are, its references, its directives);
//! and resolution's part in LTCG, driven through a stand-in for the back
//! end: a library's `/GL` member opened at its offset, the generated
//! object laid out first in the stand-ins' place, and the libraries
//! searched again for what it refers to.

use turbonop_global_alloc as _;

use std::path::{Path, PathBuf};

use turbonop_linker::c2::{IlObject, Public};
use turbonop_linker::object::{Object, Selection, SymbolKind};
use turbonop_linker::resolve::{self, Def, Ltcg, LtcgView};
use turbonop_linker::{args, config};

fn public(name: &str, kind: u32, comdat: Option<u8>) -> Public {
    Public {
        name: name.as_bytes().to_vec(),
        kind,
        handle: 0,
        comdat,
        refs: Vec::new(),
        size: 0,
        default: None,
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

fn n32(n: usize) -> u32 {
    u32::try_from(n).unwrap()
}

/// An x64 object of one `.text` section (`text`; a REL32 relocation at
/// each `(offset, symbol index)`) and the external symbols `syms` (name,
/// 1 defined in `.text` at 0, 0 undefined).
fn object(text: &[u8], relocations: &[(u32, u32)], syms: &[(&str, i16)]) -> Vec<u8> {
    let mut strings = Vec::new();
    let mut records = Vec::new();
    for (name, section) in syms {
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
        records.extend_from_slice(&0u32.to_le_bytes());
        records.extend_from_slice(&section.to_le_bytes());
        records.extend_from_slice(&0x20u16.to_le_bytes());
        records.push(2);
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
    let relocs_at = if relocations.is_empty() {
        0
    } else {
        n32(relocations_at)
    };
    for v in [0u32, 0, n32(text.len()), n32(text_at), relocs_at, 0] {
        o.extend_from_slice(&v.to_le_bytes());
    }
    o.extend_from_slice(&u16::try_from(relocations.len()).unwrap().to_le_bytes());
    o.extend_from_slice(&0u16.to_le_bytes());
    o.extend_from_slice(&0x6050_0020u32.to_le_bytes());
    o.extend_from_slice(text);
    for &(at, symbol) in relocations {
        o.extend_from_slice(&at.to_le_bytes());
        o.extend_from_slice(&symbol.to_le_bytes());
        o.extend_from_slice(&4u16.to_le_bytes());
    }
    o.extend_from_slice(&records);
    o.extend_from_slice(&(4 + n32(strings.len())).to_le_bytes());
    o.extend_from_slice(&strings);
    o
}

/// An archive of `members` (short names, each defining the names given),
/// with both linker members.
fn archive(members: &[(&str, &[u8], &[&str])]) -> Vec<u8> {
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

/// A `/GL` object's header (what tells one), then `id`: the stand-in back
/// end knows each by it.
fn il(id: u8) -> Vec<u8> {
    let mut d = vec![0u8; 32];
    d[0..4].copy_from_slice(&[0, 0, 0xFF, 0xFF]);
    d[4] = 1;
    d[12..28].copy_from_slice(&turbonop_pe::ltcg::CLASS_ID);
    d.push(id);
    d
}

/// What the stand-in back end opened: each object's file, offset, size.
type Opened = std::sync::Arc<std::sync::Mutex<Vec<(PathBuf, u64, usize)>>>;

/// The back end's stand-in: object 1 defines `main` and calls `lib_f`;
/// object 2 (a library's) defines `lib_f` and calls `native_g`. The code
/// it "generates" defines both and calls `native_g` and `late_h`.
#[derive(Default)]
struct FakeBackEnd {
    opened: Opened,
}

impl Ltcg for FakeBackEnd {
    fn open(
        &mut self,
        _: &config::Config,
        _: u32,
        path: &Path,
        offset: u64,
        data: &'static [u8],
    ) -> Result<&'static [u8], String> {
        self.opened
            .lock()
            .unwrap()
            .push((path.to_path_buf(), offset, data.len()));
        let publics = match data[32] {
            1 => vec![public("main", 2, None), public("lib_f", 1, None)],
            _ => vec![public("lib_f", 2, None), public("native_g", 1, None)],
        };
        let il = IlObject {
            publics,
            directives: Vec::new(),
        };
        Ok(Box::leak(
            turbonop_linker::ltcg::stand_in(&il).into_boxed_slice(),
        ))
    }

    fn generate(
        &mut self,
        link: &LtcgView<'_>,
    ) -> Result<&'static [u8], Vec<turbonop_linker::resolve::LinkError>> {
        // Before it: lib_f from the library member's stand-in, native_g
        // from the native library; late_h unheard of.
        let def = |n: &str| link.globals[link.names[n.as_bytes()] as usize].def;
        assert!(matches!(def("lib_f"), Def::Section { .. }));
        assert!(matches!(def("native_g"), Def::Section { .. }));
        assert!(!link.names.contains_key(&b"late_h"[..]));
        let code = object(
            &[0xE8, 0, 0, 0, 0, 0xE8, 0, 0, 0, 0, 0xC3],
            &[(1, 2), (6, 3)],
            &[("main", 1), ("lib_f", 1), ("native_g", 0), ("late_h", 0)],
        );
        Ok(Box::leak(code.into_boxed_slice()))
    }
}

/// A library's `/GL` member is opened in the library, at its data's
/// offset, when the search loads it; the generated object comes first,
/// the stand-ins leave, and what it alone refers to (`late_h`) is searched
/// for afterwards.
#[test]
fn a_librarys_gl_member_is_generated_with_the_rest() {
    let dir = std::env::temp_dir().join(format!("turbolink-ltcg-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let native = object(&[0xC3], &[], &[("native_g", 1)]);
    let late = object(&[0xC3], &[], &[("late_h", 1)]);
    let member = il(2);
    let gl = archive(&[("libf.obj", &member, &["lib_f"])]);
    let nat = archive(&[
        ("g.obj", &native, &["native_g"]),
        ("h.obj", &late, &["late_h"]),
    ]);
    std::fs::write(dir.join("main.obj"), il(1)).unwrap();
    std::fs::write(
        dir.join("first.obj"),
        object(&[0xC3], &[], &[("first_f", 1)]),
    )
    .unwrap();
    std::fs::write(dir.join("gl.lib"), &gl).unwrap();
    std::fs::write(dir.join("nat.lib"), &nat).unwrap();
    let p = |f: &str| dir.join(f).display().to_string();
    // (The inputs given as such: an absolute path elsewhere than Windows
    // begins with `/`, which the command line would take for an option.)
    let argv = [
        "/LTCG",
        "/ENTRY:main",
        "/SUBSYSTEM:CONSOLE",
        "/NODEFAULTLIB",
    ]
    .map(str::to_owned);
    let words = args::command_line(argv, None, None, &|f| std::fs::read(f)).unwrap();
    let mut diags = Vec::new();
    let mut config = config::parse(&words, &mut diags);
    config.inputs = ["first.obj", "main.obj", "gl.lib", "nat.lib"]
        .map(p)
        .to_vec();
    let backend = FakeBackEnd::default();
    let opened = backend.opened.clone();
    let mut errors = Vec::new();
    let resolved = resolve::resolve_with(config, Some(Box::new(backend)), &mut errors)
        .unwrap_or_else(|| panic!("{errors:?}"));
    // Opened: the command line's object whole, the member in its library.
    let opened = opened.lock().unwrap().clone();
    let at = gl.windows(member.len()).position(|w| w == member).unwrap();
    assert_eq!(
        opened,
        vec![
            (dir.join("main.obj"), 0, member.len()),
            (dir.join("gl.lib"), u64::try_from(at).unwrap(), member.len()),
        ]
    );
    // The generated object first, then the native files; no stand-ins.
    assert!(resolved.ltcg);
    let names: Vec<&str> = resolved.files.iter().map(|f| f.name.as_str()).collect();
    assert_eq!(names[0], resolve::LTCG_FILE);
    assert_eq!(names[1], p("first.obj"));
    assert!(
        names
            .iter()
            .all(|n| !n.contains("main.obj") && !n.contains("libf.obj")),
        "{names:?}"
    );
    let file_of = |n: &str| match resolved.globals[resolved.names[n.as_bytes()] as usize].def {
        Def::Section { file, .. } => resolved.files[file as usize].name.clone(),
        d => panic!("{n}: {d:?}"),
    };
    assert_eq!(file_of("main"), resolve::LTCG_FILE);
    assert_eq!(file_of("lib_f"), resolve::LTCG_FILE);
    assert!(file_of("native_g").ends_with("(g.obj)"));
    assert!(file_of("late_h").ends_with("(h.obj)"));
    let _ = std::fs::remove_dir_all(&dir);
}
