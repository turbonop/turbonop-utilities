//! Hermetic tests: Mach-O files built in memory, 32- and 64-bit, both byte
//! orders, universal, and damaged.
#![allow(clippy::cast_possible_truncation, reason = "small test files")]

use alloc::vec;
use alloc::vec::Vec;

use super::*;

struct B {
    out: Vec<u8>,
    endian: Endian,
}

impl B {
    fn u(&mut self, v: u64, n: usize) -> &mut Self {
        let b = v.to_le_bytes();
        if self.endian == Endian::Little {
            self.out.extend_from_slice(&b[..n]);
        } else {
            self.out.extend(b[..n].iter().rev());
        }
        self
    }
    fn name(&mut self, s: &[u8]) -> &mut Self {
        let mut b = [0u8; 16];
        b[..s.len()].copy_from_slice(s);
        self.out.extend_from_slice(&b);
        self
    }
}

/// An executable: a `__TEXT` segment with `__text` and `__bss`
/// (zero-fill) sections, `LC_UUID`, and `LC_SYMTAB` with two symbols.
fn sample(is64: bool, endian: Endian) -> Vec<u8> {
    let w = if is64 { 8 } else { 4 };
    let header = if is64 { 32 } else { 28 };
    let seg_cmd = if is64 { 72 } else { 56 } + 2 * if is64 { 80 } else { 68 };
    let cmds = seg_cmd + 24 + 24;
    let text_off = (header + cmds) as u64;
    let nl = if is64 { 16 } else { 12 };
    let symoff = text_off + 16;
    let stroff = symoff + 2 * nl;
    let strings = b"\0_main\0_x\0";
    let mut b = B {
        out: Vec::new(),
        endian,
    };
    let magic = if is64 { MH_MAGIC_64 } else { MH_MAGIC };
    b.u(u64::from(magic), 4)
        .u(u64::from(CPU_TYPE_ARM64), 4)
        .u(0, 4)
        .u(u64::from(MH_EXECUTE), 4);
    b.u(3, 4).u(cmds as u64, 4).u(0, 4);
    if is64 {
        b.u(0, 4);
    }
    // The segment.
    b.u(u64::from(if is64 { LC_SEGMENT_64 } else { LC_SEGMENT }), 4)
        .u(seg_cmd as u64, 4)
        .name(b"__TEXT");
    b.u(0x1000, w)
        .u(0x2000, w)
        .u(0, w)
        .u(stroff + strings.len() as u64, w);
    b.u(5, 4).u(5, 4).u(2, 4).u(0, 4);
    for (sect, addr, size, off, flags) in [
        (
            &b"__text"[..],
            0x1000 + text_off,
            16u64,
            text_off,
            0x8000_0400u32,
        ),
        (b"__bss", 0x1800, 0x100, 0, S_ZEROFILL),
    ] {
        b.name(sect).name(b"__TEXT").u(addr, w).u(size, w);
        b.u(off, 4)
            .u(2, 4)
            .u(0, 4)
            .u(0, 4)
            .u(u64::from(flags), 4)
            .u(0, 4)
            .u(0, 4);
        if is64 {
            b.u(0, 4);
        }
    }
    b.u(u64::from(LC_UUID), 4).u(24, 4);
    b.out.extend_from_slice(&[7; 16]);
    b.u(u64::from(LC_SYMTAB), 4)
        .u(24, 4)
        .u(symoff, 4)
        .u(2, 4)
        .u(stroff, 4)
        .u(strings.len() as u64, 4);
    assert_eq!(b.out.len() as u64, text_off);
    b.out.extend_from_slice(&[0xAA; 16]);
    for (strx, value) in [(1u64, 0x1000 + text_off), (7, 0x1800)] {
        b.u(strx, 4)
            .u(u64::from(N_SECT | N_EXT), 1)
            .u(1, 1)
            .u(0, 2)
            .u(value, w);
    }
    b.out.extend_from_slice(strings);
    b.out
}

#[test]
fn parses_every_width_and_byte_order() {
    for is64 in [false, true] {
        for endian in [Endian::Little, Endian::Big] {
            let data = sample(is64, endian);
            assert!(is_macho(&data));
            let m = MachO::parse(&data).unwrap();
            let ctx = (is64, endian);
            assert_eq!(m.header().is64, is64);
            assert_eq!(m.endian(), endian);
            assert_eq!(m.header().filetype, MH_EXECUTE);
            assert!(!m.is_object());
            assert_eq!(m.load_commands().len(), 3, "{ctx:?}");
            let segs = m.segments();
            assert_eq!(segs.len(), 1);
            assert_eq!(segs[0].name, b"__TEXT");
            assert_eq!((segs[0].vmaddr, segs[0].vmsize), (0x1000, 0x2000));
            assert_eq!(segs[0].maxprot, 5);
            let s = &segs[0].sections;
            assert_eq!(s.len(), 2);
            assert_eq!(s[0].sectname, b"__text");
            assert_eq!(s[0].segname, b"__TEXT");
            assert_eq!(s[0].size, 16);
            assert_eq!(s[0].file_size(), 16);
            assert_eq!(s[0].flags, 0x8000_0400);
            assert!(s[1].is_zerofill());
            assert_eq!(s[1].file_size(), 0);
            assert_eq!((s[0].index, s[1].index), (0, 1));
            assert_eq!(m.uuid(), Some([7; 16]));
            let st = m.symtab().unwrap();
            assert_eq!(st.nsyms, 2);
            let syms = m.symbols();
            assert_eq!(syms[0].name, b"_main");
            assert_eq!(syms[1].name, b"_x");
            assert_eq!(syms[1].n_value, 0x1800);
            assert!(syms[0].is_section_defined() && !syms[0].is_stab());
            assert_eq!(m.nlist_size(), if is64 { 16 } else { 12 });
            assert!(m.dysymtab().is_none());
        }
    }
}

#[test]
fn universal_files() {
    let a = sample(true, Endian::Little);
    let mut fat = Vec::new();
    fat.extend_from_slice(&FAT_MAGIC.to_be_bytes());
    fat.extend_from_slice(&1u32.to_be_bytes());
    for v in [CPU_TYPE_ARM64, 0, 64, a.len() as u32, 6] {
        fat.extend_from_slice(&v.to_be_bytes());
    }
    fat.resize(64, 0);
    fat.extend_from_slice(&a);
    assert!(is_fat(&fat) && !is_macho(&fat));
    let archs = fat_archs(&fat).unwrap();
    assert_eq!(
        archs,
        [FatArch {
            cputype: CPU_TYPE_ARM64,
            cpusubtype: 0,
            offset: 64,
            size: a.len() as u64,
            align: 6
        }]
    );
    assert_eq!(fat_header_size(&fat), 28);
    let m = MachO::parse(&fat[64..]).unwrap();
    assert_eq!(m.segments().len(), 1);
    // A Java class file has the same magic and a large second word.
    let java = [0xCA, 0xFE, 0xBA, 0xBE, 0, 0, 0, 52];
    assert!(!is_fat(&java));
    assert!(fat_archs(&java).is_none());
    // Truncated headers give fewer archs.
    assert!(fat_archs(&fat[..20]).unwrap().is_empty());
    assert_eq!(arch_name(CPU_TYPE_X86_64, 3), "x86_64");
    assert_eq!(arch_name(CPU_TYPE_ARM64, 0x8000_0002), "arm64e");
    assert_eq!(arch_name(99, 0), "cpu_99");
}

#[test]
fn rejects_and_survives_damage() {
    assert_eq!(MachO::parse(b"").unwrap_err().kind, ErrorKind::Truncated);
    assert_eq!(
        MachO::parse(b"\x7fELF\0\0\0\0").unwrap_err().kind,
        ErrorKind::BadMagic
    );
    let clean = sample(true, Endian::Little);
    // A zero command size stops the walk.
    let mut d = clean.clone();
    d[36..40].copy_from_slice(&0u32.to_le_bytes());
    assert_eq!(MachO::parse(&d).unwrap().load_commands().len(), 0);
    // A section count past the command's size reads only what fits.
    let mut d = clean.clone();
    d[32 + 64..32 + 68].copy_from_slice(&100u32.to_le_bytes());
    assert_eq!(MachO::parse(&d).unwrap().segments()[0].sections.len(), 2);
    // A huge symbol count stops at the end of the file.
    let mut d = clean.clone();
    let st = clean.len() - 10 - 2 * 16 - 16 - 24 + 12;
    d[st..st + 4].copy_from_slice(&u32::MAX.to_le_bytes());
    let m = MachO::parse(&d).unwrap();
    assert!(m.symbols().len() < 10);
    // Every truncation and single-byte corruption parses or fails cleanly.
    let walk = |d: &[u8]| {
        let Ok(m) = MachO::parse(d) else { return };
        for c in m.load_commands() {
            let _ = m.dyld_info(&c);
            let _ = m.linkedit_data(&c);
        }
        let _ = m.segments();
        let _ = m.symbols();
        let _ = m.dysymtab();
        let _ = m.uuid();
    };
    for is64 in [false, true] {
        for endian in [Endian::Little, Endian::Big] {
            let clean = sample(is64, endian);
            for n in 0..clean.len() {
                walk(&clean[..n]);
            }
            for i in 0..clean.len() {
                for v in [0u8, 0xFF, 0x80] {
                    let mut d = clean.clone();
                    d[i] = v;
                    walk(&d);
                }
            }
        }
    }
    let _ = vec![0u8; 0];
}

#[test]
fn cpu_types_name_architectures() {
    use hexcavator_core::Arch;
    assert_eq!(arch(CPU_TYPE_X86_64), Arch::X64);
    assert_eq!(arch(CPU_TYPE_X86), Arch::X86);
    assert_eq!(arch(CPU_TYPE_ARM64), Arch::Arm64);
    assert_eq!(arch(CPU_TYPE_ARM64_32), Arch::Arm64);
    assert_eq!(arch(CPU_TYPE_ARM), Arch::Arm);
    assert_eq!(arch(CPU_TYPE_POWERPC64), Arch::PowerPc64);
    assert_eq!(arch(CPU_TYPE_MC680X0), Arch::M68k);
    assert_eq!(arch(CPU_TYPE_HPPA), Arch::Unknown);
}

/// Relocation entries, as LLVM reads them: a 32-bit target's scattered
/// ones (an address with its high bit set), a big-endian target's plain
/// ones packed from the high bits; and on ARM64 (and x86-64), which have
/// no scattered entries, a high bit that's just the address's.
#[test]
fn relocation_entries() {
    // (`count` entries claimed, from `at`.)
    let entries = |data: &[u8], at: usize, count: u32| {
        let m = MachO::parse(data).unwrap();
        let mut s = m.sections()[0];
        s.reloff = u32::try_from(at).unwrap();
        s.nreloc = count;
        m.relocations(&s)
    };
    // PowerPC (big-endian, 32-bit): a plain entry (symbol 5, PC-relative,
    // four bytes, external, type 3), a scattered one (type 4, at 0x20, of
    // 0x1234), and a section's (section 1, not external).
    let mut data = sample(false, Endian::Big);
    data[4..8].copy_from_slice(&18u32.to_be_bytes());
    let at = data.len();
    for w in [0x10u32, 0x5D3, 0xA400_0020, 0x1234, 0x30, 0x140] {
        data.extend_from_slice(&w.to_be_bytes());
    }
    let r = entries(&data, at, 4);
    // (The fourth runs past the file: none.)
    assert_eq!(r.len(), 3);
    assert_eq!(
        r[0],
        Relocation {
            address: 0x10,
            symbolnum: 5,
            value: None,
            pcrel: true,
            length: 2,
            external: true,
            kind: 3,
        }
    );
    assert!(r[1].is_scattered());
    assert_eq!(
        (r[1].address, r[1].value, r[1].length, r[1].kind, r[1].pcrel),
        (0x20, Some(0x1234), 2, 4, false)
    );
    assert_eq!(
        (r[2].symbolnum, r[2].external, r[2].length, r[2].kind),
        (1, false, 2, 0)
    );
    // ARM64 (little-endian): the high bit is the address's.
    let mut data = sample(true, Endian::Little);
    let at = data.len();
    for w in [0x8000_0010u32, 0x2D00_0007] {
        data.extend_from_slice(&w.to_le_bytes());
    }
    let r = entries(&data, at, 1);
    assert!(!r[0].is_scattered());
    assert_eq!(
        (
            r[0].address,
            r[0].symbolnum,
            r[0].length,
            r[0].external,
            r[0].kind
        ),
        (0x8000_0010, 7, 2, true, 2)
    );
}

#[test]
fn library_short_names_as_llvm_gives_them() {
    for (name, short) in [
        ("/usr/lib/libSystem.B.dylib", Some("libSystem")),
        // (A one-character version, letter or digit, is taken off.)
        ("/usr/lib/libc++.1.dylib", Some("libc++")),
        ("/usr/lib/libobjc.A.dylib", Some("libobjc")),
        ("@rpath/libsecond.dylib", Some("libsecond")),
        ("libfoo.dylib", Some("libfoo")),
        ("/usr/lib/libfoo_debug.dylib", Some("libfoo")),
        ("/usr/lib/libfoo_profile.A.dylib", Some("libfoo")),
        ("/usr/lib/libATS.A_profile.dylib", Some("libATS")),
        ("/usr/lib/lib_x.dylib", Some("lib_x")),
        (
            "/System/Library/Frameworks/Foundation.framework/Versions/C/Foundation",
            Some("Foundation"),
        ),
        (
            "/System/Library/Frameworks/AppKit.framework/AppKit",
            Some("AppKit"),
        ),
        (
            "/System/Library/Frameworks/AppKit.framework/AppKit_debug",
            Some("AppKit"),
        ),
        ("/Library/QuickTime/QT.A.qtx", Some("QT")),
        ("/usr/lib/libfoo.so", None),
        ("/usr/lib/noextension", None),
        ("noslash", None),
    ] {
        assert_eq!(
            library_short_name(name.as_bytes()),
            short.map(str::as_bytes),
            "{name}"
        );
    }
}

/// An arm64e dylib: `__TEXT` (0..0x4000, from the file's start),
/// `__DATA` (0x4000.., `data` at file offset 0x4000), `__LINKEDIT`
/// (`linkedit` at file offset 0x5000), and `command` (its payload after
/// cmd and cmdsize).
fn arm64e_dylib(command: (u32, Vec<u8>), data: &[u8], linkedit: &[u8]) -> Vec<u8> {
    let mut b = B {
        out: Vec::new(),
        endian: Endian::Little,
    };
    let cmds = 3 * 72 + 8 + command.1.len();
    b.u(u64::from(MH_MAGIC_64), 4)
        .u(u64::from(CPU_TYPE_ARM64), 4)
        .u(u64::from(CPU_SUBTYPE_ARM64E | CPU_SUBTYPE_PTRAUTH_ABI), 4)
        .u(u64::from(MH_DYLIB), 4)
        .u(4, 4)
        .u(cmds as u64, 4)
        .u(0, 4)
        .u(0, 4);
    for (name, vm, vmsize, off, size) in [
        (&b"__TEXT"[..], 0u64, 0x4000u64, 0u64, 0x4000u64),
        (b"__DATA", 0x4000, 0x4000, 0x4000, 0x1000),
        (b"__LINKEDIT", 0x8000, 0x4000, 0x5000, linkedit.len() as u64),
    ] {
        b.u(u64::from(LC_SEGMENT_64), 4).u(72, 4).name(name);
        b.u(vm, 8).u(vmsize, 8).u(off, 8).u(size, 8);
        b.u(3, 4).u(3, 4).u(0, 4).u(0, 4);
    }
    b.u(u64::from(command.0), 4)
        .u(8 + command.1.len() as u64, 4);
    b.out.extend_from_slice(&command.1);
    b.out.resize(0x4000, 0);
    b.out.extend_from_slice(data);
    b.out.resize(0x5000, 0);
    b.out.extend_from_slice(linkedit);
    b.out
}

fn le_u32s(words: &[u32]) -> Vec<u8> {
    words.iter().flat_map(|w| w.to_le_bytes()).collect()
}

/// Threaded binds (arm64e's first ABI): the binds after the ordinal
/// table's size fill the table; APPLY walks a chain of arm64e pointers
/// from the segment and offset set: rebases (plain and authenticated) and
/// binds of the table (with an addend, and authenticated), each signed
/// pointer's signing kept.
#[test]
fn threaded_binds() {
    use crate::fixups::{FixupKind, PtrAuth, PtrAuthKey};
    let next = |n: u64| n << 51;
    let data: Vec<u8> = [
        // A bind of _b (ordinal 1), addend 5.
        1u64 << 62 | 5 << 32 | 1 | next(1),
        // A rebase to 0x1234.
        0x1234 | next(1),
        // An authenticated rebase: base + 0x40, DA, diversity 0x77, address.
        1u64 << 63 | 0x40 | 0x77 << 32 | 1 << 48 | 2 << 49 | next(1),
        // An authenticated bind of _a (ordinal 0), IA, diversity 1; the end.
        1u64 << 63 | 1 << 62 | 1 << 32,
    ]
    .iter()
    .flat_map(|v| v.to_le_bytes())
    .collect();
    let mut bind = vec![0x11u8]; // library 1
    bind.extend_from_slice(&[0xD0, 2]); // threaded: a table of 2
    bind.extend_from_slice(b"\x40_a\0\x90\x40_b\0\x90");
    bind.extend_from_slice(&[0x71, 0]); // segment 1, offset 0
    bind.extend_from_slice(&[0xD1, 0x00]); // apply; done
    let info = le_u32s(&[0, 0, 0x5000, bind.len() as u32, 0, 0, 0, 0, 0, 0]);
    let file = arm64e_dylib((LC_DYLD_INFO_ONLY, info), &data, &bind);
    let m = MachO::parse(&file).unwrap();
    let f = m.fixups();
    assert_eq!(f.len(), 4, "{f:#?}");
    let bind_of = |k: &FixupKind<'_>| match *k {
        FixupKind::Bind {
            symbol,
            library,
            addend,
            ..
        } => Some((symbol.to_vec(), library, addend)),
        FixupKind::Rebase { .. } => None,
    };
    assert_eq!(f[0].addr, 0x4000);
    assert_eq!(bind_of(&f[0].kind), Some((b"_b".to_vec(), 1, 5)));
    assert_eq!(f[0].auth, None);
    assert_eq!(f[1].kind, FixupKind::Rebase { target: 0x1234 });
    assert_eq!(f[2].kind, FixupKind::Rebase { target: 0x40 });
    assert_eq!(
        f[2].auth,
        Some(PtrAuth {
            key: PtrAuthKey::DA,
            diversity: 0x77,
            address: true
        })
    );
    assert_eq!(bind_of(&f[3].kind), Some((b"_a".to_vec(), 1, 0)));
    assert_eq!(
        f[3].auth,
        Some(PtrAuth {
            key: PtrAuthKey::IA,
            diversity: 1,
            address: false
        })
    );
    assert!(
        f.iter()
            .all(|x| x.format == crate::fixups::DYLD_CHAINED_PTR_ARM64E)
    );
}

/// DYLD_CHAINED_PTR_ARM64E_SEGMENTED (firmware's): rebases by segment
/// index and offset, plain and authenticated, a stride of 4.
#[test]
fn segmented_chained_fixups() {
    use crate::fixups::{DYLD_CHAINED_PTR_ARM64E_SEGMENTED, FixupKind, PtrAuth, PtrAuthKey};
    let data: Vec<u8> = [
        // __TEXT + 0x100; the next 8 bytes on (two strides).
        0x100u64 | 2 << 51,
        // __DATA + 0x10, authenticated: IB, diversity 0x1234; the end.
        1u64 << 63 | 1 << 28 | 0x10 | 0x1234 << 32 | 1 << 49,
    ]
    .iter()
    .flat_map(|v| v.to_le_bytes())
    .collect();
    // The header; the starts in the image (two segments, the second's at
    // 12); the second segment's starts (one 16 KiB page, from 0).
    let mut blob = le_u32s(&[0, 32, 72, 72, 0, 1, 0, 0]);
    blob.extend_from_slice(&le_u32s(&[3, 0, 16, 0]));
    let mut seg = 24u32.to_le_bytes().to_vec();
    seg.extend_from_slice(&0x4000u16.to_le_bytes());
    seg.extend_from_slice(&DYLD_CHAINED_PTR_ARM64E_SEGMENTED.to_le_bytes());
    seg.extend_from_slice(&0x4000u64.to_le_bytes());
    seg.extend_from_slice(&0u32.to_le_bytes());
    seg.extend_from_slice(&1u16.to_le_bytes());
    seg.extend_from_slice(&0u16.to_le_bytes());
    blob.extend_from_slice(&seg);
    let cmd = le_u32s(&[0x5000, blob.len() as u32]);
    let file = arm64e_dylib((LC_DYLD_CHAINED_FIXUPS, cmd), &data, &blob);
    let m = MachO::parse(&file).unwrap();
    let f = m.fixups();
    assert_eq!(f.len(), 2, "{f:#?}");
    assert_eq!(
        (f[0].addr, f[0].kind),
        (0x4000, FixupKind::Rebase { target: 0x100 })
    );
    assert_eq!(f[0].auth, None);
    assert_eq!(
        (f[1].addr, f[1].kind),
        (0x4008, FixupKind::Rebase { target: 0x4010 })
    );
    assert_eq!(
        f[1].auth,
        Some(PtrAuth {
            key: PtrAuthKey::IB,
            diversity: 0x1234,
            address: false
        })
    );
}
