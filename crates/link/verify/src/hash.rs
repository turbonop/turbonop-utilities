//! 128-bit content hashes: two lanes of a multiply-fold hash (as
//! `turbonop-hash`'s), computed in one pass, each lane with its own keys
//! drawn from the process's random seed. Equal fingerprints are taken as
//! equal canonical forms; for inputs that weren't made knowing the
//! process's keys, two different forms collide with probability about
//! 2^-128 per pair. The keys aren't the folding engine's (whose hashes are
//! fixed, for names that must be the same every run), so a collision the
//! engine meets is not one here.

use std::hash::BuildHasher;
use std::sync::OnceLock;

/// The 128-bit product of `a` and `b`, its halves xored.
#[inline]
fn fold(a: u64, b: u64) -> u64 {
    let p = u128::from(a) * u128::from(b);
    (p as u64) ^ ((p >> 64) as u64)
}

/// Each lane's keys: (start, word key, pair key, finish key).
fn keys() -> &'static [[u64; 4]; 2] {
    static KEYS: OnceLock<[[u64; 4]; 2]> = OnceLock::new();
    KEYS.get_or_init(|| {
        let state = turbonop_hash::State::default();
        let mut k = [[0u64; 4]; 2];
        for (lane, keys) in k.iter_mut().enumerate() {
            for (i, key) in keys.iter_mut().enumerate() {
                // Odd, so no key multiplies a lane to zero.
                *key = state.hash_one((lane * 4 + i) as u64 ^ 0x5A5A_A5A5_0F0F_F0F0) | 1;
            }
        }
        k
    })
}

pub(crate) struct H128 {
    a: u64,
    b: u64,
    k: &'static [[u64; 4]; 2],
}

impl H128 {
    pub(crate) fn new() -> Self {
        let k = keys();
        Self {
            a: k[0][0],
            b: k[1][0],
            k,
        }
    }

    #[inline]
    fn word(&mut self, w: u64) {
        self.a = fold(self.a ^ w, self.k[0][1]);
        self.b = fold(self.b ^ w, self.k[1][1]);
    }

    pub(crate) fn bytes(&mut self, bytes: &[u8]) {
        let (words, rest) = bytes.as_chunks::<8>();
        let (pairs, odd) = words.as_chunks::<2>();
        let (mut a, mut b) = (self.a, self.b);
        let (ka, kb) = (self.k[0][2], self.k[1][2]);
        for [x, y] in pairs {
            let (x, y) = (u64::from_le_bytes(*x), u64::from_le_bytes(*y));
            a = fold(a ^ x, y ^ ka);
            b = fold(b ^ x, y ^ kb);
        }
        self.a = a;
        self.b = b;
        for w in odd {
            self.word(u64::from_le_bytes(*w));
        }
        if !rest.is_empty() {
            let mut last = [0u8; 8];
            last[..rest.len()].copy_from_slice(rest);
            self.word(u64::from_le_bytes(last));
        }
        // The length, so that zeros at the end, and where one string ends
        // and the next begins, tell forms apart.
        self.word(bytes.len() as u64);
    }

    #[inline]
    pub(crate) fn u64(&mut self, v: u64) {
        self.word(v);
    }

    pub(crate) fn u128(&mut self, v: u128) {
        self.word(v as u64);
        self.word((v >> 64) as u64);
    }

    pub(crate) fn finish(&self) -> u128 {
        let a = fold(self.a, self.k[0][3]);
        let b = fold(self.b, self.k[1][3]);
        (u128::from(a) << 64) | u128::from(b)
    }
}

#[cfg(test)]
mod tests {
    use super::H128;

    fn h(b: &[u8]) -> u128 {
        let mut x = H128::new();
        x.bytes(b);
        x.finish()
    }

    #[test]
    fn contents_and_lengths_apart() {
        assert_ne!(h(b"a"), h(b"a\0"));
        assert_ne!(h(b""), h(b"\0"));
        assert_ne!(h(b"abcdefgh"), h(b"abcdefgi"));
        let long: Vec<u8> = (0..1000u32).flat_map(u32::to_le_bytes).collect();
        let mut other = long.clone();
        other[517] ^= 1;
        assert_ne!(h(&long), h(&other));
        assert_eq!(h(&long), h(&long.clone()));
        // Where one string ends and the next begins.
        let two = |a: &[u8], b: &[u8]| {
            let mut x = H128::new();
            x.bytes(a);
            x.bytes(b);
            x.finish()
        };
        assert_ne!(two(b"ab", b"c"), two(b"a", b"bc"));
        // The lanes differ, so the halves aren't one 64-bit hash twice.
        let v = h(b"turbolib");
        assert_ne!(v as u64, (v >> 64) as u64);
    }
}
