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
