//! 128-bit content hashes, for telling records and fold units apart.
//!
//! Two lanes of `turbonop-hash`'s keyed hash, separated by a tag. Equal
//! hashes are taken as equal content by the analyzer, whose figures are
//! estimates; folding compares the bytes too before relying on them.

use std::hash::{BuildHasher, Hasher};

pub(crate) struct H128 {
    a: turbonop_hash::Hash,
    b: turbonop_hash::Hash,
}

impl H128 {
    pub(crate) fn new() -> Self {
        let state = turbonop_hash::State::default();
        let mut a = state.build_hasher();
        let mut b = state.build_hasher();
        a.write_u64(0x5A5A_0000_0000_0001);
        b.write_u64(0xA5A5_0000_0000_0002);
        Self { a, b }
    }

    pub(crate) fn bytes(&mut self, bytes: &[u8]) {
        self.a.write(bytes);
        self.b.write(bytes);
    }

    pub(crate) fn u64(&mut self, v: u64) {
        self.a.write_u64(v);
        self.b.write_u64(v);
    }

    pub(crate) fn u128(&mut self, v: u128) {
        self.u64(v as u64);
        self.u64((v >> 64) as u64);
    }

    pub(crate) fn finish(&self) -> u128 {
        (u128::from(self.a.finish()) << 64) | u128::from(self.b.finish())
    }
}

/// A 64-bit hash of `bytes`, for sharding and maps.
pub(crate) fn h64(bytes: &[u8]) -> u64 {
    turbonop_hash::State::default().hash_one(bytes)
}
