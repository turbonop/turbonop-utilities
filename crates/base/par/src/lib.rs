//! Data parallelism: work over a slice split across every core, in order,
//! with scoped threads (no pool, no dependencies). Below a size where
//! threads cost more than they save, the work runs on the caller's thread.

use std::num::NonZero;
use std::sync::OnceLock;

/// How many threads work is split across: the machine's parallelism
/// (`HEXC_THREADS` overrides it; 1 runs everything on the caller's
/// thread).
pub fn threads() -> usize {
    static THREADS: OnceLock<usize> = OnceLock::new();
    *THREADS.get_or_init(|| {
        std::env::var("HEXC_THREADS")
            .ok()
            .and_then(|v| v.parse::<usize>().ok())
            .filter(|&n| n > 0)
            .unwrap_or_else(|| std::thread::available_parallelism().map_or(1, NonZero::get))
    })
}

/// The chunk size that splits `len` items across the threads, each chunk
/// at least `min` items; `None` if one thread should do it all.
fn chunk(len: usize, min: usize) -> Option<usize> {
    let threads = threads();
    if threads < 2 || len < min.max(1).saturating_mul(2) {
        return None;
    }
    Some(len.div_ceil(threads).max(min.max(1)))
}

/// `f` of each item, in order; split across the threads when there are
/// at least twice `min` items.
pub fn map<T: Sync, R: Send>(items: &[T], min: usize, f: impl Fn(&T) -> R + Sync) -> Vec<R> {
    map_chunks(items, min, |part| part.iter().map(&f).collect())
}

/// `f` of each chunk of `items` (each chunk's results in order, the
/// chunks' in order): for work that keeps state across a chunk's items
/// (a cache, a reusable buffer).
pub fn map_chunks<T: Sync, R: Send>(
    items: &[T],
    min: usize,
    f: impl Fn(&[T]) -> Vec<R> + Sync,
) -> Vec<R> {
    let Some(size) = chunk(items.len(), min) else {
        return f(items);
    };
    let f = &f;
    std::thread::scope(|scope| {
        let handles: Vec<_> = items
            .chunks(size)
            .map(|part| scope.spawn(move || f(part)))
            .collect();
        let mut out = Vec::with_capacity(items.len());
        for h in handles {
            out.extend(h.join().unwrap_or_else(|e| std::panic::resume_unwind(e)));
        }
        out
    })
}

/// `f` of each item, in order, for items whose costs differ widely (one
/// may take a thousand times another's): threads take `batch` items at a
/// time as they finish, rather than an equal share each up front.
pub fn map_balanced<T: Sync, R: Send>(
    items: &[T],
    batch: usize,
    f: impl Fn(&T) -> R + Sync,
) -> Vec<R> {
    use std::sync::atomic::{AtomicUsize, Ordering};
    let batch = batch.max(1);
    let batches = items.len().div_ceil(batch);
    let threads = threads().min(batches);
    if threads < 2 {
        return items.iter().map(f).collect();
    }
    let next = AtomicUsize::new(0);
    let (f, next) = (&f, &next);
    let done: Vec<Vec<(usize, Vec<R>)>> = std::thread::scope(|scope| {
        let handles: Vec<_> = (0..threads)
            .map(|_| {
                scope.spawn(move || {
                    let mut mine = Vec::new();
                    loop {
                        let b = next.fetch_add(1, Ordering::Relaxed);
                        let start = b.saturating_mul(batch);
                        let Some(part) = items.get(start..items.len().min(start + batch)) else {
                            break;
                        };
                        if part.is_empty() {
                            break;
                        }
                        mine.push((b, part.iter().map(f).collect()));
                    }
                    mine
                })
            })
            .collect();
        handles
            .into_iter()
            .map(|h| h.join().unwrap_or_else(|e| std::panic::resume_unwind(e)))
            .collect()
    });
    let mut parts: Vec<Option<Vec<R>>> = (0..batches).map(|_| None).collect();
    for (b, part) in done.into_iter().flatten() {
        parts[b] = Some(part);
    }
    let mut out = Vec::with_capacity(items.len());
    for part in parts.into_iter().flatten() {
        out.extend(part);
    }
    out
}

/// `f` over chunks of `items`, mutably, in parallel.
pub fn for_each_chunk_mut<T: Send>(items: &mut [T], min: usize, f: impl Fn(&mut [T]) + Sync) {
    let Some(size) = chunk(items.len(), min) else {
        f(items);
        return;
    };
    let f = &f;
    std::thread::scope(|scope| {
        for part in items.chunks_mut(size) {
            scope.spawn(move || f(part));
        }
    });
}

/// Sorts `v` stably by `cmp`, on every core: each chunk sorted on its
/// own thread, then the sorted runs merged, pairs at once (of equal
/// items, the earlier run's first: as `slice::sort_by` orders them).
pub fn sort_by<T: Send>(v: &mut Vec<T>, cmp: impl Fn(&T, &T) -> std::cmp::Ordering + Sync) {
    let Some(size) = chunk(v.len(), 16 * 1024) else {
        v.sort_by(cmp);
        return;
    };
    let cmp = &cmp;
    let mut runs: Vec<Vec<T>> = Vec::new();
    while !v.is_empty() {
        let at = v.len().saturating_sub(size);
        runs.push(v.split_off(at));
    }
    runs.reverse();
    std::thread::scope(|scope| {
        for run in &mut runs {
            scope.spawn(move || run.sort_by(cmp));
        }
    });
    while runs.len() > 1 {
        let mut pairs = Vec::with_capacity(runs.len().div_ceil(2));
        let mut it = runs.into_iter();
        while let Some(a) = it.next() {
            pairs.push((a, it.next()));
        }
        runs = std::thread::scope(|scope| {
            let handles: Vec<_> = pairs
                .into_iter()
                .map(|(a, b)| {
                    scope.spawn(move || match b {
                        Some(b) => merge(a, b, cmp),
                        None => a,
                    })
                })
                .collect();
            handles
                .into_iter()
                .map(|h| h.join().unwrap_or_else(|e| std::panic::resume_unwind(e)))
                .collect()
        });
    }
    *v = runs.pop().unwrap_or_default();
}

/// `a` and `b` (each sorted) merged; of equal items, `a`'s first.
fn merge<T>(a: Vec<T>, b: Vec<T>, cmp: &impl Fn(&T, &T) -> std::cmp::Ordering) -> Vec<T> {
    let mut out = Vec::with_capacity(a.len() + b.len());
    let mut a = a.into_iter().peekable();
    let mut b = b.into_iter().peekable();
    loop {
        let take_a = match (a.peek(), b.peek()) {
            (Some(x), Some(y)) => cmp(y, x) != std::cmp::Ordering::Less,
            (Some(_), None) => true,
            (None, Some(_)) => false,
            (None, None) => break,
        };
        out.extend(if take_a { a.next() } else { b.next() });
    }
    out
}

/// `a` and `b`, run at once.
pub fn join<A: Send, B: Send>(a: impl FnOnce() -> A + Send, b: impl FnOnce() -> B) -> (A, B) {
    if threads() < 2 {
        return (a(), b());
    }
    std::thread::scope(|scope| {
        let a = scope.spawn(a);
        let b = b();
        (a.join().unwrap_or_else(|e| std::panic::resume_unwind(e)), b)
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn in_order() {
        let items: Vec<u32> = (0..10_000).collect();
        assert_eq!(
            map(&items, 10, |x| x * 2),
            items.iter().map(|x| x * 2).collect::<Vec<_>>()
        );
        let chunked = map_chunks(&items, 10, |p| p.iter().map(|x| x + 1).collect());
        assert_eq!(chunked, items.iter().map(|x| x + 1).collect::<Vec<_>>());
        let mut m = items.clone();
        for_each_chunk_mut(&mut m, 10, |p| p.iter_mut().for_each(|x| *x += 3));
        assert_eq!(m, items.iter().map(|x| x + 3).collect::<Vec<_>>());
        assert!(map(&[] as &[u32], 1, |x| *x).is_empty());
        for batch in [1, 3, 64, 10_000, 20_000] {
            assert_eq!(
                map_balanced(&items, batch, |x| x * 5),
                items.iter().map(|x| x * 5).collect::<Vec<_>>()
            );
        }
        assert!(map_balanced(&[] as &[u32], 4, |x| *x).is_empty());
        assert_eq!(join(|| 1, || 2), (1, 2));
        // A stable sort: of equal keys, the original order.
        let mut pairs: Vec<(u32, u32)> = (0..200_000u32).map(|i| ((i * 7919) % 1000, i)).collect();
        let mut want = pairs.clone();
        want.sort_by_key(|p| p.0);
        sort_by(&mut pairs, |a, b| a.0.cmp(&b.0));
        assert_eq!(pairs, want);
    }
}
