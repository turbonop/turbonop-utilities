//! Damaged DEX files: the fixtures with bytes changed, header fields
//! scrambled, cut short, and stretched, read through every reader. Any
//! result is fine but a panic (or a hang).

#![allow(clippy::cast_possible_truncation)]

use turbonop_global_alloc as _;

mod walk;

use std::path::PathBuf;

const FIXTURES: [&str; 8] = [
    "Sample-035.dex",
    "Sample-037.dex",
    "Sample-038.dex",
    "Sample-039.dex",
    "Sample-041.dex",
    "Handmade-039.dex",
    "Handmade-040.dex",
    "Handmade-container.dex",
];

fn testdata(name: &str) -> Vec<u8> {
    std::fs::read(
        PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../../../testdata/dex")
            .join(name),
    )
    .unwrap()
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

/// The good files read with no errors at all.
#[test]
fn fixtures_read_without_errors() {
    for name in FIXTURES {
        let mut e = walk::Errors::default();
        let n = walk::file(&testdata(name), &mut e);
        assert!(e.0.is_empty(), "{name}: {:?}", e.0);
        assert_eq!(n, if name.contains("container") { 2 } else { 1 }, "{name}");
    }
}

fn mutate(data: &[u8], seed: u64) -> Vec<u8> {
    let mut rng = Rng(seed.wrapping_mul(0x9E37_79B9_7F4A_7C15) | 1);
    let mut d = data.to_vec();
    for _ in 0..1 + rng.below(4) {
        let at = rng.below(d.len());
        d[at] = match rng.below(6) {
            0 => 0,
            1 => 0xFF,
            2 => d[at] ^ (1 << rng.below(8)),
            3 => d[at].wrapping_add(1),
            4 => 0x80,
            _ => rng.next() as u8,
        };
    }
    d
}

/// A header field (after the magic, checksum and signature) set to
/// something hostile: huge, zero, past the end, one less or more.
fn scramble_header(data: &[u8], seed: u64) -> Vec<u8> {
    let mut rng = Rng(seed.wrapping_mul(0xD1B5_4A32_D192_ED03) | 1);
    let mut d = data.to_vec();
    let fields = (0x70 - 0x20) / 4;
    let at = 0x20 + 4 * rng.below(fields);
    let old = u32::from_le_bytes(d[at..at + 4].try_into().unwrap());
    let len = d.len() as u32;
    let v = match rng.below(7) {
        0 => 0,
        1 => u32::MAX,
        2 => len,
        3 => len.wrapping_sub(1),
        4 => old.wrapping_add(1),
        5 => old.wrapping_sub(1),
        _ => old.wrapping_mul(16),
    };
    d[at..at + 4].copy_from_slice(&v.to_le_bytes());
    d
}

#[test]
fn mutated_files_never_panic() {
    let seeds: Vec<u64> = (0..3000).collect();
    for name in FIXTURES {
        let data = testdata(name);
        turbonop_par::map(&seeds, 16, |&s| {
            let mut e = walk::Errors::default();
            walk::file(&mutate(&data, s), &mut e);
        });
    }
}

#[test]
fn scrambled_headers_never_panic() {
    let seeds: Vec<u64> = (0..1000).collect();
    for name in FIXTURES {
        let data = testdata(name);
        turbonop_par::map(&seeds, 16, |&s| {
            let mut e = walk::Errors::default();
            walk::file(&scramble_header(&data, s), &mut e);
        });
    }
}

#[test]
fn truncated_and_stretched_files_never_panic() {
    for name in FIXTURES {
        let data = testdata(name);
        let cuts: Vec<usize> = (0..data.len()).step_by(7).chain([data.len() - 1]).collect();
        turbonop_par::map(&cuts, 16, |&n| {
            let mut e = walk::Errors::default();
            walk::file(&data[..n], &mut e);
            assert!(
                n == data.len() || !e.0.is_empty() || n >= data.len() - 3,
                "{name} cut at {n}"
            );
        });
        let mut long = data.clone();
        long.extend_from_slice(&[0xFF; 64]);
        let mut e = walk::Errors::default();
        walk::file(&long, &mut e);
    }
}
