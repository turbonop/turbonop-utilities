//! Images loaded at addresses, and address spaces over them.

use turbonop_image::Image;
use turbonop_target::{AddressSpace, LoadedImage, Memory, Provenance};

use crate::support::testdata;

/// Memory with some ranges captured.
struct Captured(Vec<(u64, Vec<u8>)>);

impl Memory for Captured {
    fn read_at(&self, address: u64, buf: &mut [u8]) -> usize {
        for (start, bytes) in &self.0 {
            if address >= *start && address - start < bytes.len() as u64 {
                let from = usize::try_from(address - start).unwrap();
                let n = (bytes.len() - from).min(buf.len());
                buf[..n].copy_from_slice(&bytes[from..from + n]);
                return n;
            }
        }
        0
    }

    fn next_available(&self, address: u64) -> Option<u64> {
        self.0.iter().map(|r| r.0).filter(|&s| s > address).min()
    }

    fn provenance(&self) -> Provenance {
        Provenance::Dump
    }
}

/// Every slot the image says the loader writes, read through a loaded
/// image: a known target as the loader wrote it, an unknown one not at
/// all.
fn check_slots(image: &Image<'_>, loaded: &LoadedImage<'_>) -> (usize, usize) {
    let (mut known, mut unknown) = (0, 0);
    let bias = loaded.bias();
    let le = image.endian() == turbonop_core::Endian::Little;
    for s in image.pointer_slots() {
        let at = s.addr.wrapping_add(bias);
        let mut buf = vec![0u8; usize::from(s.width)];
        let n = loaded.read_at(at, &mut buf);
        match s.target {
            Some(target) if n == buf.len() => {
                let mut v = [0u8; 8];
                if le {
                    v[..buf.len()].copy_from_slice(&buf);
                } else {
                    v[8 - buf.len()..].copy_from_slice(&buf);
                }
                let value = if le {
                    u64::from_le_bytes(v)
                } else {
                    u64::from_be_bytes(v)
                };
                let want = target.wrapping_add(bias);
                let want = if s.width == 4 {
                    want & 0xFFFF_FFFF
                } else {
                    want
                };
                assert_eq!(value, want, "slot {:#x}", s.addr);
                known += 1;
            }
            // (A slot in memory the file doesn't back.)
            Some(_) => {}
            None => {
                assert_eq!(n, 0, "unknown slot {:#x} read", s.addr);
                unknown += 1;
            }
        }
    }
    (known, unknown)
}

#[test]
fn pe_relocated() {
    let data = testdata("init/initdll.dll");
    let image = Image::parse(&data).unwrap();
    let preferred = image.image_base();
    // Loaded elsewhere than it prefers: its base relocations apply.
    let base = preferred + 0x1234_0000;
    let loaded = LoadedImage::new(&image, base).unwrap();
    assert_eq!(loaded.base(), base);
    assert_eq!(loaded.bias(), base);
    let mut mz = [0u8; 2];
    assert_eq!(loaded.read_at(base, &mut mz), 2);
    assert_eq!(&mz, b"MZ");
    let (known, unknown) = check_slots(&image, &loaded);
    assert!(known > 0, "no relocations checked");
    // (Its import address table's slots.)
    assert!(unknown > 0, "no imports checked");
    // Past the image: nothing.
    assert_eq!(loaded.read_at(base + loaded.size(), &mut mz), 0);
    assert_eq!(loaded.read_at(base - 1, &mut mz), 0);
}

#[test]
fn elf_relocated() {
    let data = testdata("init/libinit-x86_64-linux-gnu.so");
    let image = Image::parse(&data).unwrap();
    let base = 0x7F12_3400_0000;
    let loaded = LoadedImage::new(&image, base).unwrap();
    let mut magic = [0u8; 4];
    assert_eq!(loaded.read_at(base, &mut magic), 4);
    assert_eq!(&magic, b"\x7fELF");
    let (known, _) = check_slots(&image, &loaded);
    assert!(known > 0);
}

#[test]
fn macho_relocated() {
    for name in ["macho/pointers-arm64.dylib", "macho/pointers-x86_64.dylib"] {
        let data = testdata(name);
        let image = Image::parse(&data).unwrap();
        let base = 0x1_2345_0000;
        let loaded = LoadedImage::new(&image, base).unwrap();
        let mut magic = [0u8; 4];
        assert_eq!(loaded.read_at(base, &mut magic), 4, "{name}");
        assert_eq!(u32::from_le_bytes(magic), 0xFEED_FACF, "{name}");
        let (known, _) = check_slots(&image, &loaded);
        assert!(known > 0, "{name}");
    }
}

#[test]
fn dump_bytes_win_over_the_image() {
    let data = testdata("init/initdll.dll");
    let image = Image::parse(&data).unwrap();
    let base = 0x1800_0000;
    let loaded = LoadedImage::new(&image, base).unwrap();
    // The dump captured 16 bytes 0x100 in, and memory past the image.
    let memory = Captured(vec![
        (base + 0x100, vec![0xEE; 16]),
        (0x5000_0000, vec![7; 8]),
    ]);
    let space = AddressSpace::new(&memory, vec![loaded]);
    assert!(space.image_at(base + 0x10).is_some());
    assert!(space.image_at(0x5000_0000).is_none());
    // Entirely from the dump.
    let mut buf = [0u8; 16];
    assert_eq!(space.read(base + 0x100, &mut buf), Some(Provenance::Dump));
    assert_eq!(buf, [0xEE; 16]);
    // From the image, then the dump: the image's is the less certain.
    let mut buf = [0u8; 0x20];
    assert_eq!(space.read(base + 0xF0, &mut buf), Some(Provenance::Image));
    assert_eq!(&buf[..0x10], &data[0xF0..0x100]);
    assert_eq!(&buf[0x10..], &[0xEE; 16]);
    // Nothing there.
    assert_eq!(space.read(0x4000_0000, &mut buf), None);
    // The runs.
    let runs = space.runs(base + 0xF0, 0x30);
    let summary: Vec<_> = runs
        .iter()
        .map(|r| (r.start - base, r.len, r.provenance))
        .collect();
    assert_eq!(
        summary,
        [
            (0xF0, 0x10, Provenance::Image),
            (0x100, 0x10, Provenance::Dump),
            (0x110, 0x10, Provenance::Image)
        ]
    );
    // As the unwinders read it.
    let mut w = [0u8; 8];
    assert!(turbonop_unwind::Memory::read(&space, 0x5000_0000, &mut w));
    assert!(!turbonop_unwind::Memory::read(&space, 0x5000_0004, &mut w));
}
