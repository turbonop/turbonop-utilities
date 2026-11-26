//! Shared types (`--precomp=library`, phase 3): one synthesized
//! precompiled-types member for the library, holding the type and id
//! records that at least `k` members hold, and every member that holds
//! some of them rewritten to name it, as a `/Yu` object names its PCH.
//!
//! 1. **Hashing** ([`plan`]): every record of every member that can share
//!    is hashed structurally (its bytes, with each index replaced by the
//!    hash of the record it names), with the member's machine, so equal
//!    records hash alike whatever their indices. A member's records refer
//!    only to earlier ones ([`debug::read`] checks it), so one pass in
//!    stream order does it. PCH objects in the library are hashed first:
//!    a user's indices into its PCH hash as the PCH object's records, so
//!    the users of a PCH in the library (`/Yu` objects) share with
//!    everyone else (they are *absorbed*, 3.4; see [`debug::resolve`]).
//!    The PCH object itself stays as it is: users in other libraries may
//!    name it (LLVM's libraries reuse each other's PCHs).
//! 2. **The shared set**: the records at least `k` members hold (a PCH's
//!    records count for each user that reaches them), but never
//!    `LF_FUNC_ID` and `LF_MFUNC_ID` (with them in a PCH, link.exe drops
//!    the signatures of the functions they name: phase 0), `LF_BUILDINFO`,
//!    or the records that within a member only `LF_BUILDINFO` reaches (the
//!    member's own command line). The set is closed under references:
//!    every member that holds a record holds what it names, and a record
//!    naming one that can't be shared isn't shared either.
//! 3. **The precompiled-types member** ([`Plan::member`]): the shared
//!    records in an order where each names only earlier ones (by depth,
//!    then hash: the same for any thread count), as `.debug$P`, then
//!    `LF_ENDPRECOMP` with the signature (a hash of the records), an
//!    `S_OBJNAME` with the signature, and the defined
//!    `__@@_PchSym_turbolib_<hash>`. One for each machine whose members
//!    share records.
//! 4. **Each member that shares** ([`Plan::edit`]): its `.debug$T` becomes
//!    `LF_PRECOMP(0x1000, |shared|, signature, name)` and the records it
//!    keeps (those not shared, and an absorbed PCH's records it reaches
//!    that aren't), renumbered from `0x1000 + |shared|`; every index in
//!    its `.debug$S` is renumbered in place; it refers to the PCH symbol;
//!    and its `.debug$H` (lld's precomputed type hashes, now stale) goes.
//!    A member none of whose records is shared is left as it is.
//!
//! Synthesized members (2.4) name the same PCH and use its records where
//! theirs are shared ([`Plan::shared`]).

use super::Parsed;
use super::debug::{self, FIRST, Types, le32};
use crate::hash::H128;
use std::borrow::Cow;
use turbonop_codeview::c13::{self, CV_SIGNATURE_C13};
use turbonop_codeview::lines::subsection as sub;
use turbonop_codeview::remap;
use turbonop_codeview::type_refs::{self, Ref};
use turbonop_codeview::types::leaf;
use turbonop_codeview::write;
use turbonop_coff::{
    Aux, Format, Name, Object, Section, SectionData, SectionDefinition, SectionId, SectionNumber,
    Skeleton, Symbol, storage_class,
};
use turbonop_hash::Map as HashMap;

/// The start of the PCH symbol's name (then the PCH's hash, in hex).
pub const PCH_SYMBOL_PREFIX: &[u8] = b"__@@_PchSym_turbolib_";

/// A record's kind can be shared.
const SHAREABLE: u8 = 1;
/// Within its member, only `LF_BUILDINFO` reaches it (or it is one).
const BUILD: u8 = 2;

/// What a member is to sharing.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(super) enum Role {
    /// Its types stay as they are, and nothing is hashed.
    #[default]
    None,
    /// A PCH object: hashed (for its users and its units), not rewritten.
    Source,
    /// Hashed, and rewritten if it holds a shared record.
    Sharer,
}

/// One member's records, hashed.
#[derive(Clone, Debug, Default)]
pub(super) struct Member {
    pub role: Role,
    /// Each of its own records' structural hash, in index order.
    hashes: Vec<u128>,
    /// `SHAREABLE` and `BUILD`, by record.
    flags: Vec<u8>,
    /// Each record's depth: 0 if it names no record, else one more than the
    /// deepest it names.
    depth: Vec<u32>,
    /// A PCH object's: the records each of its records names (positions),
    /// from `child_starts[i]` to `child_starts[i + 1]`.
    children: Vec<u32>,
    child_starts: Vec<u32>,
    /// An absorbed user's: the positions of its PCH object's records that
    /// it reaches, ascending.
    reached: Vec<u32>,
    /// Why a member that could share doesn't, for reports.
    why_not: Option<String>,
    /// A precompiled-types member turbolib made (the library is being
    /// compacted again): every record of it counts for each of its users,
    /// as each was shared, so that the same records are shared again.
    ours: bool,
}

/// The library's shared types.
pub(super) struct Plan {
    pub members: Vec<Member>,
    lookup: Lookup,
    /// One precompiled-types member for each machine that shares records.
    pub pchs: Vec<Pch>,
    /// Members that could share but whose debug information couldn't be
    /// read through, and why (the first).
    pub unreadable: u64,
    pub first_unreadable: Option<String>,
}

/// One precompiled-types member.
#[derive(Clone, Debug)]
pub(super) struct Pch {
    pub machine: u16,
    /// Its records (from index `FIRST`).
    pub count: u32,
    pub records: Vec<u8>,
    pub signature: u32,
    /// The symbol it defines, which every member that uses it refers to.
    pub symbol: Vec<u8>,
    /// Its name in the archive, which the users' `LF_PRECOMP` names.
    pub name: Vec<u8>,
    /// The users' `LF_PRECOMP` record (kind and body).
    pub precomp: Vec<u8>,
    /// The `@feat.00` bits every member that shares has.
    pub feat00: Option<u32>,
}

/// A record's structural hash to its index in its machine's PCH, sharded.
#[derive(Default)]
struct Lookup {
    maps: Vec<HashMap<u128, u32>>,
}

const LOOKUP_SHARDS: usize = 64;

fn shard(h: u128) -> usize {
    (h >> 64) as usize % LOOKUP_SHARDS
}

impl Lookup {
    fn new(pairs: &[(u128, u32)]) -> Lookup {
        // Each chunk's pairs by shard, then each shard's map from them.
        let chunks: Vec<Vec<Vec<(u128, u32)>>> =
            turbonop_par::map_chunks(pairs, 1 << 16, |chunk| {
                let mut by: Vec<Vec<(u128, u32)>> = vec![Vec::new(); LOOKUP_SHARDS];
                for &(h, i) in chunk {
                    by[shard(h)].push((h, i));
                }
                vec![by]
            });
        let maps = turbonop_par::map_range(LOOKUP_SHARDS, 1, |s| {
            let n = chunks.iter().map(|c| c[s].len()).sum();
            let mut m = HashMap::with_capacity_and_hasher(n, Default::default());
            for c in &chunks {
                m.extend(c[s].iter().copied());
            }
            m
        });
        Lookup { maps }
    }

    fn get(&self, h: u128) -> Option<u32> {
        self.maps.get(shard(h))?.get(&h).copied()
    }
}

/// Whether a record of `kind` may be in a precompiled-types member.
fn shareable(kind: u16) -> bool {
    !matches!(
        kind,
        leaf::LF_FUNC_ID
            | leaf::LF_MFUNC_ID
            | leaf::LF_BUILDINFO
            | leaf::LF_ENDPRECOMP
            | leaf::LF_PRECOMP
            | leaf::LF_TYPESERVER2
    )
}

/// A member's records: the section's bytes, the first index, and where
/// each record starts.
fn records<'p>(p: &'p Parsed<'_>) -> Option<(&'p [u8], u32, &'p [u32])> {
    let Types::Records {
        section,
        base,
        offsets,
        ..
    } = &p.debug.types
    else {
        return None;
    };
    let data = p.object.as_ref()?.sections[section.index()].bytes();
    Some((data, *base, offsets))
}

/// The record at `at` in `data`: its kind and body (padding included).
fn at_record(data: &[u8], at: u32) -> (u16, &[u8]) {
    let at = at as usize;
    let len = usize::from(u16::from_le_bytes([data[at], data[at + 1]]));
    (
        u16::from_le_bytes([data[at + 2], data[at + 3]]),
        &data[at + 4..at + 2 + len],
    )
}

/// The PCH range a member's `LF_PRECOMP` names, if it has one.
fn precomp_range(p: &Parsed<'_>) -> Option<debug::Precomp> {
    match p.debug.types {
        Types::Records { precomp, .. } => precomp,
        _ => None,
    }
}

fn role(p: &Parsed<'_>) -> Role {
    if p.passed.is_some() || p.object.is_none() {
        return Role::None;
    }
    match &p.debug.types {
        Types::Records {
            precomp: None,
            end_precomp: Some(_),
            ..
        } => Role::Source,
        Types::Records { precomp: None, .. } => Role::Sharer,
        Types::Records {
            precomp: Some(_), ..
        } if p.debug.absorbed => Role::Sharer,
        _ => Role::None,
    }
}

/// Hashes member `m`'s records (a PCH user's PCH object, `pch`, already
/// hashed), and for a sharer finds what of its PCH it reaches and checks
/// that its `.debug$S` can be renumbered.
fn hash_member(parsed: &[Parsed<'_>], m: usize, role: Role, pch: Option<&Member>) -> Member {
    let p = &parsed[m];
    let mut out = Member {
        role,
        ..Member::default()
    };
    let Some((data, base, offsets)) = records(p) else {
        out.role = Role::None;
        return out;
    };
    let machine = p.object.as_ref().map_or(0, |o| o.machine);
    let range = precomp_range(p).filter(|_| pch.is_some());
    let n = offsets.len();
    out.hashes.reserve_exact(n);
    out.flags.reserve_exact(n);
    out.depth.reserve_exact(n);
    // References to each record from the member's own records, for
    // finding what only LF_BUILDINFO reaches.
    let mut referrers = vec![0u32; n];
    let mut builds: Vec<u32> = Vec::new();
    let mut reach: Vec<bool> = match (range, pch) {
        (Some(r), Some(_)) => vec![false; r.count as usize],
        _ => Vec::new(),
    };
    let source = role == Role::Source;
    if source {
        out.child_starts.reserve_exact(n + 1);
        out.child_starts.push(0);
    }
    let mut refs: Vec<Ref> = Vec::new();
    for (i, &at) in offsets.iter().enumerate() {
        let (kind, body) = at_record(data, at);
        refs.clear();
        type_refs::refs(kind, body, &mut refs).expect("records checked when read");
        let mut h = H128::new();
        h.u64(u64::from(machine));
        h.u64(u64::from(kind));
        let mut from = 0;
        let mut depth = 0;
        for r in &refs {
            let o = r.offset as usize;
            h.bytes(&body[from..o]);
            let v = le32(body, o);
            if v < FIRST {
                h.u64(0);
                h.u64(u64::from(v));
            } else if let (Some(rg), Some(pch)) = (range, pch)
                && v - rg.start < rg.count
            {
                let j = (v - FIRST) as usize;
                h.u64(1);
                h.u128(pch.hashes[j]);
                depth = depth.max(pch.depth[j] + 1);
                reach[j] = true;
            } else {
                // An earlier record (read_types checked).
                let j = (v - base) as usize;
                h.u64(1);
                h.u128(out.hashes[j]);
                depth = depth.max(out.depth[j] + 1);
                referrers[j] += 1;
                if source {
                    out.children.push(j as u32);
                }
            }
            from = o + 4;
        }
        h.bytes(&body[from..]);
        out.hashes.push(h.finish());
        out.depth.push(depth);
        out.flags.push(if shareable(kind) { SHAREABLE } else { 0 });
        if kind == leaf::LF_BUILDINFO {
            builds.push(i as u32);
        }
        if source {
            out.child_starts.push(out.children.len() as u32);
        }
    }
    if role != Role::Sharer {
        out.ours = p.object.as_ref().is_some_and(|o| {
            o.symbols.iter().any(|s| {
                s.is_external()
                    && s.section != SectionNumber::Undefined
                    && s.name.as_bytes().starts_with(PCH_SYMBOL_PREFIX)
            })
        });
        return out;
    }
    build_only(data, base, offsets, &builds, &mut referrers, &mut out.flags);

    // Its .debug$S: every index must name a record it has (or its PCH's),
    // and every field must be known, so that it can be renumbered; and
    // what of its PCH it names.
    let object = p.object.as_ref().expect("a sharer is an object");
    let own = base..base + n as u32;
    for s in &object.sections {
        if s.name.as_bytes() != b".debug$S" {
            continue;
        }
        let mut bad = None;
        let r = remap::debug_s_indices(s.bytes(), |v, _| {
            if own.contains(&v) {
                return;
            }
            match range {
                Some(rg) if v - rg.start < rg.count => reach[(v - FIRST) as usize] = true,
                _ => {
                    bad.get_or_insert(v);
                }
            }
        });
        let why = match (r, bad) {
            (Err(e), _) => Some(format!("its .debug$S: {e}")),
            (Ok(()), Some(v)) => Some(format!("its .debug$S names {v:#x}, not one of its records")),
            (Ok(()), None) => None,
        };
        if let Some(w) = why {
            out.role = Role::None;
            out.why_not = Some(w);
            return out;
        }
    }
    if let Some(pch) = pch
        && pch.ours
    {
        reach.fill(true);
    }
    if let Some(pch) = pch
        && !reach.is_empty()
    {
        // What the records it names name, within the PCH (each names only
        // earlier ones).
        for j in (0..reach.len()).rev() {
            if !reach[j] {
                continue;
            }
            let (a, b) = (
                pch.child_starts[j] as usize,
                pch.child_starts[j + 1] as usize,
            );
            for &c in &pch.children[a..b] {
                reach[c as usize] = true;
            }
        }
        out.reached = (0..reach.len() as u32)
            .filter(|&j| reach[j as usize])
            .collect();
    }
    out
}

/// Marks the records that only `LF_BUILDINFO` records reach within the
/// member (`referrers` counts each record's referrers among its records).
fn build_only(
    data: &[u8],
    base: u32,
    offsets: &[u32],
    builds: &[u32],
    referrers: &mut [u32],
    flags: &mut [u8],
) {
    if builds.is_empty() {
        return;
    }
    // What the build records reach.
    let mut set: Vec<u32> = Vec::new();
    let mut seen: turbonop_hash::Set<u32> = turbonop_hash::Set::default();
    let mut stack: Vec<u32> = builds.to_vec();
    let mut refs: Vec<Ref> = Vec::new();
    while let Some(i) = stack.pop() {
        if !seen.insert(i) {
            continue;
        }
        set.push(i);
        let (kind, body) = at_record(data, offsets[i as usize]);
        refs.clear();
        type_refs::refs(kind, body, &mut refs).expect("records checked when read");
        for r in &refs {
            let v = le32(body, r.offset as usize);
            if v >= base {
                stack.push(v - base);
            }
        }
    }
    // Latest first: each record's referrers come after it, so each is
    // decided once all of them are.
    set.sort_unstable_by(|a, b| b.cmp(a));
    for &i in &set {
        let (kind, body) = at_record(data, offsets[i as usize]);
        let only = kind == leaf::LF_BUILDINFO || referrers[i as usize] == 0;
        if !only {
            continue;
        }
        flags[i as usize] |= BUILD;
        refs.clear();
        type_refs::refs(kind, body, &mut refs).expect("records checked when read");
        for r in &refs {
            let v = le32(body, r.offset as usize);
            if v >= base {
                referrers[(v - base) as usize] -= 1;
            }
        }
    }
}

/// The hash of member `m`'s index `v` (from `FIRST`), if it is hashed.
fn hash_of(parsed: &[Parsed<'_>], members: &[Member], m: usize, v: u32) -> Option<u128> {
    let (cm, cv) = debug::canonical(parsed, m, v);
    let mem = members.get(cm)?;
    if mem.role == Role::None {
        return None;
    }
    let (_, base, _) = records(&parsed[cm])?;
    mem.hashes.get(cv.checked_sub(base)? as usize).copied()
}

/// A record a sharer holds: its hash, and where a copy is (member and
/// position).
type Held = (u128, u32, u32);

/// A record that may be shared: one of its holders' copies (the first, by
/// member and position), its depth, and how many members hold it.
#[derive(Clone, Copy, Debug)]
struct Candidate {
    hash: u128,
    machine: u16,
    depth: u32,
    member: u32,
    position: u32,
}

/// Finds the shared types of `parsed` (a library's members): the records
/// at least `min` members hold.
pub(super) fn plan(parsed: &[Parsed<'_>], min: u32) -> Plan {
    let roles: Vec<Role> = parsed.iter().map(role).collect();
    // PCH objects first: their users hash through them.
    let mut members: Vec<Member> = vec![Member::default(); parsed.len()];
    let sources: Vec<usize> = (0..parsed.len())
        .filter(|&m| roles[m] == Role::Source)
        .collect();
    for (m, h) in sources
        .iter()
        .zip(turbonop_par::map_balanced(&sources, 1, |&m| {
            hash_member(parsed, m, Role::Source, None)
        }))
    {
        members[*m] = h;
    }
    let mut sharers: Vec<usize> = (0..parsed.len())
        .filter(|&m| roles[m] == Role::Sharer)
        .collect();
    // Largest first, so the threads finish together.
    sharers.sort_by_key(|&m| std::cmp::Reverse(records(&parsed[m]).map_or(0, |r| r.2.len())));
    let hashed = turbonop_par::map_balanced(&sharers, 1, |&m| {
        let pch = parsed[m].debug.pch.map(|p| &members[p]);
        hash_member(parsed, m, Role::Sharer, pch)
    });
    for (m, h) in sharers.iter().zip(hashed) {
        members[*m] = h;
    }
    let mut unreadable = 0;
    let mut first_unreadable = None;
    for (m, mem) in members.iter().enumerate() {
        if roles[m] == Role::Sharer && mem.role == Role::None {
            unreadable += 1;
            if first_unreadable.is_none() {
                first_unreadable = mem.why_not.clone();
            }
        }
    }
    // Each sharer's distinct shareable records, by shard, so that each
    // shard counts only its own.
    let n = (turbonop_par::threads() * 4).max(1) as u64;
    let shard_of = |h: u128| (h as u64) % n;
    let sharers: Vec<usize> = (0..parsed.len())
        .filter(|&m| members[m].role == Role::Sharer)
        .collect();
    let lists: Vec<(Vec<Held>, Vec<u32>)> = turbonop_par::map_balanced(&sharers, 1, |&m| {
        let mem = &members[m];
        let mut v: Vec<Held> = Vec::with_capacity(mem.hashes.len() + mem.reached.len());
        for (i, (&h, &f)) in mem.hashes.iter().zip(&mem.flags).enumerate() {
            if f & SHAREABLE != 0 && f & BUILD == 0 {
                v.push((h, m as u32, i as u32));
            }
        }
        if let Some(p) = parsed[m].debug.pch {
            let pm = &members[p];
            for &j in &mem.reached {
                if pm.flags[j as usize] & SHAREABLE != 0 {
                    v.push((pm.hashes[j as usize], p as u32, j));
                }
            }
        }
        // By shard (a counting sort: the order within a shard doesn't
        // matter, as counting takes each member's copies once).
        let mut bounds = vec![0u32; n as usize + 1];
        for x in &v {
            bounds[shard_of(x.0) as usize + 1] += 1;
        }
        for s in 0..n as usize {
            bounds[s + 1] += bounds[s];
        }
        let mut next = bounds.clone();
        let mut out: Vec<Held> = vec![(0, 0, 0); v.len()];
        for x in v {
            let s = shard_of(x.0) as usize;
            out[next[s] as usize] = x;
            next[s] += 1;
        }
        (out, bounds)
    });
    let shards: Vec<u64> = (0..n).collect();
    let parts: Vec<Vec<Candidate>> = turbonop_par::map_balanced(&shards, 1, |&s| {
        let s = s as usize;
        // Each record: how many members hold it (each counted once: the
        // last member that counted it), and its first copy.
        let mut counts: HashMap<u128, (u32, u32, u32, u32)> = HashMap::default();
        for (k, (list, bounds)) in lists.iter().enumerate() {
            let k = k as u32;
            for &(h, a, b) in &list[bounds[s] as usize..bounds[s + 1] as usize] {
                let e = counts.entry(h).or_insert((0, a, b, u32::MAX));
                if e.3 != k {
                    e.0 += 1;
                    e.3 = k;
                }
                if (a, b) < (e.1, e.2) {
                    (e.1, e.2) = (a, b);
                }
            }
        }
        counts
            .into_iter()
            .filter(|&(_, (c, _, _, _))| c >= min)
            .map(|(hash, (_, a, b, _))| Candidate {
                hash,
                machine: parsed[a as usize].object.as_ref().map_or(0, |o| o.machine),
                depth: members[a as usize].depth[b as usize],
                member: a,
                position: b,
            })
            .collect()
    });
    drop(lists);
    let mut candidates: Vec<Candidate> = parts.into_iter().flatten().collect();

    // Each machine's PCH: by depth, so that each record names only earlier
    // ones; then by hash. A record naming one that can't be shared isn't
    // shared either (none is known to: no type refers to a function id).
    loop {
        turbonop_par::sort_by(&mut candidates, |a, b| {
            (a.machine, a.depth, a.hash).cmp(&(b.machine, b.depth, b.hash))
        });
        let mut pairs: Vec<(u128, u32)> = Vec::with_capacity(candidates.len());
        let mut groups: Vec<(u16, usize, usize)> = Vec::new();
        for (i, c) in candidates.iter().enumerate() {
            match groups.last_mut() {
                Some(g) if g.0 == c.machine => g.2 = i + 1,
                _ => groups.push((c.machine, i, i + 1)),
            }
            let start = groups.last().map_or(0, |g| g.1);
            pairs.push((c.hash, FIRST + (i - start) as u32));
        }
        let lookup = Lookup::new(&pairs);
        drop(pairs);
        // Each record's bytes, its indices renumbered into the PCH.
        let built: Vec<(Vec<u8>, Vec<u128>)> =
            turbonop_par::map_chunks(&candidates, 1 << 12, |chunk| {
                let mut bytes = Vec::new();
                let mut bad = Vec::new();
                let mut refs = Vec::new();
                for c in chunk {
                    let record =
                        debug::record(parsed, c.member as usize, base_of(parsed, c) + c.position);
                    let start = bytes.len();
                    bytes.extend_from_slice(record);
                    let kind = u16::from_le_bytes([record[2], record[3]]);
                    refs.clear();
                    type_refs::refs(kind, &record[4..], &mut refs)
                        .expect("records checked when read");
                    for r in &refs {
                        let at = start + 4 + r.offset as usize;
                        let v = le32(&bytes, at);
                        if v < FIRST {
                            continue;
                        }
                        match hash_of(parsed, &members, c.member as usize, v)
                            .and_then(|h| lookup.get(h))
                        {
                            Some(new) => bytes[at..at + 4].copy_from_slice(&new.to_le_bytes()),
                            None => {
                                bad.push(c.hash);
                                break;
                            }
                        }
                    }
                }
                vec![(bytes, bad)]
            });
        let bad: turbonop_hash::Set<u128> =
            built.iter().flat_map(|(_, b)| b.iter().copied()).collect();
        if !bad.is_empty() {
            candidates.retain(|c| !bad.contains(&c.hash));
            continue;
        }
        // The chunks' bytes in order, split by machine.
        let mut all = Vec::with_capacity(built.iter().map(|(b, _)| b.len()).sum());
        for (b, _) in built {
            all.extend_from_slice(&b);
        }
        let mut pchs = Vec::new();
        let mut at = 0;
        for &(machine, start, end) in &groups {
            let len: usize = candidates[start..end]
                .iter()
                .map(|c| {
                    debug::record(parsed, c.member as usize, base_of(parsed, c) + c.position).len()
                })
                .sum();
            let records = all[at..at + len].to_vec();
            at += len;
            pchs.push(pch(
                parsed,
                &members,
                machine,
                (end - start) as u32,
                records,
            ));
        }
        return Plan {
            members,
            lookup,
            pchs,
            unreadable,
            first_unreadable,
        };
    }
}

/// The first index of a candidate's member's own records.
fn base_of(parsed: &[Parsed<'_>], c: &Candidate) -> u32 {
    records(&parsed[c.member as usize]).map_or(FIRST, |r| r.1)
}

/// A machine's precompiled-types member's facts.
fn pch(
    parsed: &[Parsed<'_>],
    members: &[Member],
    machine: u16,
    count: u32,
    records: Vec<u8>,
) -> Pch {
    let mut h = H128::new();
    h.u64(u64::from(machine));
    h.u64(u64::from(count));
    h.bytes(&records);
    let hash = h.finish();
    let signature = match hash as u32 {
        0 => 1,
        s => s,
    };
    let mut symbol = PCH_SYMBOL_PREFIX.to_vec();
    symbol.extend_from_slice(format!("{hash:032x}").as_bytes());
    let name = format!("turbolib-types-{:016x}.obj", (hash >> 64) as u64).into_bytes();
    let mut precomp = Vec::new();
    write::precomp(FIRST, count, signature, &name)
        .write_type(&mut precomp)
        .expect("a short record");
    // (Kind and body, as the other records' facts are kept.)
    let precomp = precomp[2..].to_vec();
    let mut feat00: Option<u32> = None;
    for (m, mem) in members.iter().enumerate() {
        if mem.role != Role::Sharer || parsed[m].object.as_ref().map(|o| o.machine) != Some(machine)
        {
            continue;
        }
        if let Some(f) = parsed[m].facts.feat00 {
            feat00 = Some(feat00.map_or(f, |a| a & f));
        }
    }
    Pch {
        machine,
        count,
        records,
        signature,
        symbol,
        name,
        precomp,
        feat00,
    }
}

/// What sharing changes in one member: new contents for some of its
/// sections (by their numbers in the member as read), and the PCH symbol
/// it refers to.
pub(super) struct Edit {
    pub sections: Vec<(SectionId, Vec<u8>)>,
    pub symbol: Vec<u8>,
    /// `.debug$T` bytes before and after.
    pub before: u64,
    pub after: u64,
}

/// MSVC's `.debug$S`, `.debug$T`, and `.debug$P`: initialized,
/// discardable, read-only, 1-byte aligned.
const DEBUG_CHARACTERISTICS: u32 = 0x4210_0040;

impl Plan {
    /// The precompiled-types member for `machine`, if its members share
    /// records.
    pub(super) fn pch_for(&self, machine: u16) -> Option<&Pch> {
        self.pchs.iter().find(|p| p.machine == machine)
    }

    /// The PCH index of member `m`'s record `v` (from `FIRST`), if it is
    /// shared.
    pub(super) fn shared(&self, parsed: &[Parsed<'_>], m: usize, v: u32) -> Option<u32> {
        self.lookup.get(hash_of(parsed, &self.members, m, v)?)
    }

    /// The sharing of member `m` (not its sections `removed`, by number):
    /// `None` if it doesn't share, or none of its records is shared.
    pub(super) fn edit(&self, parsed: &[Parsed<'_>], m: usize, removed: &[bool]) -> Option<Edit> {
        let mem = &self.members[m];
        if mem.role != Role::Sharer {
            return None;
        }
        let p = &parsed[m];
        let object = p.object.as_ref()?;
        let pch = self.pch_for(object.machine)?;
        let (data, base, offsets) = records(p)?;
        let Types::Records { section, .. } = p.debug.types else {
            return None;
        };
        let range = precomp_range(p).filter(|_| p.debug.pch.is_some());
        let source = p.debug.pch.map(|s| (s, &self.members[s]));
        let mut own_new = vec![0u32; offsets.len()];
        let mut pch_new = vec![0u32; range.map_or(0, |r| r.count as usize)];
        let mut next = FIRST + pch.count;
        let mut kept = Vec::new();
        let mut any = false;
        let mut refs = Vec::new();
        // Renumbers a kept record's indices (in the member's numbering).
        let renumber = |bytes: &mut [u8], own_new: &[u32], pch_new: &[u32], refs: &mut Vec<Ref>| {
            let kind = u16::from_le_bytes([bytes[2], bytes[3]]);
            remap::type_record(kind, &mut bytes[4..], refs, &mut |v: u32, _| {
                map(v, base, range, own_new, pch_new)
            })
            .expect("a kept record's indices are mapped");
        };
        // The PCH object's records it reaches, in the PCH's order, then its
        // own: each names only earlier ones.
        if let Some((s, sm)) = source {
            for &j in &mem.reached {
                let shared = (sm.flags[j as usize] & SHAREABLE != 0)
                    .then(|| self.lookup.get(sm.hashes[j as usize]))
                    .flatten();
                pch_new[j as usize] = match shared {
                    Some(n) => {
                        any = true;
                        n
                    }
                    None => {
                        let start = kept.len();
                        kept.extend_from_slice(debug::record(parsed, s, FIRST + j));
                        renumber(&mut kept[start..], &own_new, &pch_new, &mut refs);
                        next += 1;
                        next - 1
                    }
                };
            }
        }
        for (i, &at) in offsets.iter().enumerate() {
            let f = mem.flags[i];
            let shared = (f & SHAREABLE != 0 && f & BUILD == 0)
                .then(|| self.lookup.get(mem.hashes[i]))
                .flatten();
            own_new[i] = match shared {
                Some(n) => {
                    any = true;
                    n
                }
                None => {
                    let at = at as usize;
                    let len = usize::from(u16::from_le_bytes([data[at], data[at + 1]]));
                    let start = kept.len();
                    kept.extend_from_slice(&data[at..at + 2 + len]);
                    renumber(&mut kept[start..], &own_new, &pch_new, &mut refs);
                    next += 1;
                    next - 1
                }
            };
        }
        if !any {
            return None;
        }
        let mut t = CV_SIGNATURE_C13.to_le_bytes().to_vec();
        t.extend_from_slice(&(pch.precomp.len() as u16).to_le_bytes());
        t.extend_from_slice(&pch.precomp);
        t.append(&mut kept);
        let before = object.sections[section.index()].bytes().len() as u64;
        let after = t.len() as u64;
        if after >= before {
            // Too little shared to pay for its LF_PRECOMP and the symbol.
            return None;
        }
        let mut sections = vec![(section, t)];
        for (i, s) in object.sections.iter().enumerate() {
            if s.name.as_bytes() != b".debug$S" || removed.get(i).copied().unwrap_or(false) {
                continue;
            }
            let mut d = s.bytes().to_vec();
            remap::debug_s(&mut d, &mut |v: u32, _| {
                map(v, base, range, &own_new, &pch_new)
            })
            .expect("its .debug$S checked when hashed");
            sections.push((SectionId(i as u32), d));
        }
        Some(Edit {
            sections,
            symbol: pch.symbol.clone(),
            before,
            after,
        })
    }

    /// The precompiled-types member `pch`: `.debug$S` with its `S_OBJNAME`,
    /// `.debug$P` with the records and `LF_ENDPRECOMP`, and the symbol.
    pub(super) fn member(pch: &Pch) -> Skeleton<'static> {
        let mut o = Object::new(Format::Regular, pch.machine);
        if let Some(v) = pch.feat00 {
            o.push_symbol(Symbol::new(
                Name::new(b"@feat.00".to_vec()),
                v,
                SectionNumber::Absolute,
                0,
                storage_class::STATIC,
            ));
        }
        let mut symbols = Vec::new();
        let mut objname = b"turbolib:".to_vec();
        objname.extend_from_slice(&pch.name);
        write::obj_name(pch.signature, &objname)
            .write_symbol(&mut symbols, 4)
            .expect("a short record");
        let mut s = CV_SIGNATURE_C13.to_le_bytes().to_vec();
        c13::push_subsection(&mut s, sub::DEBUG_S_SYMBOLS, &symbols);
        push(&mut o, b".debug$S", DEBUG_CHARACTERISTICS, s);
        let mut p = CV_SIGNATURE_C13.to_le_bytes().to_vec();
        p.extend_from_slice(&pch.records);
        write::end_precomp(pch.signature)
            .write_type(&mut p)
            .expect("a short record");
        push(&mut o, b".debug$P", DEBUG_CHARACTERISTICS, p);
        o.push_symbol(Symbol::new(
            Name::new(pch.symbol.clone()),
            0,
            SectionNumber::Absolute,
            0,
            storage_class::EXTERNAL,
        ));
        o.sync_section_definitions();
        o.write_skeleton(usize::MAX)
            .expect("a precompiled-types member fits its format")
    }
}

/// A member's index `v`'s new number: built-in types stay; its own records
/// and its PCH's are renumbered.
fn map(
    v: u32,
    base: u32,
    range: Option<debug::Precomp>,
    own_new: &[u32],
    pch_new: &[u32],
) -> Option<u32> {
    if v < FIRST {
        return Some(v);
    }
    if let Some(r) = range
        && v - r.start < r.count
    {
        return pch_new
            .get((v - FIRST) as usize)
            .copied()
            .filter(|&n| n != 0);
    }
    own_new
        .get(v.checked_sub(base)? as usize)
        .copied()
        .filter(|&n| n != 0)
}

/// Adds a section and its section symbol.
fn push(o: &mut Object<'_>, name: &[u8], characteristics: u32, data: Vec<u8>) -> SectionId {
    let id = o.push_section(Section::new(
        Name::new(name.to_vec()),
        characteristics,
        SectionData::Bytes(Cow::Owned(data)),
    ));
    let mut s = Symbol::new(
        Name::new(name.to_vec()),
        0,
        SectionNumber::Section(id),
        0,
        storage_class::STATIC,
    );
    s.aux = Aux::Section(SectionDefinition::default());
    o.push_symbol(s);
    id
}

#[cfg(test)]
mod tests;
