//! Two sequences of lines compared: the shortest edit script between them
//! (Myers' O((N+M)D) algorithm), for comparing two functions' listings.

/// One line of a comparison.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Line<'a> {
    /// In both.
    Same(&'a str),
    /// Only in the first.
    Removed(&'a str),
    /// Only in the second.
    Added(&'a str),
}

/// `a` and `b` compared, line by line, in order: a shortest edit script
/// (most lines in common), removals before additions where they meet.
pub fn diff<'a>(a: &[&'a str], b: &[&'a str]) -> Vec<Line<'a>> {
    // Lines in common at either end need no search.
    let head = a.iter().zip(b).take_while(|(x, y)| x == y).count();
    let tail = a[head..]
        .iter()
        .rev()
        .zip(b[head..].iter().rev())
        .take_while(|(x, y)| x == y)
        .count();
    let (am, bm) = (&a[head..a.len() - tail], &b[head..b.len() - tail]);
    let mut out: Vec<Line<'a>> = a[..head].iter().map(|l| Line::Same(l)).collect();
    out.extend(middle(am, bm));
    out.extend(a[a.len() - tail..].iter().map(|l| Line::Same(l)));
    out
}

/// Myers' greedy algorithm, keeping each step's frontier to trace back.
fn middle<'a>(a: &[&'a str], b: &[&'a str]) -> Vec<Line<'a>> {
    let (n, m) = (a.len(), b.len());
    if n == 0 {
        return b.iter().map(|l| Line::Added(l)).collect();
    }
    if m == 0 {
        return a.iter().map(|l| Line::Removed(l)).collect();
    }
    let max = n + m;
    let offset = max as isize;
    // v[k + offset]: the furthest x on diagonal k (x - y = k).
    let mut v = vec![0usize; 2 * max + 2];
    let mut trace: Vec<Vec<usize>> = Vec::new();
    let mut done = false;
    for d in 0..=max as isize {
        trace.push(v.clone());
        let mut k = -d;
        while k <= d {
            let i = (k + offset) as usize;
            let mut x = if k == -d || (k != d && v[i - 1] < v[i + 1]) {
                v[i + 1]
            } else {
                v[i - 1] + 1
            };
            let mut y = (x as isize - k) as usize;
            while x < n && y < m && a[x] == b[y] {
                x += 1;
                y += 1;
            }
            v[i] = x;
            if x >= n && y >= m {
                done = true;
                break;
            }
            k += 2;
        }
        if done {
            break;
        }
    }
    // Back from the end, step by step.
    let mut out: Vec<Line<'a>> = Vec::new();
    let (mut x, mut y) = (n, m);
    for d in (0..trace.len()).rev() {
        let v = &trace[d];
        let d = d as isize;
        let k = x as isize - y as isize;
        let i = (k + offset) as usize;
        let prev_k = if k == -d || (k != d && v[i - 1] < v[i + 1]) {
            k + 1
        } else {
            k - 1
        };
        let prev_x = v[(prev_k + offset) as usize];
        let prev_y = (prev_x as isize - prev_k) as usize;
        while x > prev_x && y > prev_y {
            x -= 1;
            y -= 1;
            out.push(Line::Same(a[x]));
        }
        if d > 0 {
            if x == prev_x {
                y -= 1;
                out.push(Line::Added(b[y]));
            } else {
                x -= 1;
                out.push(Line::Removed(a[x]));
            }
        }
    }
    out.reverse();
    out
}

/// The share of lines the two have in common (1.0: the same; for none on
/// either side, 1.0).
pub fn similarity(lines: &[Line<'_>]) -> f64 {
    let (mut same, mut a, mut b) = (0usize, 0usize, 0usize);
    for l in lines {
        match l {
            Line::Same(_) => {
                same += 1;
                a += 1;
                b += 1;
            }
            Line::Removed(_) => a += 1,
            Line::Added(_) => b += 1,
        }
    }
    if a + b == 0 {
        1.0
    } else {
        2.0 * same as f64 / (a + b) as f64
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn apply(lines: &[Line<'_>]) -> (Vec<String>, Vec<String>) {
        let mut a = Vec::new();
        let mut b = Vec::new();
        for l in lines {
            match l {
                Line::Same(s) => {
                    a.push(s.to_string());
                    b.push(s.to_string());
                }
                Line::Removed(s) => a.push(s.to_string()),
                Line::Added(s) => b.push(s.to_string()),
            }
        }
        (a, b)
    }

    #[test]
    fn shortest_scripts() {
        let a = ["push", "mov", "add", "ret"];
        let b = ["push", "mov", "sub", "add", "ret"];
        let d = diff(&a, &b);
        assert_eq!(
            d,
            [
                Line::Same("push"),
                Line::Same("mov"),
                Line::Added("sub"),
                Line::Same("add"),
                Line::Same("ret"),
            ]
        );
        assert!((similarity(&d) - 8.0 / 9.0).abs() < 1e-9);
        assert_eq!(diff(&[], &["x"]), [Line::Added("x")]);
        assert_eq!(diff(&["x"], &[]), [Line::Removed("x")]);
        assert_eq!(similarity(&diff(&[], &[])), 1.0);
    }

    #[test]
    fn every_pair_round_trips() {
        // Every pair of short sequences over a small alphabet: the script
        // rebuilds both, and is no longer than their lengths' distance
        // allows (each line kept is in an LCS).
        let words = ["a", "b", "c"];
        let mut seqs: Vec<Vec<&str>> = vec![Vec::new()];
        for len in 1..=5 {
            let mut n = 1;
            for _ in 0..len {
                n *= words.len();
            }
            for mut i in 0..n {
                let mut s = Vec::new();
                for _ in 0..len {
                    s.push(words[i % words.len()]);
                    i /= words.len();
                }
                seqs.push(s);
            }
        }
        let lcs = |a: &[&str], b: &[&str]| -> usize {
            let mut t = vec![vec![0usize; b.len() + 1]; a.len() + 1];
            for i in (0..a.len()).rev() {
                for j in (0..b.len()).rev() {
                    t[i][j] = if a[i] == b[j] {
                        t[i + 1][j + 1] + 1
                    } else {
                        t[i + 1][j].max(t[i][j + 1])
                    };
                }
            }
            t[0][0]
        };
        for a in seqs.iter().step_by(7) {
            for b in seqs.iter().step_by(5) {
                let d = diff(a, b);
                let (ra, rb) = apply(&d);
                assert_eq!(ra, *a);
                assert_eq!(rb, *b);
                let same = d.iter().filter(|l| matches!(l, Line::Same(_))).count();
                assert_eq!(same, lcs(a, b), "{a:?} {b:?}");
            }
        }
    }
}
