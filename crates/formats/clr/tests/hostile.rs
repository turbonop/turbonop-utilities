//! Damaged assemblies and PDBs: the fixtures with bytes changed, cut short,
//! and stretched, read through every reader. Any result is fine but a
//! panic (or a hang).

#![allow(clippy::cast_possible_truncation)]

use turbonop_global_alloc as _;

mod walk;

use std::path::PathBuf;

fn testdata(name: &str) -> Vec<u8> {
    std::fs::read(
        PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../../../testdata/clr")
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
    for name in [
        "Sample.dll",
        "Sample.pdb",
        "Sample.Extra.netmodule",
        "Hello.exe",
    ] {
        let mut e = walk::Errors::default();
        walk::file(&testdata(name), &mut e);
        assert!(e.0.is_empty(), "{name}: {:?}", e.0);
    }
}

/// Where the metadata is, so that most mutations land in it.
fn metadata_range(data: &[u8]) -> (usize, usize) {
    if data.starts_with(b"BSJB") {
        return (0, data.len());
    }
    let cli = turbonop_clr::CliImage::parse_bytes(data).unwrap().unwrap();
    let md = cli.metadata().data();
    let start = md.as_ptr() as usize - data.as_ptr() as usize;
    (start, start + md.len())
}

fn mutate(data: &[u8], seed: u64) -> Vec<u8> {
    let mut rng = Rng(seed.wrapping_mul(0x9E37_79B9_7F4A_7C15) | 1);
    let (start, end) = metadata_range(data);
    let mut d = data.to_vec();
    for _ in 0..1 + rng.below(4) {
        // Mostly in the metadata; sometimes anywhere (headers, bodies,
        // resources).
        let at = if rng.below(4) == 0 {
            rng.below(d.len())
        } else {
            start + rng.below(end - start)
        };
        d[at] = match rng.below(5) {
            0 => 0,
            1 => 0xFF,
            2 => d[at] ^ (1 << rng.below(8)),
            3 => d[at].wrapping_add(1),
            _ => rng.next() as u8,
        };
    }
    d
}

#[test]
fn mutated_fixtures_never_panic() {
    for name in ["Sample.dll", "Sample.pdb", "Hello.exe"] {
        let data = testdata(name);
        let seeds: Vec<u64> = (0..3000).collect();
        turbonop_par::map(&seeds, 16, |&seed| {
            let d = mutate(&data, seed);
            let r = std::panic::catch_unwind(|| {
                let mut e = walk::Errors::default();
                walk::file(&d, &mut e);
            });
            assert!(r.is_ok(), "{name}, seed {seed}");
        });
    }
}

#[test]
fn truncated_metadata_never_panics() {
    // The PDB is all metadata; cut it everywhere.
    let data = testdata("Sample.pdb");
    let lens: Vec<usize> = (0..data.len()).step_by(7).collect();
    turbonop_par::map(&lens, 64, |&n| {
        let mut e = walk::Errors::default();
        walk::file(&data[..n], &mut e);
    });
}

#[test]
fn row_counts_and_heap_flags_scrambled() {
    // The tables stream's header: heap sizes, valid bits, and row counts,
    // each value tried.
    let data = testdata("Sample.dll");
    let cli = turbonop_clr::CliImage::parse_bytes(&data).unwrap().unwrap();
    let md = cli.metadata();
    let ts = md.tables_stream().unwrap();
    let base = md.data().as_ptr() as usize - data.as_ptr() as usize + ts.offset as usize;
    let mut cases = Vec::new();
    for at in [6usize, 8, 9, 10, 11, 12, 13, 14, 15] {
        for v in [0u8, 1, 2, 4, 7, 0x40, 0x80, 0xFF] {
            cases.push((base + at, v));
        }
    }
    // Every row count's bytes.
    for i in 0..ts.data.len().min(24 + 4 * 40) - 24 {
        for v in [0u8, 1, 0x7F, 0xFF] {
            cases.push((base + 24 + i, v));
        }
    }
    turbonop_par::map(&cases, 8, |&(at, v)| {
        let mut d = data.clone();
        d[at] = v;
        let mut e = walk::Errors::default();
        walk::file(&d, &mut e);
    });
}
