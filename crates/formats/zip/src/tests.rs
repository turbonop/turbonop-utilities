//! Archives built here, a part at a time, and read back.

#![allow(clippy::cast_possible_truncation)]

use super::*;

/// How a test entry is written.
#[derive(Clone)]
struct Spec {
    name: Vec<u8>,
    data: Vec<u8>,
    method: u16,
    flags: u16,
    /// A data descriptor: (signed, ZIP64-wide).
    descriptor: Option<(bool, bool)>,
    /// ZIP64 extra fields with all-ones sizes and offset.
    zip64: bool,
    extra: Vec<u8>,
    comment: Vec<u8>,
    made_by: u16,
    external: u32,
}

fn spec(name: &str, data: &[u8]) -> Spec {
    Spec {
        name: name.as_bytes().to_vec(),
        data: data.to_vec(),
        method: method::STORED,
        flags: 0,
        descriptor: None,
        zip64: false,
        extra: Vec::new(),
        comment: Vec::new(),
        made_by: 20,
        external: 0,
    }
}

/// An archive of `specs`, after `prefix`; offsets from the archive's start
/// (as if `prefix` weren't there) unless `absolute`.
fn build(
    specs: &[Spec],
    prefix: &[u8],
    absolute: bool,
    zip64_end: bool,
    comment: &[u8],
) -> Vec<u8> {
    let mut out = prefix.to_vec();
    let rel = if absolute { 0 } else { prefix.len() as u64 };
    let mut central = Vec::new();
    for s in specs {
        let offset = out.len() as u64 - rel;
        let packed = if s.method == method::DEFLATED {
            miniz_oxide::deflate::compress_to_vec(&s.data, 6)
        } else {
            s.data.clone()
        };
        let crc = crc32(&s.data);
        let flags = s.flags
            | if s.descriptor.is_some() {
                flags::DATA_DESCRIPTOR
            } else {
                0
            };
        let mut local_extra = Vec::new();
        if s.zip64 {
            local_extra.extend_from_slice(&1u16.to_le_bytes());
            local_extra.extend_from_slice(&16u16.to_le_bytes());
            let (u, c) = if s.descriptor.is_some() {
                (0, 0)
            } else {
                (s.data.len() as u64, packed.len() as u64)
            };
            local_extra.extend_from_slice(&u.to_le_bytes());
            local_extra.extend_from_slice(&c.to_le_bytes());
        }
        local_extra.extend_from_slice(&s.extra);
        out.extend_from_slice(&LOCAL_HEADER_SIGNATURE.to_le_bytes());
        out.extend_from_slice(&20u16.to_le_bytes());
        out.extend_from_slice(&flags.to_le_bytes());
        out.extend_from_slice(&s.method.to_le_bytes());
        out.extend_from_slice(&[0x21, 0x6B, 0x58, 0x58]); // time, date
        let (lcrc, lc, lu) = match (s.descriptor.is_some(), s.zip64) {
            (true, _) => (0, 0, 0),
            (false, true) => (crc, u32::MAX, u32::MAX),
            (false, false) => (crc, packed.len() as u32, s.data.len() as u32),
        };
        let (lc, lu) = if s.zip64 {
            (u32::MAX, u32::MAX)
        } else {
            (lc, lu)
        };
        out.extend_from_slice(&lcrc.to_le_bytes());
        out.extend_from_slice(&lc.to_le_bytes());
        out.extend_from_slice(&lu.to_le_bytes());
        out.extend_from_slice(&(s.name.len() as u16).to_le_bytes());
        out.extend_from_slice(&(local_extra.len() as u16).to_le_bytes());
        out.extend_from_slice(&s.name);
        out.extend_from_slice(&local_extra);
        out.extend_from_slice(&packed);
        if let Some((signed, wide)) = s.descriptor {
            if signed {
                out.extend_from_slice(&DATA_DESCRIPTOR_SIGNATURE.to_le_bytes());
            }
            out.extend_from_slice(&crc.to_le_bytes());
            if wide {
                out.extend_from_slice(&(packed.len() as u64).to_le_bytes());
                out.extend_from_slice(&(s.data.len() as u64).to_le_bytes());
            } else {
                out.extend_from_slice(&(packed.len() as u32).to_le_bytes());
                out.extend_from_slice(&(s.data.len() as u32).to_le_bytes());
            }
        }
        let mut extra = Vec::new();
        if s.zip64 {
            extra.extend_from_slice(&1u16.to_le_bytes());
            extra.extend_from_slice(&24u16.to_le_bytes());
            extra.extend_from_slice(&(s.data.len() as u64).to_le_bytes());
            extra.extend_from_slice(&(packed.len() as u64).to_le_bytes());
            extra.extend_from_slice(&offset.to_le_bytes());
        }
        extra.extend_from_slice(&s.extra);
        central.extend_from_slice(&CENTRAL_HEADER_SIGNATURE.to_le_bytes());
        central.extend_from_slice(&s.made_by.to_le_bytes());
        central.extend_from_slice(&20u16.to_le_bytes());
        central.extend_from_slice(&flags.to_le_bytes());
        central.extend_from_slice(&s.method.to_le_bytes());
        central.extend_from_slice(&[0x21, 0x6B, 0x58, 0x58]);
        central.extend_from_slice(&crc.to_le_bytes());
        if s.zip64 {
            central.extend_from_slice(&u32::MAX.to_le_bytes());
            central.extend_from_slice(&u32::MAX.to_le_bytes());
        } else {
            central.extend_from_slice(&(packed.len() as u32).to_le_bytes());
            central.extend_from_slice(&(s.data.len() as u32).to_le_bytes());
        }
        central.extend_from_slice(&(s.name.len() as u16).to_le_bytes());
        central.extend_from_slice(&(extra.len() as u16).to_le_bytes());
        central.extend_from_slice(&(s.comment.len() as u16).to_le_bytes());
        central.extend_from_slice(&0u16.to_le_bytes()); // disk
        central.extend_from_slice(&0u16.to_le_bytes()); // internal
        central.extend_from_slice(&s.external.to_le_bytes());
        let o32 = if s.zip64 { u32::MAX } else { offset as u32 };
        central.extend_from_slice(&o32.to_le_bytes());
        central.extend_from_slice(&s.name);
        central.extend_from_slice(&extra);
        central.extend_from_slice(&s.comment);
    }
    let cd_offset = out.len() as u64 - rel;
    out.extend_from_slice(&central);
    let n = specs.len() as u64;
    if zip64_end {
        let record = out.len() as u64 - rel;
        out.extend_from_slice(&ZIP64_END_OF_CENTRAL_DIRECTORY_SIGNATURE.to_le_bytes());
        out.extend_from_slice(&44u64.to_le_bytes());
        out.extend_from_slice(&45u16.to_le_bytes());
        out.extend_from_slice(&45u16.to_le_bytes());
        out.extend_from_slice(&0u32.to_le_bytes());
        out.extend_from_slice(&0u32.to_le_bytes());
        out.extend_from_slice(&n.to_le_bytes());
        out.extend_from_slice(&n.to_le_bytes());
        out.extend_from_slice(&(central.len() as u64).to_le_bytes());
        out.extend_from_slice(&cd_offset.to_le_bytes());
        out.extend_from_slice(&ZIP64_LOCATOR_SIGNATURE.to_le_bytes());
        out.extend_from_slice(&0u32.to_le_bytes());
        out.extend_from_slice(&record.to_le_bytes());
        out.extend_from_slice(&1u32.to_le_bytes());
    }
    out.extend_from_slice(&END_OF_CENTRAL_DIRECTORY_SIGNATURE.to_le_bytes());
    out.extend_from_slice(&[0; 4]);
    let n16 = if zip64_end { u16::MAX } else { n as u16 };
    out.extend_from_slice(&n16.to_le_bytes());
    out.extend_from_slice(&n16.to_le_bytes());
    let (s32, o32) = if zip64_end {
        (u32::MAX, u32::MAX)
    } else {
        (central.len() as u32, cd_offset as u32)
    };
    out.extend_from_slice(&s32.to_le_bytes());
    out.extend_from_slice(&o32.to_le_bytes());
    out.extend_from_slice(&(comment.len() as u16).to_le_bytes());
    out.extend_from_slice(comment);
    out
}

fn simple(specs: &[Spec]) -> Vec<u8> {
    build(specs, &[], false, false, b"")
}

fn text(n: usize) -> Vec<u8> {
    (0..n).map(|i| b"abcdefgh\n"[i % 9]).collect()
}

#[test]
fn stored_and_deflated() {
    let mut d = spec("dir/deflated.txt", &text(5000));
    d.method = method::DEFLATED;
    let data = build(
        &[spec("stored.txt", b"hello"), d, spec("dir/", b"")],
        &[],
        false,
        false,
        b"a comment",
    );
    let a = Archive::parse(&data).unwrap();
    assert_eq!(a.len(), 3);
    assert_eq!(a.comment(), b"a comment");
    assert_eq!(a.base(), 0);
    let e = &a.entries()[0];
    assert_eq!(e.name(), "stored.txt");
    assert!(matches!(a.read(e).unwrap(), Cow::Borrowed(b"hello")));
    let e = a.find("dir/deflated.txt").unwrap();
    assert_eq!(e.index, 1);
    assert_eq!(e.method_name(), "deflated");
    assert!(e.compressed_size < 200);
    assert_eq!(&*a.read(e).unwrap(), &text(5000)[..]);
    assert!(a.find("dir/").unwrap().is_dir());
    assert!(!e.is_dir());
    assert!(a.find("nope").is_none());
    assert_eq!(e.modified.to_string(), "2024-02-24 13:25:02");
    // Every entry at once.
    let all = a.read_all();
    assert_eq!(all.len(), 3);
    assert!(all.iter().all(Result::is_ok));
    // The parts: header, data, nothing after.
    let x = a.extent(e).unwrap();
    assert_eq!(x.local_header.end - x.local_header.start, 30 + 16);
    assert_eq!(x.data.end - x.data.start, e.compressed_size);
    assert_eq!(x.descriptor, None);
    assert_eq!(x.whole(), x.local_header.start..x.data.end);
    assert_eq!(
        a.central_directory().end,
        a.end_of_central_directory().offset
    );
    assert!(is_zip(&data));
}

#[test]
fn data_descriptors_of_each_layout() {
    for (signed, wide, zip64) in [
        (true, false, false),
        (false, false, false),
        (true, true, true),
        (false, true, true),
    ] {
        let mut s = spec("a.txt", &text(300));
        s.method = method::DEFLATED;
        s.descriptor = Some((signed, wide));
        s.zip64 = zip64;
        let data = simple(&[s, spec("b", b"b")]);
        let a = Archive::parse(&data).unwrap();
        let e = &a.entries()[0];
        assert!(e.has_data_descriptor());
        assert_eq!(e.zip64, zip64);
        let d = a.data_descriptor(e).unwrap().unwrap();
        assert_eq!((d.signed, d.zip64), (signed, wide), "{signed} {wide}");
        assert_eq!(d.crc32, e.crc32);
        assert_eq!(d.uncompressed_size, 300);
        assert_eq!(&*a.read(e).unwrap(), &text(300)[..]);
        let h = a.local_header(e).unwrap();
        assert_eq!((h.crc32, h.has_zip64()), (0, zip64));
        // The next entry's header follows the descriptor.
        let x = a.extent(e).unwrap();
        let next = a.local_header(&a.entries()[1]).unwrap();
        assert_eq!(x.descriptor.unwrap().end, next.offset);
        assert_eq!(a.data_descriptor(&a.entries()[1]).unwrap(), None);
    }
}

#[test]
fn zip64_records_and_fields() {
    let mut s = spec("big.bin", &text(1000));
    s.zip64 = true;
    let data = build(&[s, spec("small", b"x")], &[], false, true, b"");
    let a = Archive::parse(&data).unwrap();
    let end = a.end_of_central_directory();
    let z = end.zip64.unwrap();
    assert_eq!(
        (z.total_entries, z.total_disks, z.version_needed),
        (2, 1, 45)
    );
    assert_eq!(end.entries(), 2);
    assert_eq!(end.total_entries, u16::MAX);
    let e = &a.entries()[0];
    assert!(e.zip64);
    assert_eq!((e.compressed_size, e.uncompressed_size), (1000, 1000));
    assert_eq!(e.local_header_offset, 0);
    let h = a.local_header(e).unwrap();
    assert_eq!(h.uncompressed_size, 1000);
    assert_eq!(&*a.read(e).unwrap(), &text(1000)[..]);
    assert!(!a.entries()[1].zip64);
}

#[test]
fn data_before_the_archive() {
    let prefix = b"MZ a self-extractor's program, or a JMOD's header".repeat(3);
    for zip64 in [false, true] {
        // Offsets from the archive's start: shifted by the prefix.
        let data = build(
            &[spec("a", b"one"), spec("b", b"two")],
            &prefix,
            false,
            zip64,
            b"",
        );
        let a = Archive::parse(&data).unwrap();
        assert_eq!(a.base(), prefix.len() as u64, "zip64 {zip64}");
        assert_eq!(&*a.read_by_name("b").unwrap().unwrap(), b"two");
        // Offsets from the file's start: none needed.
        let data = build(
            &[spec("a", b"one"), spec("b", b"two")],
            &prefix,
            true,
            zip64,
            b"",
        );
        let a = Archive::parse(&data).unwrap();
        assert_eq!(a.base(), 0);
        assert_eq!(&*a.read_by_name("a").unwrap().unwrap(), b"one");
    }
}

#[test]
fn trailing_bytes_and_comments_that_look_like_records() {
    // A comment holding an end record's signature.
    let mut comment = END_OF_CENTRAL_DIRECTORY_SIGNATURE.to_le_bytes().to_vec();
    comment.extend_from_slice(&[0; 30]);
    let data = build(&[spec("a", b"1")], &[], false, false, &comment);
    let a = Archive::parse(&data).unwrap();
    assert_eq!(a.comment(), &comment[..]);
    // Bytes after the archive.
    let mut data = simple(&[spec("a", b"1")]);
    data.extend_from_slice(b"trailing");
    let a = Archive::parse(&data).unwrap();
    assert_eq!(a.len(), 1);
}

#[test]
fn empty_archive() {
    let data = simple(&[]);
    assert_eq!(data.len(), 22);
    let a = Archive::parse(&data).unwrap();
    assert!(a.is_empty());
    assert!(is_zip(&data));
    assert!(a.find("x").is_none());
    assert!(!is_zip(b"PK"));
    assert!(Archive::parse(b"not a zip at all, not at all......").is_err());
}

#[test]
fn names_and_attributes() {
    let mut utf8 = spec("caf\u{e9}.txt", b"");
    utf8.flags = flags::UTF8;
    let mut cp437 = spec("", b"");
    cp437.name = b"caf\x82.txt".to_vec();
    // Info-ZIP's Unicode path (its CRC of the raw name).
    let mut unicode = spec("", b"");
    unicode.name = b"x?.txt".to_vec();
    let mut f = vec![1];
    f.extend_from_slice(&crc32(b"x?.txt").to_le_bytes());
    f.extend_from_slice("x\u{3bb}.txt".as_bytes());
    unicode.extra = [
        &extra::UNICODE_PATH.to_le_bytes()[..],
        &(f.len() as u16).to_le_bytes(),
        &f,
    ]
    .concat();
    // One whose CRC doesn't match: ignored.
    let mut stale = unicode.clone();
    stale.name = b"y?.txt".to_vec();
    let mut unix = spec("run.sh", b"#!/bin/sh\n");
    unix.made_by = (3 << 8) | 30;
    unix.external = 0o100_755 << 16;
    unix.extra = [&[0x55, 0x54, 5, 0, 1][..], &1_700_000_000i32.to_le_bytes()].concat();
    unix.comment = b"a script".to_vec();
    let mut dos_dir = spec("FOLDER", b"");
    dos_dir.external = 0x10;
    let data = simple(&[utf8, cp437, unicode, stale, unix, dos_dir]);
    let a = Archive::parse(&data).unwrap();
    let names: Vec<_> = a.entries().iter().map(Entry::name).collect();
    assert_eq!(
        names,
        [
            "caf\u{e9}.txt",
            "caf\u{e9}.txt",
            "x\u{3bb}.txt",
            "y?.txt",
            "run.sh",
            "FOLDER"
        ]
    );
    let e = a.find("run.sh").unwrap();
    assert_eq!(e.host_system(), 3);
    assert_eq!(host_system_name(e.host_system()), "Unix");
    assert_eq!(e.unix_mode(), Some(0o100_755));
    assert_eq!(e.unix_modified(), Some(1_700_000_000));
    assert_eq!(e.comment(), "a script");
    assert!(a.find("FOLDER").unwrap().is_dir());
    assert_eq!(a.find("FOLDER").unwrap().unix_mode(), None);
    assert_eq!(a.find("x\u{3bb}.txt").unwrap().index, 2);
}

#[test]
fn encrypted_and_unsupported_methods_are_named_not_read() {
    let mut enc = spec("secret", b"ciphertext");
    enc.flags = flags::ENCRYPTED;
    let mut bz = spec("b.bz2", b"BZh9");
    bz.method = method::BZIP2;
    let mut aes = spec("aes", b"xx");
    aes.method = method::AES;
    let data = simple(&[enc, bz, aes]);
    let a = Archive::parse(&data).unwrap();
    let e = &a.entries()[0];
    assert!(e.is_encrypted());
    let err = a.read(e).unwrap_err();
    assert_eq!(
        (err.kind, err.what),
        (ErrorKind::Unsupported, "encrypted entry")
    );
    assert_eq!(a.raw_data(e).unwrap(), b"ciphertext");
    let e = &a.entries()[1];
    assert_eq!(e.method_name(), "bzip2");
    assert_eq!(a.read(e).unwrap_err().kind, ErrorKind::Unsupported);
    assert!(a.entries()[2].is_encrypted());
    assert_eq!(method_name(3), "reduced (factor 2)");
    assert_eq!(method_name(200), "method 200");
    assert_eq!(host_system_name(99), "host 99");
}

#[test]
fn multi_disk_archives_are_refused() {
    let data = simple(&[spec("a", b"1")]);
    let eocd = data.len() - 22;
    for (at, v) in [(4, 1u16), (6, 1), (8, 0)] {
        let mut d = data.clone();
        d[eocd + at..eocd + at + 2].copy_from_slice(&v.to_le_bytes());
        let e = Archive::parse(&d).unwrap_err();
        assert_eq!(e.kind, ErrorKind::Unsupported, "{at}");
        assert_eq!(e.what, "multi-disk (split or spanned) archive");
    }
    // ZIP64's disk count.
    let data = build(&[spec("a", b"1")], &[], false, true, b"");
    let loc = data.len() - 22 - 20;
    let mut d = data.clone();
    d[loc + 16..loc + 20].copy_from_slice(&3u32.to_le_bytes());
    assert_eq!(Archive::parse(&d).unwrap_err().kind, ErrorKind::Unsupported);
}

#[test]
fn damage_is_reported() {
    let mut s = spec("a.txt", &text(400));
    s.method = method::DEFLATED;
    let data = simple(&[s]);
    let a = Archive::parse(&data).unwrap();
    let e = a.entries()[0];
    // A wrong CRC.
    let mut bad = e;
    bad.crc32 ^= 1;
    assert_eq!(a.read(&bad).unwrap_err().what, "CRC-32 mismatch");
    // Sizes that disagree with the stream.
    let mut bad = e;
    bad.uncompressed_size -= 1;
    assert_eq!(
        a.read(&bad).unwrap_err().what,
        "deflated data longer than its size"
    );
    let mut bad = e;
    bad.uncompressed_size += 1;
    assert_eq!(
        a.read(&bad).unwrap_err().what,
        "deflated data shorter than its size"
    );
    let mut bad = e;
    bad.uncompressed_size = 1 << 40;
    assert_eq!(
        a.read(&bad).unwrap_err().what,
        "uncompressed size beyond deflate's ratio"
    );
    let mut bad = e;
    bad.compressed_size = 10;
    assert!(a.read(&bad).is_err());
    let mut bad = e;
    bad.local_header_offset = 1;
    assert_eq!(a.read(&bad).unwrap_err().kind, ErrorKind::BadMagic);
    let mut bad = e;
    bad.local_header_offset = u64::MAX;
    assert!(a.read(&bad).is_err());
    // A central directory that isn't where it's recorded.
    let eocd = data.len() - 22;
    let mut d = data.clone();
    d[eocd + 16] ^= 0x40;
    d[eocd + 12] ^= 0x01;
    assert!(Archive::parse(&d).is_err());
    // A count that doesn't match.
    let mut d = data.clone();
    d[eocd + 8] = 2;
    d[eocd + 10] = 2;
    assert_eq!(
        Archive::parse(&d).unwrap_err().what,
        "central directory entry count"
    );
}

#[test]
fn counts_wrapped_past_65535_are_accepted() {
    let specs: Vec<Spec> = (0..65_537).map(|i| spec(&format!("{i}"), b"")).collect();
    let data = simple(&specs);
    let a = Archive::parse(&data).unwrap();
    assert_eq!(a.len(), 65_537);
    assert_eq!(a.end_of_central_directory().total_entries, 1);
    assert_eq!(a.find("65536").unwrap().index, 65_536);
}

#[test]
fn every_truncation_and_byte_flip_is_handled() {
    let mut d = spec("d.txt", &text(700));
    d.method = method::DEFLATED;
    d.descriptor = Some((true, false));
    let mut z = spec("z", &text(50));
    z.zip64 = true;
    let data = build(&[spec("a", b"hello"), d, z], b"PFX", false, true, b"c");
    let walk = |d: &[u8]| {
        if let Ok(a) = Archive::parse(d) {
            for e in a.entries() {
                let _ = (e.name(), e.comment(), e.unix_modified(), e.is_dir());
                let _ = e.extra_fields().count();
                let _ = (a.read(e), a.extent(e), a.data_descriptor(e));
            }
            let _ = a.find("d.txt");
        }
    };
    for n in 0..data.len() {
        walk(&data[..n]);
    }
    for at in 0..data.len() {
        for v in [0u8, 0xFF, data[at] ^ 0x80] {
            let mut d = data.clone();
            d[at] = v;
            walk(&d);
        }
    }
}
