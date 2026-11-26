//! Tests of the grouping's cost model and search.

use super::*;

/// A small deterministic generator (xorshift).
struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }

    fn below(&mut self, n: u64) -> u64 {
        self.next() % n
    }
}

/// `n` members, random classes, and closures from a random load graph.
fn instance(r: &mut Rng, n: usize) -> (Vec<Class>, Vec<Vec<u32>>) {
    let mut classes = Vec::new();
    for _ in 0..r.below(40) + 5 {
        let mut holders: Vec<u32> = (0..n as u32).filter(|_| r.below(3) == 0).collect();
        while holders.len() < 2 {
            holders.push(r.below(n as u64) as u32);
            holders.sort_unstable();
            holders.dedup();
        }
        classes.push(Class {
            bytes: r.below(1000) + 1,
            holders,
        });
    }
    let edges: Vec<Vec<u32>> = (0..n)
        .map(|_| (0..n as u32).filter(|_| r.below(5) == 0).collect())
        .collect();
    let closures = (0..n)
        .map(|s| {
            let mut seen = vec![false; n];
            let mut stack = vec![s as u32];
            seen[s] = true;
            while let Some(x) = stack.pop() {
                for &y in &edges[x as usize] {
                    if !seen[y as usize] {
                        seen[y as usize] = true;
                        stack.push(y);
                    }
                }
            }
            (0..n as u32).filter(|&p| seen[p as usize]).collect()
        })
        .collect();
    (classes, closures)
}

/// Every move's computed change in score is what recomputing gives, and
/// the state kept up to date is what building it again gives.
#[test]
fn deltas_are_exact() {
    let mut r = Rng(0x9E37_79B9_7F4A_7C15);
    for case in 0..200 {
        let n = 2 + case % 9;
        let (classes, closures) = instance(&mut r, n);
        let t = 1 + case % 4;
        let min = 2 + (case % 3 == 0) as u32;
        let group: Vec<u32> = (0..n)
            .map(|_| match r.below(t as u64 + 1) {
                x if x == t as u64 => OUT,
                x => x as u32,
            })
            .collect();
        let mut st = State::new(&classes, &closures, min, 1 + case as u64 % 5, t, group);
        let mut d = Vec::new();
        for _ in 0..30 {
            let p = r.below(n as u64) as usize;
            st.deltas(p, &mut d);
            let z = r.below(t as u64 + 1) as usize;
            let before = st.cost().score() as i128;
            let target = if z == t { OUT } else { z as u32 };
            st.apply(p, target);
            let after = st.cost();
            assert_eq!(
                after.score() as i128 - before,
                d[z],
                "case {case}: move {p} to {z}"
            );
            let fresh = State::new(
                &classes,
                &closures,
                min,
                1 + case as u64 % 5,
                t,
                st.group.clone(),
            )
            .cost();
            assert_eq!(after, fresh, "case {case}");
        }
    }
}

/// One group and every member out cost what `--precomp=library` and
/// `none` hold: each class once, and each class once per holder.
#[test]
fn library_and_none() {
    let classes = vec![
        Class {
            bytes: 100,
            holders: vec![0, 1],
        },
        Class {
            bytes: 10,
            holders: vec![0, 1, 2],
        },
    ];
    // 0 loads 1; 1 and 2 load only themselves.
    let closures = vec![vec![0, 1], vec![1], vec![2]];
    let c = choose(3, &classes, &closures, 2, 1, &[1]);
    assert_eq!(c.library.library, 110);
    assert_eq!(c.none.library, 230);
    // One group: every seed reads 110.
    assert_eq!(c.library.reads, 330);
    // None: seed 0 reads 0's and 1's copies (220), 1 reads 110, 2 reads 10.
    assert_eq!(c.none.reads, 340);
}

/// Two parts of a library that nothing loads together, sharing a common
/// header between them and much within each, get a group each; a member
/// sharing almost nothing stays out.
#[test]
fn separate_parts_get_separate_groups() {
    let mut classes = vec![Class {
        bytes: 10,
        holders: (0..8).collect(),
    }];
    // Within part 0 (members 0 to 3) and part 1 (4 to 7).
    classes.push(Class {
        bytes: 5000,
        holders: vec![0, 1, 2, 3],
    });
    classes.push(Class {
        bytes: 5000,
        holders: vec![4, 5, 6, 7],
    });
    // Member 8 shares a little with everyone.
    classes.push(Class {
        bytes: 1,
        holders: (0..9).collect(),
    });
    // Each part's members load each other.
    let closures: Vec<Vec<u32>> = (0..9)
        .map(|s| match s {
            0..4 => (0..4).collect(),
            4..8 => (4..8).collect(),
            _ => vec![8],
        })
        .collect();
    let c = choose(9, &classes, &closures, 2, 1, &[1, 2, 4]);
    assert_eq!(c.groups, 2, "{c:?}");
    assert!(c.group[..4].iter().all(|&g| g == c.group[0] && g.is_some()));
    assert!(
        c.group[4..8]
            .iter()
            .all(|&g| g == c.group[4] && g.is_some())
    );
    assert_ne!(c.group[0], c.group[4]);
    assert!(c.cost.score() < c.library.score());
}

/// Members that every link loads together are best in one group.
#[test]
fn loaded_together_stay_together() {
    let classes = vec![
        Class {
            bytes: 300,
            holders: vec![0, 1, 2, 3],
        },
        Class {
            bytes: 200,
            holders: vec![0, 1],
        },
        Class {
            bytes: 200,
            holders: vec![2, 3],
        },
    ];
    let all: Vec<u32> = (0..4).collect();
    let closures = vec![all.clone(); 4];
    let c = choose(4, &classes, &closures, 2, 1, &[1, 2, 4]);
    assert_eq!(c.groups, 1, "{c:?}");
    assert_eq!(c.cost, c.library);
}

/// The same instance gives the same choice.
#[test]
fn deterministic() {
    let mut r = Rng(42);
    let (classes, closures) = instance(&mut r, 12);
    let a = choose(12, &classes, &closures, 2, 1, &[1, 2, 4, 8]);
    let b = choose(12, &classes, &closures, 2, 1, &[1, 2, 4, 8]);
    assert_eq!(a.group, b.group);
    assert_eq!(a.cost, b.cost);
    assert!(a.cost.score() <= a.library.score());
}
