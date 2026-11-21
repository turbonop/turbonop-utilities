//! Chained fixups' size, estimated from an image's rebase and bind
//! opcodes (testdata/macho's pointers-*-opcodes.dylib and
//! objc-*-opcodes.dylib, table-*-opcodes.dylib), against what ld64.lld wrote linking the same
//! objects with chained fixups (their twins).

use std::path::Path;

fn macho(name: &str) -> Vec<u8> {
    std::fs::read(
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../../testdata/macho")
            .join(name),
    )
    .unwrap()
}

#[test]
fn estimates_match_the_linkers() {
    use hexcavator_macho::consts::LC_DYLD_CHAINED_FIXUPS;
    for name in [
        "pointers-arm64",
        "pointers-x86_64",
        "objc-arm64",
        "objc-x86_64",
        "table-arm64",
        "table-x86_64",
    ] {
        let opcodes = macho(&format!("{name}-opcodes.dylib"));
        let m = hexcavator_macho::MachO::parse(&opcodes).unwrap();
        let page = if m.header().cputype == hexcavator_macho::consts::CPU_TYPE_ARM64 {
            0x4000
        } else {
            0x1000
        };
        let estimate =
            hexcavator_size::recommend::chained_fixups_size(&m.segments(), &m.fixups(), page);
        let chained = macho(&format!("{name}.dylib"));
        let c = hexcavator_macho::MachO::parse(&chained).unwrap();
        let lc = c
            .load_commands()
            .into_iter()
            .find(|l| l.cmd == LC_DYLD_CHAINED_FIXUPS)
            .unwrap();
        let (_, size) = c.linkedit_data(&lc).unwrap();
        assert_eq!(estimate, u64::from(size), "{name}");
    }
}
