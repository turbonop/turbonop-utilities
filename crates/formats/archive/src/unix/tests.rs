// Test archives are small: offsets fit in 32 bits.
#![allow(clippy::cast_possible_truncation)]

extern crate std;

use alloc::vec;
use std::string::String;

use super::*;

fn sym(name: &[u8], member: u32) -> IndexSymbol<'_> {
    IndexSymbol {
        name: Cow::Borrowed(name),
        member,
    }
}

fn det() -> Fields {
    Fields::new(b"0", b"0", b"0", b"644").unwrap()
}

fn member<'a>(name: &'a [u8], data: &'a [u8]) -> NewMember<'a> {
    NewMember {
        entry: Entry {
            name,
            origin: None,
            fields: det(),
            size: data.len() as u64,
        },
        data,
    }
}

fn hdr(name: &str, date: &str, uid: &str, gid: &str, mode: &str, size: u64) -> Vec<u8> {
    let s = std::format!("{name:<16}{date:<12}{uid:<6}{gid:<6}{mode:<8}{size:<10}`\n");
    assert_eq!(s.len(), 60);
    s.into_bytes()
}

fn text(b: &[u8]) -> String {
    String::from_utf8_lossy(b).into_owned()
}

/// A GNU symbol table's body: big-endian count, offsets, names.
fn gnu_table(entries: &[(&[u8], u32)]) -> Vec<u8> {
    let mut b = Vec::new();
    b.extend_from_slice(&(entries.len() as u32).to_be_bytes());
    for (_, o) in entries {
        b.extend_from_slice(&o.to_be_bytes());
    }
    for (n, _) in entries {
        b.extend_from_slice(n);
        b.push(0);
    }
    b
}

#[test]
fn gnu_short_and_long_names() {
    let members = vec![
        member(b"a.o", b"AAA"),
        member(b"exactly15chars.", b"BB"),
        member(b"sixteen_chars_.o", b"C"),
        member(b"sixteen_chars_.o", b"DD"),
        member(b"sub/x.o", b"E"),
    ];
    let symbols = vec![sym(b"f", 0), sym(b"g", 2), sym(b"f", 3)];
    let gnu = write(
        &members,
        Some(&symbols),
        &Options::new(Archiver::GnuAr, Dialect::Gnu),
    )
    .unwrap();
    // GNU: a name twice in the long names, and `sub/x.o` short.
    let long = b"sixteen_chars_.o/\nsixteen_chars_.o/\n";
    let table_len = 4 + 3 * 4 + 6;
    let first = 8 + 60 + table_len + 60 + long.len();
    let offsets = [
        first,
        first + 60 + 4,
        first + 60 + 4 + 60 + 2,
        first + 60 + 4 + 60 + 2 + 60 + 2,
    ];
    let mut want = Vec::from(&MAGIC[..]);
    want.extend(hdr("/", "0", "0", "0", "0", table_len as u64));
    want.extend(gnu_table(&[
        (b"f", offsets[0] as u32),
        (b"g", offsets[2] as u32),
        (b"f", offsets[3] as u32),
    ]));
    want.extend(hdr("//", "", "", "", "", long.len() as u64));
    want.extend_from_slice(long);
    want.extend(hdr("a.o/", "0", "0", "0", "644", 3));
    want.extend_from_slice(b"AAA\n");
    want.extend(hdr("exactly15chars./", "0", "0", "0", "644", 2));
    want.extend_from_slice(b"BB");
    want.extend(hdr("/0", "0", "0", "0", "644", 1));
    want.extend_from_slice(b"C\n");
    want.extend(hdr("/18", "0", "0", "0", "644", 2));
    want.extend_from_slice(b"DD");
    want.extend(hdr("sub/x.o/", "0", "0", "0", "644", 1));
    want.extend_from_slice(b"E\n");
    assert_eq!(text(&gnu), text(&want));

    // llvm-ar: each long name once, and a name with `/` long too.
    let llvm = write(
        &members,
        Some(&symbols),
        &Options::new(Archiver::LlvmAr, Dialect::Gnu),
    )
    .unwrap();
    let archive = Archive::parse(&llvm).unwrap();
    let names: Vec<_> = archive
        .members()
        .map(|m| text(&archive.data()[m.header_offset as usize..][..16]))
        .collect();
    assert_eq!(
        names,
        [
            "/               ",
            "//              ",
            "a.o/            ",
            "exactly15chars./",
            "/0              ",
            "/0              ",
            "/18             "
        ]
    );
    let long = archive
        .members()
        .find(|m| m.kind == MemberKind::LongNames)
        .unwrap();
    // (Padded to an even size, inside it.)
    assert_eq!(long.data, b"sixteen_chars_.o/\nsub/x.o/\n\n");
    // Both read back, and write again the same.
    for (bytes, archiver) in [(&gnu, Archiver::GnuAr), (&llvm, Archiver::LlvmAr)] {
        let a = Archive::parse(bytes).unwrap();
        let c = Contents::read(&a).unwrap();
        assert_eq!(c.members.len(), 5);
        assert_eq!(&c.write(archiver).unwrap(), bytes);
    }
}

#[test]
fn tables_without_symbols() {
    let members = vec![member(b"a.o", b"AAAA")];
    let gnu = write(
        &members,
        Some(&[]),
        &Options::new(Archiver::GnuAr, Dialect::Gnu),
    )
    .unwrap();
    let mut want = Vec::from(&MAGIC[..]);
    want.extend(hdr("/", "0", "0", "0", "0", 4));
    want.extend_from_slice(&[0; 4]);
    want.extend(hdr("a.o/", "0", "0", "0", "644", 4));
    want.extend_from_slice(b"AAAA");
    assert_eq!(gnu, want);
    let llvm = write(
        &members,
        Some(&[]),
        &Options::new(Archiver::LlvmAr, Dialect::Gnu),
    )
    .unwrap();
    let mut want = Vec::from(&MAGIC[..]);
    want.extend(hdr("/", "0", "0", "0", "0", 8));
    want.extend_from_slice(&[0; 8]);
    want.extend(hdr("a.o/", "0", "0", "0", "644", 4));
    want.extend_from_slice(b"AAAA");
    assert_eq!(llvm, want);
    // No table at all.
    let none = write(&members, None, &Options::new(Archiver::GnuAr, Dialect::Gnu)).unwrap();
    assert_eq!(none.len(), 8 + 64);
    // An empty archive is its signature.
    let empty = write(&[], None, &Options::new(Archiver::LlvmAr, Dialect::Gnu)).unwrap();
    assert_eq!(empty, MAGIC);
}

#[test]
fn table_date_and_odd_table() {
    let members = vec![member(b"a.o", b"A")];
    let mut o = Options::new(Archiver::GnuAr, Dialect::Gnu);
    o.table_date = 1_795_682_044;
    let a = write(&members, Some(&[sym(b"ab", 0)]), &o).unwrap();
    // 4 + 4 + 3: padded to 12 with a NUL inside.
    assert_eq!(
        text(&a[8..68]),
        text(&hdr("/", "1795682044", "0", "0", "0", 12))
    );
    assert_eq!(&a[68 + 8..68 + 12], b"ab\0\0");
}

#[test]
fn thin() {
    let entries = [
        Entry {
            name: b"a.o",
            origin: None,
            fields: det(),
            size: 1992,
        },
        Entry {
            name: b"a.o",
            origin: None,
            fields: det(),
            size: 1992,
        },
        Entry {
            name: b"sub/a.o",
            origin: None,
            fields: det(),
            size: 7,
        },
        Entry {
            name: b"g1.a",
            origin: Some(298),
            fields: det(),
            size: 1992,
        },
        Entry {
            name: b"g1.a",
            origin: Some(2350),
            fields: det(),
            size: 3,
        },
    ];
    let symbols = [sym(b"f", 0), sym(b"f", 1), sym(b"g", 4)];
    let mut o = Options::new(Archiver::GnuAr, Dialect::Gnu);
    o.thin = true;
    let l = Layout::new(&entries, Some(&symbols), &o).unwrap();
    let long = b"a.o/\nsub/a.o/\ng1.a/\n";
    let first = 8 + 60 + (4 + 12 + 6) + 60 + long.len();
    assert_eq!(&l.head()[..8], THIN_MAGIC);
    let names: Vec<String> = (0..5).map(|i| text(&l.header(i)[..16])).collect();
    assert_eq!(
        names,
        [
            "/0              ",
            "/0              ",
            "/5              ",
            "/14:298         ",
            "/14:2350        "
        ]
    );
    assert_eq!(l.offset(1), first as u64 + 60);
    assert_eq!(l.size(), first as u64 + 5 * 60);
    assert!(l.head().ends_with(long));
    assert_eq!(&l.header(2)[48..58], b"7         ");
    // llvm-ar: every name, each time.
    o.archiver = Archiver::LlvmAr;
    let plain = &entries[..3];
    let l = Layout::new(plain, Some(&symbols[..2]), &o).unwrap();
    assert!(l.head().ends_with(b"a.o/\na.o/\nsub/a.o/\n\n"));
    // It writes no origins.
    assert_eq!(
        Layout::new(&entries, None, &o).unwrap_err(),
        WriteError::NotWritable
    );
    // Round trip through the reader.
    o.archiver = Archiver::GnuAr;
    let members: Vec<NewMember<'_>> = entries
        .iter()
        .map(|&entry| NewMember { entry, data: &[] })
        .collect();
    let bytes = write(&members, Some(&symbols), &o).unwrap();
    let archive = Archive::parse(&bytes).unwrap();
    let c = Contents::read(&archive).unwrap();
    assert!(c.options.thin);
    assert_eq!(c.members[3].entry.origin, Some(298));
    assert_eq!(c.members[3].entry.name, b"g1.a");
    assert_eq!(c.members[2].entry.size, 7);
    assert_eq!(c.write(Archiver::GnuAr).unwrap(), bytes);
}

/// Members whose sizes put later ones past 4 GiB (laid out, not written).
fn big(sizes: &[u64]) -> Vec<Entry<'static>> {
    sizes
        .iter()
        .map(|&size| Entry {
            name: b"m.o",
            origin: None,
            fields: det(),
            size,
        })
        .collect()
}

#[test]
fn sym64() {
    let entries = big(&[0xFFFF_0000, 0x2_0000, 16]);
    // GNU: only when a member with symbols is past 32 bits.
    let early = [sym(b"a", 0), sym(b"b", 1)];
    let gnu = Options::new(Archiver::GnuAr, Dialect::Gnu);
    let l = Layout::new(&entries, Some(&early), &gnu).unwrap();
    assert!(!l.is_sym64());
    let late = [sym(b"a", 0), sym(b"c", 2)];
    let l = Layout::new(&entries, Some(&late), &gnu).unwrap();
    assert!(l.is_sym64());
    // 8 + 16 + 4 = 28, padded to 32.
    assert_eq!(&l.head()[8..24], b"/SYM64/         ");
    assert_eq!(&l.head()[56..68], b"32        `\n");
    let body = &l.head()[68..100];
    assert_eq!(body[..8], 2u64.to_be_bytes());
    assert_eq!(body[8..16], l.offset(0).to_be_bytes());
    assert_eq!(body[16..24], l.offset(2).to_be_bytes());
    assert_eq!(&body[24..], b"a\0c\0\0\0\0\0");
    assert!(l.offset(2) > u64::from(u32::MAX));
    // llvm-ar: when the last member's header reaches 2^32, whatever
    // defines what; padded to 2.
    let llvm = Options::new(Archiver::LlvmAr, Dialect::Gnu);
    let l = Layout::new(&entries, Some(&early), &llvm).unwrap();
    assert!(l.is_sym64());
    assert_eq!(&l.head()[56..68], b"28        `\n");
    // ...and at a threshold of its environment's choosing.
    let mut low = llvm;
    low.sym64_threshold = 100;
    let small = big(&[10, 10]);
    assert!(
        Layout::new(&small, Some(&[sym(b"a", 0)]), &low)
            .unwrap()
            .is_sym64()
    );
    // Without a symbol table, nothing changes.
    assert!(!Layout::new(&entries, None, &llvm).unwrap().is_sym64());
}

#[test]
fn bsd() {
    let members = vec![
        member(b"a.o", b"AAAAAAAAA"),
        member(b"a_long_name_object.o", b"B"),
    ];
    let symbols = [sym(b"foo", 0), sym(b"bar", 1)];
    let o = Options::new(Archiver::LlvmAr, Dialect::Bsd);
    let a = write(&members, Some(&symbols), &o).unwrap();
    // The table: "#1/12" with "__.SYMDEF\0\0\0", then 4 + 16 + 4 + 8 = 32.
    let mut want = Vec::from(&MAGIC[..]);
    want.extend(hdr("#1/12", "0", "0", "0", "0", 12 + 32));
    want.extend_from_slice(b"__.SYMDEF\0\0\0");
    want.extend_from_slice(&16u32.to_le_bytes());
    let first = 8 + 60 + 12 + 32;
    // a.o: 60 + 3 = 63 from the first member, padded to 64.
    let second = first + 60 + 4 + 9 + 1;
    for (strx, at) in [(0u32, first as u32), (4, second as u32)] {
        want.extend_from_slice(&strx.to_le_bytes());
        want.extend_from_slice(&at.to_le_bytes());
    }
    want.extend_from_slice(&8u32.to_le_bytes());
    want.extend_from_slice(b"foo\0bar\0");
    want.extend(hdr("#1/4", "0", "0", "0", "644", 4 + 9));
    want.extend_from_slice(b"a.o\0AAAAAAAAA\n");
    // 74 + 60 + 20 = 154 from the first: padded to 160.
    want.extend(hdr("#1/26", "0", "0", "0", "644", 26 + 1));
    want.extend_from_slice(b"a_long_name_object.o\0\0\0\0\0\0B\n");
    assert_eq!(text(&a), text(&want));
    let c = Contents::read(&Archive::parse(&a).unwrap()).unwrap();
    assert_eq!(c.options.dialect, Dialect::Bsd);
    assert_eq!(c.members[1].entry.name, b"a_long_name_object.o");
    assert_eq!(c.write(Archiver::LlvmAr).unwrap(), a);

    // Darwin pads each member's data to 8 bytes.
    let d = write(
        &members,
        Some(&symbols),
        &Options::new(Archiver::LlvmAr, Dialect::Darwin),
    )
    .unwrap();
    let archive = Archive::parse(&d).unwrap();
    let files: Vec<_> = archive.files().collect();
    assert_eq!(files[0].data, b"AAAAAAAAA\n\n\n\n\n\n\n");
    assert_eq!(files[1].data, b"B\n\n\n\n\n\n\n");
    let c = Contents::read(&archive).unwrap();
    assert_eq!(c.options.dialect, Dialect::Darwin);
    assert_eq!(c.write(Archiver::LlvmAr).unwrap(), d);

    // GNU ar's BSD dialect isn't written, nor a thin BSD archive.
    assert_eq!(
        write(&members, None, &Options::new(Archiver::GnuAr, Dialect::Bsd)).unwrap_err(),
        WriteError::NotWritable
    );
}

#[test]
fn bad_input() {
    let members = vec![member(b"a\0.o", b"A")];
    let o = Options::new(Archiver::GnuAr, Dialect::Gnu);
    assert_eq!(
        write(&members, None, &o).unwrap_err(),
        WriteError::BadName { member: 0 }
    );
    let members = vec![member(b"a.o", b"A")];
    assert_eq!(
        write(&members, Some(&[sym(b"x", 1)]), &o).unwrap_err(),
        WriteError::NoSuchMember { symbol: 0 }
    );
    assert_eq!(
        write(&members, Some(&[sym(b"x\0", 0)]), &o).unwrap_err(),
        WriteError::BadSymbol { symbol: 0 }
    );
    let huge = big(&[10_000_000_000]);
    assert_eq!(
        Layout::new(&huge, None, &o).unwrap_err(),
        WriteError::MemberTooLarge { member: 0 }
    );
}

#[test]
fn layout_pieces() {
    let members = vec![member(b"a.o", b"AAA"), member(b"b.o", b"BBBBBBBB")];
    let o = Options::new(Archiver::GnuAr, Dialect::Gnu);
    let whole = write(&members, Some(&[sym(b"x", 1)]), &o).unwrap();
    let entries: Vec<Entry<'_>> = members.iter().map(|m| m.entry).collect();
    let l = Layout::new(&entries, Some(&[sym(b"x", 1)]), &o).unwrap();
    let mut out = vec![0u8; l.size() as usize];
    let copies = l.prepare_mut(&mut out, |i| members[i].data, 3);
    assert_eq!(copies.len(), 1 + 3);
    for (to, from) in copies {
        to.copy_from_slice(from);
    }
    assert_eq!(out, whole);
}
