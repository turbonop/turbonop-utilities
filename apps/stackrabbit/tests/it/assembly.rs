//! A function's loops, and the paths its image files are read from.

use serde_json::json;
use stackrabbit::assembly::{loops, system_root};
use turbonop_size::disasm::{Function, Labels, decode};

#[test]
fn a_loop_is_its_blocks_from_its_head_to_the_branch_back() {
    // xor eax,eax / L: inc eax / cmp eax,10 / jl L / ret
    let code = [0x31, 0xc0, 0xff, 0xc0, 0x83, 0xf8, 0x0a, 0x7c, 0xf9, 0xc3];
    let listing = decode(
        &Function {
            name: "f".to_string(),
            isa: turbonop_image::Arch::X64,
            thumb: false,
            data: None,
            address: 0x1000,
            code: &code,
            symbols: Vec::new(),
            riscv_arch: None,
            big_endian: false,
            mips_flags: 0,
        },
        Labels::Blocks,
    );
    assert_eq!(listing.lines.len(), 5);
    // Samples: 1 at the xor, 5 at the inc, 3 at the cmp, 2 at the jl;
    // the loop's lines are all at line 7 but the head, at line 6.
    let own = [1, 5, 3, 2, 0];
    let under = [0, 0, 4, 0, 0];
    let sources = [Some((0, 5)), Some((0, 6)), Some((0, 7)), Some((0, 7)), None];
    let found = loops(&listing, &own, &under, &sources);
    assert_eq!(found.len(), 1, "{found:?}");
    let l = &found[0];
    assert_eq!((&l["first"], &l["end"]), (&json!(1), &json!(4)));
    assert_eq!((&l["own"], &l["samples"]), (&json!(10), &json!(14)));
    assert_eq!((&l["file"], &l["line"]), (&json!(0), &json!(6)));
}

#[test]
fn the_kernels_images_are_in_the_windows_directory() {
    let w = r"W:\Win";
    assert_eq!(
        system_root(r"\SystemRoot\sys\kernel.exe", w),
        r"W:\Win\sys\kernel.exe"
    );
    assert_eq!(
        system_root(r"\systemroot\Sys\d\t.sys", w),
        r"W:\Win\Sys\d\t.sys"
    );
    assert_eq!(system_root(r"C:\app\a.exe", w), r"C:\app\a.exe");
    assert_eq!(system_root("é", w), "é");
}
