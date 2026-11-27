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

/// mimalloc 2's `mi_option_purge_delay` (its `mi_option_e`'s 16th entry;
/// the bindings name 14 and 16 but not this one). A test checks it by its
/// default, 10 ms.
#[cfg(not(target_family = "wasm"))]
const MI_OPTION_PURGE_DELAY: libmimalloc_sys::mi_option_t = 15;

/// mimalloc 2's `mi_option_eager_commit` (its `mi_option_e`'s 4th entry,
/// which the bindings don't name either). A test checks it by its
/// default, 1.
#[cfg(not(target_family = "wasm"))]
const MI_OPTION_EAGER_COMMIT: libmimalloc_sys::mi_option_t = 3;

/// Tunes the allocator for a batch program (one that reads its inputs,
/// works, writes, and exits), at the start of `main`: freed memory is kept
/// for reuse rather than returned to the operating system after mimalloc's
/// purge delay (10 ms). A program that frees and allocates large amounts
/// again and again otherwise spends much of its time returning pages and
/// faulting them back in (turbolib's ELF folding: about two fifths of its
/// CPU time was system time, and `MIMALLOC_PURGE_DELAY=-1` cut that about
/// fourfold). Not for long-running programs, whose memory would then only
/// grow to its peak and stay there.
///
/// And segments are committed as their pages are used, not whole when a
/// thread takes one (mimalloc's default after a thread's first segment):
/// a program whose threads come and go (`turbonop_par` makes them for each
/// parallel step) otherwise holds a mostly unused 32 MiB segment for each.
/// Measured on turbolib making `clangSema.lib` (984 MiB) on 8 threads, its
/// peak commit went from 289 MiB to 167 MiB without folding, in the same
/// CPU time.
///
/// Either is left as it is where its environment variable
/// (`MIMALLOC_PURGE_DELAY`, `MIMALLOC_EAGER_COMMIT`) sets it, to measure or
/// tune them.
pub fn for_batch_work() {
    #[cfg(not(target_family = "wasm"))]
    {
        let set = |name: &str| std::env::var_os(name).is_some();
        let purge = !set("MIMALLOC_PURGE_DELAY");
        let eager = !set("MIMALLOC_EAGER_COMMIT");
        // SAFETY: `mi_option_set` sets one of mimalloc's option values,
        // which it reads with relaxed atomics; any thread may call it at
        // any time.
        #[allow(unsafe_code)]
        unsafe {
            if purge {
                libmimalloc_sys::mi_option_set(MI_OPTION_PURGE_DELAY, -1);
            }
            if eager {
                libmimalloc_sys::mi_option_set(MI_OPTION_EAGER_COMMIT, 0);
            }
        }
    }
}

#[cfg(target_family = "wasm")]
#[global_allocator]
static ALLOCATOR: turbonop_alloc::Cached<std::alloc::System> =
    turbonop_alloc::Cached::new(std::alloc::System);

#[cfg(test)]
mod tests {
    /// Option 15 is the purge delay (its default is 10 ms), tuning for
    /// batch work sets it to -1, and allocation still works after it.
    #[test]
    #[cfg(not(target_family = "wasm"))]
    fn batch_tuning() {
        // SAFETY: reading an option value, as any thread may.
        #[allow(unsafe_code)]
        let get = || unsafe { libmimalloc_sys::mi_option_get(super::MI_OPTION_PURGE_DELAY) };
        let before = get();
        assert!(before == 10 || before == -1, "{before}");
        // SAFETY: as above.
        #[allow(unsafe_code)]
        let eager = || unsafe { libmimalloc_sys::mi_option_get(super::MI_OPTION_EAGER_COMMIT) };
        let e = eager();
        assert!(e == 1 || e == 0, "{e}");
        super::for_batch_work();
        assert_eq!(get(), -1);
        assert_eq!(eager(), 0);
        let v = vec![7u8; 1 << 20];
        drop(v);
        let w = vec![9u8; 1 << 20];
        assert_eq!(w[12_345], 9);
    }

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
