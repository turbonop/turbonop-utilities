//! A fast keyed hash for hash maps of what hostile files hold (names,
//! addresses): several times std's SipHash on long names, and keyed by a
//! random seed per process, so that inputs can't be made to collide
//! without knowing it (each step a folded multiply of the input with the
//! state, which is not linear: collisions for one seed are not
//! collisions for another). It is not a cryptographic hash.

use std::hash::{BuildHasher, Hasher};

/// A `HashMap` with [`State`].
pub type Map<K, V> = std::collections::HashMap<K, V, State>;
/// A `HashSet` with [`State`].
pub type Set<K> = std::collections::HashSet<K, State>;

/// A map with room for `n` entries.
pub fn map_with_capacity<K, V>(n: usize) -> Map<K, V> {
    Map::with_capacity_and_hasher(n, State::default())
}

/// A set with room for `n` entries.
pub fn set_with_capacity<K>(n: usize) -> Set<K> {
    Set::with_capacity_and_hasher(n, State::default())
}

/// The process's seed, from std's random keys.
fn seed() -> [u64; 2] {
    static SEED: std::sync::OnceLock<[u64; 2]> = std::sync::OnceLock::new();
    *SEED.get_or_init(|| {
        let s = std::collections::hash_map::RandomState::new();
        [
            s.hash_one(0x9E37_79B9_7F4A_7C15u64),
            s.hash_one(0xD6E8_FEB8_6659_FD93u64) | 1,
        ]
    })
}

/// The 128-bit product of `a` and `b`, its halves xored.
#[inline]
fn fold(a: u64, b: u64) -> u64 {
    let p = u128::from(a) * u128::from(b);
    let lo = u64::try_from(p & u128::from(u64::MAX)).unwrap_or(0);
    let hi = u64::try_from(p >> 64).unwrap_or(0);
    lo ^ hi
}

/// Builds [`Hash`]ers keyed by the process's seed.
#[derive(Clone, Copy, Debug)]
pub struct State {
    seed: [u64; 2],
}

impl Default for State {
    fn default() -> Self {
        Self { seed: seed() }
    }
}

impl BuildHasher for State {
    type Hasher = Hash;
    #[inline]
    fn build_hasher(&self) -> Hash {
        Hash {
            state: self.seed[0],
            key: self.seed[1],
        }
    }
}

/// The hasher.
#[derive(Clone, Debug)]
pub struct Hash {
    state: u64,
    key: u64,
}

const K1: u64 = 0x243F_6A88_85A3_08D3;
const K2: u64 = 0x1319_8A2E_0370_7344;

impl Hash {
    #[inline]
    fn word(&mut self, w: u64) {
        self.state = fold(self.state ^ w, self.key ^ K1);
    }
}

impl Hasher for Hash {
    #[inline]
    fn write(&mut self, bytes: &[u8]) {
        let (words, rest) = bytes.as_chunks::<8>();
        // Two words a step (16 bytes), each folded with the state.
        let (pairs, odd) = words.as_chunks::<2>();
        for [a, b] in pairs {
            let (a, b) = (u64::from_le_bytes(*a), u64::from_le_bytes(*b));
            self.state = fold(self.state ^ a, b ^ self.key ^ K2);
        }
        for w in odd {
            self.word(u64::from_le_bytes(*w));
        }
        if !rest.is_empty() {
            let mut last = [0u8; 8];
            last[..rest.len()].copy_from_slice(rest);
            self.word(u64::from_le_bytes(last));
        }
        // (The length, so that zeros at the end tell lengths apart.)
        self.word(bytes.len() as u64);
    }

    #[inline]
    fn write_u8(&mut self, i: u8) {
        self.word(u64::from(i));
    }

    #[inline]
    fn write_u16(&mut self, i: u16) {
        self.word(u64::from(i));
    }

    #[inline]
    fn write_u32(&mut self, i: u32) {
        self.word(u64::from(i));
    }

    #[inline]
    fn write_u64(&mut self, i: u64) {
        self.word(i);
    }

    #[inline]
    fn write_usize(&mut self, i: usize) {
        self.word(i as u64);
    }

    #[inline]
    fn finish(&self) -> u64 {
        fold(self.state, self.key ^ K1 ^ K2)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn maps_and_spread() {
        let mut m: Map<String, usize> = Map::default();
        for i in 0..100_000 {
            m.insert(format!("??_C@_0BC@{i:08}@name"), i);
        }
        for i in (0..100_000).step_by(997) {
            assert_eq!(m[&format!("??_C@_0BC@{i:08}@name")], i);
        }
        // Lengths and contents apart.
        let h = |b: &[u8]| State::default().hash_one(b);
        assert_ne!(h(b"a"), h(b"a\0"));
        assert_ne!(h(b""), h(b"\0"));
        assert_ne!(h(b"abcdefgh"), h(b"abcdefgi"));
        // The low bits (a table's bucket) of sequential addresses spread.
        let mut buckets = [0u32; 64];
        for a in 0..64_000u64 {
            buckets[(State::default().hash_one(a * 16) & 63) as usize] += 1;
        }
        assert!(
            buckets.iter().all(|&n| (800..1200).contains(&n)),
            "{buckets:?}"
        );
        let s: Set<u64> = (0..1000).collect();
        assert_eq!(s.len(), 1000);
    }
}

// (Tests allocate with mimalloc, as the programs do.)
#[cfg(all(test, not(target_family = "wasm")))]
#[global_allocator]
static TEST_ALLOCATOR: mimalloc::MiMalloc = mimalloc::MiMalloc;
