//! The allocator over the system's, natively: its classes, and many threads
//! allocating, writing, reallocating, and freeing (each block's bytes its
//! own throughout), across classes and past them.
#![allow(unsafe_code, clippy::cast_possible_truncation)]

extern crate std;

use std::alloc::System;
use std::vec::Vec;

use super::*;

static A: Cached<System> = Cached::new(System);

#[test]
fn classes_cover_every_size() {
    let mut last = 0;
    for class in 0..CLASSES {
        let size = size_of_class(class);
        assert!(size > last && size.is_multiple_of(16), "{class}: {size}");
        assert_eq!(class_of(size), class, "{size}");
        assert_eq!(class_of(last + 1), class, "{}", last + 1);
        last = size;
    }
    assert_eq!(last, MAX_SMALL);
    for size in 1..=MAX_SMALL {
        let c = class_of(size);
        assert!(
            size_of_class(c) >= size && (c == 0 || size_of_class(c - 1) < size),
            "{size}"
        );
    }
}

/// A block's bytes, filled with `tag`, and checked.
fn fill(p: *mut u8, n: usize, tag: u8) {
    // SAFETY: `p` holds `n` bytes.
    unsafe { ptr::write_bytes(p, tag, n) };
}
fn check(p: *const u8, n: usize, tag: u8) {
    // SAFETY: `p` holds `n` bytes.
    let s = unsafe { core::slice::from_raw_parts(p, n) };
    assert!(s.iter().all(|&b| b == tag), "a block's bytes changed");
}

#[test]
fn threads_allocate_and_free_apart() {
    let threads: Vec<_> = (0..16u8)
        .map(|t| {
            std::thread::spawn(move || {
                let mut live: Vec<(*mut u8, Layout, u8)> = Vec::new();
                let mut seed = 0x9E37_79B9_7F4A_7C15u64 ^ u64::from(t);
                let mut next = || {
                    seed ^= seed << 13;
                    seed ^= seed >> 7;
                    seed ^= seed << 17;
                    seed
                };
                for i in 0..40_000u32 {
                    let r = next();
                    if r % 3 != 0 || live.is_empty() {
                        // Mostly small; some past the classes; some aligned.
                        let size = match r % 10 {
                            0 => 1 + (r >> 8) as usize % (MAX_SMALL * 2),
                            1 => 1 + (r >> 8) as usize % MAX_SMALL,
                            _ => 1 + (r >> 8) as usize % 300,
                        };
                        let align = if r % 17 == 0 { 64 } else { 8 };
                        let layout = Layout::from_size_align(size, align).unwrap();
                        // SAFETY: a non-zero size.
                        let p = unsafe { A.alloc(layout) };
                        assert!(!p.is_null() && (p as usize).is_multiple_of(align));
                        let tag = (i as u8) ^ t;
                        fill(p, size, tag);
                        live.push((p, layout, tag));
                    } else if r % 7 == 0 {
                        // Grown or shrunk: its bytes kept.
                        let k = (r >> 16) as usize % live.len();
                        let (p, layout, tag) = live[k];
                        let new_size = 1 + (r >> 24) as usize % (MAX_SMALL + 100);
                        // SAFETY: `p` is live with `layout`.
                        let q = unsafe { A.realloc(p, layout, new_size) };
                        assert!(!q.is_null());
                        check(q, layout.size().min(new_size), tag);
                        fill(q, new_size, tag);
                        live[k] = (
                            q,
                            Layout::from_size_align(new_size, layout.align()).unwrap(),
                            tag,
                        );
                    } else {
                        let k = (r >> 16) as usize % live.len();
                        let (p, layout, tag) = live.swap_remove(k);
                        check(p, layout.size(), tag);
                        // SAFETY: `p` is live with `layout`.
                        unsafe { A.dealloc(p, layout) };
                    }
                }
                // Some freed here, the rest given to another thread to free.
                live.into_iter()
                    .map(|(p, l, t)| (p as usize, l, t))
                    .collect::<Vec<_>>()
            })
        })
        .collect();
    let leftovers: Vec<_> = threads
        .into_iter()
        .flat_map(|h| h.join().unwrap())
        .collect();
    std::thread::spawn(move || {
        for (p, layout, tag) in leftovers {
            check(p as *const u8, layout.size(), tag);
            // SAFETY: live with `layout`.
            unsafe { A.dealloc(p as *mut u8, layout) };
        }
    })
    .join()
    .unwrap();
}

#[test]
fn freed_blocks_are_reused() {
    let layout = Layout::from_size_align(48, 8).unwrap();
    // SAFETY: a non-zero size; each freed as allocated.
    unsafe {
        let a = A.alloc(layout);
        A.dealloc(a, layout);
        let b = A.alloc(layout);
        assert_eq!(a, b, "the same thread's shard gives its freed block back");
        A.dealloc(b, layout);
    }
}
