//! `any::unwind`: the unwinder an image's format, architecture, and unwind
//! data call for, on images of each (in testdata).
//!
//! At a function's first instruction its frame isn't set up: whatever
//! describes it, the caller's PC is the return address at the stack
//! pointer (x86-64) or in the link register (ARM64, ARM), and the caller's
//! stack pointer is past it. Unwinders that know prologs (Windows',
//! DWARF's) are checked so at every function's start. Compact unwind
//! encodings and EHABI entries describe a function's body, and are
//! checked there: by a frame record built to match, or against the
//! unwinder they're for.

// Synthetic test data: small values, casts are exact.
#![allow(clippy::cast_possible_truncation)]

use std::path::Path;

use hexcavator_dwarf::cfi::{Cfi, CfiKind, Entry};
use hexcavator_image::Image;
use hexcavator_unwind::any::{self, Method};
use hexcavator_unwind::dwarf_cfi::Registers;

use crate::support::{STACK, Stack};

const RETURN: u64 = 0x7777_0000;
/// Where ELF and Mach-O images are loaded, from where they were linked.
const SLIDE: u64 = 0x5550_0000;

fn testdata(rel: &str) -> Vec<u8> {
    let p = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../../testdata")
        .join(rel);
    std::fs::read(&p).unwrap_or_else(|e| panic!("{}: {e}", p.display()))
}

fn registers(pairs: &[(u16, u64)]) -> Registers {
    let mut r = Registers::default();
    for &(n, v) in pairs {
        r.set(n, Some(v));
    }
    r
}

/// The starts of the functions the image's `.eh_frame` describes.
fn fde_starts(image: &Image<'_>) -> Vec<u64> {
    let s = image
        .sections()
        .into_iter()
        .find(|s| s.name == b".eh_frame" || s.name == b"__eh_frame")
        .expect(".eh_frame");
    let data = image.bytes_at(s.addr, s.file_size.min(s.vm_size)).unwrap();
    let cfi = Cfi {
        data,
        kind: CfiKind::EhFrame,
        endian: image.endian(),
        address: s.addr,
        address_size: image.arch().pointer_size().unwrap(),
        text_base: None,
        data_base: None,
    };
    cfi.entries()
        .filter_map(|e| match e {
            Entry::Fde(f) => Some(f.pc_begin),
            Entry::Cie(_) => None,
        })
        .collect()
}

/// Unwinds from the first instruction at `start` (an image address):
/// the caller's PC and stack pointer, and how.
fn at_entry(image: &Image<'_>, bias: u64, start: u64) -> (Method, u64, u64) {
    let (pc, sp) = any::registers_of(image.arch()).unwrap();
    let mut stack = Stack::new();
    let top = STACK + 0x100;
    stack.set(top, RETURN);
    let lr = match image.arch() {
        hexcavator_core::Arch::Arm64 => Some(30),
        hexcavator_core::Arch::Arm => Some(14),
        hexcavator_core::Arch::RiscV32
        | hexcavator_core::Arch::RiscV64
        | hexcavator_core::Arch::LoongArch32
        | hexcavator_core::Arch::LoongArch64 => Some(1),
        _ => None,
    };
    let mut r = registers(&[(pc, bias + start), (sp, top)]);
    if let Some(lr) = lr {
        r.set(lr, Some(RETURN));
    }
    let u = any::unwind(image, bias, false, &mut r, &stack)
        .unwrap_or_else(|e| panic!("{start:#x}: {e}"));
    assert_eq!(u.frame.return_address, Some(RETURN), "{start:#x}");
    (u.method, r.get(pc).unwrap(), r.get(sp).unwrap())
}

#[test]
fn elf_dwarf_at_function_starts() {
    for (file, sp_after) in [
        ("elf/unwind-x86_64-linux-gnu", 8),
        ("elf/unwind-aarch64_be-linux-gnu", 0),
        ("elf/unwind-i386-linux-gnu", 4),
        ("elf/unwind-riscv64-linux-gnu", 0),
        ("elf/unwind-loongarch64-linux-gnu", 0),
    ] {
        let data = testdata(file);
        let image = Image::parse(&data).unwrap();
        let starts = fde_starts(&image);
        assert!(!starts.is_empty(), "{file}");
        for start in starts {
            let (method, pc, sp) = at_entry(&image, SLIDE, start);
            assert_eq!(method, Method::Dwarf(CfiKind::EhFrame), "{file}");
            assert_eq!(pc, RETURN, "{file} {start:#x}");
            assert_eq!(sp, STACK + 0x100 + sp_after, "{file} {start:#x}");
        }
    }
}

/// A PE image's `.pdata` entries' function starts (x64's are 12 bytes,
/// ARM's 8): functions' own, not a chained entry's (a function's part
/// whose unwinding is its function's: its prolog has run), nor ARM64's
/// link records or chained scopes (`end_c`: its function's prolog has run
/// before its own), nor ARM's fragments (no prolog); ARM's Thumb bits
/// left out.
fn pdata_starts(image: &Image<'_>) -> Vec<u64> {
    let pe = image.pe().unwrap();
    let dir = pe
        .data_directory(hexcavator_pe::consts::directory::EXCEPTION)
        .unwrap();
    let (rva, size) = (dir.virtual_address.get(), dir.size.get());
    let data = image.bytes_at(u64::from(rva), u64::from(size)).unwrap();
    let stride = if image.arch() == hexcavator_core::Arch::X64 {
        12
    } else {
        8
    };
    let chained = |e: &[u8]| {
        let unwind = u32::from_le_bytes([e[8], e[9], e[10], e[11]]);
        // (UNW_FLAG_CHAININFO, or an indirect entry.)
        unwind & 1 != 0
            || image
                .bytes_at(u64::from(unwind), 1)
                .is_none_or(|b| (b[0] >> 3) & 4 != 0)
    };
    // (An ARM64 `.xdata` record's unwind codes, with an `end_c`.)
    let end_c = |e: &[u8]| {
        let unwind = u32::from_le_bytes([e[4], e[5], e[6], e[7]]);
        if image.arch() != hexcavator_core::Arch::Arm64 || unwind & 3 != 0 {
            return false;
        }
        let Some(h) = image.bytes_at(u64::from(unwind), 4) else {
            return true;
        };
        let h = u32::from_le_bytes([h[0], h[1], h[2], h[3]]);
        let (words, epilogs, e_bit) = (h >> 27, (h >> 22) & 0x1F, h & (1 << 21) != 0);
        let scopes = if e_bit { 0 } else { u64::from(epilogs) * 4 };
        image
            .bytes_at(u64::from(unwind) + 4 + scopes, u64::from(words) * 4)
            .is_none_or(|codes| codes.contains(&0xE5))
    };
    // (An ARM fragment: packed (flag 2), or `.xdata` with its F bit.)
    let fragment = |e: &[u8]| {
        let unwind = u32::from_le_bytes([e[4], e[5], e[6], e[7]]);
        if image.arch() != hexcavator_core::Arch::Arm {
            return false;
        }
        match unwind & 3 {
            2 => true,
            0 => image
                .bytes_at(u64::from(unwind), 4)
                .is_none_or(|h| h[2] & 0x40 != 0),
            _ => false,
        }
    };
    data.chunks_exact(stride)
        .filter(|e| {
            if stride == 12 {
                !chained(e)
            } else {
                e[4] & 3 != 3 && !end_c(e) && !fragment(e)
            }
        })
        .map(|e| u64::from(u32::from_le_bytes([e[0], e[1], e[2], e[3]]) & !1))
        .collect()
}

#[test]
fn pe_pdata_at_function_starts() {
    const BASE: u64 = 0x1_4000_0000;
    for (file, sp_after) in [
        ("xref/xref-x64.exe", 8),
        ("xref/xref-arm64.exe", 0),
        ("sizebench/bin/PEParser.Tests.Dllarm32.dll", 0),
    ] {
        let data = testdata(file);
        let image = Image::parse(&data).unwrap();
        // (ARM's addresses are 32 bits.)
        let base = if image.arch() == hexcavator_core::Arch::Arm {
            0x40_0000
        } else {
            BASE
        };
        let starts = pdata_starts(&image);
        assert!(!starts.is_empty(), "{file}");

        for start in starts {
            let (method, pc, sp) = at_entry(&image, base, start);
            assert_eq!(method, Method::Pdata, "{file}");
            assert_eq!(pc, RETURN, "{file} {start:#x}");
            assert_eq!(sp, STACK + 0x100 + sp_after, "{file} {start:#x}");
        }
    }
}

/// Code no `.pdata` entry covers is a leaf: its return address at RSP
/// (x64), or in LR.
#[test]
fn pe_leaves() {
    let data = testdata("xref/xref-x64.exe");
    let image = Image::parse(&data).unwrap();
    let mut stack = Stack::new();
    stack.set(STACK, RETURN);
    // (The headers: no function's.)
    let mut r = registers(&[(16, 0x1_4000_0010), (7, STACK)]);
    let u = any::unwind(&image, 0x1_4000_0000, false, &mut r, &stack).unwrap();
    assert_eq!(u.method, Method::Leaf);
    assert_eq!((r.get(16), r.get(7)), (Some(RETURN), Some(STACK + 8)));
    // (An image with no `.pdata` at all: MinGW's, freestanding, and an
    // ARM one; at a function's start.)
    for (file, start) in [("mingw/program.exe", 0x1010), ("arm32/program.exe", 0x1000)] {
        let data = testdata(file);
        let image = Image::parse(&data).unwrap();
        let (method, pc, _) = at_entry(&image, 0x40_0000, start);
        assert_eq!((method, pc), (Method::Leaf, RETURN), "{file}");
    }
}

/// Mach-O: each function by its compact encoding, its PC in its body: a
/// frame-pointer frame's record built where its frame pointer points, a
/// frameless function's return address in LR (ARM64); or by the FDE its
/// encoding names.
#[test]
fn macho_compact_in_bodies() {
    use hexcavator_macho::unwind_info::UnwindInfo;
    for file in ["macho/unwind-arm64.dylib", "macho/unwind-x86_64.dylib"] {
        let data = testdata(file);
        let image = Image::parse(&data).unwrap();
        let arm64 = image.arch() == hexcavator_core::Arch::Arm64;
        let m = image.macho().unwrap();
        let s = image
            .sections()
            .into_iter()
            .find(|s| s.name == b"__unwind_info")
            .unwrap();
        let bytes = &data[s.file_offset as usize..][..s.file_size as usize];
        let info = UnwindInfo::parse(bytes, image.endian()).unwrap();
        let base = m
            .segments()
            .iter()
            .find(|s| s.fileoff == 0 && s.filesize > 0)
            .unwrap()
            .vmaddr;
        let entries = info.entries();
        assert!(!entries.is_empty(), "{file}");
        let mut framed = 0;
        for e in entries.iter().filter(|e| e.encoding != 0) {
            let start = base + u64::from(e.function);
            let (pc, sp, fp) = if arm64 { (32, 31, 29) } else { (16, 7, 6) };
            let frame = STACK + 0x80;
            let mut stack = Stack::new();
            stack.set(frame, 0x1111);
            stack.set(frame + 8, RETURN);
            let mut r = registers(&[(pc, SLIDE + start + 4), (sp, STACK), (fp, frame)]);
            if arm64 {
                r.set(30, Some(RETURN));
            }
            let u = any::unwind(&image, SLIDE, false, &mut r, &stack);
            let mode = e.encoding & 0x0F00_0000;
            // (A frame-pointer frame: x86-64's mode 1, ARM64's 4.)
            if mode == if arm64 { 0x0400_0000 } else { 0x0100_0000 } {
                framed += 1;
                let u = u.unwrap_or_else(|err| panic!("{file} {start:#x}: {err}"));
                assert_eq!(u.method, Method::Compact, "{file} {start:#x}");
                assert_eq!(u.frame.cfa, frame + 16, "{file} {start:#x}");
                assert_eq!(r.get(fp), Some(0x1111), "{file} {start:#x}");
                assert_eq!(r.get(pc), Some(RETURN), "{file} {start:#x}");
            } else if mode == if arm64 { 0x0300_0000 } else { 0x0400_0000 } {
                // (Its FDE says what's at the PC; this stack isn't its.)
                if let Ok(u) = u {
                    assert_eq!(u.method, Method::Dwarf(CfiKind::EhFrame), "{file}");
                }
            }
        }
        assert!(framed > 0, "{file}");
    }
}

/// ARM EHABI: each `.ARM.exidx` entry's opcodes, as `arm_ehabi::unwind`
/// applies them.
#[test]
fn elf_arm_exidx() {
    use hexcavator_unwind::arm_ehabi;
    let data = testdata("arm32/eh-linux");
    let image = Image::parse(&data).unwrap();
    let s = image
        .sections()
        .into_iter()
        .find(|s| s.name == b".ARM.exidx")
        .unwrap();
    let index = arm_ehabi::Exidx {
        data: image.bytes_at(s.addr, s.vm_size).unwrap(),
        address: s.addr,
        endian: image.endian(),
    };
    struct Bytes<'a, 'd>(&'a Image<'d>);
    impl hexcavator_unwind::Memory for Bytes<'_, '_> {
        fn read(&self, address: u64, buf: &mut [u8]) -> bool {
            self.0
                .bytes_at(address, buf.len() as u64)
                .map(|b| buf.copy_from_slice(b))
                .is_some()
        }
    }
    let mut checked = 0;
    for i in 0..index.len() {
        let Some(entry) = index.entry(i, &Bytes(&image)) else {
            continue;
        };
        if entry.model == arm_ehabi::Model::CantUnwind {
            continue;
        }
        let stack = Stack::new();
        let start = entry.function;
        let regs = registers(&[(15, SLIDE + start + 4), (13, STACK + 0x40), (14, RETURN)]);
        let mut want = regs.clone();
        let expected = arm_ehabi::unwind(&entry, image.endian(), &mut want, &stack);
        let mut got = regs;
        let u = any::unwind(&image, SLIDE, false, &mut got, &stack);
        match (expected, u) {
            (Ok(f), Ok(u)) => {
                assert_eq!(u.method, Method::ArmExidx);
                assert_eq!(u.frame, f, "{start:#x}");
                checked += 1;
            }
            (Err(a), Err(b)) => assert_eq!(a, b),
            (a, b) => panic!("{start:#x}: {a:?} {b:?}"),
        }
    }
    assert!(checked > 0);
}

/// What has no unwinder here: WebAssembly, and x86's frames (a PDB's).
#[test]
fn unsupported() {
    let data = testdata("wasm/sample.wasm");
    let image = Image::parse(&data).unwrap();
    let mut r = Registers::default();
    assert!(any::unwind(&image, 0, false, &mut r, &Stack::new()).is_err());
}
