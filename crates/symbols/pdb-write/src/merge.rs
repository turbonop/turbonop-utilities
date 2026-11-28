//! Merging CodeView type and id records from many sources (objects'
//! `.debug$T`, and type server PDBs for `/Zi`) into one TPI and one IPI,
//! on every core.
//!
//! The approach is lld's GHASH: each record's hash covers its bytes and,
//! in place of each index it holds, the hash of the record that index
//! names, so that two records are the same exactly when their hashes are
//! (128 bits: a collision is far less likely than a cosmic ray). Each
//! source hashes its records in order (a record only refers to earlier
//! ones) and the sources hash in parallel. The hashes are then binned by
//! shard; each shard, on its own thread, keeps a hash's first occurrence
//! in (source, record) order; the winners, sorted into that order, are
//! numbered from 0x1000; and the records are rewritten with their indices
//! renumbered, in parallel, into the streams' buffers.

use turbonop_codeview::type_refs::{self, Ref};

/// Id records' kinds (they go in the IPI).
pub fn is_id_kind(kind: u16) -> bool {
    (0x1601..=0x1607).contains(&kind)
}

/// Kinds that are dropped: they describe how types were stored, not types
/// (`LF_ENDPRECOMP`, `LF_TYPESERVER2`, `LF_TYPESERVER`, `LF_PRECOMP`).
fn is_dropped_kind(kind: u16) -> bool {
    matches!(kind, 0x0014 | 0x1515 | 0x1501 | 0x1509)
}

/// Where type records come from.
pub enum Source<'a> {
    /// An object's records (whole records: length, kind, body), in one
    /// index space from 0x1000 (for `/Yu`, the precompiled header's first).
    Object(Vec<&'a [u8]>),
    /// A type server's (a PDB's) TPI and IPI records.
    Pdb {
        /// TPI records.
        types: Vec<&'a [u8]>,
        /// IPI records.
        ids: Vec<&'a [u8]>,
        /// Its `/names` (what its `LF_UDT_MOD_SRC_LINE` records name).
        names: Option<turbonop_pdb::StringTable<'a>>,
    },
}

/// A source's new indices.
#[derive(Clone, Debug, Default)]
pub struct IndexMap {
    /// For an object, every record's; for a PDB, its TPI records'.
    pub types: Vec<u32>,
    /// For a PDB, its IPI records' (empty for an object).
    pub ids: Vec<u32>,
    /// For an object, whether each record is an id (its new index the
    /// IPI's).
    pub is_id: Vec<bool>,
    /// Why the source's records couldn't be merged, if they couldn't (its
    /// symbols' indices are then all 0, `T_NOTYPE`).
    pub error: Option<String>,
}

impl IndexMap {
    /// The new index of `ti` (an index into the source's types, or its ids
    /// if `id`, for a PDB source).
    pub fn map(&self, ti: u32, id: bool) -> u32 {
        if ti < 0x1000 {
            return ti;
        }
        let table = if id && !self.ids.is_empty() {
            &self.ids
        } else {
            &self.types
        };
        table.get((ti - 0x1000) as usize).copied().unwrap_or(0)
    }
}

/// A merged stream: records (each 4-byte aligned), each one's offset, and
/// each one's TPI hash.
#[derive(Debug, Default)]
pub struct Stream {
    /// The records.
    pub records: Vec<u8>,
    /// Each record's offset.
    pub offsets: Vec<u32>,
    /// Each record's hash ([`crate::streams::type_hash`]).
    pub hashes: Vec<u32>,
    /// Each record's source (the first that has it).
    pub sources: Vec<u32>,
}

/// The merge's result.
#[derive(Debug, Default)]
pub struct Merged {
    /// The TPI's records.
    pub types: Stream,
    /// The IPI's records.
    pub ids: Stream,
    /// Each source's new indices.
    pub maps: Vec<IndexMap>,
    /// The kept UDTs' source lines (one each), for the linker to write as
    /// `LF_UDT_MOD_SRC_LINE`: the UDT, its file's name, its line, and the
    /// source.
    pub src_lines: Vec<(u32, Vec<u8>, u32, u32)>,
}

/// A 128-bit hash, two lanes of folded multiplies.
#[derive(Clone, Copy)]
struct Hasher {
    a: u64,
    b: u64,
}

const K: [u64; 4] = [
    0x9E37_79B9_7F4A_7C15,
    0xC2B2_AE3D_27D4_EB4F,
    0x1656_67B1_9E37_79F9,
    0xD6E8_FEB8_6659_FD93,
];

fn fold(x: u64, y: u64) -> u64 {
    let p = u128::from(x) * u128::from(y);
    crate::lo(p) ^ crate::hi(p)
}

impl Hasher {
    fn new(kind: u16, len: usize) -> Hasher {
        let s = u64::from(kind) | ((len as u64) << 16);
        Hasher {
            a: fold(s ^ K[0], K[1]),
            b: fold(s ^ K[2], K[3]),
        }
    }

    fn word(&mut self, w: u64) {
        self.a = fold(self.a ^ w, K[1]);
        self.b = fold(self.b ^ w.rotate_left(29), K[3]);
    }

    fn bytes(&mut self, bytes: &[u8]) {
        let (words, rest) = bytes.as_chunks::<8>();
        for w in words {
            self.word(u64::from_le_bytes(*w));
        }
        if !rest.is_empty() {
            let mut last = [0u8; 8];
            last[..rest.len()].copy_from_slice(rest);
            self.word(u64::from_le_bytes(last) ^ (rest.len() as u64) << 59);
        }
    }

    fn finish(self) -> u128 {
        let a = fold(self.a ^ K[2], self.b ^ K[0]);
        let b = fold(self.b ^ K[1], self.a ^ K[3]);
        (u128::from(a) << 64) | u128::from(b)
    }
}

/// One record's hash: its kind, its bytes, and in place of each index
/// its target's hash (`hash_of`: the hash of a non-simple index's record).
fn record_hash(
    record: &[u8],
    refs: &mut Vec<Ref>,
    hash_of: impl Fn(u32, bool) -> Option<u128>,
) -> Result<u128, String> {
    if record.len() < 4 {
        return Err("a truncated type record".into());
    }
    let kind = u16::from_le_bytes([record[2], record[3]]);
    let body = &record[4..];
    refs.clear();
    type_refs::refs(kind, body, refs).map_err(|e| e.to_string())?;
    let mut h = Hasher::new(kind, body.len());
    let mut at = 0usize;
    for r in refs.iter() {
        let o = r.offset as usize;
        h.bytes(&body[at..o]);
        let ti = u32::from_le_bytes([body[o], body[o + 1], body[o + 2], body[o + 3]]);
        if ti < 0x1000 {
            h.word(u64::from(ti) | (1 << 63));
        } else {
            let t = hash_of(ti, r.id).ok_or_else(|| {
                format!("a type record (kind {kind:#06x}) refers to {ti:#x}, not an earlier record")
            })?;
            h.word(crate::hi(t));
            h.word(crate::lo(t));
        }
        at = o + 4;
    }
    h.bytes(&body[at..]);
    Ok(h.finish())
}

/// A source's records' hashes: its types (or every record, for an object)
/// and its ids.
type HashedSource = (Vec<Hashed>, Vec<Hashed>);

/// One record's place: its hash, whether it is an id, whether it is
/// dropped.
#[derive(Clone, Copy)]
struct Hashed {
    hash: u128,
    id: bool,
    dropped: bool,
}

/// Hashes a source's records (or says why they can't be).
fn hash_source(source: &Source<'_>) -> Result<(Vec<Hashed>, Vec<Hashed>), String> {
    let mut refs = Vec::new();
    match source {
        Source::Object(records) => {
            let mut out: Vec<Hashed> = Vec::with_capacity(records.len());
            for r in records {
                let kind = if r.len() >= 4 {
                    u16::from_le_bytes([r[2], r[3]])
                } else {
                    0
                };
                if is_dropped_kind(kind) {
                    out.push(Hashed {
                        hash: 0,
                        id: false,
                        dropped: true,
                    });
                    continue;
                }
                let hash = record_hash(r, &mut refs, |ti, _| {
                    let h = out.get((ti - 0x1000) as usize)?;
                    (!h.dropped).then_some(h.hash)
                })?;
                out.push(Hashed {
                    hash,
                    id: is_id_kind(kind),
                    dropped: false,
                });
            }
            Ok((out, Vec::new()))
        }
        Source::Pdb { types, ids, .. } => {
            let mut t: Vec<Hashed> = Vec::with_capacity(types.len());
            for r in types {
                let hash = record_hash(r, &mut refs, |ti, _| {
                    t.get((ti - 0x1000) as usize).map(|h| h.hash)
                })?;
                t.push(Hashed {
                    hash,
                    id: false,
                    dropped: false,
                });
            }
            let mut d: Vec<Hashed> = Vec::with_capacity(ids.len());
            for r in ids {
                let hash = record_hash(r, &mut refs, |ti, id| {
                    let table = if id { &d } else { &t };
                    table.get((ti - 0x1000) as usize).map(|h| h.hash)
                })?;
                d.push(Hashed {
                    hash,
                    id: true,
                    dropped: false,
                });
            }
            Ok((t, d))
        }
    }
}

const SHARDS: usize = 64;

/// A hasher for keys that are hashes already: their high 64 bits.
#[derive(Default)]
struct PassThrough(u64);

impl std::hash::Hasher for PassThrough {
    fn finish(&self) -> u64 {
        self.0
    }

    fn write(&mut self, bytes: &[u8]) {
        for &b in bytes {
            self.0 = self.0.rotate_left(8) ^ u64::from(b);
        }
    }

    fn write_u128(&mut self, i: u128) {
        self.0 = crate::hi(i);
    }
}

/// A shard's map: a record's hash to its slot.
type HashMap = std::collections::HashMap<u128, u32, std::hash::BuildHasherDefault<PassThrough>>;

fn shard(h: u128) -> usize {
    (h as usize) % SHARDS
}

/// The record a winner is: (source, which table (0 types or object, 1
/// PDB ids), index).
type Place = (u32, u8, u32);

/// A winner: its place, its shard, its slot there.
type Winner = (Place, u32, u32);

/// A source's records by shard: (hash, table, index).
type Bins = Vec<Vec<(u128, u8, u32)>>;

/// Merges `sources`' records.
pub fn merge(
    sources: &[Source<'_>],
    roots: impl FnOnce(&[IndexMap]) -> (Vec<u32>, Vec<u32>),
) -> Merged {
    let timing = std::env::var_os("TURBOLINK_PDB_TIME").is_some();
    let mut last = std::time::Instant::now();
    let mut lap = |what: &str| {
        if timing {
            eprintln!(
                "  merge: {what}: {:.1} ms",
                last.elapsed().as_secs_f64() * 1000.0
            );
        }
        last = std::time::Instant::now();
    };
    // 1. Every source's hashes, in parallel.
    // (Each source's records binned by shard as they are hashed.)
    let (hashed, binned): (Vec<Result<HashedSource, String>>, Vec<Bins>) =
        turbonop_par::map_balanced(sources, 1, |src| {
            let h = hash_source(src);
            let mut bins: Vec<Vec<(u128, u8, u32)>> = vec![Vec::new(); SHARDS];
            if let Ok((a, b)) = &h {
                for (t, list) in [(0u8, a), (1u8, b)] {
                    for (i, x) in list.iter().enumerate() {
                        if !x.dropped {
                            bins[shard(x.hash)].push((x.hash, t, crate::n32(i)));
                        }
                    }
                }
            }
            (h, bins)
        })
        .into_iter()
        .unzip();
    lap("hash");
    // 2. Each source's records, by shard (records that are dropped, or of
    // a source that failed, take no part).
    let empty: (Vec<Hashed>, Vec<Hashed>) = (Vec::new(), Vec::new());
    let tables = |s: usize| -> &(Vec<Hashed>, Vec<Hashed>) { hashed[s].as_ref().unwrap_or(&empty) };
    // 3. Each shard's first occurrences, in (source, record) order (the
    // keys are hashes already: their high half is the map's hash).
    let shards: Vec<(HashMap, Vec<Place>)> = turbonop_par::map_range(SHARDS, 1, |k| {
        let n: usize = binned.iter().map(|b| b[k].len()).sum();
        let mut map = HashMap::with_capacity_and_hasher(n, Default::default());
        let mut winners: Vec<Place> = Vec::new();
        for (s, bins) in binned.iter().enumerate() {
            for &(hash, t, i) in &bins[k] {
                map.entry(hash).or_insert_with(|| {
                    winners.push((u32::try_from(s).expect("few sources"), t, i));
                    u32::try_from(winners.len() - 1).expect("small")
                });
            }
        }
        (map, winners)
    });
    drop(binned);
    lap("shards");
    // 4. The winners in order, types and ids apart; their new indices.
    let is_id = |&(s, t, i): &Place| {
        let (a, b) = tables(s as usize);
        if t == 0 {
            a[i as usize].id
        } else {
            b[i as usize].id
        }
    };
    // Each shard's winners are in source order: each source gathers its
    // own from every shard, in parallel, and puts them in record order.
    let per_source: Vec<(Vec<Winner>, Vec<Winner>)> =
        turbonop_par::map_range(sources.len(), 1, |s| {
            let s32 = u32::try_from(s).expect("few sources");
            let mut mine: Vec<(Place, u32, u32)> = Vec::new();
            for (k, (_, w)) in shards.iter().enumerate() {
                let from = w.partition_point(|p| p.0 < s32);
                let to = w.partition_point(|p| p.0 <= s32);
                for (slot, p) in w[from..to].iter().enumerate() {
                    mine.push((*p, crate::n32(k), crate::n32(from + slot)));
                }
            }
            mine.sort_unstable_by_key(|x| (x.0.1, x.0.2));
            mine.into_iter().partition(|(p, _, _)| !is_id(p))
        });
    let mut ty: Vec<(Place, u32, u32)> = Vec::new();
    let mut id: Vec<(Place, u32, u32)> = Vec::new();
    for (t, i) in per_source {
        ty.extend(t);
        id.extend(i);
    }
    let mut new_index: Vec<Vec<u32>> = shards.iter().map(|(_, w)| vec![0; w.len()]).collect();
    for list in [&ty, &id] {
        for (n, &(_, k, slot)) in list.iter().enumerate() {
            new_index[k as usize][slot as usize] =
                0x1000 + u32::try_from(n).expect("fewer than 4 billion");
        }
    }
    lap("order");
    // 5. Each source's map, in parallel.
    let lookup = |h: u128| -> u32 {
        let k = shard(h);
        shards[k]
            .0
            .get(&h)
            .map_or(0, |&slot| new_index[k][slot as usize])
    };
    let order: Vec<usize> = (0..sources.len()).collect();
    let maps: Vec<IndexMap> = turbonop_par::map_balanced(&order, 1, |&s| match &hashed[s] {
        Err(e) => IndexMap {
            types: Vec::new(),
            ids: Vec::new(),
            is_id: Vec::new(),
            error: Some(e.clone()),
        },
        Ok((a, b)) => IndexMap {
            types: a
                .iter()
                .map(|h| if h.dropped { 0 } else { lookup(h.hash) })
                .collect(),
            ids: b.iter().map(|h| lookup(h.hash)).collect(),
            is_id: if matches!(sources[s], Source::Object(_)) {
                a.iter().map(|h| h.id).collect()
            } else {
                Vec::new()
            },
            error: None,
        },
    });
    lap("maps");
    // 6. What the image refers to: the roots (from the maps), and what
    // they name, through the sources' records.
    let record = |&(s, t, i): &Place| -> &[u8] {
        match &sources[s as usize] {
            Source::Object(r) => r[i as usize],
            Source::Pdb { types, ids, .. } => {
                if t == 0 {
                    types[i as usize]
                } else {
                    ids[i as usize]
                }
            }
        }
    };
    // A source's local index (in its types, or ids if `id`) as a winner:
    // (whether it is an id, its index before pruning).
    let target = |s: usize, ti: u32, id: bool| -> Option<(bool, u32)> {
        if ti < 0x1000 {
            return None;
        }
        let m = &maps[s];
        let local = (ti - 0x1000) as usize;
        let (class, g) = if m.is_id.is_empty() {
            (id, m.map(ti, id))
        } else {
            (*m.is_id.get(local)?, *m.types.get(local)?)
        };
        (g >= 0x1000).then(|| (class, g - 0x1000))
    };
    let (type_roots, id_roots) = roots(&maps);
    // Each forward reference's definitions (by tag key, in order): the
    // keys worked out in parallel.
    let keys = turbonop_par::map(&ty, 1024, |(p, _, _)| match tag_key(record(p)) {
        Some((false, class, key)) => Some((class, key)),
        _ => None,
    });
    let mut defs: turbonop_hash::Map<(u16, &[u8]), Vec<u32>> = turbonop_hash::Map::default();
    for (g, k) in keys.into_iter().enumerate() {
        if let Some(k) = k {
            defs.entry(k).or_default().push(crate::n32(g));
        }
    }
    lap("reach: definitions");
    // Everything the roots reach, by whichever thread is free (the result
    // is the same in any order).
    use std::sync::atomic::{AtomicBool, Ordering};
    let seen_t: Vec<AtomicBool> = (0..ty.len()).map(|_| AtomicBool::new(false)).collect();
    let seen_i: Vec<AtomicBool> = (0..id.len()).map(|_| AtomicBool::new(false)).collect();
    let claim = |(is_id, g): (bool, u32)| {
        let seen = if is_id { &seen_i } else { &seen_t };
        seen.get(g as usize)
            .is_some_and(|b| !b.swap(true, Ordering::Relaxed))
    };
    let roots: Vec<(bool, u32)> = type_roots
        .iter()
        .filter(|&&t| t >= 0x1000)
        .map(|&t| (false, t - 0x1000))
        .chain(
            id_roots
                .iter()
                .filter(|&&t| t >= 0x1000)
                .map(|&t| (true, t - 0x1000)),
        )
        .filter(|&x| claim(x))
        .collect();
    let parallel = ty.len() + id.len() >= 4096;
    turbonop_par::traverse(roots, parallel, Vec::new, |(is_id, g), more, refs| {
        let list = if is_id { &id } else { &ty };
        let p = &list[g as usize].0;
        let r = record(p);
        let kind = u16::from_le_bytes([r[2], r[3]]);
        // (Source lines are written apart, below.)
        if matches!(kind, 0x1606 | 0x1607) {
            return;
        }
        refs.clear();
        let _ = type_refs::refs(kind, &r[4..], refs);
        for f in refs.iter() {
            let o = 4 + f.offset as usize;
            let ti = u32::from_le_bytes([r[o], r[o + 1], r[o + 2], r[o + 3]]);
            if let Some(x) = target(p.0 as usize, ti, f.id)
                && claim(x)
            {
                more.push(x);
            }
        }
        if !is_id && let Some((true, class, key)) = tag_key(r) {
            for &d in defs.get(&(class, key)).into_iter().flatten() {
                if claim((false, d)) {
                    more.push((false, d));
                }
            }
        }
    });
    drop(defs);
    // Kept: what was reached, but source lines (written apart).
    let kept = |list: &[(Place, u32, u32)], seen: &[AtomicBool]| -> Vec<bool> {
        turbonop_par::map_range(list.len(), 4096, |g| {
            seen[g].load(Ordering::Relaxed) && {
                let r = record(&list[g].0);
                !matches!(u16::from_le_bytes([r[2], r[3]]), 0x1606 | 0x1607)
            }
        })
    };
    let keep_t = kept(&ty, &seen_t);
    let keep_i = kept(&id, &seen_i);
    drop((seen_t, seen_i));
    lap("reach: walk");
    // Source lines of kept UDTs, one each (the first source's).
    let mut lined = vec![false; ty.len()];
    let mut src_lines: Vec<(u32, Vec<u8>, u32, u32)> = Vec::new();
    for (p, _, _) in &id {
        let r = record(p);
        let kind = u16::from_le_bytes([r[2], r[3]]);
        if !matches!(kind, 0x1606 | 0x1607) || r.len() < 16 {
            continue;
        }
        let u32_at = |o: usize| u32::from_le_bytes([r[o], r[o + 1], r[o + 2], r[o + 3]]);
        let s = p.0 as usize;
        let Some((false, udt)) = target(s, u32_at(4), false) else {
            continue;
        };
        if !keep_t[udt as usize] || lined[udt as usize] {
            continue;
        }
        let file: Vec<u8> = if kind == 0x1606 {
            // An LF_STRING_ID: the substring list, then the name.
            let src = u32_at(8);
            let string = match &sources[s] {
                Source::Object(recs) => src
                    .checked_sub(0x1000)
                    .and_then(|i| recs.get(i as usize))
                    .copied(),
                Source::Pdb { ids, .. } => src
                    .checked_sub(0x1000)
                    .and_then(|i| ids.get(i as usize))
                    .copied(),
            };
            match string {
                Some(x) if x.len() > 8 && u16::from_le_bytes([x[2], x[3]]) == 0x1605 => {
                    let n = &x[8..];
                    n[..n.iter().position(|&b| b == 0).unwrap_or(n.len())].to_vec()
                }
                _ => continue,
            }
        } else {
            match &sources[s] {
                Source::Pdb {
                    names: Some(names), ..
                } => names.get(u32_at(8)).unwrap_or_default().to_vec(),
                _ => continue,
            }
        };
        lined[udt as usize] = true;
        src_lines.push((udt, file, u32_at(12), p.0));
    }
    lap("reach: source lines");
    // Renumbered: the kept records, in order.
    let renumber = |keep: &[bool]| -> Vec<u32> {
        let mut n = 0x1000u32;
        keep.iter()
            .map(|&k| {
                if k {
                    n += 1;
                    n - 1
                } else {
                    0
                }
            })
            .collect()
    };
    let new_t = renumber(&keep_t);
    let new_i = renumber(&keep_i);
    let to_new = |table: &[u32], g: u32| -> u32 {
        if g < 0x1000 {
            g
        } else {
            table.get((g - 0x1000) as usize).copied().unwrap_or(0)
        }
    };
    for x in &mut src_lines {
        x.0 = new_t[x.0 as usize];
    }
    let mut maps = maps;
    turbonop_par::for_each_chunk_mut(&mut maps, 1, |ms| {
        for m in ms.iter_mut() {
            if m.is_id.is_empty() {
                for v in &mut m.types {
                    *v = to_new(&new_t, *v);
                }
                for v in &mut m.ids {
                    *v = to_new(&new_i, *v);
                }
            } else {
                for (v, &i) in m.types.iter_mut().zip(&m.is_id) {
                    *v = if i {
                        to_new(&new_i, *v)
                    } else {
                        to_new(&new_t, *v)
                    };
                }
            }
        }
    });
    let ty: Vec<(Place, u32, u32)> = ty
        .into_iter()
        .zip(&keep_t)
        .filter(|(_, k)| **k)
        .map(|(x, _)| x)
        .collect();
    let id: Vec<(Place, u32, u32)> = id
        .into_iter()
        .zip(&keep_i)
        .filter(|(_, k)| **k)
        .map(|(x, _)| x)
        .collect();
    lap("reach");
    // 7. The kept records, rewritten, in parallel.
    let emit = |list: &[(Place, u32, u32)]| -> Stream {
        let places: Vec<Place> = list.iter().map(|x| x.0).collect();
        let sizes: Vec<u32> = places
            .iter()
            .map(|p| u32::try_from(record(p).len().next_multiple_of(4)).expect("small"))
            .collect();
        let mut offsets = Vec::with_capacity(sizes.len());
        let mut total = 0u32;
        for &z in &sizes {
            offsets.push(total);
            total = total.checked_add(z).expect("type records under 4 GB");
        }
        let mut records = vec![0u8; total as usize];
        let mut hashes = vec![0u32; places.len()];
        // Chunks of records, each given its part of the buffer.
        const CHUNK: usize = 2048;
        let mut parts: Vec<(usize, &mut [u8], &mut [u32])> = Vec::new();
        let mut rest: &mut [u8] = &mut records;
        let mut hrest: &mut [u32] = &mut hashes;
        let mut start = 0usize;
        while start < places.len() {
            let end = (start + CHUNK).min(places.len());
            let bytes = offsets.get(end).map_or(total, |&o| o) - offsets[start];
            let (mine, tail) = rest.split_at_mut(bytes as usize);
            let (hm, htail) = hrest.split_at_mut(end - start);
            parts.push((start, mine, hm));
            rest = tail;
            hrest = htail;
            start = end;
        }
        turbonop_par::for_each_chunk_mut(&mut parts, 1, |ps| {
            let mut refs = Vec::new();
            for (start, out, hs) in ps.iter_mut() {
                let mut at = 0usize;
                for (j, h) in hs.iter_mut().enumerate() {
                    let p = &places[*start + j];
                    let r = record(p);
                    let n = r.len().next_multiple_of(4);
                    let dst = &mut out[at..at + n];
                    dst[..r.len()].copy_from_slice(r);
                    // Padding (LF_PAD3, LF_PAD2, LF_PAD1), and the length.
                    let pad = n - r.len();
                    for (q, b) in dst[r.len()..].iter_mut().enumerate() {
                        *b = 0xF0 + u8::try_from(pad - q).expect("small");
                    }
                    let len = u16::try_from(n - 2).expect("a record under 64 KB");
                    dst[..2].copy_from_slice(&len.to_le_bytes());
                    let kind = u16::from_le_bytes([r[2], r[3]]);
                    refs.clear();
                    if type_refs::refs(kind, &r[4..], &mut refs).is_ok() {
                        let map = &maps[p.0 as usize];
                        for f in &refs {
                            let o = 4 + f.offset as usize;
                            let ti =
                                u32::from_le_bytes([dst[o], dst[o + 1], dst[o + 2], dst[o + 3]]);
                            let id = match &sources[p.0 as usize] {
                                Source::Object(_) => false,
                                Source::Pdb { .. } => f.id,
                            };
                            dst[o..o + 4].copy_from_slice(&map.map(ti, id).to_le_bytes());
                        }
                    }
                    *h = crate::streams::type_hash(dst);
                    at += n;
                }
            }
        });
        drop(parts);
        Stream {
            records,
            offsets,
            hashes,
            sources: places.iter().map(|p| p.0).collect(),
        }
    };
    let (types, ids) = turbonop_par::join(|| emit(&ty), || emit(&id));
    lap("emit");
    Merged {
        types,
        ids,
        maps,
        src_lines,
    }
}

/// The tag key a forward reference and its definitions share: kind
/// (classes and structures alike), and unique name (or name).
fn tag_key(record: &[u8]) -> Option<(bool, u16, &[u8])> {
    let (props, name, unique) = crate::streams::tag(record)?;
    let kind = u16::from_le_bytes([record[2], record[3]]);
    let class = if matches!(kind, 0x1504 | 0x1505 | 0x1519) {
        0x1504
    } else {
        kind
    };
    let key = if props & 0x200 != 0 && !unique.is_empty() {
        unique
    } else {
        name
    };
    Some((props & 0x80 != 0, class, key))
}

#[cfg(test)]
mod tests;
