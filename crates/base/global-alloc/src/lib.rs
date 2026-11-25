//! The allocator of every TurboNOP program and test. Linking this crate
//! makes it the program's global allocator, so each program's root (its
//! `main.rs`), each test binary's, and each library's unit tests say
//!
//! ```ignore
//! use turbonop_global_alloc as _;
//! ```
//!
//! (A crate no code names isn't linked, and its allocator with it.)
//!
//! - Natively, mimalloc. Each thread has its own heap, where the system
//!   allocators share theirs: musl's serializes every thread's
//!   allocations, and the others take locks that analyses on every core
//!   contend for. What is shipped is what is tested.
//! - On WebAssembly, `turbonop_alloc::Cached` over the system's
//!   allocator: wasi-libc's dlmalloc takes one lock for every thread's
//!   allocations, and mimalloc doesn't build for WASI.
//!
//! (A program with an allocator of its own, as the fuzzer's counting one,
//! doesn't link this crate, whose allocator would be a second: it puts its
//! own over mimalloc instead.)

#[cfg(not(target_family = "wasm"))]
#[global_allocator]
static ALLOCATOR: mimalloc::MiMalloc = mimalloc::MiMalloc;

#[cfg(target_family = "wasm")]
#[global_allocator]
static ALLOCATOR: turbonop_alloc::Cached<std::alloc::System> =
    turbonop_alloc::Cached::new(std::alloc::System);

#[cfg(test)]
mod tests {
    /// Allocations go through it: many threads allocating and freeing at
    /// once, of every size class's neighborhood.
    #[test]
    fn threads_allocate_and_free() {
        let threads: Vec<_> = (0..16)
            .map(|t| {
                std::thread::spawn(move || {
                    let mut kept: Vec<Vec<u8>> = Vec::new();
                    for i in 0..20_000usize {
                        let n = (i * 7 + t) % 70_000;
                        let v = vec![u8::try_from(i % 251).unwrap_or(0); n];
                        assert!(v.iter().all(|&b| b == v.first().copied().unwrap_or(0)));
                        if i % 3 == 0 {
                            kept.push(v);
                        }
                        if kept.len() > 64 {
                            kept.clear();
                        }
                    }
                    kept.len()
                })
            })
            .collect();
        for t in threads {
            assert!(t.join().unwrap() <= 64);
        }
    }
}
