//! The fixtures Python's zipfile wrote (testdata/zip, by tools/zip/make.py),
//! read back; and damaged copies of them, which give errors, never a panic.

#![allow(clippy::cast_possible_truncation)]

use turbonop_global_alloc as _;

use std::path::PathBuf;

use turbonop_core::ErrorKind;
use turbonop_zip::{Archive, ExtraField, extra, flags, method};

fn testdata(name: &str) -> Vec<u8> {
    std::fs::read(
        PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../../../testdata/zip")
            .join(name),
    )
    .unwrap()
}

/// The text make.py's entries hold.
fn text() -> Vec<u8> {
    (0..200)
        .flat_map(|i| format!("line {i} of a text that deflates well\n").into_bytes())
        .collect()
}

fn names(a: &Archive<'_>) -> Vec<String> {
    a.entries().iter().map(|e| e.name().into_owned()).collect()
}

#[test]
fn stored() {
    let data = testdata("stored.zip");
    let a = Archive::parse(&data).unwrap();
    assert_eq!(
        names(&a),
        ["hello.txt", "dir/", "dir/empty", "dir/text.txt"]
    );
    for e in a.entries() {
        assert_eq!(e.method, method::STORED);
        assert_eq!(e.modified.to_string(), "2024-02-29 13:37:58");
        assert_eq!(e.host_system(), 3);
    }
    assert_eq!(
        &*a.read_by_name("hello.txt").unwrap().unwrap(),
        b"Hello, world!\n"
    );
    assert_eq!(
        &*a.read_by_name("dir/text.txt").unwrap().unwrap(),
        &text()[..]
    );
    assert!(a.read_by_name("dir/empty").unwrap().unwrap().is_empty());
    let dir = a.find("dir/").unwrap();
    assert!(dir.is_dir());
    assert_eq!(dir.unix_mode(), Some(0o40755));
    assert_eq!(a.find("hello.txt").unwrap().unix_mode(), Some(0o100_644));
    // The entries and the central directory cover the file.
    let mut at = 0;
    for e in a.entries() {
        let x = a.extent(e).unwrap();
        assert_eq!(x.local_header.start, at);
        at = x.whole().end;
    }
    assert_eq!(at, a.central_directory().start);
}

#[test]
fn deflated() {
    let data = testdata("deflated.zip");
    let a = Archive::parse(&data).unwrap();
    assert_eq!(a.comment(), b"the archive's comment");
    assert_eq!(names(&a), ["text.txt", "random.bin", "tiny", "stored.txt"]);
    let e = a.find("text.txt").unwrap();
    assert_eq!(e.method, method::DEFLATED);
    assert_eq!(e.comment(), "an entry's comment");
    assert!(e.compressed_size * 10 < e.uncompressed_size);
    assert_eq!(&*a.read(e).unwrap(), &text()[..]);
    let random: Vec<u8> = (0..3000u32)
        .map(|i| ((i * 7919 + 13) % 251) as u8)
        .collect();
    assert_eq!(
        &*a.read_by_name("random.bin").unwrap().unwrap(),
        &random[..]
    );
    assert_eq!(&*a.read_by_name("tiny").unwrap().unwrap(), b"x");
    let all = a.read_all();
    assert!(all.iter().all(Result::is_ok));
}

#[test]
fn data_descriptors() {
    for (name, zip64) in [("descriptor.zip", false), ("zip64-descriptor.zip", true)] {
        let data = testdata(name);
        let a = Archive::parse(&data).unwrap();
        let mut at = 0;
        for e in a.entries() {
            assert!(e.has_data_descriptor(), "{name}");
            let d = a.data_descriptor(e).unwrap().unwrap();
            // zipfile signs them, and widens them with a ZIP64 field.
            assert!(d.signed);
            assert_eq!(d.zip64, zip64, "{name}");
            assert_eq!(
                (d.crc32, d.compressed_size, d.uncompressed_size),
                (e.crc32, e.compressed_size, e.uncompressed_size)
            );
            let h = a.local_header(e).unwrap();
            assert_eq!(h.crc32, 0);
            let x = a.extent(e).unwrap();
            assert_eq!(x.local_header.start, at);
            at = x.whole().end;
            assert!(a.read(e).is_ok());
        }
        assert_eq!(at, a.central_directory().start, "{name}");
    }
    let data = testdata("descriptor.zip");
    let a = Archive::parse(&data).unwrap();
    assert_eq!(&*a.read_by_name("a.txt").unwrap().unwrap(), &text()[..]);
    assert_eq!(
        &*a.read_by_name("b.txt").unwrap().unwrap(),
        b"stored, with a descriptor\n"
    );
}

#[test]
fn zip64() {
    let data = testdata("zip64.zip");
    let a = Archive::parse(&data).unwrap();
    let end = a.end_of_central_directory();
    let z = end.zip64.unwrap();
    assert_eq!(z.total_entries, 2);
    assert_eq!(z.version_needed, 45);
    assert_eq!(z.total_disks, 1);
    assert_eq!(z.central_directory_offset, a.central_directory().start);
    for e in a.entries() {
        assert!(e.zip64, "{}", e.name());
        assert!(a.local_header(e).unwrap().has_zip64());
        let f: Vec<_> = e.extra_fields().collect::<Result<_, _>>().unwrap();
        assert!(matches!(f[0], ExtraField::Zip64(_)));
    }
    assert_eq!(a.entries()[1].local_header_offset, 571);
    assert_eq!(&*a.read_by_name("first.txt").unwrap().unwrap(), &text()[..]);
    assert_eq!(
        &*a.read_by_name("second.txt").unwrap().unwrap(),
        b"second\n"
    );
}

#[test]
fn unicode_names_and_extra_fields() {
    let data = testdata("unicode.zip");
    let a = Archive::parse(&data).unwrap();
    assert_eq!(
        names(&a),
        [
            "caf\u{e9}/na\u{ef}ve.txt",
            "\u{65e5}\u{672c}\u{8a9e}.txt",
            "plain.txt"
        ]
    );
    let e = &a.entries()[0];
    assert!(e.flags & flags::UTF8 != 0);
    // (zipfile sets the flag only for names that aren't ASCII.)
    assert!(a.entries()[2].flags & flags::UTF8 == 0);
    assert_eq!(e.unix_modified(), Some(1_709_213_878));
    let fields: Vec<_> = e.extra_fields().collect::<Result<_, _>>().unwrap();
    assert_eq!(
        fields,
        [
            ExtraField::ExtendedTimestamp {
                flags: 1,
                modified: Some(1_709_213_878),
                accessed: None,
                created: None
            },
            ExtraField::UnixOwner {
                uid: 1000,
                gid: 1000
            }
        ]
    );
    assert_eq!(fields[1].id(), extra::INFO_ZIP_UNIX);
    assert!(a.find("\u{65e5}\u{672c}\u{8a9e}.txt").is_some());
    assert!(a.read_all().iter().all(Result::is_ok));
}

#[test]
fn empty() {
    let data = testdata("empty.zip");
    let a = Archive::parse(&data).unwrap();
    assert!(a.is_empty());
    assert_eq!(a.central_directory(), 0..0);
}

#[test]
fn data_before_the_archive() {
    // Offsets from the archive's start.
    let data = testdata("prefixed.zip");
    let a = Archive::parse(&data).unwrap();
    assert_eq!(a.base(), 46);
    assert_eq!(
        &*a.read_by_name("inside.txt").unwrap().unwrap(),
        &text()[..]
    );
    // Offsets from the file's start.
    let data = testdata("appended.zip");
    let a = Archive::parse(&data).unwrap();
    assert_eq!(a.base(), 0);
    assert_eq!(a.entries()[0].local_header_offset, 64);
    assert_eq!(
        &*a.read_by_name("appended.txt").unwrap().unwrap(),
        &text()[..]
    );
}

#[test]
fn other_methods_are_named() {
    let data = testdata("methods.zip");
    let a = Archive::parse(&data).unwrap();
    let methods: Vec<_> = a
        .entries()
        .iter()
        .map(turbonop_zip::Entry::method_name)
        .collect();
    assert_eq!(methods, ["bzip2", "LZMA", "deflated"]);
    for e in &a.entries()[..2] {
        assert_eq!(a.read(e).unwrap_err().kind, ErrorKind::Unsupported);
        // Their data is there to size.
        assert_eq!(a.raw_data(e).unwrap().len() as u64, e.compressed_size);
    }
    assert_eq!(&*a.read_by_name("d.txt").unwrap().unwrap(), &text()[..]);
}

#[test]
fn encrypted_is_flagged() {
    let data = testdata("encrypted.zip");
    let a = Archive::parse(&data).unwrap();
    let e = &a.entries()[0];
    assert!(e.is_encrypted());
    let err = a.read(e).unwrap_err();
    assert_eq!(
        (err.kind, err.what),
        (ErrorKind::Unsupported, "encrypted entry")
    );
}

#[test]
fn split_archives_are_refused() {
    let data = testdata("split.zip");
    let err = Archive::parse(&data).unwrap_err();
    assert_eq!(err.kind, ErrorKind::Unsupported);
    assert_eq!(err.what, "multi-disk (split or spanned) archive");
}

/// xorshift64*: deterministic, so a failure reproduces.
struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 >> 12;
        self.0 ^= self.0 << 25;
        self.0 ^= self.0 >> 27;
        self.0.wrapping_mul(0x2545_F491_4F6C_DD1D)
    }

    fn below(&mut self, n: usize) -> usize {
        (self.next() % n as u64) as usize
    }
}

fn walk(data: &[u8]) {
    let Ok(a) = Archive::parse(data) else {
        return;
    };
    for e in a.entries() {
        let _ = (e.name(), e.comment(), e.unix_modified(), e.unix_mode());
        let _ = e.extra_fields().count();
        let _ = (a.extent(e), a.data_descriptor(e), a.local_header(e));
        if let Ok(h) = a.local_header(e) {
            let _ = h.extra_fields().count();
        }
        let _ = a.read(e);
    }
    let _ = a.find("text.txt");
}

const ALL: &[&str] = &[
    "stored.zip",
    "deflated.zip",
    "descriptor.zip",
    "zip64.zip",
    "zip64-descriptor.zip",
    "unicode.zip",
    "empty.zip",
    "prefixed.zip",
    "appended.zip",
    "methods.zip",
    "encrypted.zip",
    "split.zip",
];

#[test]
fn mutated_fixtures_never_panic() {
    for name in ALL {
        let data = testdata(name);
        let seeds: Vec<u64> = (0..4000).collect();
        turbonop_par::map(&seeds, 64, |&seed| {
            let mut rng = Rng(seed.wrapping_mul(0x9E37_79B9_7F4A_7C15) | 1);
            let mut d = data.clone();
            for _ in 0..1 + rng.below(4) {
                let at = rng.below(d.len());
                d[at] = match rng.below(5) {
                    0 => 0,
                    1 => 0xFF,
                    2 => d[at] ^ (1 << rng.below(8)),
                    3 => d[at].wrapping_add(1),
                    _ => rng.next() as u8,
                };
            }
            walk(&d);
        });
    }
}

#[test]
fn truncated_fixtures_never_panic() {
    for name in ALL {
        let data = testdata(name);
        let lens: Vec<usize> = (0..data.len()).collect();
        turbonop_par::map(&lens, 64, |&n| walk(&data[..n]));
        // And cut from the front.
        turbonop_par::map(&lens, 64, |&n| walk(&data[n..]));
    }
}
