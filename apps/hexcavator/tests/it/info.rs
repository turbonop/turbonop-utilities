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
fn bitcode() {
    let kinds = info("bitcode/kinds.bc");
    assert!(kinds.starts_with("LLVM bitcode, "), "{kinds}");
    assert!(
        kinds.contains(
            "\n  target x86_64-unknown-linux-gnu, from R:\\tools\\bitcode\\kinds.c (LLVM24"
        ),
        "{kinds}"
    );
    assert!(kinds.contains("\n  data layout: e-m:e-"), "{kinds}");
    assert!(
        kinds.contains(
            "\n  17 functions defined, 8 declared; 9 globals, 1 aliases, 1 ifuncs, 0 comdats\n"
        ),
        "{kinds}"
    );
    assert!(
        kinds.contains("\n  170 instructions in 33 basic blocks; function bodies "),
        "{kinds}"
    );
    assert!(
        kinds.contains("\n  debug information: 1 compile unit (clang version 24"),
        "{kinds}"
    );
    let thin = info("bitcode/kinds-thin.bc");
    assert!(
        thin.contains("\n  ThinLTO summary\n  symbol table: 23 symbols\n"),
        "{thin}"
    );
    let multi = info("bitcode/multi.bc");
    assert!(
        multi.contains("\n  module 1:\n    target x86_64-unknown-linux-gnu"),
        "{multi}"
    );
    assert!(
        multi.contains("\n  module 2:\n    target x86_64-unknown-linux-gnu, from every.ll"),
        "{multi}"
    );
    assert!(info("bitcode/embed-darwin.bc").starts_with("LLVM bitcode (wrapped), 4064 bytes\n"));
    assert!(info("bitcode/warn.dia").starts_with(
        "clang serialized diagnostics (bitstream), 1224 bytes\n  6 top-level blocks\n"
    ));
    assert!(
        info("bitcode/embed.opt.bitstream").starts_with("LLVM optimization remarks (bitstream), ")
    );
    // An object's embedded bitcode.
    let elf = info("bitcode/embed-elf.o");
    assert!(
        elf.contains("\n  embedded bitcode: .llvmbc, 3716 bytes (x86_64-unknown-linux-gnu)\n"),
        "{elf}"
    );
    let macho = info("bitcode/embed-macho.o");
    assert!(
        macho.contains("\n  embedded bitcode: __LLVM,__bitcode, "),
        "{macho}"
    );
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

#[test]
fn java() {
    assert_eq!(
        info("java/main/fixture/Hello.class"),
        "Java class file, version 65.0 (Java 21), 1098 bytes\n  \
         class fixture.Hello\n  \
         0 fields, 2 methods, 0 interfaces; constant pool of 57 entries (853 bytes)\n  \
         source file: Hello.java\n"
    );
    assert!(info("java/preview/preview/Preview.class").contains("(Java 21, preview features)"));
    let jar = info("java/fixtures.jar");
    assert!(jar.starts_with("JAR (ZIP archive), 42 entries, "), "{jar}");
    for line in [
        "\n  Main-Class: fixture.Hello\n",
        "\n  36 classes in 3 packages, class version 65 (Java 21)\n",
        "\n  modular JAR (module-info.class)\n",
        "\n  signed: META-INF/FIXTURE.SF, META-INF/FIXTURE.EC\n",
    ] {
        assert!(jar.contains(line), "{jar}");
    }
    let mr = info("java/mr.jar");
    assert!(
        mr.contains("class versions 52 to 61 (Java 8 to Java 17)"),
        "{mr}"
    );
    assert!(mr.contains("multi-release: versions 17"), "{mr}");
    assert_eq!(
        info("zip/prefixed.zip"),
        "ZIP archive, 1 entries, 676 bytes\n  1 entries, 7490 bytes uncompressed, after 46 bytes of other data\n"
    );
}
