extern crate std;

use alloc::borrow::Cow;
use alloc::vec;
use alloc::vec::Vec;

use super::*;
use crate::reloc::{arm64, x86_64};

fn section<'a>(
    sect: &[u8],
    seg: &[u8],
    flags: u32,
    align: u32,
    contents: Contents<'a>,
) -> Section<'a> {
    Section {
        sectname: name_field(sect),
        segname: name_field(seg),
        addr: 0,
        align,
        flags,
        reserved1: 0,
        reserved2: 0,
        reserved3: 0,
        contents,
        relocations: Vec::new(),
    }
}

fn sym<'a>(name: &'a [u8], n_type: u8, n_sect: u8, n_value: u64) -> Symbol<'a> {
    Symbol {
        name: Cow::Borrowed(name),
        name_offset: 0,
        n_type,
        n_sect,
        n_desc: 0,
        n_value,
        indirect: None,
    }
}

fn header(cputype: u32, wide: bool, endian: Endian) -> Header {
    Header {
        magic: if wide { MH_MAGIC_64 } else { MH_MAGIC },
        endian,
        cputype,
        cpusubtype: 3,
        filetype: MH_OBJECT,
        flags: MH_SUBSECTIONS_VIA_SYMBOLS,
        reserved: 0,
    }
}

/// An x86_64-like object: __text (a call to an undefined symbol, a local
/// RIP-relative reference to __data), __data (a local pointer to __text,
/// a SUBTRACTOR pair), a zero-fill __bss between them in file order,
/// __cstring, a build version, data in code, and a symbol table of
/// locals, external definitions and undefined symbols.
fn object(cputype: u32, wide: bool, endian: Endian) -> Object<'static> {
    let mut text = section(
        b"__text",
        b"__TEXT",
        S_ATTR_PURE_INSTRUCTIONS | S_ATTR_SOME_INSTRUCTIONS,
        4,
        Contents::Bytes(Cow::Owned(vec![0x90; 21])),
    );
    text.relocations = vec![
        Relocation {
            pcrel: true,
            ..Relocation::plain(1, 4, true, x86_64::BRANCH, 2)
        },
        Relocation {
            pcrel: true,
            ..Relocation::plain(8, 3, false, x86_64::SIGNED, 2)
        },
    ];
    let bss = section(b"__bss", b"__DATA", S_ZEROFILL, 3, Contents::Zerofill(40));
    let mut data = section(
        b"__data",
        b"__DATA",
        0,
        3,
        Contents::Bytes(Cow::Owned(vec![0; 24])),
    );
    data.relocations = vec![
        Relocation::plain(0, 1, false, x86_64::UNSIGNED, 3),
        Relocation::plain(8, 0, true, x86_64::SUBTRACTOR, 3),
        Relocation::plain(8, 1, true, x86_64::UNSIGNED, 3),
    ];
    let cstring = section(
        b"__cstring",
        b"__TEXT",
        S_CSTRING_LITERALS,
        0,
        Contents::Bytes(Cow::Borrowed(b"hi\0there\0")),
    );
    let segment = Segment {
        segname: [0; 16],
        vmaddr: 0,
        maxprot: 7,
        initprot: 7,
        flags: 0,
        sections: vec![text, bss, data, cstring],
    };
    let mut build = Vec::new();
    for v in [1u32, 0x000D_0000, 0, 0] {
        match endian {
            Endian::Little => build.extend_from_slice(&v.to_le_bytes()),
            Endian::Big => build.extend_from_slice(&v.to_be_bytes()),
        }
    }
    let dic = linkedit::encode_data_in_code(
        &[linkedit::DataInCode {
            offset: 16,
            length: 4,
            kind: 1,
        }],
        endian,
    );
    let mut o = Object::new(
        header(cputype, wide, endian),
        vec![
            Command::Segment(segment),
            Command::Other {
                cmd: LC_BUILD_VERSION,
                body: Cow::Owned(build),
            },
            Command::LinkeditData {
                cmd: LC_DATA_IN_CODE,
                data: Cow::Owned(dic),
            },
            Command::Symtab,
            Command::Dysymtab(Dysymtab::default()),
        ],
    );
    o.symbols = vec![
        sym(b"ltmp0", N_SECT, 1, 0),
        sym(b"l_.str", N_SECT, 4, 0),
        sym(b"_f", N_SECT | N_EXT, 1, 0),
        sym(b"_g", N_SECT | N_EXT, 3, 0),
        sym(b"_undef", N_UNDF | N_EXT, 0, 0),
    ];
    let addresses = o.llvm_addresses();
    for (s, a) in o.sections_mut().zip(addresses) {
        s.addr = a;
    }
    // The values the relocations encode, at those addresses: __text's
    // SIGNED reference to __data+8 (from the end of its 4 bytes), __data's
    // pointer to __text+4.
    let data_addr = o.section(2).unwrap().addr;
    let text_addr = o.section(0).unwrap().addr;
    let disp = (data_addr + 8).wrapping_sub(text_addr + 8 + 4) as u32;
    let put = |s: &mut Section<'_>, at: usize, b: &[u8]| {
        let Contents::Bytes(c) = &mut s.contents else {
            unreachable!()
        };
        c.to_mut()[at..at + b.len()].copy_from_slice(b);
    };
    let (d, p) = match endian {
        Endian::Little => (disp.to_le_bytes(), (text_addr + 4).to_le_bytes()),
        Endian::Big => (disp.to_be_bytes(), (text_addr + 4).to_be_bytes()),
    };
    {
        let mut secs = o.sections_mut();
        put(secs.next().unwrap(), 8, &d);
        secs.next();
        put(secs.next().unwrap(), 0, &p);
    }
    // The symbols at their sections.
    for i in 0..o.symbols.len() {
        let n = o.symbols[i].n_sect;
        if n != 0 {
            o.symbols[i].n_value = o.section(usize::from(n) - 1).unwrap().addr;
        }
    }
    o.symbols[3].n_value += 8;
    o.update_dysymtab();
    o
}

fn same(a: &Object<'_>, b: &Object<'_>) {
    assert_eq!(a.header, b.header);
    assert_eq!(a.commands, b.commands);
    assert_eq!(a.indirect_symbols, b.indirect_symbols);
    assert_eq!(a.symbols.len(), b.symbols.len());
    for (s, t) in a.symbols.iter().zip(&b.symbols) {
        assert_eq!(
            (&s.name, s.n_type, s.n_sect, s.n_desc, s.n_value),
            (&t.name, t.n_type, t.n_sect, t.n_desc, t.n_value)
        );
    }
}

#[test]
fn writes_and_reads_back() {
    for (cputype, wide, endian) in [
        (CPU_TYPE_X86_64, true, Endian::Little),
        (CPU_TYPE_X86, false, Endian::Little),
        (CPU_TYPE_POWERPC, false, Endian::Big),
        (CPU_TYPE_POWERPC64, true, Endian::Big),
    ] {
        let o = object(cputype, wide, endian);
        let bytes = o.write().unwrap();
        let back = Object::parse(&bytes).unwrap();
        same(&o, &back);
        assert!(back.keeps_layout());
        assert_eq!(back.write().unwrap(), bytes);
        // A fresh layout of what was read is the same file.
        let mut fresh = back.clone();
        fresh.relayout();
        assert!(!fresh.keeps_layout());
        assert_eq!(fresh.write().unwrap(), bytes);
    }
}

#[test]
fn lays_out_as_llvm() {
    let o = object(CPU_TYPE_X86_64, true, Endian::Little);
    // __text at 0 (21 bytes), __data at 24 (align 8), __cstring at 48,
    // then the zero-fill __bss after everything, aligned: 57 -> 64.
    let addresses: Vec<u64> = o.sections().map(|s| s.addr).collect();
    assert_eq!(addresses, [0, 64, 24, 48]);
    let bytes = o.write().unwrap();
    let back = turbonop_macho::MachO::parse(&bytes).unwrap();
    let sections = back.sections();
    // Header 32, segment 72 + 4 * 80, build version 24, data in code 16,
    // symtab 24, dysymtab 80.
    let start = 32 + 72 + 4 * 80 + 24 + 16 + 24 + 80;
    assert_eq!(sections[0].offset, start);
    assert_eq!(sections[1].offset, 0);
    assert_eq!(sections[2].offset, start + 24);
    // The section data (57 bytes) padded to 8, then the relocations.
    let relocs = start + 64;
    assert_eq!(sections[0].reloff, relocs);
    assert_eq!(sections[2].reloff, relocs + 16);
    let syms = back.symtab().unwrap();
    // Relocations (5 * 8), data in code (8), then the symbols.
    assert_eq!(u64::from(syms.symoff), u64::from(relocs) + 40 + 8);
    assert_eq!(syms.stroff, syms.symoff + 5 * 16);
    let d = back.dysymtab().unwrap();
    assert_eq!((d.nlocalsym, d.nextdefsym, d.nundefsym), (2, 2, 1));
}

#[test]
fn string_table_is_llvms() {
    let names: [&[u8]; 5] = [b"_foo", b"_barfoo", b"_x", b"foo", b"_x"];
    let (table, at) = llvm_string_table(names.iter().copied(), true);
    assert_eq!(&table[..], b"\0_x\0_barfoo\0_foo\0\0\0\0\0\0\0\0");
    assert_eq!(table.len(), 24);
    assert_eq!(
        (
            at[&b"_x"[..]],
            at[&b"_barfoo"[..]],
            at[&b"_foo"[..]],
            at[&b"foo"[..]]
        ),
        (1, 4, 12, 13)
    );
    let (table, _) = llvm_string_table(names.iter().copied(), false);
    assert_eq!(table.len(), 20);
    // A name that is a suffix of the one before it shares its bytes.
    let (table, at) = llvm_string_table([&b"_ab"[..], b"ab", b"b"].into_iter(), false);
    assert_eq!(&table[..], b"\0_ab\0\0\0\0");
    assert_eq!((at[&b"ab"[..]], at[&b"b"[..]]), (2, 3));
}

#[test]
fn keeps_the_string_table_while_names_are_found() {
    let mut o = object(CPU_TYPE_X86_64, true, Endian::Little);
    // A table of its own order, with a name no symbol has.
    let table = b"\0junk\0_undef\0_g\0_f\0l_.str\0ltmp0\0".to_vec();
    for s in &mut o.symbols {
        let at = table
            .windows(s.name.len() + 1)
            .position(|w| w[..s.name.len()] == s.name[..] && w[s.name.len()] == 0)
            .unwrap();
        s.name_offset = at as u32;
    }
    o.strings = Some(Cow::Owned(table.clone()));
    let bytes = o.write().unwrap();
    let back = Object::parse(&bytes).unwrap();
    assert_eq!(back.strings.as_deref(), Some(&table[..]));
    // Renaming one rebuilds it.
    let mut renamed = back.clone();
    renamed.symbols[2].name = Cow::Borrowed(b"_renamed");
    let bytes = renamed.write().unwrap();
    let again = Object::parse(&bytes).unwrap();
    assert_eq!(&*again.symbols[2].name, b"_renamed");
    assert!(!again.strings.as_deref().unwrap().starts_with(b"\0junk"));
}

#[test]
fn in_place_edits_keep_the_layout() {
    let o = object(CPU_TYPE_X86_64, true, Endian::Little);
    let bytes = o.write().unwrap();
    let mut back = Object::parse(&bytes).unwrap();
    let Some(Command::Segment(seg)) = back.commands.first_mut() else {
        unreachable!()
    };
    let Contents::Bytes(b) = &mut seg.sections[3].contents else {
        unreachable!()
    };
    b.to_mut()[0] = b'H';
    back.symbols[4].n_desc = 0x10;
    assert!(back.keeps_layout());
    let out = back.write().unwrap();
    let changed: Vec<usize> = (0..out.len()).filter(|&i| out[i] != bytes[i]).collect();
    assert_eq!(changed.len(), 2);
    let again = Object::parse(&out).unwrap();
    assert_eq!(again.section(3).unwrap().bytes().unwrap()[0], b'H');
    assert_eq!(again.symbols[4].n_desc, 0x10);
}

#[test]
fn relocations_encode_both_ways() {
    let samples = [
        Relocation::plain(0x1234, 0x00AB_CDEF, true, 0xF, 3),
        Relocation {
            pcrel: true,
            ..Relocation::plain(0x7FFF_FFFF, 0, false, 0, 0)
        },
        Relocation {
            address: 0x00FF_FFFF,
            symbolnum: 0,
            pcrel: true,
            length: 2,
            external: false,
            kind: 4,
            value: Some(0xDEAD_BEEF),
        },
    ];
    for endian in [Endian::Little, Endian::Big] {
        for r in samples {
            let (w0, w1) = write::encode_relocation(&r, endian).unwrap();
            assert_eq!(read::decode_relocation(w0, w1, endian), r);
        }
    }
    // Little-endian plain: symbolnum low 24 bits, then pcrel, length,
    // extern, type.
    let (_, w1) = write::encode_relocation(&samples[0], Endian::Little).unwrap();
    assert_eq!(w1, 0xFEAB_CDEF);
    let (_, w1) = write::encode_relocation(&samples[0], Endian::Big).unwrap();
    assert_eq!(w1, 0xABCD_EF7F);
    assert!(
        write::encode_relocation(
            &Relocation::plain(0x8000_0000, 0, false, 0, 2),
            Endian::Little
        )
        .is_err()
    );
}

#[test]
fn sorts_symbols_as_llvm() {
    let mut o = object(CPU_TYPE_X86_64, true, Endian::Little);
    // Undefined first, then a definition, then the locals.
    o.symbols.rotate_left(4);
    o.symbols.swap(1, 2);
    // _undef, l_.str, ltmp0, _f, _g: the call names _undef (now 0).
    let Some(Command::Segment(seg)) = o.commands.first_mut() else {
        unreachable!()
    };
    seg.sections[0].relocations[0].symbolnum = 0;
    seg.sections[2].relocations[1].symbolnum = 3;
    assert!(!o.symbols_in_order());
    let map = o.sort_symbols();
    assert!(o.symbols_in_order());
    let names: Vec<&[u8]> = o.symbols.iter().map(|s| &s.name[..]).collect();
    assert_eq!(names, [&b"l_.str"[..], b"ltmp0", b"_f", b"_g", b"_undef"]);
    assert_eq!(map[0], Some(4));
    assert_eq!(o.section(0).unwrap().relocations[0].symbolnum, 4);
    assert_eq!(o.section(2).unwrap().relocations[1].symbolnum, 2);
    assert_eq!(
        *o.dysymtab().unwrap(),
        Dysymtab {
            ilocalsym: 0,
            nlocalsym: 2,
            iextdefsym: 2,
            nextdefsym: 2,
            iundefsym: 4,
            nundefsym: 1,
        }
    );
}

#[test]
fn retains_symbols() {
    let mut o = object(CPU_TYPE_X86_64, true, Endian::Little);
    o.indirect_symbols = vec![4, INDIRECT_SYMBOL_LOCAL, 2];
    let map = o.retain_symbols(&[true, false, true, false, true]);
    assert_eq!(map, [Some(0), None, Some(1), None, Some(2)]);
    assert_eq!(o.section(0).unwrap().relocations[0].symbolnum, 2);
    assert_eq!(o.indirect_symbols, [2, INDIRECT_SYMBOL_LOCAL, 1]);
    assert_eq!(o.dysymtab().unwrap().nlocalsym, 1);
    assert_eq!(o.dysymtab().unwrap().iundefsym, 2);
    let bytes = o.write().unwrap();
    let back = Object::parse(&bytes).unwrap();
    same(&o, &back);
    assert!(!back.strings.unwrap().windows(6).any(|w| w == b"l_.str"));
}

#[test]
fn retains_sections_and_moves_addresses() {
    let mut o = object(CPU_TYPE_X86_64, true, Endian::Little);
    // Data in code in __cstring, which goes, and in __text, which stays.
    for c in &mut o.commands {
        if let Command::LinkeditData { data, .. } = c {
            *data = Cow::Owned(linkedit::encode_data_in_code(
                &[
                    linkedit::DataInCode {
                        offset: 16,
                        length: 4,
                        kind: 1,
                    },
                    linkedit::DataInCode {
                        offset: 49,
                        length: 2,
                        kind: 1,
                    },
                ],
                Endian::Little,
            ));
        }
    }
    // Remove __bss and __cstring (and l_.str with it).
    o.retain_symbols(&[true, false]);
    let map = o.retain_sections(&[true, false, true, false]);
    assert_eq!(map, [Some(0), None, Some(1), None]);
    // __data is section 2 now: its symbol and the reference to it.
    assert_eq!(o.symbols[2].n_sect, 2);
    assert_eq!(o.section(0).unwrap().relocations[1].symbolnum, 2);
    assert_eq!(o.section(1).unwrap().relocations[0].symbolnum, 1);
    let dic =
        linkedit::decode_data_in_code(o.linkedit_data(LC_DATA_IN_CODE).unwrap(), Endian::Little)
            .unwrap();
    assert_eq!(dic.len(), 1);
    // Move __data before __text by hand, then give both LLVM's addresses.
    let old_data = o.local_address(0, 1).unwrap();
    assert_eq!(old_data, 24 + 8);
    assert_eq!(o.local_address(1, 0), Some(4));
    let unadjusted = o.move_sections(&[0x100, 0], &[(0, 21), (24, 24)]);
    assert!(unadjusted.is_empty());
    assert_eq!(o.local_address(0, 1), Some(8));
    assert_eq!(o.local_address(1, 0), Some(0x104));
    assert_eq!(o.symbols[1].n_value, 0x100);
    assert_eq!(o.symbols[2].n_value, 8);
    let unadjusted = o.assign_addresses();
    assert!(unadjusted.is_empty());
    assert_eq!(o.llvm_addresses(), [0, 24]);
    assert_eq!(o.local_address(0, 1), Some(24 + 8));
    assert_eq!(o.local_address(1, 0), Some(4));
    assert_eq!(o.symbols[2].n_value, 24 + 8);
    let dic =
        linkedit::decode_data_in_code(o.linkedit_data(LC_DATA_IN_CODE).unwrap(), Endian::Little)
            .unwrap();
    assert_eq!(dic[0].offset, 16);
    let bytes = o.write().unwrap();
    same(&o, &Object::parse(&bytes).unwrap());
}

#[test]
fn removed_sections_leave_undefined_symbols() {
    let mut o = object(CPU_TYPE_X86_64, true, Endian::Little);
    o.retain_sections(&[false]);
    assert_eq!(o.symbols[0].n_type & N_TYPE, N_UNDF);
    assert_eq!(o.symbols[0].n_sect, NO_SECT);
    assert_eq!(o.symbols[3].n_sect, 2);
    // The local pointer to __text names no section now.
    assert_eq!(o.section(1).unwrap().relocations[0].symbolnum, 0);
}

#[test]
fn arm64_pairs_and_addends() {
    let mut o = object(CPU_TYPE_ARM64, true, Endian::Little);
    let Some(Command::Segment(seg)) = o.commands.first_mut() else {
        unreachable!()
    };
    // ADDEND 3 (in r_symbolnum) then PAGE21; a SUBTRACTOR pair; a local
    // pointer to __cstring.
    seg.sections[0].relocations = vec![
        Relocation::plain(0, 3, false, arm64::ADDEND, 2),
        Relocation {
            pcrel: true,
            ..Relocation::plain(0, 2, true, arm64::PAGE21, 2)
        },
        Relocation::plain(4, 2, true, arm64::SUBTRACTOR, 3),
        Relocation::plain(4, 3, true, arm64::UNSIGNED, 3),
        Relocation::plain(12, 4, false, arm64::UNSIGNED, 3),
    ];
    let groups = reloc::fixups(CPU_TYPE_ARM64, &seg.sections[0].relocations);
    assert_eq!(groups, [0..2, 2..4, 4..5]);
    o.retain_sections(&[true, false]);
    let r = &o.section(0).unwrap().relocations;
    // The addend is kept; the ordinal of __cstring is 3 now.
    assert_eq!(r[0].symbolnum, 3);
    assert_eq!(r[4].symbolnum, 3);
    assert_eq!(
        reloc::fixups(CPU_TYPE_X86_64, &o.section(1).unwrap().relocations),
        [0..1, 1..3]
    );
}

#[test]
fn unknown_encodings_are_listed() {
    let mut o = object(CPU_TYPE_ARM64, true, Endian::Little);
    let Some(Command::Segment(seg)) = o.commands.first_mut() else {
        unreachable!()
    };
    // A local PAGE21 to __data: an instruction's immediate.
    seg.sections[0].relocations = vec![Relocation {
        pcrel: true,
        ..Relocation::plain(0, 3, false, arm64::PAGE21, 2)
    }];
    let unadjusted = o.move_sections(&[0, 64, 32, 48], &[(0, 21), (64, 40), (24, 24), (48, 9)]);
    assert_eq!(
        unadjusted,
        [Unadjusted {
            section: 0,
            relocation: Some(0)
        }]
    );
}

#[test]
fn hints_encode_and_decode() {
    let hints = [
        linkedit::Hint {
            kind: 1,
            args: vec![0, 0x1234],
        },
        linkedit::Hint {
            kind: 8,
            args: vec![300],
        },
    ];
    let data = linkedit::encode_hints(&hints, true);
    assert_eq!(data.len() % 8, 0);
    assert_eq!(&data[..6], &[1, 2, 0, 0xB4, 0x24, 8]);
    assert_eq!(linkedit::decode_hints(&data).unwrap(), hints);
    assert_eq!(linkedit::encode_hints(&hints, false).len() % 4, 0);
}

#[test]
fn eh_frame_pointers() {
    // A CIE "zPLR" (personality udata4|pcrel|indirect, LSDA and FDE
    // sdata4|pcrel), then an FDE with an LSDA.
    let mut d = Vec::new();
    let cie = [
        &[0u8, 0, 0, 0][..], // CIE id
        &[1],                // version
        b"zPLR\0",
        &[1, 0x78, 0x10], // code align, data align, return register
        &[7],             // augmentation data length
        &[0x9B, 0, 0, 0, 0],
        &[0x1B],
        &[0x1B],
    ]
    .concat();
    d.extend_from_slice(&(cie.len() as u32 + 3).to_le_bytes());
    d.extend_from_slice(&cie);
    d.extend_from_slice(&[0, 0, 0]);
    let fde_at = d.len();
    let fde = [
        &((fde_at + 4) as u32).to_le_bytes()[..],
        &(-0x40i32 as u32).to_le_bytes(),
        &16u32.to_le_bytes(),
        &[4],
        &(0x10u32).to_le_bytes(),
    ]
    .concat();
    d.extend_from_slice(&(fde.len() as u32).to_le_bytes());
    d.extend_from_slice(&fde);
    let p = ehframe::pointers(&d, true, Endian::Little).unwrap();
    let kinds: Vec<(ehframe::PointerKind, usize)> = p.iter().map(|p| (p.kind, p.offset)).collect();
    assert_eq!(
        kinds,
        [
            (ehframe::PointerKind::Personality, 19),
            (ehframe::PointerKind::PcBegin, fde_at + 8),
            (ehframe::PointerKind::Lsda, fde_at + 17),
        ]
    );
    // Moved with an object: __text at 0, __eh_frame at 0x40 moving to
    // 0x80 (the FDE's function at 0x40 + fde_at + 8 - 0x40).
    let mut text = section(
        b"__text",
        b"__TEXT",
        S_ATTR_PURE_INSTRUCTIONS,
        4,
        Contents::Bytes(Cow::Owned(vec![0; 0x40])),
    );
    text.addr = 0;
    let mut eh = section(
        b"__eh_frame",
        b"__TEXT",
        S_COALESCED,
        3,
        Contents::Bytes(Cow::Owned(d.clone())),
    );
    eh.addr = 0x40;
    let mut o = Object::new(
        header(CPU_TYPE_X86_64, true, Endian::Little),
        vec![Command::Segment(Segment {
            segname: [0; 16],
            vmaddr: 0,
            maxprot: 7,
            initprot: 7,
            flags: 0,
            sections: vec![text, eh],
        })],
    );
    let size = d.len() as u64;
    assert!(
        o.move_sections(&[0, 0x80], &[(0, 0x40), (0x40, size)])
            .is_empty()
    );
    let moved = o.section(1).unwrap().bytes().unwrap();
    let at = fde_at + 8;
    let pc_begin = i32::from_le_bytes(moved[at..at + 4].try_into().unwrap());
    assert_eq!(pc_begin, -0x40 - 0x40);
    // The personality pointer (0, relative to itself: into __eh_frame)
    // moved with its section.
    assert_eq!(&moved[19..23], &[0, 0, 0, 0]);
}

#[test]
fn rejects_what_isnt_an_object() {
    assert_eq!(Object::parse(b"\x7fELF").unwrap_err(), ReadError::NotMachO);
    let mut o = object(CPU_TYPE_X86_64, true, Endian::Little);
    o.header.filetype = MH_EXECUTE;
    let bytes = o.write().unwrap();
    assert!(matches!(
        Object::parse(&bytes),
        Err(ReadError::Unsupported(_))
    ));
    let mut o = object(CPU_TYPE_X86, false, Endian::Little);
    o.symbols[2].n_value = 1 << 40;
    assert!(matches!(o.write(), Err(WriteError::TooLarge(_))));
    let o = object(CPU_TYPE_X86_64, true, Endian::Little);
    let bytes = o.write().unwrap();
    for cut in [10, 40, bytes.len() - 1] {
        assert!(Object::parse(&bytes[..cut]).is_err());
    }
}
