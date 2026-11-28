//! Data parallelism: work over a slice split across every core, in order,
//! with scoped threads (no pool, no dependencies). Below a size where
//! threads cost more than they save, the work runs on the caller's thread.

#[cfg(test)]
use turbonop_global_alloc as _;

use std::num::NonZero;
use std::sync::OnceLock;

static THREADS: OnceLock<usize> = OnceLock::new();

/// Sets how many threads work is split across (a program's `-j`), before
/// anything has asked; 1 runs everything on the caller's thread, with no
/// thread created. Returns whether it was set (false once [`threads`] has
/// been read or this already called).
pub fn set_threads(n: usize) -> bool {
    THREADS.set(n.max(1)).is_ok()
}

/// How many threads work is split across: what [`set_threads`] set, or the
/// machine's parallelism (`TURBONOP_THREADS` overrides it; 1 runs
/// everything on the caller's thread); no more than [`set_limit`]'s.
pub fn threads() -> usize {
    let n = configured();
    match LIMIT.load(std::sync::atomic::Ordering::Relaxed) {
        0 => n,
        limit => n.min(limit),
    }
}

/// The threads [`threads`] gives without a limit.
pub fn configured() -> usize {
    *THREADS.get_or_init(|| {
        std::env::var("TURBONOP_THREADS")
            .ok()
            .and_then(|v| v.parse::<usize>().ok())
            .filter(|&n| n > 0)
            .unwrap_or_else(|| std::thread::available_parallelism().map_or(1, NonZero::get))
    })
}

/// The most threads, for now (0: no limit): a program sharing the machine
/// with others (see turbolib's slots) changes it as it goes. Work already
/// split keeps its threads.
pub fn set_limit(n: usize) {
    LIMIT.store(n, std::sync::atomic::Ordering::Relaxed);
}

static LIMIT: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);

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

/// `f` of each item of `items` (taken: for work that consumes them), in
/// order; split across the threads when there are at least twice `min`.
pub fn map_into<T: Send, R: Send>(
    mut items: Vec<T>,
    min: usize,
    f: impl Fn(T) -> R + Sync,
) -> Vec<R> {
    let Some(size) = chunk(items.len(), min) else {
        return items.into_iter().map(f).collect();
    };
    let mut parts: Vec<Vec<T>> = Vec::new();
    while !items.is_empty() {
        let at = items.len().saturating_sub(size);
        parts.push(items.split_off(at));
    }
    parts.reverse();
    let f = &f;
    std::thread::scope(|scope| {
        let handles: Vec<_> = parts
            .into_iter()
            .map(|part| scope.spawn(move || part.into_iter().map(f).collect::<Vec<R>>()))
            .collect();
        let mut out = Vec::new();
        for h in handles {
            out.extend(h.join().unwrap_or_else(|e| std::panic::resume_unwind(e)));
        }
        out
    })
}

/// `f` of each index of `0..len`, in order; split across the threads when
/// there are at least twice `min` of them.
pub fn map_range<R: Send>(len: usize, min: usize, f: impl Fn(usize) -> R + Sync) -> Vec<R> {
    let Some(size) = chunk(len, min) else {
        return (0..len).map(f).collect();
    };
    let f = &f;
    std::thread::scope(|scope| {
        let handles: Vec<_> = (0..len)
            .step_by(size)
            .map(|at| scope.spawn(move || (at..len.min(at + size)).map(f).collect::<Vec<R>>()))
            .collect();
        let mut out = Vec::with_capacity(len);
        for h in handles {
            out.extend(h.join().unwrap_or_else(|e| std::panic::resume_unwind(e)));
        }
        out
    })
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

/// `f` of each index of `0..len`, in order, for work whose cost `cost`
/// estimates: threads take the dearest first, one at a time as they
/// finish (so one large item, met last, doesn't leave the others idle).
/// The results don't depend on the schedule.
pub fn map_costed<R: Send>(
    len: usize,
    cost: impl Fn(usize) -> u64,
    f: impl Fn(usize) -> R + Sync,
) -> Vec<R> {
    use std::sync::atomic::{AtomicUsize, Ordering};
    let threads = threads().min(len);
    if threads < 2 {
        return (0..len).map(f).collect();
    }
    let mut order: Vec<usize> = (0..len).collect();
    order.sort_by_key(|&i| std::cmp::Reverse(cost(i)));
    let next = AtomicUsize::new(0);
    let (f, next, order) = (&f, &next, &order);
    let done: Vec<Vec<(usize, R)>> = std::thread::scope(|scope| {
        let handles: Vec<_> = (0..threads)
            .map(|_| {
                scope.spawn(move || {
                    let mut mine = Vec::new();
                    while let Some(&i) = order.get(next.fetch_add(1, Ordering::Relaxed)) {
                        mine.push((i, f(i)));
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
    let mut slots: Vec<Option<R>> = (0..len).map(|_| None).collect();
    for (i, r) in done.into_iter().flatten() {
        slots[i] = Some(r);
    }
    slots.into_iter().flatten().collect()
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

/// Everything reachable from `roots`, each item visited once by whichever
/// thread is free: `visit(item, more, acc)` handles an item, pushing what
/// it leads to onto `more` (claiming them first, so nothing is pushed
/// twice) and its findings into `acc`, one `acc` (from `new_acc`) per
/// thread; the accumulators come back in no particular order.
///
/// Each thread works from a stack of its own, and while no work is queued
/// for the others it hands half of its stack to the queue; one that runs
/// out takes half of what's queued. There's no step to wait for the
/// slowest thread at: on a machine busy with other programs, which pause
/// some threads, the others take the work. Serial if `parallel` is false
/// or there's one thread.
pub fn traverse<T: Send, A: Send>(
    roots: Vec<T>,
    parallel: bool,
    new_acc: impl Fn() -> A + Sync,
    visit: impl Fn(T, &mut Vec<T>, &mut A) + Sync,
) -> Vec<A> {
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::{Condvar, Mutex, PoisonError};
    let threads = threads();
    if threads < 2 || !parallel {
        let mut acc = new_acc();
        let mut stack = roots;
        while let Some(x) = stack.pop() {
            visit(x, &mut stack, &mut acc);
        }
        return vec![acc];
    }
    /// The queue, how many threads wait on it, and whether all is done.
    struct Queue<T> {
        items: Vec<T>,
        waiting: usize,
        done: bool,
    }
    let queue = Mutex::new(Queue {
        items: roots,
        waiting: 0,
        done: false,
    });
    let wake = Condvar::new();
    // How many items are queued (a hint, read without the lock).
    let queued = AtomicUsize::new(1);
    /// Ends the traversal if its thread panics, so the others don't wait
    /// for it forever (the scope then passes the panic on).
    struct OnPanic<'a, T>(&'a Mutex<Queue<T>>, &'a Condvar);
    impl<T> Drop for OnPanic<'_, T> {
        fn drop(&mut self) {
            if std::thread::panicking() {
                self.0.lock().unwrap_or_else(PoisonError::into_inner).done = true;
                self.1.notify_all();
            }
        }
    }
    let work = || {
        let _guard = OnPanic(&queue, &wake);
        let mut acc = new_acc();
        let mut stack: Vec<T> = Vec::new();
        loop {
            while let Some(x) = stack.pop() {
                visit(x, &mut stack, &mut acc);
                if stack.len() >= 2 && queued.load(Ordering::Relaxed) == 0 {
                    let mut q = queue.lock().unwrap_or_else(PoisonError::into_inner);
                    let half = stack.len() / 2;
                    // The oldest half: what leads to the most.
                    q.items.extend(stack.drain(..half));
                    queued.store(q.items.len(), Ordering::Relaxed);
                    if q.waiting > 0 {
                        wake.notify_all();
                    }
                }
            }
            let mut q = queue.lock().unwrap_or_else(PoisonError::into_inner);
            loop {
                if q.done {
                    return acc;
                }
                if !q.items.is_empty() {
                    let take = q.items.len().div_ceil(2);
                    let from = q.items.len() - take;
                    stack.extend(q.items.drain(from..));
                    queued.store(q.items.len(), Ordering::Relaxed);
                    break;
                }
                q.waiting += 1;
                if q.waiting == threads {
                    q.done = true;
                    wake.notify_all();
                    return acc;
                }
                q = wake.wait(q).unwrap_or_else(PoisonError::into_inner);
                q.waiting -= 1;
            }
        }
    };
    let work = &work;
    std::thread::scope(|scope| {
        let handles: Vec<_> = (1..threads).map(|_| scope.spawn(work)).collect();
        let mut all = vec![work()];
        for h in handles {
            all.push(h.join().unwrap_or_else(|e| std::panic::resume_unwind(e)));
        }
        all
    })
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
    fn traverse_visits_everything_reachable_once() {
        use std::sync::atomic::{AtomicBool, Ordering};
        // A graph: i leads to 2i+1 and 2i+2 (a binary tree) and to i/3,
        // below 200_000; from roots 0 and 7.
        let n = 200_000usize;
        let seen: Vec<AtomicBool> = (0..n).map(|_| AtomicBool::new(false)).collect();
        let claim = |i: usize| i < n && !seen[i].swap(true, Ordering::Relaxed);
        let mut roots = Vec::new();
        for r in [0usize, 7] {
            if claim(r) {
                roots.push(r);
            }
        }
        for parallel in [false, true] {
            if parallel {
                for b in &seen {
                    b.store(false, Ordering::Relaxed);
                }
                for &r in &roots {
                    seen[r].store(true, Ordering::Relaxed);
                }
            }
            let counts = traverse(
                roots.clone(),
                parallel,
                || 0usize,
                |i, more, count| {
                    *count += 1;
                    for j in [2 * i + 1, 2 * i + 2, i / 3] {
                        if claim(j) {
                            more.push(j);
                        }
                    }
                },
            );
            assert_eq!(counts.iter().sum::<usize>(), n, "parallel {parallel}");
            assert!(seen.iter().all(|b| b.load(Ordering::Relaxed)));
        }
        // Nothing to do: no thread waits forever.
        assert_eq!(
            traverse(Vec::<u32>::new(), true, || 0u32, |_, _, _| {}).len(),
            threads()
        );
    }

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
        for len in [0usize, 1, 19, 10_000] {
            let owned: Vec<String> = (0..len).map(|i| i.to_string()).collect();
            assert_eq!(
                map_into(owned.clone(), 3, |s| s + "!"),
                owned.iter().map(|s| format!("{s}!")).collect::<Vec<_>>()
            );
        }
        for len in [0, 1, 19, 10_000] {
            assert_eq!(
                map_range(len, 3, |i| i * 7),
                (0..len).map(|i| i * 7).collect::<Vec<_>>()
            );
        }
        for len in [0usize, 1, 19, 10_000] {
            assert_eq!(
                map_costed(len, |i| (i as u64 * 7919) % 101, |i| i * 3),
                (0..len).map(|i| i * 3).collect::<Vec<_>>()
            );
        }
        assert_eq!(join(|| 1, || 2), (1, 2));
        // A limit caps the threads, and splitting still gives the same.
        set_limit(1);
        assert_eq!(threads(), 1);
        assert_eq!(
            map(&items, 10, |x| x * 2),
            items.iter().map(|x| x * 2).collect::<Vec<_>>()
        );
        set_limit(0);
        assert_eq!(threads(), configured());
        // A stable sort: of equal keys, the original order.
        let mut pairs: Vec<(u32, u32)> = (0..200_000u32).map(|i| ((i * 7919) % 1000, i)).collect();
        let mut want = pairs.clone();
        want.sort_by_key(|p| p.0);
        sort_by(&mut pairs, |a, b| a.0.cmp(&b.0));
        assert_eq!(pairs, want);
    }
}
