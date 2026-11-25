//! What folding would save in a library, under each of turbolib's modes
//! (`turbolib analyze`). Nothing is written.
//!
//! COMDATs are grouped by leader name. Within a group, the modes differ in
//! which copies count as one:
//!
//! - `identical`: equal identities in one environment, leaving out units
//!   that reach internal-linkage symbols (`--locals=keep`);
//! - `identical, pull=preserve`: as `identical`, but a copy in the
//!   archive index's first definer that has side effects stays where it
//!   is, so the synthesized member holds a second copy;
//! - `identical, locals=copy`: as `identical`, units reaching locals
//!   included;
//! - `select-any, locals=copy`: by the COMDAT's selection rule in one
//!   environment, whatever the bytes (`ANY`, `SAME_SIZE` by size,
//!   `EXACT_MATCH` by checksum, `LARGEST` keeping the largest;
//!   `NODUPLICATES` never).
//!
//! No mode folds `NODUPLICATES` COMDATs: they are strong definitions (with
//! `/Gy`, every function is a COMDAT), and two of them are an error the
//! linker reports.
//!
//! Types are counted as `--precomp=library` would share them: every
//! record that two or more members hold, once, in a synthesized
//! precompiled-types member, except function ids, which `link.exe` won't
//! take from a PCH (see [`Hashed::shareable`]).

use crate::member::{self, Bytes, Category, Hashed, Kind, Member, Types, Unit};
use serde::Serialize;
use serde::ser::SerializeMap;
use turbonop_pe::comdat_select;

impl Serialize for Bytes {
    fn serialize<S: serde::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        let mut m = s.serialize_map(Some(Category::ALL.len() + 1))?;
        for c in Category::ALL {
            m.serialize_entry(c.name(), &self.get(c))?;
        }
        m.serialize_entry("total", &self.total())?;
        m.end()
    }
}

/// The folding modes the analyzer reports, in order.
pub const MODES: [&str; 4] = [
    "identical",
    "identical, pull=preserve",
    "identical, locals=copy",
    "select-any, locals=copy",
];

/// Counts of members by kind.
#[derive(Clone, Copy, Debug, Default, Serialize)]
pub struct Members {
    pub coff: u64,
    pub import: u64,
    pub ltcg: u64,
    pub unrecognized: u64,
}

/// The COMDAT side.
#[derive(Clone, Debug, Default, Serialize)]
pub struct Comdats {
    /// Fold units (COMDAT leaders) in all members.
    pub units: u64,
    /// Distinct leader names.
    pub names: u64,
    /// Names held by more than one member.
    pub duplicated_names: u64,
    /// The bytes of every unit.
    pub bytes: Bytes,
    /// What each of [`MODES`] would save.
    pub saved: Vec<Bytes>,
    /// Names with copies that differ within one environment: an ODR
    /// violation or flags outside the environment key. Counted, not
    /// listed.
    pub differing_names: u64,
    /// Some of those names (for `--diagnose`).
    #[serde(skip)]
    pub differing_sample: Vec<Vec<u8>>,
    /// Units whose relocations reach internal-linkage symbols.
    pub reaching_locals: u64,
    /// Names of `NODUPLICATES` COMDATs (strong definitions) in more than
    /// one member: duplicate-symbol errors waiting for a link that loads
    /// both. Never folded.
    pub strong_duplicates: u64,
    /// Names whose archive-index definer has side effects (so
    /// `--pull=preserve` keeps a second copy).
    pub pull_hazards: u64,
}

impl Comdats {
    fn new() -> Self {
        Comdats {
            saved: vec![Bytes::default(); MODES.len()],
            ..Comdats::default()
        }
    }

    fn add(&mut self, o: &Comdats) {
        self.units += o.units;
        self.names += o.names;
        self.duplicated_names += o.duplicated_names;
        self.bytes.add_all(&o.bytes);
        if self.saved.len() < o.saved.len() {
            self.saved.resize(o.saved.len(), Bytes::default());
        }
        for (a, b) in self.saved.iter_mut().zip(&o.saved) {
            a.add_all(b);
        }
        self.differing_names += o.differing_names;
        for n in &o.differing_sample {
            if self.differing_sample.len() < DIFFERING_SAMPLE {
                self.differing_sample.push(n.clone());
            }
        }
        self.reaching_locals += o.reaching_locals;
        self.strong_duplicates += o.strong_duplicates;
        self.pull_hazards += o.pull_hazards;
    }
}

/// The `.debug$T` side.
#[derive(Clone, Copy, Debug, Default, Serialize)]
pub struct TypeSummary {
    /// Members with records hashed, with `/Zi` type-server references,
    /// using a PCH (`LF_PRECOMP`), that are PCH objects, and whose records
    /// couldn't all be read.
    pub members: u64,
    pub type_server: u64,
    pub precomp: u64,
    pub pch: u64,
    pub unreadable: u64,
    /// Records naming an index not before their own.
    pub forward_references: u64,
    pub records: u64,
    /// The bytes of the hashed records (with their sections' signatures).
    pub bytes: u64,
    pub distinct_records: u64,
    pub distinct_bytes: u64,
    /// Records held by two or more members, and their bytes: what the
    /// precompiled-types member would hold (and what a PDB merges from it
    /// whenever any member is linked).
    pub shared_records: u64,
    pub shared_bytes: u64,
    /// The bytes of the copies of records that two or more members hold
    /// but that a PCH can't (`LF_FUNC_ID`, `LF_MFUNC_ID`), beyond the
    /// first: what stays duplicated.
    pub unshareable_duplicate_bytes: u64,
    /// `.debug$T` bytes after sharing, and what that saves.
    pub after: u64,
    pub saved: u64,
}

impl TypeSummary {
    fn add(&mut self, o: &TypeSummary) {
        self.members += o.members;
        self.type_server += o.type_server;
        self.precomp += o.precomp;
        self.pch += o.pch;
        self.unreadable += o.unreadable;
        self.forward_references += o.forward_references;
        self.records += o.records;
        self.bytes += o.bytes;
        self.distinct_records += o.distinct_records;
        self.distinct_bytes += o.distinct_bytes;
        self.shared_records += o.shared_records;
        self.shared_bytes += o.shared_bytes;
        self.unshareable_duplicate_bytes += o.unshareable_duplicate_bytes;
        self.after += o.after;
        self.saved += o.saved;
    }
}

/// One library's (or all libraries') figures.
#[derive(Clone, Debug, Default, Serialize)]
pub struct Summary {
    pub libraries: u64,
    pub size: u64,
    pub members: Members,
    pub bytes: Bytes,
    pub comdats: Comdats,
    pub types: TypeSummary,
    /// The conservative default's saving: `identical, pull=preserve` plus
    /// shared types.
    pub saved: u64,
}

impl Summary {
    pub fn add(&mut self, o: &Summary) {
        self.libraries += o.libraries;
        self.size += o.size;
        self.members.coff += o.members.coff;
        self.members.import += o.members.import;
        self.members.ltcg += o.members.ltcg;
        self.members.unrecognized += o.members.unrecognized;
        self.bytes.add_all(&o.bytes);
        if self.comdats.saved.is_empty() {
            self.comdats = Comdats::new();
        }
        self.comdats.add(&o.comdats);
        self.types.add(&o.types);
        self.saved += o.saved;
    }
}

/// A library's figures, and what `--diagnose` reports of it.
#[derive(Clone, Debug, Serialize)]
pub struct Library {
    pub path: String,
    pub summary: Summary,
    /// Members passed through, and why; section names nothing here knows,
    /// with how many members hold them.
    pub diagnostics: Vec<String>,
}

/// How many names whose copies differ `--diagnose` lists per library.
const DIFFERING_SAMPLE: usize = 20;

/// The overhead `--precomp=library` adds to each member that shares
/// types (its `LF_PRECOMP` record and the PCH symbol's reference), and to
/// the library (the precompiled-types member's headers and symbols).
const PRECOMP_PER_MEMBER: u64 = 64;
const PRECOMP_PER_LIBRARY: u64 = 512;

/// Analyzes one library: `members` are its members' names and data, in
/// archive order.
pub fn library(path: &str, size: u64, members: &[(&[u8], &[u8])]) -> Library {
    let facts: Vec<Member> = turbonop_par::map_balanced(members, 1, |(_, data)| member::read(data));
    let mut s = Summary {
        libraries: 1,
        size,
        comdats: Comdats::new(),
        ..Summary::default()
    };
    let mut diagnostics = Vec::new();
    let mut unknown: std::collections::BTreeMap<Vec<u8>, u64> = std::collections::BTreeMap::new();
    for ((name, _), m) in members.iter().zip(&facts) {
        match &m.kind {
            Kind::Coff => s.members.coff += 1,
            Kind::Import => s.members.import += 1,
            Kind::Ltcg => {
                s.members.ltcg += 1;
                diagnostics.push(format!(
                    "{}: an LTCG (/GL) object, passed through",
                    lossy(name)
                ));
            }
            Kind::Unrecognized(why) => {
                s.members.unrecognized += 1;
                diagnostics.push(format!(
                    "{}: not recognized ({why}), passed through",
                    lossy(name)
                ));
            }
        }
        s.bytes.add_all(&m.bytes);
        let mut seen: Vec<&Vec<u8>> = m.unknown_sections.iter().collect();
        seen.sort_unstable();
        seen.dedup();
        for n in seen {
            *unknown.entry(n.clone()).or_default() += 1;
        }
        if let Types::Unreadable(why) = &m.types {
            diagnostics.push(format!("{}: .debug$T not shared ({why})", lossy(name)));
        }
    }
    for (n, count) in unknown {
        diagnostics.push(format!(
            "section {} in {count} member(s): not known, so those members would pass through",
            lossy(&n)
        ));
    }
    s.comdats = comdats(&facts);
    s.comdats.differing_sample.sort();
    for n in &s.comdats.differing_sample {
        diagnostics.push(format!(
            "COMDAT {}: copies differ in one environment",
            lossy(n)
        ));
    }
    s.types = types(&facts);
    s.saved = s.comdats.saved[1].total() + s.types.saved;
    Library {
        path: path.to_owned(),
        summary: s,
        diagnostics,
    }
}

fn lossy(b: &[u8]) -> String {
    String::from_utf8_lossy(b).into_owned()
}

fn shards() -> usize {
    (turbonop_par::threads() * 4).max(1)
}

/// One copy in a group: its member's index, the member, and the unit.
type Copy<'m> = (usize, &'m Member, &'m Unit);

/// Where each shard's items start in a list sorted by shard: `n + 1`
/// offsets.
fn shard_bounds<T>(list: &[T], n: u64, shard_of: impl Fn(&T) -> u64) -> Vec<u32> {
    let mut bounds = Vec::with_capacity(n as usize + 1);
    for s in 0..=n {
        bounds.push(list.partition_point(|x| shard_of(x) < s) as u32);
    }
    bounds
}

fn comdats(facts: &[Member]) -> Comdats {
    let n = shards() as u64;
    // Each member's units, sorted by shard (one pass over each member,
    // in parallel), so that each shard reads only its own.
    let sorted: Vec<(Vec<u32>, Vec<u32>)> = turbonop_par::map_balanced(facts, 1, |m| {
        let mut order: Vec<u32> = (0..m.units.len() as u32).collect();
        order.sort_unstable_by_key(|&u| m.units[u as usize].leader_hash % n);
        let bounds = shard_bounds(&order, n, |&u| m.units[u as usize].leader_hash % n);
        (order, bounds)
    });
    let shards: Vec<u64> = (0..n).collect();
    let parts = turbonop_par::map_balanced(&shards, 1, |&shard| {
        let mut groups: turbonop_hash::Map<&[u8], Vec<Copy<'_>>> = turbonop_hash::Map::default();
        for (i, (m, (order, bounds))) in facts.iter().zip(&sorted).enumerate() {
            let s = shard as usize;
            for &u in &order[bounds[s] as usize..bounds[s + 1] as usize] {
                let u = &m.units[u as usize];
                groups.entry(&u.leader).or_default().push((i, m, u));
            }
        }
        let mut c = Comdats::new();
        for copies in groups.values() {
            group(copies, &mut c);
        }
        c
    });
    let mut c = Comdats::new();
    for p in &parts {
        c.add(p);
    }
    c
}

/// The copies of one name, in member order.
fn group(copies: &[Copy<'_>], c: &mut Comdats) {
    c.names += 1;
    c.units += copies.len() as u64;
    for (_, _, u) in copies {
        c.bytes.add_all(&u.bytes);
        c.reaching_locals += u64::from(u.reaches_locals);
    }
    if copies.len() < 2 {
        return;
    }
    c.duplicated_names += 1;
    let (_, first_member, first_unit) = copies[0];
    // A strong definition (`/Gy` puts every function in a COMDAT): its
    // copies are a duplicate-symbol error, which folding must keep.
    if first_unit.selection == comdat_select::NODUPLICATES {
        c.strong_duplicates += 1;
        return;
    }
    let hazard = first_member.side_effects;
    c.pull_hazards += u64::from(hazard);

    // Identical: by (environment, identity).
    let mut by_identity: Vec<(u64, u128, usize)> = copies
        .iter()
        .enumerate()
        .map(|(i, (_, m, u))| (m.environment, u.identity, i))
        .collect();
    by_identity.sort_unstable();
    let mut differing = false;
    for run in by_identity.chunk_by(|a, b| a.0 == b.0 && a.1 == b.1) {
        let survivor = copies[run[0].2].2;
        let mut rest = Bytes::default();
        for &(_, _, i) in &run[1..] {
            rest.add_all(&copies[i].2.bytes);
        }
        // locals=copy
        c.saved[2].add_all(&rest);
        if !survivor.reaches_locals {
            c.saved[0].add_all(&rest);
            // pull=preserve: the definer's copy stays, and the synthesized
            // member needs one of its own.
            let mut preserved = rest;
            if hazard && run.iter().any(|&(_, _, i)| i == 0) {
                preserved.sub_all(&first_unit.bytes);
            }
            c.saved[1].add_all(&preserved);
        }
    }
    for env_run in by_identity.chunk_by(|a, b| a.0 == b.0) {
        if env_run.iter().any(|x| x.1 != env_run[0].1) {
            differing = true;
        }
    }
    c.differing_names += u64::from(differing);
    if differing && c.differing_sample.len() < DIFFERING_SAMPLE {
        c.differing_sample.push(first_unit.leader.clone());
    }

    // Select-any: by the first copy's selection rule, per environment.
    let selection = first_unit.selection;
    let mut by_env: Vec<(u64, u64, usize)> = copies
        .iter()
        .enumerate()
        .map(|(i, (_, m, u))| {
            let key = match selection {
                comdat_select::SAME_SIZE => u64::from(u.size),
                comdat_select::EXACT_MATCH => u64::from(u.checksum),
                _ => 0,
            };
            (m.environment, key, i)
        })
        .collect();
    by_env.sort_unstable();
    for run in by_env.chunk_by(|a, b| a.0 == b.0 && a.1 == b.1) {
        let keep = if selection == comdat_select::LARGEST {
            run.iter()
                .max_by_key(|&&(_, _, i)| (copies[i].2.size, std::cmp::Reverse(i)))
                .map_or(run[0].2, |x| x.2)
        } else {
            run[0].2
        };
        for &(_, _, i) in run {
            if i != keep {
                c.saved[3].add_all(&copies[i].2.bytes);
            }
        }
    }
}

fn types(facts: &[Member]) -> TypeSummary {
    let mut t = TypeSummary::default();
    for m in facts {
        match &m.types {
            Types::None => {}
            Types::TypeServer => t.type_server += 1,
            Types::Unreadable(_) => t.unreadable += 1,
            Types::Records {
                records,
                precomp,
                pch,
                forward,
            } => {
                t.members += 1;
                t.precomp += u64::from(*precomp);
                t.pch += u64::from(*pch);
                t.forward_references += u64::from(*forward);
                t.records += records.len() as u64;
                t.bytes += 4 + records.iter().map(|r| u64::from(r.size)).sum::<u64>();
            }
        }
    }
    let with_records: Vec<&Vec<Hashed>> = facts
        .iter()
        .filter_map(|m| match &m.types {
            Types::Records { records, .. } => Some(records),
            _ => None,
        })
        .collect();
    // Each member's distinct records (a member may hold a record twice),
    // sorted by shard, so that each shard reads only its own.
    let n = shards() as u64;
    let shard_of = |r: &Hashed| (r.hash as u64) % n;
    let deduped: Vec<(Vec<Hashed>, Vec<u32>)> = turbonop_par::map_balanced(&with_records, 1, |r| {
        let mut v: Vec<Hashed> = (*r).clone();
        v.sort_unstable_by_key(|x| (shard_of(x), x.hash));
        v.dedup_by_key(|x| x.hash);
        let bounds = shard_bounds(&v, n, shard_of);
        (v, bounds)
    });
    let shards: Vec<u64> = (0..n).collect();
    let parts = turbonop_par::map_balanced(&shards, 1, |&shard| {
        // Each record: its size, whether it may be shared, and how many
        // members hold it.
        let mut counts: turbonop_hash::Map<u128, (u32, bool, u32)> = turbonop_hash::Map::default();
        let s = shard as usize;
        for (list, bounds) in &deduped {
            for r in &list[bounds[s] as usize..bounds[s + 1] as usize] {
                counts.entry(r.hash).or_insert((r.size, r.shareable, 0)).2 += 1;
            }
        }
        let mut p = TypeSummary::default();
        for &(size, shareable, count) in counts.values() {
            let size = u64::from(size);
            p.distinct_records += 1;
            p.distinct_bytes += size;
            if count >= 2 && shareable {
                p.shared_records += 1;
                p.shared_bytes += size;
                p.after += size;
            } else {
                // Each member that holds it keeps its own.
                p.after += size * u64::from(count);
                if count >= 2 {
                    p.unshareable_duplicate_bytes += size * u64::from(count - 1);
                }
            }
        }
        p
    });
    for p in &parts {
        t.distinct_records += p.distinct_records;
        t.distinct_bytes += p.distinct_bytes;
        t.shared_records += p.shared_records;
        t.shared_bytes += p.shared_bytes;
        t.after += p.after;
        t.unshareable_duplicate_bytes += p.unshareable_duplicate_bytes;
    }
    if t.members > 0 {
        // Each member: its signature, LF_PRECOMP, and its own records; the
        // library: one precompiled-types member with the shared records.
        t.after += t.members * (4 + PRECOMP_PER_MEMBER) + PRECOMP_PER_LIBRARY;
        t.saved = t.bytes.saturating_sub(t.after);
    }
    t
}

#[cfg(test)]
mod tests;
