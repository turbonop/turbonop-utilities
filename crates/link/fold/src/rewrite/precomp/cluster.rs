//! Grouping a machine's sharing members for `--precomp=clustered`
//! (phase 3.5): each member names one `LF_PRECOMP`, so each is in one
//! group, and each group with records its members share gets its own
//! precompiled-types member.
//!
//! **The cost model.** A link that loads any member of a group merges all
//! of that group's precompiled-types member (lld-link puts every record of
//! it in the PDB; link.exe reads it all). Which members links load isn't
//! known to turbolib, but what each member needs from the library is: a
//! link that loads member `s` loads every member that defines (as the
//! archive index's first definer) a name `s` refers to and doesn't define
//! itself, and so on, its *closure*. Each sharing member is taken as one
//! such link (a *seed*), all weighed the same.
//!
//! Records are counted in *classes*: the records held by the same set of
//! members behave alike under every grouping, so a class is its members
//! and its records' bytes. Under a grouping, a class's records are in a
//! group's precompiled-types member when at least `min` of its members are
//! in the group, and otherwise in each of those members (a member *out* of
//! every group keeps all of its records). Then:
//!
//! - `S`, the library's bytes of those records: each class's bytes once
//!   for each group sharing it, and once for each member keeping a copy;
//! - `C`, what the seeds' links read of them, summed over seeds: each
//!   group's precompiled-types bytes once for each seed whose closure
//!   holds a member of the group, and each kept copy once for each seed
//!   whose closure holds its member.
//!
//! A grouping's score is `seeds × S + C`: the library's bytes, plus what an
//! average one-member link reads from it, weighed alike. Lower is better.
//! (One group is `--precomp=library`; every member out is `none`.)
//!
//! **The search.** For each number of groups tried, the members start in
//! groups by what loads them (the members whose loaders most differ from
//! the chosen ones head new groups; each member joins the head whose
//! loaders are most like its own), and then each member in turn moves to
//! the group, or out, that lowers the score most, by an exact incremental
//! computation, until no move lowers it (or a bound on sweeps). The
//! lowest-scoring result wins, fewer groups on a tie. Everything is in
//! integers and in member order, so the result is the same for any number
//! of threads.

/// The records a set of members holds alike: their bytes, and the members
/// (positions among the machine's sharers, ascending, at least two).
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct Class {
    pub bytes: u64,
    pub holders: Vec<u32>,
}

/// What a grouping costs (see the module's notes).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Cost {
    /// The library's bytes of the shareable records.
    pub library: u64,
    /// What the seeds' links read of them, in all.
    pub reads: u64,
    pub seeds: u64,
}

impl Cost {
    pub fn score(&self) -> u128 {
        u128::from(self.seeds) * u128::from(self.library) + u128::from(self.reads)
    }

    /// What an average one-member link reads.
    pub fn per_link(&self) -> u64 {
        self.reads.checked_div(self.seeds).unwrap_or(0)
    }
}

/// The grouping chosen for one machine's sharers.
#[derive(Clone, Debug)]
pub(super) struct Choice {
    /// Each sharer's group (`0..groups`), or `None` if it is out.
    pub group: Vec<Option<u32>>,
    pub groups: u32,
    /// Its cost, and one group's (`--precomp=library`'s) and every member
    /// out's (`none`'s) under the same model.
    pub cost: Cost,
    pub library: Cost,
    pub none: Cost,
    /// Each number of groups tried, with its cost.
    pub tried: Vec<(u32, Cost)>,
}

/// The search's bound on sweeps over the members.
const SWEEPS: u32 = 32;

const OUT: u32 = u32::MAX;

/// A grouping and what it costs, kept up to date as members move.
struct State<'x> {
    t: usize,
    min: u32,
    classes: &'x [Class],
    /// Each member's classes (CSR).
    cls_start: Vec<u32>,
    cls: Vec<u32>,
    /// Each member's loaders: the seeds whose closure holds it (CSR).
    ld_start: Vec<u32>,
    ld: Vec<u32>,
    group: Vec<u32>,
    /// By class and group: its members there, and the sum of their
    /// loaders' counts.
    cnt: Vec<u32>,
    sld: Vec<u64>,
    /// By group: its precompiled-types bytes, and the seeds touching it.
    pch: Vec<u64>,
    touch: Vec<u64>,
    /// By seed and group: the members of the group its closure holds.
    loaded: Vec<u32>,
    seeds: u64,
}

fn csr(n: usize, pairs: impl Iterator<Item = (u32, u32)> + Clone) -> (Vec<u32>, Vec<u32>) {
    let mut start = vec![0u32; n + 1];
    for (a, _) in pairs.clone() {
        start[a as usize + 1] += 1;
    }
    for i in 0..n {
        start[i + 1] += start[i];
    }
    let mut next = start.clone();
    let mut out = vec![0u32; start[n] as usize];
    for (a, b) in pairs {
        out[next[a as usize] as usize] = b;
        next[a as usize] += 1;
    }
    (start, out)
}

impl<'x> State<'x> {
    fn new(
        classes: &'x [Class],
        closures: &[Vec<u32>],
        min: u32,
        t: usize,
        group: Vec<u32>,
    ) -> State<'x> {
        let n = group.len();
        let (cls_start, cls) = csr(
            n,
            classes.iter().enumerate().flat_map(|(c, k)| {
                k.holders.iter().map(move |&p| (p, c as u32))
            }),
        );
        let (ld_start, ld) = csr(
            n,
            closures
                .iter()
                .enumerate()
                .flat_map(|(s, cl)| cl.iter().map(move |&p| (p, s as u32))),
        );
        let mut st = State {
            t,
            min,
            classes,
            cls_start,
            cls,
            ld_start,
            ld,
            group,
            cnt: vec![0; classes.len() * t],
            sld: vec![0; classes.len() * t],
            pch: vec![0; t],
            touch: vec![0; t],
            loaded: vec![0; closures.len() * t],
            seeds: closures.len() as u64,
        };
        for (c, k) in classes.iter().enumerate() {
            for &p in &k.holders {
                let g = st.group[p as usize];
                if g != OUT {
                    st.cnt[c * t + g as usize] += 1;
                    st.sld[c * t + g as usize] += st.lds(p as usize);
                }
            }
            for g in 0..t {
                if st.cnt[c * t + g] >= min {
                    st.pch[g] += k.bytes;
                }
            }
        }
        for (s, cl) in closures.iter().enumerate() {
            for &p in cl {
                let g = st.group[p as usize];
                if g != OUT {
                    st.loaded[s * t + g as usize] += 1;
                }
            }
            for g in 0..t {
                st.touch[g] += u64::from(st.loaded[s * t + g] > 0);
            }
        }
        st
    }

    /// How many seeds load member `p`.
    fn lds(&self, p: usize) -> u64 {
        u64::from(self.ld_start[p + 1] - self.ld_start[p])
    }

    fn classes_of(&self, p: usize) -> &[u32] {
        &self.cls[self.cls_start[p] as usize..self.cls_start[p + 1] as usize]
    }

    fn loaders_of(&self, p: usize) -> &[u32] {
        &self.ld[self.ld_start[p] as usize..self.ld_start[p + 1] as usize]
    }

    /// A class's copies in a group of `n` of its members: one if they
    /// share it, else one each.
    fn copies(&self, n: u32) -> u64 {
        match n {
            0 => 0,
            n if n >= self.min => 1,
            n => u64::from(n),
        }
    }

    /// What the seeds read of a class's copies kept in a group's `n`
    /// members, whose loaders number `sld`.
    fn kept(&self, n: u32, sld: u64) -> u64 {
        if n >= 1 && n < self.min { sld } else { 0 }
    }

    /// The cost, computed from scratch.
    fn cost(&self) -> Cost {
        let mut library = 0u64;
        let mut reads = 0u64;
        for (c, k) in self.classes.iter().enumerate() {
            for g in 0..self.t {
                let n = self.cnt[c * self.t + g];
                library += k.bytes * self.copies(n);
                reads += k.bytes * self.kept(n, self.sld[c * self.t + g]);
            }
            for &p in &k.holders {
                if self.group[p as usize] == OUT {
                    library += k.bytes;
                    reads += k.bytes * self.lds(p as usize);
                }
            }
        }
        for g in 0..self.t {
            reads += self.pch[g] * self.touch[g];
        }
        Cost {
            library,
            reads,
            seeds: self.seeds,
        }
    }

    /// The change in score of moving member `p` to each group (`0..t`)
    /// and out (`t`).
    fn deltas(&self, p: usize, out: &mut Vec<i128>) {
        let t = self.t;
        let a = self.group[p];
        let ld = self.lds(p);
        let seeds = i128::from(self.seeds);
        // Leaving a: library and kept-reads changes, and its PCH's bytes.
        let mut d_lib_a: i128 = 0;
        let mut d_kept_a: i128 = 0;
        let mut d_pch_a: i128 = 0;
        // Joining each z (and out, at t).
        let mut d_lib = vec![0i128; t + 1];
        let mut d_kept = vec![0i128; t + 1];
        let mut d_pch = vec![0i128; t];
        for &c in self.classes_of(p) {
            let c = c as usize;
            let b = i128::from(self.classes[c].bytes);
            if a == OUT {
                d_lib_a -= b;
                d_kept_a -= b * i128::from(ld);
            } else {
                let i = c * t + a as usize;
                let (n, sl) = (self.cnt[i], self.sld[i]);
                d_lib_a += b * (self.copies(n - 1) as i128 - self.copies(n) as i128);
                d_kept_a +=
                    b * (self.kept(n - 1, sl - ld) as i128 - self.kept(n, sl) as i128);
                d_pch_a += b
                    * (i128::from(n > self.min) - i128::from(n >= self.min));
            }
            for z in 0..t {
                if z as u32 == a {
                    continue;
                }
                let i = c * t + z;
                let (n, sl) = (self.cnt[i], self.sld[i]);
                d_lib[z] += b * (self.copies(n + 1) as i128 - self.copies(n) as i128);
                d_kept[z] +=
                    b * (self.kept(n + 1, sl + ld) as i128 - self.kept(n, sl) as i128);
                d_pch[z] += b * (i128::from(n + 1 >= self.min) - i128::from(n >= self.min));
            }
            d_lib[t] += b;
            d_kept[t] += b * i128::from(ld);
        }
        // The seeds that stop touching a, and start touching each z.
        let mut d_touch_a: i128 = 0;
        let mut d_touch = vec![0i128; t];
        for &s in self.loaders_of(p) {
            let row = &self.loaded[s as usize * t..s as usize * t + t];
            if a != OUT && row[a as usize] == 1 {
                d_touch_a -= 1;
            }
            for z in 0..t {
                if row[z] == 0 {
                    d_touch[z] += 1;
                }
            }
        }
        let group_reads = |g: usize, dp: i128, dt: i128| -> i128 {
            let (p0, t0) = (i128::from(self.pch[g]), i128::from(self.touch[g]));
            (p0 + dp) * (t0 + dt) - p0 * t0
        };
        let leave = seeds * d_lib_a
            + d_kept_a
            + if a == OUT {
                0
            } else {
                group_reads(a as usize, d_pch_a, d_touch_a)
            };
        out.clear();
        for z in 0..=t {
            if z as u32 == a || (z == t && a == OUT) {
                out.push(0);
                continue;
            }
            let join = seeds * d_lib[z]
                + d_kept[z]
                + if z == t {
                    0
                } else {
                    group_reads(z, d_pch[z], d_touch[z])
                };
            out.push(leave + join);
        }
    }

    /// Moves member `p` to group `z` (`OUT` for out).
    fn apply(&mut self, p: usize, z: u32) {
        let t = self.t;
        let a = self.group[p];
        let ld = self.lds(p);
        let (start, end) = (self.cls_start[p] as usize, self.cls_start[p + 1] as usize);
        for k in start..end {
            let c = self.cls[k] as usize;
            let b = self.classes[c].bytes;
            if a != OUT {
                let i = c * t + a as usize;
                if self.cnt[i] == self.min {
                    self.pch[a as usize] -= b;
                }
                self.cnt[i] -= 1;
                self.sld[i] -= ld;
            }
            if z != OUT {
                let i = c * t + z as usize;
                self.cnt[i] += 1;
                self.sld[i] += ld;
                if self.cnt[i] == self.min {
                    self.pch[z as usize] += b;
                }
            }
        }
        let (start, end) = (self.ld_start[p] as usize, self.ld_start[p + 1] as usize);
        for k in start..end {
            let s = self.ld[k] as usize;
            if a != OUT {
                let i = s * t + a as usize;
                self.loaded[i] -= 1;
                if self.loaded[i] == 0 {
                    self.touch[a as usize] -= 1;
                }
            }
            if z != OUT {
                let i = s * t + z as usize;
                if self.loaded[i] == 0 {
                    self.touch[z as usize] += 1;
                }
                self.loaded[i] += 1;
            }
        }
        self.group[p] = z;
    }

    /// Moves members while a move lowers the score.
    fn descend(&mut self) {
        let mut d = Vec::new();
        for _ in 0..SWEEPS {
            let mut moved = false;
            for p in 0..self.group.len() {
                self.deltas(p, &mut d);
                let mut best = (0i128, usize::MAX);
                for (z, &v) in d.iter().enumerate() {
                    if v < best.0 {
                        best = (v, z);
                    }
                }
                if best.1 != usize::MAX {
                    let z = if best.1 == self.t { OUT } else { best.1 as u32 };
                    self.apply(p, z);
                    moved = true;
                }
            }
            if !moved {
                break;
            }
        }
    }
}

/// Bitsets of each member's loaders, for the starting groups.
fn loader_sets(n: usize, closures: &[Vec<u32>]) -> (Vec<u64>, usize) {
    let words = closures.len().div_ceil(64).max(1);
    let mut bits = vec![0u64; n * words];
    for (s, cl) in closures.iter().enumerate() {
        for &p in cl {
            bits[p as usize * words + s / 64] |= 1 << (s % 64);
        }
    }
    (bits, words)
}

/// `|a ∩ b|` and `|a ∪ b|` of two bitsets.
fn overlap(a: &[u64], b: &[u64]) -> (u32, u32) {
    let mut i = 0;
    let mut u = 0;
    for (x, y) in a.iter().zip(b) {
        i += (x & y).count_ones();
        u += (x | y).count_ones();
    }
    (i, u)
}

/// Whether `a / b > c / d` (as fractions; `0 / 0` is 0).
fn greater(a: u32, b: u32, c: u32, d: u32) -> bool {
    u64::from(a) * u64::from(d.max(1)) > u64::from(c) * u64::from(b.max(1))
}

/// The starting groups for `t` groups: heads chosen by their loaders (the
/// most loaded member first, then each time the member whose loaders are
/// least like any head's), and each member in the group of the head whose
/// loaders are most like its own (the first such).
fn start(n: usize, closures: &[Vec<u32>], t: usize) -> Vec<u32> {
    let (bits, w) = loader_sets(n, closures);
    let set = |p: usize| &bits[p * w..p * w + w];
    let size = |p: usize| set(p).iter().map(|x| x.count_ones()).sum::<u32>();
    let mut heads = vec![(0..n).max_by_key(|&p| (size(p), std::cmp::Reverse(p))).unwrap_or(0)];
    // Each member's best similarity to a head so far (intersection, union).
    let mut best: Vec<(u32, u32)> = (0..n).map(|p| overlap(set(p), set(heads[0]))).collect();
    let mut group = vec![0u32; n];
    while heads.len() < t {
        // The least similar member (the first of those), not a head.
        let mut pick = None;
        for p in 0..n {
            if heads.contains(&p) {
                continue;
            }
            match pick {
                None => pick = Some(p),
                Some(q) => {
                    let (a, b) = best[p];
                    let (c, d) = best[q];
                    if greater(c, d, a, b) {
                        pick = Some(p);
                    }
                }
            }
        }
        let Some(h) = pick else { break };
        let g = heads.len() as u32;
        heads.push(h);
        for p in 0..n {
            let (a, b) = overlap(set(p), set(h));
            let (c, d) = best[p];
            if p == h || greater(a, b, c, d) {
                best[p] = (a, b);
                group[p] = g;
            }
        }
    }
    group
}

/// Chooses the groups of one machine's `n` sharers: `classes` are the
/// records they share (each held by two or more), `closures` each seed's
/// (each sharer's, in order) closure among them (positions, ascending,
/// itself included); `tries` the numbers of groups to try.
pub(super) fn choose(
    n: usize,
    classes: &[Class],
    closures: &[Vec<u32>],
    min: u32,
    tries: &[u32],
) -> Choice {
    let min = min.max(2);
    let library = State::new(classes, closures, min, 1, vec![0; n]).cost();
    let none = State::new(classes, closures, min, 1, vec![OUT; n]).cost();
    let mut tried = Vec::new();
    let mut chosen: Option<(Cost, Vec<u32>, u32)> = None;
    for &t in tries {
        let t = (t as usize).clamp(1, n.max(1));
        if tried.iter().any(|&(x, _)| x == t as u32) {
            continue;
        }
        let mut st = State::new(classes, closures, min, t, start(n, closures, t));
        st.descend();
        let cost = st.cost();
        tried.push((t as u32, cost));
        if chosen
            .as_ref()
            .is_none_or(|(c, _, _)| cost.score() < c.score())
        {
            chosen = Some((cost, st.group, t as u32));
        }
    }
    let (cost, raw, _) = chosen.unwrap_or((library, vec![0; n], 1));
    // Groups renumbered by their first member, empty ones dropped.
    let mut renumber: Vec<Option<u32>> = vec![None; raw.iter().filter(|&&g| g != OUT).map(|&g| g as usize + 1).max().unwrap_or(0)];
    let mut next = 0;
    let group: Vec<Option<u32>> = raw
        .iter()
        .map(|&g| {
            (g != OUT).then(|| {
                *renumber[g as usize].get_or_insert_with(|| {
                    next += 1;
                    next - 1
                })
            })
        })
        .collect();
    Choice {
        group,
        groups: next,
        cost,
        library,
        none,
        tried,
    }
}

#[cfg(test)]
mod tests;
