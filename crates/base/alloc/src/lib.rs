//! A thread-caching allocator over another (the system's), for
//! WebAssembly's threads (wasm32-wasip1-threads), where the system
//! allocator, wasi-libc's dlmalloc, takes one lock for every thread's
//! allocations: with eight threads allocating, they wait on it more than
//! they work.
//!
//! Small blocks (up to [`MAX_SMALL`] bytes, aligned to 16 or less) are
//! kept in free lists by size class, in [`SHARDS`] shards, each with its
//! own lock; a thread uses the shard its stack's address picks (threads'
//! stacks lie apart, so threads mostly have shards of their own, and no
//! thread-local storage is needed, which an allocator can't use safely).
//! A shard with no block of a class takes another shard's list, else
//! carves a run of blocks from the system allocator at once (one system
//! allocation for many). Freed blocks go to the freeing thread's shard.
//! Larger and more aligned blocks go to the system allocator as they are.
//!
//! Small blocks are never given back to the system allocator: each class
//! keeps what it has held at most. (A program's peaks are its big
//! tables, which aren't small.)
//!
//! Native builds don't use it (their allocators have per-thread caches
//! already); it is tested natively all the same.
#![no_std]
// The allocator's own: raw blocks, and the GlobalAlloc contract.
#![allow(unsafe_code)]

use core::alloc::{GlobalAlloc, Layout};
use core::cell::UnsafeCell;
use core::ptr;
use core::sync::atomic::{AtomicBool, Ordering};

/// The largest block kept in the free lists.
pub const MAX_SMALL: usize = 32 << 10;
/// How many shards (each a lock and a free list per class).
pub const SHARDS: usize = 64;
/// How much a run carved for a class holds, at least (and at least
/// [`MIN_RUN_BLOCKS`] blocks).
const RUN_BYTES: usize = 256 << 10;
const MIN_RUN_BLOCKS: usize = 4;
const ALIGN: usize = 16;

/// The size classes: every 16 bytes to 128, then four a doubling to
/// [`MAX_SMALL`].
const CLASSES: usize = 8 + 4 * (MAX_SMALL.trailing_zeros() as usize - 7);

/// The class of a block of `size` bytes (`size` from 1 to `MAX_SMALL`).
const fn class_of(size: usize) -> usize {
    if size <= 128 {
        return size.div_ceil(16) - 1;
    }
    // Above 128: within the doubling [2^k, 2^(k+1)), four steps of 2^(k-2).
    let k = usize::BITS - 1 - (size - 1).leading_zeros(); // 2^k < size <= 2^(k+1)
    let step = 1usize << (k - 2);
    let base = 1usize << k;
    let i = (size - base).div_ceil(step); // 1..=4
    8 + (k as usize - 7) * 4 + i - 1
}

/// A class's block size.
const fn size_of_class(class: usize) -> usize {
    if class < 8 {
        return (class + 1) * 16;
    }
    let c = class - 8;
    let k = c / 4 + 7;
    let i = c % 4 + 1;
    (1usize << k) + i * (1usize << (k - 2))
}

/// One shard: a lock, and each class's free list (the first word of each
/// free block points to the next; 0 ends it) and its length. (A cache
/// line of its own, at least: threads don't contend over neighbors.)
#[repr(align(128))]
struct Shard {
    locked: AtomicBool,
    lists: UnsafeCell<Lists>,
}

struct Lists {
    heads: [usize; CLASSES],
    counts: [usize; CLASSES],
}

// SAFETY: `lists` is read and written only with `locked` held.
unsafe impl Sync for Shard {}

/// The most blocks of a class a shard keeps: two runs' worth. Past it,
/// freed blocks go to the shared overflow lists, which a shard out of a
/// class takes before carving more (so that blocks freed by one thread
/// and wanted by another, as a parallel step's results are, aren't
/// stranded).
const fn kept(class: usize) -> usize {
    2 * run_blocks(class)
}

/// How many blocks a run of `class` has.
const fn run_blocks(class: usize) -> usize {
    let n = RUN_BYTES / size_of_class(class);
    if n > MIN_RUN_BLOCKS {
        n
    } else {
        MIN_RUN_BLOCKS
    }
}

impl Shard {
    const fn new() -> Self {
        Self {
            locked: AtomicBool::new(false),
            lists: UnsafeCell::new(Lists {
                heads: [0; CLASSES],
                counts: [0; CLASSES],
            }),
        }
    }

    fn try_lock(&self) -> bool {
        self.locked
            .compare_exchange(false, true, Ordering::Acquire, Ordering::Relaxed)
            .is_ok()
    }

    fn lock(&self) {
        while !self.try_lock() {
            while self.locked.load(Ordering::Relaxed) {
                core::hint::spin_loop();
            }
        }
    }

    fn unlock(&self) {
        self.locked.store(false, Ordering::Release);
    }
}

/// The thread-caching allocator over `A` (the system allocator, say).
pub struct Cached<A> {
    inner: A,
    shards: [Shard; SHARDS],
    /// Blocks freed past what a shard keeps, for any shard to take.
    overflow: Shard,
    /// Where runs are carved from: a region taken from `inner`, many
    /// megabytes at once (its next free byte and end), behind its lock.
    region: UnsafeCell<(usize, usize)>,
    region_locked: AtomicBool,
}

// SAFETY: `region` is read and written only with `region_locked` held.
unsafe impl<A: Sync> Sync for Cached<A> {}

impl<A> Cached<A> {
    pub const fn new(inner: A) -> Self {
        Self {
            inner,
            shards: [const { Shard::new() }; SHARDS],
            overflow: Shard::new(),
            region: UnsafeCell::new((0, 0)),
            region_locked: AtomicBool::new(false),
        }
    }
}

/// Whether a layout's blocks are this allocator's own (else `A`'s).
fn small(layout: Layout) -> bool {
    layout.size() <= MAX_SMALL && layout.align() <= ALIGN && layout.size() > 0
}

/// The shard of the calling thread: its stack's address, hashed (threads'
/// stacks lie a megabyte and more apart).
#[inline]
fn shard_index() -> usize {
    let here = 0u8;
    let addr = ptr::from_ref(&here) as usize;
    (addr >> 20).wrapping_mul(0x9E37_79B9) >> 7 & (SHARDS - 1)
}

impl<A: GlobalAlloc> Cached<A> {
    /// A block of `class` from shard `s`'s list (its lock held), if any.
    ///
    /// SAFETY: the caller holds shard `s`'s lock.
    unsafe fn pop(&self, s: usize, class: usize) -> *mut u8 {
        // SAFETY: the lock is held (the caller's promise).
        let lists = unsafe { &mut *self.shards[s].lists.get() };
        let head = lists.heads[class];
        if head != 0 {
            // SAFETY: a free block's first word is the next free block.
            lists.heads[class] = unsafe { ptr::read(head as *const usize) };
            lists.counts[class] -= 1;
        }
        head as *mut u8
    }

    /// Takes the overflow list of `class` into shard `s` (its lock held,
    /// its list empty); whether there was one.
    ///
    /// SAFETY: the caller holds shard `s`'s lock.
    unsafe fn refill(&self, s: usize, class: usize) -> bool {
        self.overflow.lock();
        // SAFETY: the overflow's lock is held.
        let (head, count) = unsafe {
            let o = &mut *self.overflow.lists.get();
            let taken = (o.heads[class], o.counts[class]);
            o.heads[class] = 0;
            o.counts[class] = 0;
            taken
        };
        self.overflow.unlock();
        if head == 0 {
            return false;
        }
        // SAFETY: the caller holds `s`'s lock; its list was empty.
        unsafe {
            let lists = &mut *self.shards[s].lists.get();
            lists.heads[class] = head;
            lists.counts[class] = count;
        }
        true
    }

    /// `size` bytes (a multiple of 16) for a run, 16-aligned, from the
    /// region (a new one taken from `inner` when it runs out); null if
    /// `inner` has no more.
    ///
    /// Regions are large (each growth of WebAssembly's shared memory
    /// stops every thread, and the system allocator grows it a few pages
    /// at a time), and taken from `inner` itself rather than from linear
    /// memory directly: dlmalloc assumes it alone grows memory (two
    /// growths in a row, adjacent), and would take a region grown between
    /// them for its own. What is left of a region when a run doesn't fit
    /// is abandoned (a run is at most a few hundred KiB of 16 MiB).
    fn take(&self, size: usize) -> *mut u8 {
        const REGION: usize = 16 << 20;
        while self
            .region_locked
            .compare_exchange(false, true, Ordering::Acquire, Ordering::Relaxed)
            .is_err()
        {
            core::hint::spin_loop();
        }
        // SAFETY: the region's lock is held.
        let region = unsafe { &mut *self.region.get() };
        if region.1 - region.0 < size {
            let len = REGION.max(size);
            let fresh = Layout::from_size_align(len, ALIGN)
                // SAFETY: a non-zero size.
                .map_or(ptr::null_mut(), |l| unsafe { self.inner.alloc(l) });
            if fresh.is_null() {
                self.region_locked.store(false, Ordering::Release);
                return ptr::null_mut();
            }
            *region = (fresh as usize, fresh as usize + len);
        }
        let at = region.0;
        region.0 += size;
        self.region_locked.store(false, Ordering::Release);
        at as *mut u8
    }

    /// Carves a run of `class`'s blocks from the inner allocator into
    /// shard `s`'s list (its lock held); whether it could.
    ///
    /// SAFETY: the caller holds shard `s`'s lock.
    unsafe fn carve(&self, s: usize, class: usize) -> bool {
        let size = size_of_class(class);
        let n = run_blocks(class);
        let run = self.take(size * n);
        if run.is_null() {
            return false;
        }
        // Each block points to the next; the last, to the list as it was
        // (empty).
        // SAFETY: the caller holds the lock; every block is within the run.
        unsafe {
            let lists = &mut *self.shards[s].lists.get();
            let mut next = lists.heads[class];
            for i in (0..n).rev() {
                let block = run.add(i * size);
                ptr::write(block.cast::<usize>(), next);
                next = block as usize;
            }
            lists.heads[class] = next;
            lists.counts[class] += n;
        }
        true
    }
}

// SAFETY: small blocks are handed out once each (popped under their
// shard's lock) and returned to a list only when freed; every block is
// `size_of_class` bytes, at least the size asked for, and 16-aligned
// (runs are 16-aligned and classes multiples of 16); the rest is `A`'s.
unsafe impl<A: GlobalAlloc> GlobalAlloc for Cached<A> {
    #[inline]
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        if !small(layout) {
            // SAFETY: the caller's layout, as given.
            return unsafe { self.inner.alloc(layout) };
        }
        let class = class_of(layout.size());
        let s = shard_index();
        let shard = &self.shards[s];
        shard.lock();
        // SAFETY: the shard's lock is held.
        let block = unsafe {
            let mut b = self.pop(s, class);
            if b.is_null() && (self.refill(s, class) || self.carve(s, class)) {
                b = self.pop(s, class);
            }
            b
        };
        shard.unlock();
        block
    }

    #[inline]
    unsafe fn dealloc(&self, block: *mut u8, layout: Layout) {
        if !small(layout) {
            // SAFETY: allocated by `inner` with this layout.
            unsafe { self.inner.dealloc(block, layout) };
            return;
        }
        let class = class_of(layout.size());
        let s = shard_index();
        let shard = &self.shards[s];
        shard.lock();
        // SAFETY: the lock is held; the block is the caller's to give back,
        // at least a word long and aligned to one.
        let kept_here = unsafe {
            let lists = &mut *shard.lists.get();
            let keep = lists.counts[class] < kept(class);
            if keep {
                ptr::write(block.cast::<usize>(), lists.heads[class]);
                lists.heads[class] = block as usize;
                lists.counts[class] += 1;
            }
            keep
        };
        shard.unlock();
        if !kept_here {
            self.overflow.lock();
            // SAFETY: the overflow's lock is held.
            unsafe {
                let o = &mut *self.overflow.lists.get();
                ptr::write(block.cast::<usize>(), o.heads[class]);
                o.heads[class] = block as usize;
                o.counts[class] += 1;
            }
            self.overflow.unlock();
        }
    }

    #[inline]
    unsafe fn realloc(&self, block: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        let new = Layout::from_size_align(new_size, layout.align());
        let Ok(new) = new else {
            return ptr::null_mut();
        };
        match (small(layout), small(new)) {
            // The same class: the block as it is.
            (true, true) if class_of(layout.size()) == class_of(new_size) => block,
            // SAFETY: both `inner`'s.
            (false, false) => unsafe { self.inner.realloc(block, layout, new_size) },
            _ => {
                // SAFETY: a new block, the old one's bytes copied (as many
                // as both have), then the old one freed.
                unsafe {
                    let to = self.alloc(new);
                    if !to.is_null() {
                        ptr::copy_nonoverlapping(block, to, layout.size().min(new_size));
                        self.dealloc(block, layout);
                    }
                    to
                }
            }
        }
    }
}

#[cfg(test)]
mod tests;
