//! Bitcode where the fixtures keep it (tools/bitcode/build.cmd): embedded
//! in ELF, Mach-O, COFF, and WebAssembly objects (`-fembed-bitcode`),
//! wrapped (Darwin's LTO objects), in an archive's members, two modules in
//! one file, back to back in a section, and in a universal file's slice.

use turbonop_global_alloc as _;

use std::path::PathBuf;

use turbonop_bitcode::Bitcode;
use turbonop_bitcode::embedded::{Container, find, split};

fn read(name: &str) -> Vec<u8> {
    let p = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../../testdata/bitcode")
        .join(name);
    std::fs::read(&p).unwrap_or_else(|e| panic!("{}: {e}", p.display()))
}

fn defines(bc: &Bitcode<'_>, name: &[u8]) -> bool {
    bc.modules.iter().any(|m| {
        m.functions
            .iter()
            .any(|f| &*f.name == name && !f.is_declaration)
    })
}

#[test]
fn objects_embed_bitcode() {
    for (file, container, section, triple, entry) in [
        (
            "embed-elf.o",
            Container::Elf,
            &b".llvmbc"[..],
            "x86_64-unknown-linux-gnu",
            &b"embedded_entry"[..],
        ),
        (
            "embed-macho.o",
            Container::MachO,
            b"__LLVM,__bitcode",
            "x86_64-apple-macosx11.0.0",
            b"embedded_entry",
        ),
        (
            "embed-coff.obj",
            Container::Coff,
            b".llvmbc",
            "x86_64-pc-windows-msvc",
            b"embedded_entry",
        ),
        (
            "embed-wasm.o",
            Container::Wasm,
            b".llvmbc",
            "wasm32-unknown-unknown",
            b"embedded_entry",
        ),
    ] {
        let data = read(file);
        let found = find(&data);
        assert_eq!(found.len(), 1, "{file}");
        let e = &found[0];
        assert_eq!(e.container, container, "{file}");
        assert_eq!(e.section.as_deref(), Some(section), "{file}");
        assert_eq!(e.member, None);
        // Where it is.
        let at = usize::try_from(e.offset).unwrap();
        assert_eq!(&data[at..at + e.data.len()], e.data, "{file}");
        // The command line beside it: clang's -cc1 arguments.
        let cmd = e
            .command_line
            .unwrap_or_else(|| panic!("{file}: no command line"));
        assert!(
            cmd.split(|&b| b == 0).any(|a| a == b"-triple"),
            "{file}: {}",
            String::from_utf8_lossy(cmd)
        );
        let bc = Bitcode::parse(e.data).unwrap();
        assert_eq!(bc.modules.len(), 1);
        // (MSVC's triple names its version: msvc19.52.37029.)
        assert!(
            String::from_utf8_lossy(&bc.modules[0].triple).starts_with(triple),
            "{file}"
        );
        assert!(defines(&bc, entry), "{file}");
    }
}

#[test]
fn wrapped_bitcode() {
    let data = read("embed-darwin.bc");
    let found = find(&data);
    assert_eq!(found.len(), 1);
    assert_eq!(found[0].container, Container::File);
    let bc = Bitcode::parse(found[0].data).unwrap();
    let w = bc.wrapper.unwrap();
    assert_eq!(w.magic, 0x0B17_C0DE);
    assert_eq!(w.offset, 20);
    // CPU_TYPE_X86_64.
    assert_eq!(w.cpu_type, 0x0100_0007);
    assert_eq!(bc.stream_offset, 20);
    assert_eq!(bc.stream.len(), w.size as usize);
    assert!(defines(&bc, b"embedded_entry"));
}

#[test]
fn archive_members() {
    let data = read("libbitcode.a");
    let found = find(&data);
    let got: Vec<(Option<&[u8]>, Container)> = found
        .iter()
        .map(|e| (e.member.as_deref(), e.container))
        .collect();
    assert_eq!(
        got,
        [
            (Some(&b"kinds-thin.bc"[..]), Container::File),
            (Some(&b"every.bc"[..]), Container::File),
            (Some(&b"embed-elf.o"[..]), Container::Elf),
        ]
    );
    for e in &found {
        let at = usize::try_from(e.offset).unwrap();
        assert_eq!(&data[at..at + e.data.len()], e.data);
        Bitcode::parse(e.data).unwrap();
    }
    // The members are the fixtures as they were.
    assert_eq!(found[0].data, &read("kinds-thin.bc")[..]);
    assert_eq!(found[1].data, &read("every.bc")[..]);
}

#[test]
fn two_modules_in_one_file() {
    let data = read("multi.bc");
    let bc = Bitcode::parse(&data).unwrap();
    assert_eq!(bc.modules.len(), 2);
    // Each module's names are in the string table after it.
    let names = |m: &turbonop_bitcode::Module<'_>| -> Vec<Vec<u8>> {
        m.functions.iter().map(|f| f.name.to_vec()).collect()
    };
    assert!(names(&bc.modules[0]).iter().any(|n| n == b"kinds_main"));
    assert!(names(&bc.modules[1]).iter().any(|n| n == b"memory"));
    assert!(!names(&bc.modules[1]).iter().any(|n| n == b"kinds_main"));
    assert_eq!(bc.modules[1].source_filename, b"every.ll");
    for m in &bc.modules {
        assert!(m.identification.is_some());
        assert!(m.errors.is_empty(), "{:?}", m.errors);
        assert!(
            m.bodies
                .iter()
                .all(|b| b.error.is_none() && b.function.is_some())
        );
    }
}

#[test]
fn back_to_back_in_a_section() {
    // As a linker concatenates .llvmbc sections: each file padded to 4.
    let a = read("kinds.bc");
    let b = read("embed-darwin.bc");
    let c = read("every.bc");
    let mut joined = a.clone();
    joined.extend_from_slice(&b);
    while !joined.len().is_multiple_of(4) {
        joined.push(0);
    }
    joined.extend_from_slice(&c);
    let parts = split(&joined);
    assert_eq!(parts.len(), 3);
    assert_eq!(parts[0], (0, &a[..]));
    assert_eq!(parts[1].0, a.len());
    // The wrapper's bitcode (Darwin pads the file past it).
    assert_eq!(parts[1].1, &b[..parts[1].1.len()]);
    assert!(b.len() - parts[1].1.len() < 16);
    assert_eq!(parts[2].1, &c[..]);
    // Trailing padding isn't a file.
    joined.extend_from_slice(&[0; 8]);
    assert_eq!(split(&joined).len(), 3);
}

#[test]
fn universal_slices() {
    // A universal file with the Mach-O object as its one slice.
    let obj = read("embed-macho.o");
    let mut fat = Vec::new();
    for v in [
        0xCAFE_BABE_u32,
        1,
        0x0100_0007,
        3,
        4096,
        u32::try_from(obj.len()).unwrap(),
        12,
    ] {
        fat.extend_from_slice(&v.to_be_bytes());
    }
    fat.resize(4096, 0);
    fat.extend_from_slice(&obj);
    let found = find(&fat);
    assert_eq!(found.len(), 1);
    assert_eq!(found[0].slice, Some((0x0100_0007, 3)));
    assert_eq!(found[0].container, Container::MachO);
    let inner = find(&obj);
    assert_eq!(found[0].offset, inner[0].offset + 4096);
    assert_eq!(found[0].data, inner[0].data);
}

#[test]
fn nothing_found_elsewhere() {
    for data in [
        &b""[..],
        b"BC",
        b"\x7fELF",
        b"!<arch>\n",
        b"\0asm\x01\0\0\0",
        &[0u8; 64][..],
        b"not bitcode at all, just text",
    ] {
        assert!(find(data).is_empty());
    }
}
