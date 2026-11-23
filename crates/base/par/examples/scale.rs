//! How work splits across threads (`HEXC_THREADS`): computation alone,
//! small allocations, and large ones, each timed. (For the web version:
//! WebAssembly's threads, against native ones.)

use std::time::Instant;

#[cfg(target_family = "wasm")]
#[global_allocator]
static A: hexcavator_alloc::Cached<std::alloc::System> =
    hexcavator_alloc::Cached::new(std::alloc::System);

fn main() {
    for round in 0..3 {
        println!("round {round}");
        let items: Vec<u64> = (0..2_000_000).collect();
        let t = Instant::now();
        let sum: u64 = hexcavator_par::map(&items, 1024, |&x| {
            let mut v = x;
            for _ in 0..200 {
                v = v
                    .wrapping_mul(6364136223846793005)
                    .wrapping_add(1442695040888963407);
            }
            v
        })
        .iter()
        .fold(0, |a, b| a ^ b);
        println!("compute: {} ms ({sum})", t.elapsed().as_millis());
        let t = Instant::now();
        let lens: usize = hexcavator_par::map(&items, 1024, |&x| format!("item {x}").len())
            .iter()
            .sum();
        println!("small allocations: {} ms ({lens})", t.elapsed().as_millis());
        let t = Instant::now();
        let big: usize = hexcavator_par::map(&items[..20_000], 16, |&x| {
            vec![(x & 0xFF) as u8; 64 << 10].len()
        })
        .iter()
        .sum();
        println!("64 KiB allocations: {} ms ({big})", t.elapsed().as_millis());
    }
}
