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
//! its member that comes first in the link's order (deterministic).

use std::hash::{BuildHasher, Hasher};

use turbonop_hash::Map;

use crate::object::{Selection, SymbolKind, scn};
use crate::resolve::{Def, FileKind, NONE, Resolved};

/// A 128-bit digest.
type Digest = (u64, u64);

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
    // The candidates, in link order; and each one's associative sections.
    let mut candidates: Vec<(u32, u32)> = Vec::new();
    let mut index: Map<(u32, u32), u32> = Map::default();
    let mut children: Map<(u32, u32), Vec<u32>> = Map::default();
    for (f, file) in r.files.iter().enumerate() {
        let FileKind::Object { object, .. } = &file.kind else {
            continue;
        };
        for (s, sec) in object.sections.iter().enumerate() {
            if !live[f][s] {
                continue;
            }
            let Some((selection, _)) = sec.comdat else {
                continue;
            };
            if let Selection::Associative(parent) = selection {
                // Debug information and /GUARD's tables don't count.
                if !sec.name.starts_with(b".debug")
                    && !matches!(
                        sec.name,
                        b".gfids$y" | b".giats$y" | b".gljmp$y" | b".gehcont$y"
                    )
                {
                    children
                        .entry((crate::n32(f), parent))
                        .or_default()
                        .push(crate::n32(s));
                }
                continue;
            }
            let c = sec.characteristics;
            let read_only = c & scn::MEM_WRITE == 0;
            let code = c & scn::MEM_EXECUTE != 0;
            let constant =
                c & scn::CNT_INITIALIZED_DATA != 0 && c & scn::CNT_UNINITIALIZED_DATA == 0;
            if read_only && (code || constant) {
                index.insert((crate::n32(f), crate::n32(s)), crate::n32(candidates.len()));
                candidates.push((crate::n32(f), crate::n32(s)));
            }
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
        let at = |file: u32, section: u32, value: u32| match index.get(&(file, section)) {
            Some(&c) => Target::Candidate(c, value),
            None => Target::Local(file, section, value),
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
    let parts = turbonop_par::map(&candidates, 64, |&(f, s)| {
        let FileKind::Object { object, .. } = &r.files[f as usize].kind else {
            return ((0, 0), Vec::new());
        };
        let sec = &object.sections[s as usize];
        let mut h = Hash2::new();
        h.bytes(sec.name);
        h.u64(u64::from(sec.characteristics & scn::OUTPUT_MASK));
        h.u64(u64::from(sec.size));
        h.bytes(sec.data);
        let mut edges = Vec::new();
        let kids: &[u32] = children.get(&(f, s)).map_or(&[], Vec::as_slice);
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
                    // function) is the same in every copy.
                    Target::Candidate(c, v) if candidates[c as usize] == own => {
                        h.u64(1);
                        h.u64(u64::from(v));
                    }
                    Target::Candidate(c, v) => {
                        h.u64(2);
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
    let (mut digests, edges): (Vec<Digest>, Vec<Vec<u32>>) = parts.into_iter().unzip();

    // Rounds: each digest mixed with its targets' digests, until the
    // classes stop splitting (at most as many rounds as candidates).
    let distinct = |d: &[Digest]| {
        let mut v = d.to_vec();
        v.sort_unstable();
        v.dedup();
        v.len()
    };
    let mut classes = distinct(&digests);
    let indices: Vec<usize> = (0..candidates.len()).collect();
    for _ in 0..candidates.len() {
        let next = turbonop_par::map(&indices, 1024, |&i| {
            let mut h = Hash2::new();
            h.u64(digests[i].0);
            h.u64(digests[i].1);
            for &t in &edges[i] {
                h.u64(digests[t as usize].0);
                h.u64(digests[t as usize].1);
            }
            h.digest()
        });
        let n = distinct(&next);
        digests = next;
        if n == classes {
            break;
        }
        classes = n;
    }

    // Each class's leader: its first member.
    let mut leader: Map<Digest, u32> = Map::default();
    let mut folded = Map::default();
    for (i, d) in digests.iter().enumerate() {
        let l = *leader.entry(*d).or_insert(crate::n32(i));
        if l != crate::n32(i) {
            folded.insert(candidates[i], candidates[l as usize]);
        }
    }
    folded
}
