use super::*;
use std::path::Path;

fn fixture(rel: &str) -> Vec<u8> {
    std::fs::read(
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../testdata/coff")
            .join(rel),
    )
    .unwrap()
}

#[test]
fn classifies_the_fixtures() {
    let lib = Flavor::Lib;
    assert_eq!(
        classify(lib, "x", &fixture("objects.obj")),
        Ok(InputKind::Object(X64))
    );
    assert_eq!(
        classify(lib, "x", &fixture("objects-big.obj")),
        Ok(InputKind::Object(X64))
    );
    assert_eq!(
        classify(lib, "x", &fixture("ltcg.obj")),
        Ok(InputKind::Ltcg(X64))
    );
    assert_eq!(
        classify(lib, "x", &fixture("ltcg-x86.obj")),
        Ok(InputKind::Ltcg(X86))
    );
    assert_eq!(
        classify(lib, "x", &fixture("objects.lib")),
        Ok(InputKind::Library)
    );
    let imports = fixture("imports.lib");
    let archive = turbonop_archive::Archive::parse(&imports).unwrap();
    let first = archive
        .files()
        .find(|m| turbonop_pe::import_object::ImportObject::is_import_object(m.data))
        .unwrap();
    assert_eq!(classify(lib, "x", first.data), Ok(InputKind::Import(X64)));
}

#[test]
fn resources_bitcode_and_junk() {
    let mut res = RES_HEADER.to_vec();
    res.extend_from_slice(&[0; 16]);
    assert_eq!(
        classify(Flavor::Lib, "r.res", &res),
        Ok(InputKind::Resource)
    );
    let bitcode = b"BC\xC0\xDE\x35\x14\x00\x00";
    assert_eq!(
        classify(Flavor::LlvmLib, "b.obj", bitcode),
        Ok(InputKind::Bitcode)
    );
    // Measured: lib.exe reports the file's size, in hex.
    assert_eq!(
        classify(Flavor::Lib, "bc.obj", bitcode)
            .unwrap_err()
            .to_string(),
        "bc.obj : fatal error LNK1107: invalid or corrupt file: cannot read at 0x8"
    );
    assert_eq!(
        classify(Flavor::Lib, "junk100.bin", &[b'J'; 100])
            .unwrap_err()
            .to_string(),
        "junk100.bin : fatal error LNK1107: invalid or corrupt file: cannot read at 0x64"
    );
    assert_eq!(
        classify(Flavor::Lib, "zero.bin", &[])
            .unwrap_err()
            .to_string(),
        "zero.bin : fatal error LNK1136: invalid or corrupt file"
    );
    assert_eq!(
        classify(Flavor::LlvmLib, "j", b"junk")
            .unwrap_err()
            .render(Flavor::LlvmLib),
        "j: not a COFF object, bitcode, archive, import library or resource file"
    );
}

#[test]
fn lib_machine_rules() {
    // The first object sets it; later ones must match.
    let mut c = MachineCheck::new(None, Flavor::Lib);
    c.lib_object("a64.obj", InputKind::Object(X64)).unwrap();
    assert_eq!(
        c.lib_object("c86.obj", InputKind::Object(X86))
            .unwrap_err()
            .to_string(),
        "c86.obj : fatal error LNK1112: module machine type 'x86' conflicts with target \
         machine type 'x64'"
    );
    // /GL objects are checked too.
    let mut c = MachineCheck::new(None, Flavor::Lib);
    c.lib_object("c86.obj", InputKind::Object(X86)).unwrap();
    assert!(c.lib_object("gl.obj", InputKind::Ltcg(X64)).is_err());
    // ARM64EC needs /MACHINE:ARM64EC or ARM64X, which take x64 and ARM64.
    for m in [None, Some(ARM64)] {
        let mut c = MachineCheck::new(m, Flavor::Lib);
        assert_eq!(
            c.lib_object("cec.obj", InputKind::Object(ARM64EC))
                .unwrap_err()
                .code,
            1392
        );
    }
    for m in [ARM64EC, ARM64X] {
        let mut c = MachineCheck::new(Some(m), Flavor::Lib);
        for o in [X64, ARM64, ARM64EC] {
            c.lib_object("o.obj", InputKind::Object(o)).unwrap();
        }
    }
    // Resources have no machine.
    let mut c = MachineCheck::new(None, Flavor::Lib);
    c.lib_object("r.res", InputKind::Resource).unwrap();
    assert_eq!(c.machine, None);
}

#[test]
fn llvm_machine_rules() {
    let mut c = MachineCheck::new(None, Flavor::LlvmLib);
    c.llvm("a.obj", InputKind::Object(X64)).unwrap();
    assert_eq!(
        c.llvm("b.obj", InputKind::Object(ARM64))
            .unwrap_err()
            .render(Flavor::LlvmLib),
        "b.obj: file machine type arm64 conflicts with library machine type x64 (inferred \
         from earlier file 'a.obj')"
    );
    let mut c = MachineCheck::new(Some(X86), Flavor::LlvmLib);
    assert_eq!(
        c.llvm("a.obj", InputKind::Object(X64))
            .unwrap_err()
            .render(Flavor::LlvmLib),
        "a.obj: file machine type x64 conflicts with library machine type x86 (from \
         '/machine:x86' flag)"
    );
    let mut c = MachineCheck::new(None, Flavor::LlvmLib);
    assert!(c.llvm("ec.obj", InputKind::Object(ARM64EC)).is_err());
    // Import objects aren't checked by llvm-lib.
    let mut c = MachineCheck::new(Some(X86), Flavor::LlvmLib);
    c.llvm("i", InputKind::Import(X64)).unwrap();
}
