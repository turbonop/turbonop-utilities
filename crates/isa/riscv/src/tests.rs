//! Hermetic tests: a few encodings of each kind, as LLVM writes them.

use alloc::string::String;

use crate::format::format;
use crate::{Features, decode};

fn text(bytes: &[u8], rv64: bool) -> String {
    let f = Features::standard(rv64);
    let i = decode(bytes, &f).unwrap_or_else(|| panic!("{bytes:02x?}: doesn't decode"));
    format(&i, 0x1000, &f)
}

#[test]
fn base_and_aliases() {
    // addi a0, a0, 1; li a0, 5; mv a0, a1; nop; ret; jal 0x1010
    assert_eq!(
        text(&0x0015_0513u32.to_le_bytes(), true),
        "addi\ta0, a0, 0x1"
    );
    assert_eq!(text(&0x0050_0513u32.to_le_bytes(), true), "li\ta0, 0x5");
    assert_eq!(text(&0x0005_8513u32.to_le_bytes(), true), "mv\ta0, a1");
    assert_eq!(text(&0x0000_0013u32.to_le_bytes(), true), "nop");
    assert_eq!(text(&0x0000_8067u32.to_le_bytes(), true), "ret");
    assert_eq!(text(&0x0100_00EFu32.to_le_bytes(), true), "jal\t0x1010");
}

#[test]
fn compressed_as_full() {
    // c.addi a0, 1; c.li a0, 5; c.ret
    assert_eq!(text(&0x0505u16.to_le_bytes(), true), "addi\ta0, a0, 0x1");
    assert_eq!(text(&0x4515u16.to_le_bytes(), true), "li\ta0, 0x5");
    assert_eq!(text(&0x8082u16.to_le_bytes(), true), "ret");
}

/// An image's ISA string (its `.riscv.attributes` arch): the extensions
/// it names, versions aside, and what they imply (C with D: Zcd); and
/// only those decode.
#[test]
fn features_from_arch() {
    let f = Features::from_arch(
        "rv64i2p1_m2p0_a2p1_f2p2_d2p2_c2p0_zicsr2p0_zifencei2p0_zmmul1p0_zaamo1p0_zalrsc1p0_zca1p0_zcd1p0",
    );
    for n in [
        "64bit", "m", "a", "f", "d", "c", "zicsr", "zifencei", "zca", "zcd", "zaamo",
    ] {
        assert!(f.contains(n), "{n}");
    }
    assert!(!f.contains("zba") && !f.contains("v"));
    let g = Features::from_arch("rv32gc");
    for n in ["m", "a", "f", "d", "c", "zicsr", "zifencei", "zcf", "zcd"] {
        assert!(g.contains(n), "{n}");
    }
    assert!(!g.rv64());
    // sh1add a0, a1, a2 (Zba): not rv64gc's.
    let sh1add = 0x20C5_A533u32.to_le_bytes();
    assert!(decode(&sh1add, &f).is_none());
    assert_eq!(text(&sh1add, true), "sh1add\ta0, a1, a2");
}
