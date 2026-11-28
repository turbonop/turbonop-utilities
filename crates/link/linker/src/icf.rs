//! Identical COMDAT folding (`/OPT:ICF`): COMDATs that are read-only
//! (code, constant data) and the same — contents, characteristics, name,
//! relocations, what those refer to, and their associative sections
//! (unwind data) — are kept once.
//!
//! As mold's ICF does: every candidate's own parts are hashed (in
//! parallel); then, round by round, each candidate's hash is mixed with
//! the hashes of the candidates its relocations refer to, until the
//! number of distinct hashes stops growing. Candidates with one final
//! hash (128 bits: two independent 64-bit hashes) are one class, kept as
//! its member that comes first in the image (its group, then the link's
//! order: deterministic).

use std::hash::{BuildHasher, Hasher};

use turbonop_hash::Map;

use crate::object::{Selection, SymbolKind, scn};
use crate::resolve::{Def, FileKind, NONE, Resolved};

/// A 128-bit digest.
type Digest = (u64, u64);

/// One file's candidates (its sections, in order) and each section's
/// associative sections.
struct Scan {
    candidates: Vec<u32>,
    children: Map<u32, Vec<u32>>,
}

/// How a relocation's target is described, for hashing: a candidate (by
/// its index, and the offset into it), or anything else by identity.
#[derive(Clone, Copy, Debug, Hash, PartialEq, Eq)]
enum Target {
    Candidate(u32, u32),
    Global(u32),
    Local(u32, u32, u32),
    Nowhere,
}

/// A hasher of two independent 64-bit hashes.
struct Hash2 {
    a: turbonop_hash::Hash,
    b: turbonop_hash::Hash,
}

impl Hash2 {
    fn new() -> Hash2 {
        let s = turbonop_hash::State::default();
        let mut a = s.build_hasher();
        let mut b = s.build_hasher();
        a.write_u64(0x5A5A_5A5A_1111_1111);
        b.write_u64(0xC3C3_C3C3_2222_2222);
        Hash2 { a, b }
    }
    fn bytes(&mut self, v: &[u8]) {
        self.a.write(v);
        self.b.write(v);
    }
    fn u64(&mut self, v: u64) {
        self.a.write_u64(v);
        self.b.write_u64(v);
    }
    fn digest(&self) -> Digest {
        (self.a.finish(), self.b.finish())
    }
}

/// The sections `/OPT:ICF` folds: for each, the section it is folded
/// into (both as (file, section)). `live` says which sections are in the
/// image.
pub fn fold(r: &Resolved, live: &[Vec<bool>]) -> Map<(u32, u32), (u32, u32)> {
    let started = std::time::Instant::now();
    // The candidates, in link order, and each one's associative sections:
    // file by file, in parallel.
    let mut base = Vec::with_capacity(r.files.len() + 1);
    let mut total = 0usize;
    for file in &r.files {
        base.push(total);
        if let FileKind::Object { object, .. } = &file.kind {
            total += object.sections.len();
        }
    }
    base.push(total);
    let scans: Vec<Scan> = turbonop_par::map_range(r.files.len(), 64, |f| {
        let mut candidates: Vec<u32> = Vec::new();
        let mut children: Map<u32, Vec<u32>> = Map::default();
        let FileKind::Object { object, .. } = &r.files[f].kind else {
            return Scan {
                candidates,
                children,
            };
        };
        // Only the C/C++ compiler's code folds (link.exe keeps an assembler's
        // functions apart, as identical as they are: libvcruntime's
        // __NLG_Dispatch2 and __NLG_Return2; their unwind data still folds).
        let compiled = object
            .comp_id
            .is_none_or(|id| (0x104..=0x109).contains(&(id >> 16)));
        for (s, sec) in object.sections.iter().enumerate() {
            if !live[f][s] {
                continue;
            }
            let Some((selection, _)) = sec.comdat else {
                continue;
            };
            let c = sec.characteristics;
            let read_only = c & scn::MEM_WRITE == 0;
            let code = c & scn::MEM_EXECUTE != 0 && compiled;
            // Unwind data folds too (link.exe shares identical .xdata);
            // other data doesn't (link.exe keeps every constant apart:
            // .00cfg's pointers, `static const` objects).
            let unwind = sec.name.starts_with(b".xdata");
            if let Selection::Associative(parent) = selection {
                // Debug information and /GUARD's tables don't count.
                let bookkeeping = sec.name.starts_with(b".debug")
                    || matches!(
                        sec.name,
                        b".gfids$y" | b".giats$y" | b".gljmp$y" | b".gehcont$y"
                    );
                if !bookkeeping {
                    children.entry(parent).or_default().push(crate::n32(s));
                }
                // Associative code folds too (C++ EH funclets, .text$x,
                // as link.exe folds them), and so does a function's other
                // read-only data (its C++ EH FuncInfo, in .rdata: libucrt's
                // $cppxdata$__acrt_getptd_head folds into an identical one,
                // though the functions don't), but not its .pdata entry.
                let executable = c & scn::MEM_EXECUTE != 0;
                if !read_only || bookkeeping || sec.name == b".pdata" || (executable && !code) {
                    continue;
                }
                candidates.push(crate::n32(s));
                continue;
            }
            // Of other read-only data, only vtables (a COMDAT named `??_7`):
            // link.exe folds identical ones (LLVM's CheckAllNode's and
            // CheckAnyNode's, whose destructors fold), and no other constant
            // (not `??_8`, `??_R`, `??_C`, `?x@@3QBHB`, however identical).
            let leader = object
                .comdat_leaders
                .get(s)
                .and_then(|&l| object.symbols.get(l as usize))
                .map_or(&b""[..], |sym| sym.name);
            let vtable = leader.starts_with(b"??_7");
            // (Nor a dynamic initializer or atexit destructor, `??__E` and
            // `??__F`, whatever its group: link.exe folds none of LLVM's
            // two thousand.)
            let dynamic = leader.starts_with(b"??__E") || leader.starts_with(b"??__F");
            if read_only && (code || unwind || vtable) && !dynamic {
                candidates.push(crate::n32(s));
            }
        }
        Scan {
            candidates,
            children,
        }
    });
    // Each section's candidate (by its number, `base[file] + section`).
    let mut index = vec![u32::MAX; total];
    let mut candidates: Vec<(u32, u32)> = Vec::new();
    for (f, scan) in scans.iter().enumerate() {
        for &s in &scan.candidates {
            index[base[f] + s as usize] = crate::n32(candidates.len());
            candidates.push((crate::n32(f), s));
        }
    }
    if candidates.len() < 2 {
        return Map::default();
    }

    // Where a relocation of file `f` to its symbol `k` goes.
    let target = |f: u32, k: u32| -> Target {
        let FileKind::Object {
            object, globals, ..
        } = &r.files[f as usize].kind
        else {
            return Target::Nowhere;
        };
        let at = |file: u32, section: u32, value: u32| {
            let c = base
                .get(file as usize)
                .and_then(|&b| index.get(b + section as usize))
                .copied()
                .unwrap_or(u32::MAX);
            if c == u32::MAX {
                Target::Local(file, section, value)
            } else {
                Target::Candidate(c, value)
            }
        };
        match globals.get(k as usize) {
            Some(&g) if g != NONE => {
                let mut g = g;
                for _ in 0..64 {
                    match r.globals[g as usize].def {
                        Def::Alias(t) => g = t,
                        Def::Section {
                            file,
                            section,
                            value,
                        } => return at(file, section, value),
                        _ => return Target::Global(g),
                    }
                }
                Target::Global(g)
            }
            _ => match object.symbols.get(k as usize).map(|s| s.kind) {
                Some(SymbolKind::Local { section, value }) => at(f, section, value),
                _ => Target::Nowhere,
            },
        }
    };

    // Each candidate's own parts, and the candidates it refers to.
    let gathered = started.elapsed();
    let parts = turbonop_par::map(&candidates, 64, |&(f, s)| {
        let FileKind::Object { object, .. } = &r.files[f as usize].kind else {
            return ((0, 0), Vec::new());
        };
        let sec = &object.sections[s as usize];
        let mut h = Hash2::new();
        // (The name without its `$` suffix: link.exe folds a .text$di
        // function into a .text$mn one.)
        let base = sec
            .name
            .iter()
            .position(|&b| b == b'$')
            .map_or(sec.name, |p| &sec.name[..p]);
        h.bytes(base);
        h.u64(u64::from(sec.characteristics & scn::OUTPUT_MASK));
        h.u64(u64::from(sec.size));
        h.bytes(sec.data);
        let mut edges = Vec::new();
        let kids: &[u32] = scans[f as usize]
            .children
            .get(&s)
            .map_or(&[], Vec::as_slice);
        let hash_relocations = |h: &mut Hash2,
                                sec: &crate::object::Section,
                                own: (u32, u32),
                                edges: &mut Vec<u32>| {
            h.u64(sec.relocations.len() as u64);
            for rel in sec.relocations {
                h.u64(u64::from(rel.virtual_address.get()));
                h.u64(u64::from(rel.kind.get()));
                match target(f, rel.symbol_table_index.get()) {
                    // A reference to the parent itself (a .pdata entry's
                    // function) is the same in every copy, as long as it
                    // names the parent the same way: link.exe keeps a copy
                    // whose .pdata names the function's own COMDAT symbol
                    // (a static function's, as cl writes it: LIBCMTD's
                    // ReadNoFence64, LLVM's vector<GPUEntry>::_Change_array)
                    // apart from one whose .pdata names a label in it
                    // (`$LN4`, an external function's).
                    Target::Candidate(c, v) if candidates[c as usize] == own => {
                        // (And by that symbol's kind: a static one isn't an
                        // external one.)
                        let k = rel.symbol_table_index.get();
                        let how = if object.comdat_leaders.get(own.1 as usize) == Some(&k) {
                            if object
                                .symbols
                                .get(k as usize)
                                .is_some_and(crate::object::Symbol::is_external)
                            {
                                8
                            } else {
                                7
                            }
                        } else {
                            1
                        };
                        h.u64(how);
                        h.u64(u64::from(v));
                    }
                    // Another candidate: by what it folds to (its digest,
                    // round by round), and by the kind of symbol that names
                    // it: link.exe keeps a jump through an external symbol
                    // apart from one through a static symbol, even when the
                    // targets fold (LLVM's static get_deleter).
                    Target::Candidate(c, v) => {
                        let external = object
                            .symbols
                            .get(rel.symbol_table_index.get() as usize)
                            .is_some_and(crate::object::Symbol::is_external);
                        h.u64(if external { 2 } else { 9 });
                        h.u64(u64::from(v));
                        edges.push(c);
                    }
                    Target::Global(g) => {
                        h.u64(3);
                        h.u64(u64::from(g));
                    }
                    // One of the copy's own associative sections (its
                    // .pdata's .xdata): by its place among them, the same
                    // in every copy.
                    Target::Local(file, section, value) if file == f && kids.contains(&section) => {
                        h.u64(6);
                        h.u64(kids.iter().position(|&k| k == section).unwrap_or(0) as u64);
                        h.u64(u64::from(value));
                    }
                    Target::Local(file, section, value) => {
                        h.u64(4);
                        h.u64(u64::from(file));
                        h.u64(u64::from(section));
                        h.u64(u64::from(value));
                    }
                    Target::Nowhere => h.u64(5),
                }
            }
        };
        hash_relocations(&mut h, sec, (f, s), &mut edges);
        // The associative sections (unwind data), in order.
        {
            for &k in kids {
                let kid = &object.sections[k as usize];
                h.bytes(kid.name);
                h.u64(u64::from(kid.size));
                h.bytes(kid.data);
                hash_relocations(&mut h, kid, (f, s), &mut edges);
            }
        }
        (h.digest(), edges)
    });
    let (digests, edges): (Vec<Digest>, Vec<Vec<u32>>) = parts.into_iter().unzip();
    let hashed = started.elapsed();

    // The classes: candidates with the same digest, then split, round by
    // round, by the classes their targets are in, until none splits (as
    // mixing each digest with its targets' digests, round by round, would,
    // but only the classes of two or more are looked at again, by their
    // targets' classes exactly). A class is a run of `order`, its number
    // the run's start.
    let n = candidates.len();
    // (Sorted by digest in buckets, by the digest's low byte, in parallel:
    // where a class's run is doesn't matter, only what's in it.)
    const BUCKETS: usize = 256;
    let piece = n.div_ceil(turbonop_par::threads() * 4).max(4096);
    let scattered = turbonop_par::map_range(n.div_ceil(piece), 1, |p| {
        let mut b: Vec<Vec<u32>> = vec![Vec::new(); BUCKETS];
        for i in p * piece..n.min((p + 1) * piece) {
            b[usize::from(digests[i].0.to_le_bytes()[0])].push(crate::n32(i));
        }
        b
    });
    let buckets = turbonop_par::map_range(BUCKETS, 1, |k| {
        let mut v: Vec<u32> = scattered
            .iter()
            .flat_map(|b| b[k].iter().copied())
            .collect();
        v.sort_unstable_by_key(|&i| (digests[i as usize], i));
        v
    });
    drop(scattered);
    let mut order: Vec<u32> = Vec::with_capacity(n);
    for b in buckets {
        order.extend(b);
    }
    let mut class = vec![0u32; n];
    let mut runs: Vec<(u32, u32)> = Vec::new();
    let mut at = 0;
    while at < n {
        let mut end = at + 1;
        while end < n && digests[order[end] as usize] == digests[order[at] as usize] {
            end += 1;
        }
        for &i in &order[at..end] {
            class[i as usize] = crate::n32(at);
        }
        if end - at >= 2 {
            runs.push((crate::n32(at), crate::n32(end)));
        }
        at = end;
    }
    drop(digests);
    let classed = started.elapsed();
    let mut rounds = 0;
    while !runs.is_empty() {
        rounds += 1;
        // Each run's members by their targets' classes (then by number),
        // and where it splits.
        // Whether candidates `i` and `j`'s targets are in the same classes.
        let same = |i: u32, j: u32| {
            let (x, y) = (&edges[i as usize], &edges[j as usize]);
            x.len() == y.len()
                && x.iter()
                    .zip(y)
                    .all(|(&p, &q)| class[p as usize] == class[q as usize])
        };
        // (In parallel only when there's much to do: a round is little work,
        // and a parallel step waits for its slowest thread, which on a busy
        // machine costs more than the round.)
        let split = turbonop_par::map(&runs, 1 << 16, |&(a, b)| {
            // (Most classes don't split: seen at a glance.)
            let members = &order[a as usize..b as usize];
            if members[1..].iter().all(|&i| same(i, members[0])) {
                return (a, b, Vec::new(), Vec::new());
            }
            let mut keyed: Vec<(Vec<u32>, u32)> = order[a as usize..b as usize]
                .iter()
                .map(|&i| {
                    let key = edges[i as usize]
                        .iter()
                        .map(|&t| class[t as usize])
                        .collect();
                    (key, i)
                })
                .collect();
            keyed.sort_unstable();
            let mut cuts = Vec::new();
            for k in 1..keyed.len() {
                if keyed[k].0 != keyed[k - 1].0 {
                    cuts.push(a + crate::n32(k));
                }
            }
            let members: Vec<u32> = keyed.into_iter().map(|(_, i)| i).collect();
            (a, b, members, cuts)
        });
        // A round that splits nothing is the last; otherwise every class of
        // two or more goes round again (one whose targets split may split).
        let mut next = Vec::new();
        let mut any = false;
        for (a, b, members, cuts) in split {
            if cuts.is_empty() {
                next.push((a, b));
                continue;
            }
            any = true;
            order[a as usize..b as usize].copy_from_slice(&members);
            let mut start = a;
            for end in cuts.into_iter().chain(std::iter::once(b)) {
                for &i in &order[start as usize..end as usize] {
                    class[i as usize] = start;
                }
                if end - start >= 2 {
                    next.push((start, end));
                }
                start = end;
            }
        }
        if !any {
            break;
        }
        runs = next;
    }

    let refined = started.elapsed();
    // Each class's leader: its member first in the image's order, as
    // link.exe keeps (its group, by name, then the link's order: a
    // .text$di copy before a .text$mn one).
    let name = |i: usize| {
        let (f, s) = candidates[i];
        match &r.files[f as usize].kind {
            FileKind::Object { object, .. } => object.sections[s as usize].name,
            FileKind::Import { .. } => &b""[..],
        }
    };
    let mut classes: Vec<(u32, u32)> = Vec::new();
    let mut at = 0;
    while at < n {
        let c = class[order[at] as usize];
        let mut end = at + 1;
        while end < n && class[order[end] as usize] == c {
            end += 1;
        }
        if end - at >= 2 {
            classes.push((crate::n32(at), crate::n32(end)));
        }
        at = end;
    }
    let leaders = turbonop_par::map(&classes, 64, |&(a, b)| {
        let members = &order[a as usize..b as usize];
        members
            .iter()
            .copied()
            .min_by(|&x, &y| (name(x as usize), x).cmp(&(name(y as usize), y)))
            .unwrap_or(members[0])
    });
    let mut folded =
        turbonop_hash::map_with_capacity(classes.iter().map(|&(a, b)| (b - a) as usize).sum());
    for (&(a, b), leader) in classes.iter().zip(leaders) {
        for &i in &order[a as usize..b as usize] {
            if i != leader {
                folded.insert(candidates[i as usize], candidates[leader as usize]);
            }
        }
    }
    if r.config.time {
        eprintln!(
            "turbolink:     ICF: {} candidates gathered {:.3} ms, hashed {:.3} ms, classes {:.3} ms, {rounds} rounds {:.3} ms, leaders {:.3} ms",
            candidates.len(),
            gathered.as_secs_f64() * 1e3,
            (hashed - gathered).as_secs_f64() * 1e3,
            (classed - hashed).as_secs_f64() * 1e3,
            (refined - classed).as_secs_f64() * 1e3,
            (started.elapsed() - refined).as_secs_f64() * 1e3,
        );
    }
    folded
}
