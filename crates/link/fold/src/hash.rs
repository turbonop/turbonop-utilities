//! 128-bit content hashes, for telling records and fold units apart.
//!
//! Two lanes of a multiply-fold hash with fixed keys, computed in one pass
//! (the lanes' chains are independent, so the processor runs them side by
//! side, at about the cost of one). The keys are fixed so that the same
//! content hashes the same in every run: names made from these hashes
//! (synthesized members' anchors) must be the same every time the same
//! library is made.
//!
//! Equal hashes are taken as equal content: with 128 bits, a collision
//! among even billions of units is negligible.

use std::hash::BuildHasher;

/// The 128-bit product of `a` and `b`, its halves xored.
#[inline]
fn fold(a: u64, b: u64) -> u64 {
    let p = u128::from(a) * u128::from(b);
    (p as u64) ^ ((p >> 64) as u64)
}

const KEY_A: u64 = 0x9E37_79B9_7F4A_7C15;
const KEY_B: u64 = 0xD6E8_FEB8_6659_FD93;
const K1: u64 = 0x243F_6A88_85A3_08D3;
const K2: u64 = 0x1319_8A2E_0370_7344;

pub(crate) struct H128 {
    a: u64,
    b: u64,
}

impl H128 {
    pub(crate) fn new() -> Self {
        Self {
            a: 0x5A5A_0000_0000_0001,
            b: 0xA5A5_0000_0000_0002,
        }
    }

    #[inline]
    fn word(&mut self, w: u64) {
        self.a = fold(self.a ^ w, KEY_A ^ K1);
        self.b = fold(self.b ^ w, KEY_B ^ K1);
    }

    pub(crate) fn bytes(&mut self, bytes: &[u8]) {
        let (words, rest) = bytes.as_chunks::<8>();
        let (pairs, odd) = words.as_chunks::<2>();
        let (mut a, mut b) = (self.a, self.b);
        for [x, y] in pairs {
            let (x, y) = (u64::from_le_bytes(*x), u64::from_le_bytes(*y));
            a = fold(a ^ x, y ^ KEY_A ^ K2);
            b = fold(b ^ x, y ^ KEY_B ^ K2);
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
        // The length, so that zeros at the end tell lengths apart.
        self.word(bytes.len() as u64);
    }

    pub(crate) fn u64(&mut self, v: u64) {
        self.word(v);
    }

    pub(crate) fn u128(&mut self, v: u128) {
        self.word(v as u64);
        self.word((v >> 64) as u64);
    }

    pub(crate) fn finish(&self) -> u128 {
        let a = fold(self.a, KEY_A ^ K1 ^ K2);
        let b = fold(self.b, KEY_B ^ K1 ^ K2);
        (u128::from(a) << 64) | u128::from(b)
    }
}

/// A 64-bit hash of `bytes`, for sharding and maps (keyed per process, as
/// only this run sees it).
pub(crate) fn h64(bytes: &[u8]) -> u64 {
    turbonop_hash::State::default().hash_one(bytes)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn h(b: &[u8]) -> u128 {
        let mut x = H128::new();
        x.bytes(b);
        x.finish()
    }

    #[test]
    fn the_same_every_run() {
        // A fixed value: if the hash changes, anchors change, and with them
        // every folded library's bytes.
        assert_eq!(
            h(b"turbolib"),
            241_873_383_922_378_141_867_176_214_079_990_414_503
        );
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
        // The lanes differ, so the halves aren't one 64-bit hash twice.
        let v = h(b"lanes");
        assert_ne!(v as u64, (v >> 64) as u64);
    }
}
