//! `hexcavator info`: each format's one-line description and details.

use std::path::Path;
use std::process::Command;

fn info(path: &str) -> String {
    let p = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../testdata")
        .join(path);
    let out = Command::new(env!("CARGO_BIN_EXE_hexcavator"))
        .arg("info")
        .arg(&p)
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    // (Without the path, which is the platform's spelling.)
    let text = String::from_utf8(out.stdout).unwrap();
    let (_, rest) = text.split_once(": ").unwrap();
    rest.replace("\r\n", "\n")
}

#[test]
fn wasm() {
    assert_eq!(
        info("wasm/module.wasm"),
        "WebAssembly module (version 1), 353 bytes\n  \
         4 functions (1 imported), 3 data segments\n  \
         custom sections: name, producers, sourceMappingURL\n  \
         source map: module.wasm.map\n"
    );
}

#[test]
fn elf() {
    assert_eq!(
        info("elf/consumer.so"),
        "ELF64 little-endian, x86-64, 2712 bytes\n  \
         shared object\n  \
         needs: libversions.so\n  \
         build ID: 92264d77f3e8bfb8325ad02dd89d05f51b92ff72\n  \
         17 sections, 7 symbols\n"
    );
    assert!(info("elf/names.o").contains("\n  relocatable object\n"));
    assert!(info("elf/dwarf5-x86_64-linux-gnu").contains("\n  executable, entry 0x201290\n"));
}

#[test]
fn coff_objects() {
    assert_eq!(
        info("coff/objects.obj"),
        "COFF object, x64, 13231 bytes\n  \
         48 sections, 139 symbols\n  \
         debug information: CodeView (.debug$S, .debug$T)\n"
    );
    assert!(info("coff/objects-big.obj").starts_with("COFF object (bigobj), x64, "));
}

#[test]
fn managed() {
    let sample = info("clr/Sample.dll");
    assert!(
        sample.starts_with(
            "PE32 image, x86, 18432 bytes\n  \
             managed: CLR 2.5, metadata v4.0.30319 (IL only, strong-name signed)\n  \
             assembly Sample 1.2.3.4, 5 assembly references\n  \
             24 types, 105 methods, 51 fields; 82 type references, 89 member references\n  \
             metadata tables: Module 1, TypeRef 82, TypeDef 24, Field 51, MethodDef 105, "
        ),
        "{sample}"
    );
    assert!(
        info("clr/Sample.Extra.netmodule")
            .contains("\n  module Sample.Extra.netmodule (no assembly manifest)\n")
    );
}
