//! Prints sample words for checking the access data against LLVM: up to
//! `per` (default 4) random words of every encoding that decode as it
//! (not CONSTRAINED UNPREDICTABLE), one hex word per line, the same each
//! run. `tools/arm64/access_golden.py` runs them through llvm-mc for
//! `tests/data/access.txt`:
//!
//!     cargo run --release -p turbonop-arm64 --example access_sample -- [per] > words.txt

#![allow(
    clippy::cast_possible_truncation,
    reason = "word arithmetic over a 32-bit space"
)]

use std::collections::BTreeSet;
use std::fmt::Write as _;

fn main() {
    let per: usize = std::env::args()
        .nth(1)
        .and_then(|a| a.parse().ok())
        .unwrap_or(4);
    // xorshift64*, seeded: the same words every run.
    let mut state = 0x9E37_79B9_7F4A_7C15u64;
    let mut next = move || {
        state ^= state >> 12;
        state ^= state << 25;
        state ^= state >> 27;
        (state.wrapping_mul(0x2545_F491_4F6C_DD1D) >> 32) as u32
    };
    let mut words = BTreeSet::new();
    for (name, mask, value) in turbonop_arm64::encodings() {
        let mut found = 0;
        for _ in 0..400 {
            if found == per {
                break;
            }
            let w = (next() & !mask) | value;
            let Some(inst) = turbonop_arm64::decode(w, 0x1000) else {
                continue;
            };
            if inst.encoding_name() != name || inst.is_unpredictable() {
                continue;
            }
            if words.insert(w) {
                found += 1;
            }
        }
    }
    let mut out = String::new();
    for w in words {
        let _ = writeln!(out, "{w:08X}");
    }
    print!("{out}");
}
