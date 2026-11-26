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

pub mod cluster;

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
    /// A sharer's group (an index into [`Plan::groups`]), or `None` if it
    /// is out of every group (`--precomp=clustered`) and keeps its types.
    pub group: Option<u32>,
}

/// The library's shared types.
pub(super) struct Plan {
    pub members: Vec<Member>,
    lookup: Lookup,
    /// One precompiled-types member for each group whose members share
    /// records: one group for each machine, or under `--precomp=clustered`
    /// the groups [`cluster::choose`] chose.
    pub pchs: Vec<Pch>,
    /// Each group's precompiled-types member (an index into `pchs`), if
    /// its members share records.
    pub groups: Vec<Option<usize>>,
    /// Under `--precomp=clustered`, each machine's choice.
    pub clusters: Vec<Clusters>,
    pub clustered: bool,
    /// Members that could share but whose debug information couldn't be
    /// read through, and why (the first).
    pub unreadable: u64,
    pub first_unreadable: Option<String>,
}

/// One precompiled-types member.
#[derive(Clone, Debug)]
pub(super) struct Pch {
    pub machine: u16,
    pub group: u32,
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

/// A record's group and structural hash to its index in the group's PCH,
/// sharded.
#[derive(Default)]
struct Lookup {
    maps: Vec<HashMap<(u32, u128), u32>>,
}

const LOOKUP_SHARDS: usize = 64;

fn shard(h: u128) -> usize {
    (h >> 64) as usize % LOOKUP_SHARDS
}

impl Lookup {
    fn new(pairs: &[((u32, u128), u32)]) -> Lookup {
        // Each chunk's pairs by shard, then each shard's map from them.
        let chunks: Vec<Vec<Vec<((u32, u128), u32)>>> =
            turbonop_par::map_chunks(pairs, 1 << 16, |chunk| {
                let mut by: Vec<Vec<((u32, u128), u32)>> = vec![Vec::new(); LOOKUP_SHARDS];
                for &(k, i) in chunk {
                    by[shard(k.1)].push((k, i));
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

    fn get(&self, group: u32, h: u128) -> Option<u32> {
        self.maps.get(shard(h))?.get(&(group, h)).copied()
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
/// member and position), its group and depth.
#[derive(Clone, Copy, Debug)]
struct Candidate {
    hash: u128,
    group: u32,
    depth: u32,
    member: u32,
    position: u32,
}

/// How to group the members (`--precomp=clustered`): the numbers of groups
/// to try, and each member's external definitions (as the archive index
/// lists them), for what links load.
pub(super) struct Clustering<'d> {
    pub tries: Vec<u32>,
    /// What a byte an average link reads weighs against a byte of the
    /// library ([`cluster`]).
    pub weight: u64,
    pub definitions: &'d [Vec<Cow<'d, [u8]>>],
}

/// One machine's grouping, for reports.
#[derive(Clone, Debug)]
pub struct Clusters {
    pub machine: u16,
    /// Its sharers, the groups chosen, and the members out of every group.
    pub members: u64,
    pub groups: u32,
    pub out: u64,
    /// The cost model's figures for the choice, for one group (`library`),
    /// and for every member out (`none`), and each number of groups tried.
    pub cost: cluster::Cost,
    pub library: cluster::Cost,
    pub none: cluster::Cost,
    pub tried: Vec<(u32, cluster::Cost)>,
}

/// The shards records are counted in.
fn shards() -> u64 {
    (turbonop_par::threads() * 4).max(1) as u64
}

/// Finds the shared types of `parsed` (a library's members): the records
/// at least `min` members of a group hold. With `clustering`, the groups
/// are chosen ([`cluster`]); without, there is one for each machine.
pub(super) fn plan(parsed: &[Parsed<'_>], min: u32, clustering: Option<&Clustering<'_>>) -> Plan {
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
    let n = shards();
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

    // Each sharer's group, and each group's candidates: one group for each
    // machine, or the ones chosen.
    let Grouped {
        group_of,
        group_machine,
        clusters,
        mut candidates,
    } = match clustering {
        None => by_machine(parsed, &members, &sharers, &lists, min),
        Some(c) => clustered(parsed, &members, &sharers, &lists, min, c),
    };
    drop(lists);
    for (k, &m) in sharers.iter().enumerate() {
        members[m].group = group_of[k];
    }

    // Each group's PCH: by depth, so that each record names only earlier
    // ones; then by hash. A record naming one that can't be shared isn't
    // shared either (none is known to: no type refers to a function id).
    loop {
        turbonop_par::sort_by(&mut candidates, |a, b| {
            (a.group, a.depth, a.hash).cmp(&(b.group, b.depth, b.hash))
        });
        let mut pairs: Vec<((u32, u128), u32)> = Vec::with_capacity(candidates.len());
        let mut runs: Vec<(u32, usize, usize)> = Vec::new();
        for (i, c) in candidates.iter().enumerate() {
            match runs.last_mut() {
                Some(g) if g.0 == c.group => g.2 = i + 1,
                _ => runs.push((c.group, i, i + 1)),
            }
            let start = runs.last().map_or(0, |g| g.1);
            pairs.push(((c.group, c.hash), FIRST + (i - start) as u32));
        }
        let lookup = Lookup::new(&pairs);
        drop(pairs);
        // Each record's bytes, its indices renumbered into the PCH.
        let built: Vec<(Vec<u8>, Vec<(u32, u128)>)> =
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
                            .and_then(|h| lookup.get(c.group, h))
                        {
                            Some(new) => bytes[at..at + 4].copy_from_slice(&new.to_le_bytes()),
                            None => {
                                bad.push((c.group, c.hash));
                                break;
                            }
                        }
                    }
                }
                vec![(bytes, bad)]
            });
        let bad: turbonop_hash::Set<(u32, u128)> =
            built.iter().flat_map(|(_, b)| b.iter().copied()).collect();
        if !bad.is_empty() {
            candidates.retain(|c| !bad.contains(&(c.group, c.hash)));
            continue;
        }
        // The chunks' bytes in order, split by group.
        let mut all = Vec::with_capacity(built.iter().map(|(b, _)| b.len()).sum());
        for (b, _) in built {
            all.extend_from_slice(&b);
        }
        let mut pchs = Vec::new();
        let mut groups = vec![None; group_machine.len()];
        let mut at = 0;
        for &(group, start, end) in &runs {
            let len: usize = candidates[start..end]
                .iter()
                .map(|c| {
                    debug::record(parsed, c.member as usize, base_of(parsed, c) + c.position).len()
                })
                .sum();
            let records = all[at..at + len].to_vec();
            at += len;
            groups[group as usize] = Some(pchs.len());
            pchs.push(pch(
                parsed,
                &members,
                group_machine[group as usize],
                group,
                (end - start) as u32,
                records,
            ));
        }
        return Plan {
            members,
            lookup,
            pchs,
            groups,
            clusters,
            clustered: clustering.is_some(),
            unreadable,
            first_unreadable,
        };
    }
}

/// Each sharer's group (by position in `sharers`), each group's machine,
/// the choices made (for reports), and each group's candidates: the
/// records at least `min` of its members hold.
struct Grouped {
    group_of: Vec<Option<u32>>,
    group_machine: Vec<u16>,
    clusters: Vec<Clusters>,
    candidates: Vec<Candidate>,
}

fn machine_of(parsed: &[Parsed<'_>], m: usize) -> u16 {
    parsed[m].object.as_ref().map_or(0, |o| o.machine)
}

/// One group for each machine (`--precomp=library`).
fn by_machine(
    parsed: &[Parsed<'_>],
    members: &[Member],
    sharers: &[usize],
    lists: &[(Vec<Held>, Vec<u32>)],
    min: u32,
) -> Grouped {
    let mut machines: Vec<u16> = sharers.iter().map(|&m| machine_of(parsed, m)).collect();
    machines.sort_unstable();
    machines.dedup();
    let group_of: Vec<Option<u32>> = sharers
        .iter()
        .map(|&m| {
            machines
                .binary_search(&machine_of(parsed, m))
                .ok()
                .map(|g| g as u32)
        })
        .collect();
    let n = lists.first().map_or(0, |l| l.1.len().saturating_sub(1));
    let shards: Vec<usize> = (0..n).collect();
    let parts: Vec<Vec<Candidate>> = turbonop_par::map_balanced(&shards, 1, |&s| {
        // Each record in each group: how many of the group's members hold
        // it (each counted once: the last member that counted it), and its
        // first copy.
        let mut counts: HashMap<(u128, u32), (u32, u32, u32, u32)> = HashMap::default();
        for (k, (list, bounds)) in lists.iter().enumerate() {
            let Some(g) = group_of[k] else { continue };
            let k = k as u32;
            for &(h, a, b) in &list[bounds[s] as usize..bounds[s + 1] as usize] {
                let e = counts.entry((h, g)).or_insert((0, a, b, u32::MAX));
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
            .map(|((hash, group), (_, a, b, _))| Candidate {
                hash,
                group,
                depth: members[a as usize].depth[b as usize],
                member: a,
                position: b,
            })
            .collect()
    });
    Grouped {
        group_of,
        group_machine: machines,
        clusters: Vec::new(),
        candidates: parts.into_iter().flatten().collect(),
    }
}

/// A step of the holder-set hashes (splitmix64's finalizer).
fn mix(x: u64) -> u64 {
    let x = (x ^ (x >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    let x = (x ^ (x >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    x ^ (x >> 31)
}

/// A record two or more sharers hold: its hash, a hash of its holders (in
/// order), its first copy, and its bytes.
type Shared = (u128, u128, u32, u32, u32);

/// The groups chosen for each machine's sharers ([`cluster::choose`]).
fn clustered(
    parsed: &[Parsed<'_>],
    members: &[Member],
    sharers: &[usize],
    lists: &[(Vec<Held>, Vec<u32>)],
    min: u32,
    clustering: &Clustering<'_>,
) -> Grouped {
    // Each machine's sharers, and each sharer's machine and position.
    let mut machines: Vec<u16> = sharers.iter().map(|&m| machine_of(parsed, m)).collect();
    machines.sort_unstable();
    machines.dedup();
    let mut positions: Vec<Vec<usize>> = vec![Vec::new(); machines.len()];
    let mut where_: Vec<(usize, u32)> = Vec::with_capacity(sharers.len());
    for (k, &m) in sharers.iter().enumerate() {
        let i = machines
            .binary_search(&machine_of(parsed, m))
            .expect("a machine");
        where_.push((i, positions[i].len() as u32));
        positions[i].push(k);
    }
    let n = lists.first().map_or(0, |l| l.1.len().saturating_sub(1));
    let shards: Vec<usize> = (0..n).collect();
    // The records two or more sharers hold, with a hash of their holders
    // (each sharer counted once: the last one that counted it).
    let parts: Vec<Vec<Shared>> = turbonop_par::map_balanced(&shards, 1, |&s| {
        let mut held: HashMap<u128, (u32, u32, u64, u64, u32, u32)> = HashMap::default();
        for (k, (list, bounds)) in lists.iter().enumerate() {
            let k = k as u32;
            for &(h, a, b) in &list[bounds[s] as usize..bounds[s + 1] as usize] {
                let e = held
                    .entry(h)
                    .or_insert((0, u32::MAX, 0x5EED, 0x0DDB_A11, a, b));
                if e.1 != k {
                    e.0 += 1;
                    e.1 = k;
                    e.2 = mix(e.2 ^ u64::from(k));
                    e.3 = mix(e.3.wrapping_add(u64::from(k) + 1));
                }
                if (a, b) < (e.4, e.5) {
                    (e.4, e.5) = (a, b);
                }
            }
        }
        held.into_iter()
            .filter(|&(_, e)| e.0 >= 2)
            .map(|(h, (_, _, x, y, a, b))| {
                let c = Candidate {
                    hash: h,
                    group: 0,
                    depth: 0,
                    member: a,
                    position: b,
                };
                let len = debug::record(parsed, a as usize, base_of(parsed, &c) + b).len();
                (h, (u128::from(x) << 64) | u128::from(y), a, b, len as u32)
            })
            .collect()
    });
    // The classes: records by holders, each with its bytes and the record
    // of least hash, whose holders it has.
    let mut by_set: HashMap<u128, (u64, u128)> = HashMap::default();
    for part in &parts {
        for &(h, set, _, _, len) in part {
            let e = by_set.entry(set).or_insert((0, h));
            e.0 += u64::from(len);
            e.1 = e.1.min(h);
        }
    }
    let mut sets: Vec<(u128, u64, u128)> =
        by_set.into_iter().map(|(k, (b, r))| (k, b, r)).collect();
    sets.sort_unstable_by_key(|x| x.0);
    let class_of: HashMap<u128, u32> = sets
        .iter()
        .enumerate()
        .map(|(i, x)| (x.0, i as u32))
        .collect();
    let reps: HashMap<u128, u32> = sets
        .iter()
        .enumerate()
        .map(|(i, x)| (x.2, i as u32))
        .collect();
    // Each class's holders: the sharers holding its record of least hash.
    let found: Vec<Vec<(u32, u32)>> = turbonop_par::map_balanced(&shards, 1, |&s| {
        let mut v = Vec::new();
        for (k, (list, bounds)) in lists.iter().enumerate() {
            for &(h, _, _) in &list[bounds[s] as usize..bounds[s + 1] as usize] {
                if let Some(&c) = reps.get(&h) {
                    v.push((c, k as u32));
                }
            }
        }
        v
    });
    let mut pairs: Vec<(u32, u32)> = found.into_iter().flatten().collect();
    pairs.sort_unstable();
    pairs.dedup();
    let mut holders: Vec<Vec<u32>> = vec![Vec::new(); sets.len()];
    for (c, k) in pairs {
        holders[c as usize].push(k);
    }
    // By machine (each class's holders are one machine's), as positions.
    let mut by_machine: Vec<Vec<cluster::Class>> = vec![Vec::new(); machines.len()];
    for (c, ks) in holders.iter().enumerate() {
        let i = ks.first().map_or(0, |&k| where_[k as usize].0);
        by_machine[i].push(cluster::Class {
            bytes: sets[c].1,
            holders: ks.iter().map(|&k| where_[k as usize].1).collect(),
        });
    }

    // What each member loads: the first definer of each name it refers to
    // and doesn't define.
    let mut first: HashMap<&[u8], u32> = HashMap::default();
    for (m, d) in clustering.definitions.iter().enumerate() {
        for name in d {
            first.entry(name.as_ref()).or_insert(m as u32);
        }
    }
    let edges: Vec<Vec<u32>> = turbonop_par::map_range(parsed.len(), 16, |m| {
        let Some(o) = &parsed[m].object else {
            return Vec::new();
        };
        let mut e: Vec<u32> = o
            .symbols
            .iter()
            .filter(|s| s.is_external() && s.section == SectionNumber::Undefined && s.value == 0)
            .filter_map(|s| first.get(s.name.as_bytes()).copied())
            .filter(|&d| d != m as u32)
            .collect();
        e.sort_unstable();
        e.dedup();
        e
    });
    // Each sharer's closure, as positions among its machine's sharers.
    let mut position_of: Vec<Option<(usize, u32)>> = vec![None; parsed.len()];
    for (k, &m) in sharers.iter().enumerate() {
        position_of[m] = Some(where_[k]);
    }
    let closures: Vec<Vec<u32>> = turbonop_par::map_range(sharers.len(), 1, |k| {
        let (s, machine) = (sharers[k], where_[k].0);
        let mut seen = vec![false; parsed.len()];
        let mut stack = vec![s];
        seen[s] = true;
        let mut out = Vec::new();
        while let Some(x) = stack.pop() {
            if let Some((i, p)) = position_of[x]
                && i == machine
            {
                out.push(p);
            }
            for &y in &edges[x] {
                if !seen[y as usize] {
                    seen[y as usize] = true;
                    stack.push(y as usize);
                }
            }
        }
        out.sort_unstable();
        out
    });

    // Each machine's choice, its groups numbered after the earlier
    // machines'.
    let mut group_of = vec![None; sharers.len()];
    let mut group_machine: Vec<u16> = Vec::new();
    let mut clusters = Vec::new();
    for (i, &machine) in machines.iter().enumerate() {
        let seeds: Vec<Vec<u32>> = positions[i].iter().map(|&k| closures[k].clone()).collect();
        let choice = cluster::choose(
            positions[i].len(),
            &by_machine[i],
            &seeds,
            min,
            clustering.weight,
            &clustering.tries,
        );
        let first = group_machine.len() as u32;
        group_machine.extend(std::iter::repeat_n(machine, choice.groups as usize));
        for (&k, g) in positions[i].iter().zip(&choice.group) {
            group_of[k] = g.map(|g| first + g);
        }
        clusters.push(Clusters {
            machine,
            members: positions[i].len() as u64,
            groups: choice.groups,
            out: choice.group.iter().filter(|g| g.is_none()).count() as u64,
            cost: choice.cost,
            library: choice.library,
            none: choice.none,
            tried: choice.tried,
        });
    }
    // Each class's groups: those holding at least `min` of its members.
    let class_groups: Vec<Vec<u32>> = holders
        .iter()
        .map(|ks| {
            let mut gs: Vec<u32> = ks.iter().filter_map(|&k| group_of[k as usize]).collect();
            gs.sort_unstable();
            let mut out = Vec::new();
            let mut i = 0;
            while i < gs.len() {
                let j = gs[i..].iter().take_while(|&&g| g == gs[i]).count();
                if j as u32 >= min {
                    out.push(gs[i]);
                }
                i += j;
            }
            out
        })
        .collect();
    let candidates: Vec<Candidate> = parts
        .iter()
        .flatten()
        .flat_map(|&(hash, set, a, b, _)| {
            let c = class_of[&set] as usize;
            class_groups[c].iter().map(move |&group| Candidate {
                hash,
                group,
                depth: members[a as usize].depth[b as usize],
                member: a,
                position: b,
            })
        })
        .collect();
    Grouped {
        group_of,
        group_machine,
        clusters,
        candidates,
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
    group: u32,
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
        if mem.role != Role::Sharer || mem.group != Some(group) {
            continue;
        }
        if let Some(f) = parsed[m].facts.feat00 {
            feat00 = Some(feat00.map_or(f, |a| a & f));
        }
    }
    Pch {
        machine,
        group,
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
    /// The precompiled-types member of `group`, if its members share
    /// records.
    pub(super) fn pch(&self, group: u32) -> Option<&Pch> {
        Some(&self.pchs[(*self.groups.get(group as usize)?)?])
    }

    /// The precompiled-types member a synthesized member holding units of
    /// `members` (one for each unit) names: the one most of their members
    /// name (the first group of those). With one group for each machine,
    /// its machine's, whether or not they share.
    pub(super) fn pch_for_units(
        &self,
        machine: u16,
        members: impl Iterator<Item = usize>,
    ) -> Option<&Pch> {
        if !self.clustered {
            return self.pchs.iter().find(|p| p.machine == machine);
        }
        let mut votes: Vec<u64> = vec![0; self.groups.len()];
        for m in members {
            if let Some(g) = self.members.get(m).and_then(|x| x.group)
                && self.pch(g).is_some_and(|p| p.machine == machine)
            {
                votes[g as usize] += 1;
            }
        }
        let best = votes.iter().copied().max().filter(|&v| v > 0)?;
        let g = votes.iter().position(|&v| v == best)?;
        self.pch(g as u32)
    }

    /// The index in `pch` of member `m`'s record `v` (from `FIRST`), if it
    /// is shared there.
    pub(super) fn shared(&self, pch: &Pch, parsed: &[Parsed<'_>], m: usize, v: u32) -> Option<u32> {
        self.lookup
            .get(pch.group, hash_of(parsed, &self.members, m, v)?)
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
        let pch = self.pch(mem.group?)?;
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
                    .then(|| self.lookup.get(pch.group, sm.hashes[j as usize]))
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
                .then(|| self.lookup.get(pch.group, mem.hashes[i]))
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
        // S_OBJNAME names exactly what the users' LF_PRECOMP names:
        // link.exe pairs a user with a PCH object whose S_OBJNAME path, or
        // the name it is linked under (its member's), equals the user's
        // path, ignoring case, and whose signature is the user's (measured
        // with link.exe 14.52). With `turbolib:` before it, only the member
        // name matched, so a library re-archived under other member names
        // (lib.exe keeps the paths it is given) lost every user's types.
        write::obj_name(pch.signature, &pch.name)
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
