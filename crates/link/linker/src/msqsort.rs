//! The C runtime's `qsort`, as the UCRT implements it: link.exe sorts some
//! of the import tables' pieces with it, and `qsort` isn't stable, so the
//! order it leaves equal elements in is part of link.exe's output. This is
//! the same algorithm (a quicksort on the median of three, insertion sort
//! at 8 elements or fewer, the smaller part first), written for Rust's
//! slices, so that it leaves equal elements as `qsort` does.

/// Sorts `v` as the UCRT's `qsort` sorts it, by `cmp` (negative, zero, or
/// positive, as `qsort`'s comparison function returns).
pub fn qsort<T>(v: &mut [T], cmp: impl Fn(&T, &T) -> i32) {
    const CUTOFF: usize = 8;
    if v.len() < 2 {
        return;
    }
    let c = |v: &[T], a: usize, b: usize| cmp(&v[a], &v[b]);
    // Sub-arrays yet to sort (inclusive bounds).
    let mut stack: Vec<(usize, usize)> = Vec::new();
    let mut lo = 0usize;
    let mut hi = v.len() - 1;
    loop {
        let size = hi - lo + 1;
        if size <= CUTOFF {
            short_sort(v, lo, hi, &c);
        } else {
            let mut mid = lo + size / 2;
            // The first, middle, and last into order.
            if c(v, lo, mid) > 0 {
                v.swap(lo, mid);
            }
            if c(v, lo, hi) > 0 {
                v.swap(lo, hi);
            }
            if c(v, mid, hi) > 0 {
                v.swap(mid, hi);
            }
            // Partition: [lo, loguy) <= mid, (higuy, hi) > mid.
            let mut loguy = lo;
            let mut higuy = hi;
            loop {
                if mid > loguy {
                    loop {
                        loguy += 1;
                        if !(loguy < mid && c(v, loguy, mid) <= 0) {
                            break;
                        }
                    }
                }
                if mid <= loguy {
                    loop {
                        loguy += 1;
                        if !(loguy <= hi && c(v, loguy, mid) <= 0) {
                            break;
                        }
                    }
                }
                loop {
                    higuy -= 1;
                    if !(higuy > mid && c(v, higuy, mid) > 0) {
                        break;
                    }
                }
                if higuy < loguy {
                    break;
                }
                v.swap(loguy, higuy);
                if mid == higuy {
                    mid = loguy;
                }
            }
            // Skip the elements equal to the partition element.
            higuy += 1;
            if mid < higuy {
                loop {
                    higuy -= 1;
                    if !(higuy > mid && c(v, higuy, mid) == 0) {
                        break;
                    }
                }
            }
            if mid >= higuy {
                loop {
                    higuy -= 1;
                    if !(higuy > lo && c(v, higuy, mid) == 0) {
                        break;
                    }
                }
            }
            // The smaller part first; the larger saved.
            if higuy - lo >= hi - loguy.min(hi) || loguy > hi {
                if lo < higuy {
                    stack.push((lo, higuy));
                }
                if loguy < hi {
                    lo = loguy;
                    continue;
                }
            } else {
                if loguy < hi {
                    stack.push((loguy, hi));
                }
                if lo < higuy {
                    hi = higuy;
                    continue;
                }
            }
        }
        match stack.pop() {
            Some((l, h)) => {
                lo = l;
                hi = h;
            }
            None => return,
        }
    }
}

/// The UCRT's insertion sort for short runs: the largest to the end,
/// repeatedly (the first of equal largest stays put; a later one moves).
fn short_sort<T>(v: &mut [T], lo: usize, mut hi: usize, c: &impl Fn(&[T], usize, usize) -> i32) {
    while hi > lo {
        let mut max = lo;
        for p in lo + 1..=hi {
            if c(v, p, max) > 0 {
                max = p;
            }
        }
        v.swap(max, hi);
        hi -= 1;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// What the UCRT's `qsort` (14.52's) does with these keys: each element's
    /// original place, after the sort (from a program built with cl and run).
    #[test]
    fn as_the_ucrt_sorts() {
        let cases: &[(&str, &[usize])] = &[
            ("0000", &[1, 2, 3, 0]),
            ("000000000", &[0, 1, 2, 3, 4, 5, 6, 7, 8]),
            (
                "0000000000000000000",
                &[
                    0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15, 16, 17, 18,
                ],
            ),
            ("1100100001001", &[6, 10, 2, 3, 8, 5, 11, 7, 9, 1, 0, 4, 12]),
            (
                "3122130211230312201",
                &[
                    12, 17, 6, 4, 14, 1, 9, 18, 8, 7, 10, 2, 16, 15, 3, 5, 11, 0, 13,
                ],
            ),
            (
                "00000000001111111111222222222",
                &[
                    0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15, 16, 17, 18, 19, 20, 21,
                    22, 23, 24, 25, 26, 27, 28,
                ],
            ),
            (
                "21021021021021021021021021021021021",
                &[
                    17, 20, 2, 32, 26, 5, 14, 11, 8, 29, 23, 7, 28, 13, 31, 4, 16, 34, 25, 19, 1,
                    10, 22, 21, 24, 18, 15, 27, 12, 9, 30, 6, 3, 33, 0,
                ],
            ),
        ];
        for (keys, want) in cases {
            let mut v: Vec<(i32, usize)> = keys
                .bytes()
                .enumerate()
                .map(|(i, b)| (i32::from(b - b'0'), i))
                .collect();
            qsort(&mut v, |a, b| a.0 - b.0);
            let got: Vec<usize> = v.iter().map(|e| e.1).collect();
            assert_eq!(&got, want, "{keys}");
        }
    }
}
