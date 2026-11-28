//! Comparing two PDBs by content, as `compare-images` does.
//!
//! `link.exe` writes a different PDB for each link of the same inputs
//! (its modules in another order, its records at other indices), and
//! folding changes which module holds a function and every address. So
//! nothing is compared by index or address. Each record is reduced to a
//! canonical form and hashed (128 bits):
//!
//! - a type or id index stands for the structural hash of the record it
//!   names ([`types`]);
//! - an address (`section:offset`, or an RVA) stands for what it names in
//!   its image: a label and the distance from it (see
//!   `image::program`), so a local's range names its function and an
//!   offset in it;
//! - a string-table offset stands for its string, and a file-checksum
//!   offset (in lines, inline sites' annotations, and records) for its
//!   file's name;
//! - the links between scope records (parent, end, next) are left out:
//!   the order of the records keeps their nesting.
//!
//! Then these are compared, each as a multiset:
//!
//! - **types** (TPI) and **ids** (IPI), by structural hash;
//! - **publics**: flags, name, and what the address names;
//! - **globals** (the globals stream): every record, with each reference
//!   to a procedure or datum (`S_PROCREF`, ...) by kind and name, and the
//!   module it names compared on its own (a folded function's module may
//!   change: reported as expected);
//! - **procedures**, by name: each one's records, from its `S_*PROC32` to
//!   its end, in order (signature, frame, locals and their ranges, blocks,
//!   labels, inline sites, call sites, ...); a procedure found in another
//!   module is expected;
//! - **modules** (the loaded members), by name: their other records
//!   (`S_OBJNAME`, `S_COMPILE3`, `S_BUILDINFO`, their own UDTs and data,
//!   ...); a module in only the second PDB whose `S_OBJNAME` starts with
//!   `turbolib:` is a synthesized member, expected;
//! - **lines**, by what each block's start names: code size, and each
//!   file's name and lines (offsets, numbers, flags, columns);
//! - **inlinees' lines** (`DEBUG_S_INLINEELINES`): the inlinee's id, its
//!   file, and its line;
//! - **section contributions**: section, characteristics, size, and
//!   CRCs, whatever module holds them.

pub(crate) mod types;

use turbonop_codeview::TypeRecord;
use turbonop_codeview::lines::{self, subsection};
use turbonop_codeview::symbol_refs::{self, Field};
use turbonop_codeview::symbols::Symbol;
use turbonop_codeview::symbols::kind::{self, closes_scope, opens_scope};
use turbonop_hash::Map;
use turbonop_pdb::StringTable;

use crate::hash::H128;
use crate::image::program::Linked;
use crate::library::PCH_SYMBOL;
use crate::library::member::{RENAMED, canonical};
use crate::report::{Report, show};
use types::Hashes;

/// A contribution, as compared: section, characteristics, size, CRCs.
type ContributionKey = (Vec<u8>, u32, u32, u32, u32);

/// What one PDB holds, reduced.
#[derive(Default)]
struct Reduced {
    types: Hashes,
    ids: Hashes,
    /// The ids' hashes but `LF_UDT_MOD_SRC_LINE`'s, sorted, each once.
    id_set: Vec<u128>,
    /// Each `LF_UDT_MOD_SRC_LINE`: its UDT's name, its file, and its line.
    udt_lines: Vec<(Vec<u8>, Vec<u8>, u32)>,
    /// (hash, description, renamed).
    publics: Vec<(u128, String, bool)>,
    globals: Vec<(u128, String)>,
    /// References' (kind and name, the module they name).
    references: Vec<(String, Vec<u8>)>,
    /// (name, hash, module).
    procedures: Vec<(Vec<u8>, u128, usize)>,
    /// Each module's name, whether it is synthesized, and its other
    /// records' hashes (sorted).
    modules: Vec<(Vec<u8>, bool, Vec<u128>)>,
    /// (what the block's start names, hash).
    lines: Vec<(Vec<u8>, u128)>,
    inlinees: Vec<u128>,
    /// Records whose fields aren't known, by kind; subsections not
    /// compared, by kind.
    unknown_records: Map<u16, u64>,
    unknown_subsections: Map<u32, u64>,
    contributions: Vec<ContributionKey>,
    /// The sizes of the linker's sections and COFF groups.
    layout: Vec<(Vec<u8>, u32)>,
    /// The `S_UDT` and `S_CONSTANT` records of the globals and of every
    /// module's top level: (hash, hash blind to translation units' tags,
    /// description).
    udts: Vec<(u128, u128, String)>,
    /// It has a module of a precompiled-types member turbolib made.
    pch: bool,
    /// The hashes of the types and ids something in the PDB reaches (see
    /// [`types::live`]), sorted, each once.
    live_types: Vec<u128>,
    live_ids: Vec<u128>,
    /// Each record's hash blind to translation units' tags (see
    /// [`types::hash`]), by index.
    tagless_types: Vec<u128>,
    tagless_ids: Vec<u128>,
}

/// Compares the PDBs of two programs (see the module's documentation).
pub(crate) fn compare(a: &Linked<'_>, b: &Linked<'_>) -> Report {
    let mut rep = Report::new();
    let (ra, rb) = turbonop_par::join(|| reduce(a), || reduce(b));
    let (ra, rb) = match (ra, rb) {
        (Ok(x), Ok(y)) => (x, y),
        (x, y) => {
            for e in [x.err(), y.err()].into_iter().flatten() {
                rep.fail("PDBs that can't be read", || e);
            }
            return rep;
        }
    };
    compare_types(a, b, &ra, &rb, &mut rep);
    compare_udt_lines(&ra, &rb, &mut rep);
    compare_symbols(a, b, &ra, &rb, &mut rep);
    compare_rest(&ra, &rb, &mut rep);
    rep
}

fn compare_types(a: &Linked<'_>, b: &Linked<'_>, ra: &Reduced, rb: &Reduced, rep: &mut Report) {
    let describe = |p: &Linked<'_>, ids: bool, hashes: &Hashes, h: u128| -> String {
        let Some(i) = hashes.hashes.iter().position(|&x| x == h) else {
            return String::new();
        };
        let stream = if ids { p.pdb.ids() } else { p.pdb.types() };
        stream
            .ok()
            .and_then(|s| s.record(turbonop_codeview::TypeIndex(i as u32 + types::FIRST)))
            .map(|r| {
                format!(
                    "{:#x} {} {}",
                    i as u32 + types::FIRST,
                    turbonop_codeview::types::leaf::name(r.kind),
                    r.name().map_or_else(String::new, show)
                )
            })
            .unwrap_or_default()
    };
    for (what, x, y, ids) in [
        ("types (TPI)", &ra.types, &rb.types, false),
        ("ids (IPI)", &ra.ids, &rb.ids, true),
    ] {
        rep.checked(&format!("{what} records"), x.hashes.len() as u64);
        let (sx, sy) = if ids {
            (ra.id_set.clone(), rb.id_set.clone())
        } else {
            let mut sx = x.hashes.clone();
            let mut sy = y.hashes.clone();
            sx.sort_unstable();
            sx.dedup();
            sy.sort_unstable();
            sy.dedup();
            (sx, sy)
        };
        let (live_a, live_b) = if ids {
            (&ra.live_ids, &rb.live_ids)
        } else {
            (&ra.live_types, &rb.live_types)
        };
        let (tagless_a, tagless_b) = if ids {
            (&ra.tagless_ids, &rb.tagless_ids)
        } else {
            (&ra.tagless_types, &rb.tagless_types)
        };
        // A record's tag-blind hash, by its hash; and each PDB's tag-blind
        // hashes.
        let blind = |hashes: &Hashes, tagless: &[u128]| -> (Map<u128, u128>, Vec<u128>) {
            let map: Map<u128, u128> = hashes
                .hashes
                .iter()
                .zip(tagless)
                .map(|(h, t)| (*h, *t))
                .collect();
            let mut set = tagless.to_vec();
            set.sort_unstable();
            set.dedup();
            (map, set)
        };
        let (blind_a, set_a) = blind(x, tagless_a);
        let (blind_b, set_b) = blind(y, tagless_b);
        let other_copy = |h: u128, map: &Map<u128, u128>, set: &[u128]| {
            map.get(&h).is_some_and(|t| set.binary_search(t).is_ok())
        };
        for h in difference(&sx, &sy) {
            if live_a.binary_search(&h).is_err() {
                // Nothing in the first PDB reaches it: link.exe merges the
                // records an object's symbols reach, those of the COMDAT
                // copies it discards included, so a record only a removed
                // copy reached (as each translation unit's own lambda
                // types) is in the first PDB alone.
                rep.expect(
                    &format!(
                        "{what} only in the first PDB, which nothing in it reaches (records of copies the link discarded)"
                    ),
                    || describe(a, ids, x, h),
                );
                continue;
            }
            if other_copy(h, &blind_a, &set_b) {
                rep.expect(
                    &format!(
                        "{what} only in the first PDB, the same but for translation units' tags in unique names (another translation unit's copy kept)"
                    ),
                    || describe(a, ids, x, h),
                );
                continue;
            }
            rep.fail(&format!("{what} only in the first PDB"), || {
                describe(a, ids, x, h)
            });
        }
        for h in difference(&sy, &sx) {
            if live_b.binary_search(&h).is_err() && !(rb.pch && !ra.pch) {
                rep.expect(
                    &format!("{what} only in the second PDB, which nothing in it reaches"),
                    || describe(b, ids, y, h),
                );
                continue;
            }
            if other_copy(h, &blind_b, &set_a) {
                rep.expect(
                    &format!(
                        "{what} only in the second PDB, the same but for translation units' tags in unique names (another translation unit's copy kept)"
                    ),
                    || describe(b, ids, y, h),
                );
                continue;
            }
            if rb.pch && !ra.pch {
                // A linker merges every record of a precompiled-types
                // member it loads, used or not (lld-link: measured), so
                // the second has the first's and more.
                rep.expect(
                    &format!(
                        "{what} only in the second PDB, which has turbolib's                          precompiled-types members (each brings all its records)"
                    ),
                    || describe(b, ids, y, h),
                );
            } else {
                rep.fail(&format!("{what} only in the second PDB"), || {
                    describe(b, ids, y, h)
                });
            }
        }
        for (r, which) in [(x, "first"), (y, "second")] {
            for k in &r.unknown {
                rep.unchecked(
                    &format!("{what} records of kinds whose fields aren't known"),
                    || format!("{which}: {}", turbonop_codeview::types::leaf::name(*k)),
                );
            }
            if r.forward > 0 {
                rep.unchecked(
                    &format!("{what} records naming later records (hashed by index)"),
                    || format!("{which}: {}", r.forward),
                );
            }
        }
    }
}

/// UDTs' source lines (`LF_UDT_MOD_SRC_LINE`), by the UDT's name:
/// link.exe writes one for a UDT in some links and not in others, and
/// gives it to one of the type records of that name, so only the files
/// and lines of the names both PDBs have lines for are compared.
fn compare_udt_lines(ra: &Reduced, rb: &Reduced, rep: &mut Report) {
    let by_name = |r: &Reduced| -> Map<Vec<u8>, Vec<(Vec<u8>, u32)>> {
        let mut m: Map<Vec<u8>, Vec<(Vec<u8>, u32)>> = Map::default();
        for (n, f, l) in &r.udt_lines {
            m.entry(n.clone()).or_default().push((f.clone(), *l));
        }
        for v in m.values_mut() {
            v.sort();
            v.dedup();
        }
        m
    };
    let (a, b) = (by_name(ra), by_name(rb));
    rep.checked("UDTs' source lines (LF_UDT_MOD_SRC_LINE)", a.len() as u64);
    let mut names: Vec<&Vec<u8>> = a.keys().chain(b.keys()).collect();
    names.sort();
    names.dedup();
    for n in names {
        match (a.get(n), b.get(n)) {
            (Some(x), Some(y)) if x != y && x.iter().any(|l| y.contains(l)) => {
                rep.expect(
                    "UDTs whose source lines differ, with one in common (the linker picked another)",
                    || show(n),
                );
            }
            (Some(x), Some(y))
                if x != y
                    && x.iter().any(|(f, _)| {
                        y.iter()
                            .any(|(g, _)| file_name(f).eq_ignore_ascii_case(file_name(g)))
                    }) =>
            {
                // The same header from another place (another SDK's copy):
                // the type itself is compared with the types.
                rep.expect(
                    "UDTs whose source lines are in another copy of the same header (the linker picked another)",
                    || show(n),
                );
            }
            (Some(x), Some(y)) if x != y => {
                rep.fail("UDTs whose source lines differ", || {
                    let line = |v: &Vec<(Vec<u8>, u32)>| {
                        v.iter()
                            .map(|(f, l)| format!("{}({l})", show(f)))
                            .collect::<Vec<_>>()
                            .join(", ")
                    };
                    format!("{}: {} and {}", show(n), line(x), line(y))
                });
            }
            (Some(_), None) | (None, Some(_)) => {
                rep.expect(
                    "UDTs with a source line in one PDB only (link.exe writes them for some links)",
                    || show(n),
                );
            }
            _ => {}
        }
    }
}

fn compare_symbols(a: &Linked<'_>, b: &Linked<'_>, ra: &Reduced, rb: &Reduced, rep: &mut Report) {
    // Publics.
    rep.checked("publics", ra.publics.len() as u64);
    // (turbolib's own names on either side: the first program may be
    // linked against a library folded before, as when a folded library
    // is folded again.)
    multisets(
        ra.publics
            .iter()
            .filter(|p| !p.2)
            .map(|(h, d, _)| (*h, d.clone())),
        rb.publics
            .iter()
            .filter(|p| !p.2)
            .map(|(h, d, _)| (*h, d.clone())),
        "publics",
        rep,
    );
    for (_, d, renamed) in ra.publics.iter().chain(&rb.publics) {
        if *renamed {
            rep.expect(
                "publics of turbolib's renamed copies, anchors, and PCH symbols",
                || d.clone(),
            );
        }
    }
    // Globals.
    rep.checked("globals", ra.globals.len() as u64);
    multisets(
        ra.globals.iter().map(|(h, d)| (*h, d.clone())),
        rb.globals.iter().map(|(h, d)| (*h, d.clone())),
        "globals",
        rep,
    );
    // UDTs and constants: the linker keeps one record of each name, and
    // which one follows the order it reads modules in, which differs
    // between links. Any one is right if they say the same: a record only
    // in one PDB that the other has but for translation units' tags in
    // unique names (another unit's copy of the same type) is expected.
    let set = |r: &Reduced| -> Vec<(u128, u128, String)> {
        let mut v = r.udts.clone();
        v.sort();
        v.dedup_by(|x, y| x.0 == y.0);
        v
    };
    let (ua, ub) = (set(ra), set(rb));
    rep.checked("UDTs and constants (S_UDT, S_CONSTANT)", ua.len() as u64);
    let blind = |v: &[(u128, u128, String)]| -> Vec<u128> {
        let mut t: Vec<u128> = v.iter().map(|x| x.1).collect();
        t.sort_unstable();
        t.dedup();
        t
    };
    let (ta, tb) = (blind(&ua), blind(&ub));
    let exact = |v: &[(u128, u128, String)]| -> Vec<u128> { v.iter().map(|x| x.0).collect() };
    let (ea, eb) = (exact(&ua), exact(&ub));
    for (only, other_exact, other_blind, which) in
        [(&ua, &eb, &tb, "first"), (&ub, &ea, &ta, "second")]
    {
        for (h, t, d) in only {
            if other_exact.binary_search(h).is_ok() {
                continue;
            }
            if other_blind.binary_search(t).is_ok() {
                rep.expect(
                    &format!(
                        "UDTs and constants only in the {which} PDB, the same but for translation units' tags (another unit's copy picked)"
                    ),
                    || d.clone(),
                );
            } else {
                rep.fail(
                    &format!("UDTs and constants (S_UDT, S_CONSTANT) only in the {which} PDB (or different)"),
                    || d.clone(),
                );
            }
        }
    }
    let modules_of = |r: &Reduced| -> Map<String, Vec<Vec<u8>>> {
        let mut m: Map<String, Vec<Vec<u8>>> = Map::default();
        for (k, module) in &r.references {
            m.entry(k.clone()).or_default().push(module.clone());
        }
        for v in m.values_mut() {
            v.sort();
        }
        m
    };
    let (ma, mb) = (modules_of(ra), modules_of(rb));
    let mut moved: Vec<&String> = ma
        .iter()
        .filter(|(k, v)| mb.get(*k).is_some_and(|w| w != *v))
        .map(|(k, _)| k)
        .collect();
    moved.sort();
    for k in moved {
        rep.expect("global references naming another module", || k.clone());
    }

    // Procedures, by name.
    rep.checked("procedures", ra.procedures.len() as u64);
    let by_name = |r: &Reduced| -> Map<Vec<u8>, Vec<(u128, usize)>> {
        let mut m: Map<Vec<u8>, Vec<(u128, usize)>> = Map::default();
        for (n, h, module) in &r.procedures {
            m.entry(n.clone()).or_default().push((*h, *module));
        }
        for v in m.values_mut() {
            v.sort_unstable();
        }
        m
    };
    let (pa, pb) = (by_name(ra), by_name(rb));
    let mut names: Vec<&Vec<u8>> = pa.keys().chain(pb.keys()).collect();
    names.sort();
    names.dedup();
    for n in names {
        match (pa.get(n), pb.get(n)) {
            (Some(_), None) => rep.fail("procedures only in the first PDB", || show(n)),
            (None, Some(_)) => rep.fail("procedures only in the second PDB", || show(n)),
            (Some(x), Some(y)) => {
                let hx: Vec<u128> = x.iter().map(|p| p.0).collect();
                let hy: Vec<u128> = y.iter().map(|p| p.0).collect();
                if hx == hy {
                    let module = |p: &Linked<'_>, m: usize| p.modules.get(m).map(|m| m.0.clone());
                    let mx: Vec<_> = x.iter().map(|p| module(a, p.1)).collect();
                    let my: Vec<_> = y.iter().map(|p| module(b, p.1)).collect();
                    if mx != my {
                        rep.expect(
                            "procedures in another module (folded functions move)",
                            || show(n),
                        );
                    }
                } else {
                    rep.fail(
                        "procedures whose symbols differ (signature, frame, locals, ranges, inline sites, ...)",
                        || show(n),
                    );
                }
            }
            (None, None) => {}
        }
    }

    // Modules.
    rep.checked("modules", ra.modules.len() as u64);
    let mods = |r: &Reduced| -> Map<Vec<u8>, (bool, Vec<Vec<u128>>)> {
        let mut m: Map<Vec<u8>, (bool, Vec<Vec<u128>>)> = Map::default();
        for (n, synthesized, h) in &r.modules {
            let e = m.entry(n.clone()).or_insert((*synthesized, Vec::new()));
            e.1.push(h.clone());
        }
        for v in m.values_mut() {
            v.1.sort();
        }
        m
    };
    let (xa, xb) = (mods(ra), mods(rb));
    let mut names: Vec<&Vec<u8>> = xa.keys().chain(xb.keys()).collect();
    names.sort();
    names.dedup();
    for n in names {
        match (xa.get(n), xb.get(n)) {
            (Some(_), None) => {
                rep.fail("modules (loaded members) only in the first PDB", || show(n));
            }
            (None, Some((true, _))) => {
                rep.expect(
                    "turbolib's synthesized members loaded by the second link",
                    || show(n),
                );
            }
            (None, Some(_)) => {
                rep.fail("modules (loaded members) only in the second PDB", || {
                    show(n)
                });
            }
            (Some(x), Some(y)) if x.1 != y.1 => {
                if n.as_slice() == b"* Linker *" {
                    rep.fail(
                        "the linker's own records differ (its command line, sections, ...)",
                        || show(n),
                    );
                } else {
                    rep.fail(
                        "modules whose own records differ (S_OBJNAME, S_COMPILE3, UDTs, data, ...)",
                        || show(n),
                    );
                }
            }
            _ => {}
        }
    }
}

fn compare_rest(ra: &Reduced, rb: &Reduced, rep: &mut Report) {
    let (mut la, mut lb) = (ra.layout.clone(), rb.layout.clone());
    la.sort();
    lb.sort();
    if la != lb {
        let first = la.iter().zip(&lb).find(|(x, y)| x != y);
        rep.expect(
            "the linker's sections and COFF groups of another size",
            || {
                first.map_or_else(String::new, |(x, y)| {
                    format!("{}: {:#x} and {:#x}", show(&x.0), x.1, y.1)
                })
            },
        );
    }
    rep.checked("line blocks", ra.lines.len() as u64);
    multisets(
        ra.lines.iter().map(|(n, h)| (*h, show(n))),
        rb.lines.iter().map(|(n, h)| (*h, show(n))),
        "line blocks",
        rep,
    );
    let mut ia = ra.inlinees.clone();
    let mut ib = rb.inlinees.clone();
    for v in [&mut ia, &mut ib] {
        v.sort_unstable();
        v.dedup();
    }
    rep.checked("inlinees' lines", ia.len() as u64);
    let only_a = difference(&ia, &ib).count();
    let only_b = difference(&ib, &ia).count();
    if only_a > 0 || only_b > 0 {
        rep.fail("inlinees' lines that differ", || {
            format!("{only_a} only in the first, {only_b} only in the second")
        });
    }
    rep.checked("section contributions", ra.contributions.len() as u64);
    let mut ca = ra.contributions.clone();
    let mut cb = rb.contributions.clone();
    ca.sort();
    cb.sort();
    if ca != cb {
        let first = ca
            .iter()
            .find(|c| cb.binary_search(c).is_err())
            .or_else(|| cb.iter().find(|c| ca.binary_search(c).is_err()));
        rep.fail(
            "section contributions that differ (section, characteristics, size, CRCs)",
            || {
                first.map_or_else(String::new, |c| {
                    format!(
                        "{} {:#x}, {:#x} bytes, CRCs {:#x} {:#x}",
                        show(&c.0),
                        c.1,
                        c.2,
                        c.3,
                        c.4
                    )
                })
            },
        );
    }
    for (r, which) in [(ra, "first"), (rb, "second")] {
        let mut kinds: Vec<(&u16, &u64)> = r.unknown_records.iter().collect();
        kinds.sort();
        for (k, n) in kinds {
            rep.unchecked(
                "symbol records of kinds whose fields aren't known (compared as bytes)",
                || format!("{which}: {} ({k:#06x}): {n}", kind::name(*k)),
            );
        }
        let mut kinds: Vec<(&u32, &u64)> = r.unknown_subsections.iter().collect();
        kinds.sort();
        for (k, n) in kinds {
            rep.unchecked("C13 subsections not compared", || {
                format!("{which}: {k:#x}: {n}")
            });
        }
    }
}

/// A record's kind and name, as text.
fn describe(s: &turbonop_codeview::SymbolRecord<'_>) -> String {
    format!(
        "{} {}",
        kind::name(s.kind),
        s.name().map_or_else(String::new, show)
    )
}

/// The items of sorted `a` that sorted `b` lacks.
fn difference<'v, T: Ord + Copy>(a: &'v [T], b: &'v [T]) -> impl Iterator<Item = T> + 'v {
    a.iter().copied().filter(|x| b.binary_search(x).is_err())
}

/// Compares two multisets of (hash, description), reporting what's only
/// in one or the other under `what`.
fn multisets(
    a: impl Iterator<Item = (u128, String)>,
    b: impl Iterator<Item = (u128, String)>,
    what: &str,
    rep: &mut Report,
) {
    let mut a: Vec<(u128, String)> = a.collect();
    let mut b: Vec<(u128, String)> = b.collect();
    a.sort();
    b.sort();
    let (mut i, mut j) = (0, 0);
    let mut only_a = Vec::new();
    let mut only_b = Vec::new();
    while i < a.len() || j < b.len() {
        match (a.get(i), b.get(j)) {
            (Some(x), Some(y)) if x.0 == y.0 => {
                i += 1;
                j += 1;
            }
            (Some(x), Some(y)) if x.0 < y.0 => {
                only_a.push(x.1.clone());
                i += 1;
            }
            (Some(x), None) => {
                only_a.push(x.1.clone());
                i += 1;
            }
            (_, Some(y)) => {
                only_b.push(y.1.clone());
                j += 1;
            }
            (None, None) => break,
        }
    }
    only_a.sort();
    only_b.sort();
    for d in only_a {
        rep.fail(
            &format!("{what} only in the first PDB (or different)"),
            || d,
        );
    }
    for d in only_b {
        rep.fail(
            &format!("{what} only in the second PDB (or different)"),
            || d,
        );
    }
}

/// What canonicalizing a module's records needs.
struct Context<'c, 'p, 'n> {
    p: &'c Linked<'p>,
    types: &'c Hashes,
    ids: &'c Hashes,
    names: Option<&'c StringTable<'n>>,
    /// The module's file checksums subsection.
    checksums: &'c [u8],
}

impl Context<'_, '_, '_> {
    fn file(&self, offset: u32) -> Option<&[u8]> {
        let c = lines::file_checksum(self.checksums, offset)?;
        self.names?.get(c.name)
    }

    fn string(&self, offset: u32) -> Option<&[u8]> {
        self.names?.get(offset)
    }

    /// Hashes what `rva` names.
    fn address(&self, rva: Option<u32>, h: &mut H128) {
        match rva.and_then(|r| self.p.name(r)) {
            Some((name, off)) => {
                h.bytes(name);
                h.u64(u64::from(off));
            }
            None => h.u64(u64::MAX),
        }
    }

    /// Hashes a symbol record's canonical form; false if its kind's
    /// fields aren't known (then its bytes are hashed as they are).
    fn symbol(&self, kind: u16, body: &[u8], h: &mut H128) -> bool {
        h.u64(u64::from(kind));
        let Ok(fields) = symbol_refs::fields_of(kind, body) else {
            h.bytes(body);
            return false;
        };
        // Each field's span: (start, length, field), in order.
        let mut spans: Vec<(usize, usize, Field)> = Vec::with_capacity(fields.len() + 1);
        for f in fields {
            match f {
                Field::Address { offset, section } => {
                    spans.push((offset as usize, 4, f));
                    spans.push((section as usize, 2, Field::Link(section)));
                }
                Field::Annotations(o) => {
                    spans.push((o as usize, body.len().saturating_sub(o as usize), f));
                }
                _ => spans.push((f.offset() as usize, 4, f)),
            }
        }
        spans.sort_unstable_by_key(|s| s.0);
        let u32_at = |o: usize| {
            body.get(o..o + 4)
                .map_or(0, |b| u32::from_le_bytes([b[0], b[1], b[2], b[3]]))
        };
        let mut from = 0;
        for (start, len, f) in spans {
            if start < from || start + len > body.len() {
                continue;
            }
            h.bytes(&body[from..start]);
            match f {
                Field::Type(o) => h.u128(self.types.of(u32_at(o as usize))),
                Field::Id(o) => h.u128(self.ids.of(u32_at(o as usize))),
                Field::Address { offset, section } => {
                    let seg = body
                        .get(section as usize..section as usize + 2)
                        .map_or(0, |b| u16::from_le_bytes([b[0], b[1]]));
                    self.address(self.p.rva_of(seg, u32_at(offset as usize)), h);
                }
                Field::Rva(o) => self.address(Some(u32_at(o as usize)), h),
                Field::String(o) => h.bytes(self.string(u32_at(o as usize)).unwrap_or(b"\xFF")),
                Field::Checksum(o) => h.bytes(self.file(u32_at(o as usize)).unwrap_or(b"\xFF")),
                Field::Annotations(o) => {
                    let a = &body[o as usize..];
                    let mut at = 0;
                    let operands =
                        turbonop_codeview::annotations::file_operands(a).unwrap_or_default();
                    for (s, l, file) in operands {
                        if s < at || s + l > a.len() {
                            continue;
                        }
                        h.bytes(&a[at..s]);
                        h.bytes(self.file(file).unwrap_or(b"\xFF"));
                        at = s + l;
                    }
                    h.bytes(&a[at..]);
                }
                // Links are left out; tokens are kept.
                Field::Link(_) => {}
                Field::Token(o) => h.u64(u64::from(u32_at(o as usize))),
            }
            from = start + len;
        }
        h.bytes(&body[from.min(body.len())..]);
        true
    }
}

/// What one module holds, reduced.
struct ModuleFacts {
    name: Vec<u8>,
    synthesized: bool,
    others: Vec<u128>,
    procedures: Vec<(Vec<u8>, u128)>,
    lines: Vec<(Vec<u8>, u128)>,
    inlinees: Vec<u128>,
    unknown_records: Map<u16, u64>,
    unknown_subsections: Map<u32, u64>,
    /// The sizes of the linker's sections and COFF groups (`S_SECTION`,
    /// `S_COFFGROUP`), by name.
    layout: Vec<(Vec<u8>, u32)>,
    /// The type and id indices its symbols and inlinee lines name.
    root_types: Vec<u32>,
    root_ids: Vec<u32>,
    /// Its top-level `S_UDT` and `S_CONSTANT` records: (hash, hash blind
    /// to translation units' tags, description).
    udts: Vec<(u128, u128, String)>,
}

/// The type and id indices a symbol record names.
fn roots_of(kind: u16, body: &[u8], types: &mut Vec<u32>, ids: &mut Vec<u32>) {
    let Ok(fields) = symbol_refs::fields_of(kind, body) else {
        return;
    };
    let u32_at = |o: usize| {
        body.get(o..o + 4)
            .map(|b| u32::from_le_bytes([b[0], b[1], b[2], b[3]]))
    };
    for f in fields {
        match f {
            Field::Type(o) => types.extend(u32_at(o as usize)),
            Field::Id(o) => ids.extend(u32_at(o as usize)),
            _ => {}
        }
    }
}

fn module(
    p: &Linked<'_>,
    m: &turbonop_pdb::Module<'_>,
    (types, ids): (&Hashes, &Hashes),
    (tagless_types, tagless_ids): (&Hashes, &Hashes),
    names: Option<&StringTable<'_>>,
) -> ModuleFacts {
    let mut out = ModuleFacts {
        name: m.name.to_vec(),
        synthesized: false,
        others: Vec::new(),
        procedures: Vec::new(),
        lines: Vec::new(),
        inlinees: Vec::new(),
        unknown_records: Map::default(),
        unknown_subsections: Map::default(),
        layout: Vec::new(),
        root_types: Vec::new(),
        root_ids: Vec::new(),
        udts: Vec::new(),
    };
    // A synthesized member: its S_OBJNAME says so (`turbolib:...`), or,
    // until its debug information is written (2.4), its name is the
    // engine's (`turbolib<n>.obj`, and since `turbolib<n>-<hex>.obj`).
    out.synthesized = m
        .name
        .strip_prefix(b"turbolib")
        .and_then(|r| r.strip_suffix(b".obj"))
        .is_some_and(|n| {
            let (number, tag) = match n.iter().position(|&c| c == b'-') {
                Some(at) => (&n[..at], &n[at + 1..]),
                None => (n, &b"0"[..]),
            };
            !number.is_empty()
                && number.iter().all(u8::is_ascii_digit)
                && !tag.is_empty()
                && tag.iter().all(u8::is_ascii_hexdigit)
        });
    let Ok(Some(stream)) = p.pdb.module_stream(m) else {
        return out;
    };
    let checksums = stream
        .subsections()
        .find(|(k, _)| *k == subsection::DEBUG_S_FILECHKSMS)
        .map_or(&[][..], |(_, b)| b);
    let cx = Context {
        p,
        types,
        ids,
        names,
        checksums,
    };
    let tagless = Context {
        p,
        types: tagless_types,
        ids: tagless_ids,
        names,
        checksums,
    };
    // Symbols: procedures (and other top-level scopes) as one hash each;
    // the rest one each.
    let mut depth = 0usize;
    let mut scope: Option<(Vec<u8>, H128, bool)> = None;
    for s in stream.symbols() {
        roots_of(s.kind, s.data, &mut out.root_types, &mut out.root_ids);
        if s.kind == kind::S_OBJNAME
            && let Ok(Symbol::ObjName { name, .. }) = s.parse()
        {
            out.synthesized |=
                name.starts_with(b"turbolib:") || m.name.starts_with(b"turbolib-types-");
        }
        let mut h = H128::new();
        // The linker's sections and COFF groups: their sizes and places
        // follow the layout, which folding changes.
        let layout = match s.parse() {
            Ok(Symbol::Section {
                alignment,
                size,
                characteristics,
                name,
                ..
            }) => Some((name, size, characteristics, u32::from(alignment))),
            Ok(Symbol::CoffGroup {
                size,
                characteristics,
                name,
                ..
            }) => Some((name, size, characteristics, 0)),
            _ => None,
        };
        if let Some((name, size, characteristics, alignment)) = layout {
            h.u64(u64::from(s.kind));
            h.bytes(name);
            h.u64(u64::from(characteristics));
            h.u64(u64::from(alignment));
            out.layout.push((name.to_vec(), size));
        } else if !cx.symbol(s.kind, s.data, &mut h) {
            *out.unknown_records.entry(s.kind).or_insert(0) += 1;
        }
        let record = h.finish();
        if let Some((_, sh, _)) = &mut scope {
            sh.u128(record);
            if opens_scope(s.kind) {
                depth += 1;
            } else if closes_scope(s.kind) {
                depth = depth.saturating_sub(1);
            }
            if depth == 0 {
                let (name, sh, procedure) = scope.take().expect("in a scope");
                if procedure {
                    out.procedures.push((name, sh.finish()));
                } else {
                    let mut x = H128::new();
                    x.bytes(&name);
                    x.u128(sh.finish());
                    out.others.push(x.finish());
                }
            }
            continue;
        }
        if opens_scope(s.kind) {
            let procedure = matches!(
                s.kind,
                kind::S_GPROC32 | kind::S_LPROC32 | kind::S_GPROC32_ID | kind::S_LPROC32_ID
            );
            let name = s.name().map_or_else(Vec::new, |n| canonical(n).to_vec());
            let mut sh = H128::new();
            sh.u128(record);
            scope = Some((name, sh, procedure));
            depth = 1;
            continue;
        }
        // A UDT or constant at the top: compared with the globals' (the
        // linker keeps one of each name there, and the others here).
        if matches!(s.kind, kind::S_UDT | kind::S_CONSTANT) {
            let mut t = H128::new();
            tagless.symbol(s.kind, s.data, &mut t);
            out.udts.push((record, t.finish(), describe(&s)));
            continue;
        }
        out.others.push(record);
    }
    // Subsections: lines and inlinees.
    for (k, body) in stream.subsections() {
        match k {
            subsection::DEBUG_S_LINES => {
                let mut blocks = Vec::new();
                if lines::parse_lines(body, &mut blocks).is_err() || blocks.is_empty() {
                    continue;
                }
                let first = &blocks[0];
                let name =
                    cx.p.rva_of(first.segment, first.offset)
                        .and_then(|rva| cx.p.name(rva))
                        .map_or_else(
                            || b"?".to_vec(),
                            |(n, o)| [n, format!("+{o:#x}").as_bytes()].concat(),
                        );
                let mut h = H128::new();
                h.u64(u64::from(first.code_size));
                for b in &blocks {
                    h.bytes(cx.file(b.file).unwrap_or(b"\xFF"));
                    h.u64(b.lines.len() as u64);
                    for l in b.lines {
                        h.u64(u64::from(l.offset.get()));
                        h.u64(u64::from(l.flags.get()));
                    }
                    for c in b.columns {
                        h.u64(u64::from(c.start.get()));
                        h.u64(u64::from(c.end.get()));
                    }
                }
                out.lines.push((name, h.finish()));
            }
            subsection::DEBUG_S_INLINEELINES => {
                for l in lines::inlinee_lines(body) {
                    out.root_ids.push(l.inlinee);
                    let mut h = H128::new();
                    h.u128(cx.ids.of(l.inlinee));
                    h.bytes(cx.file(l.file).unwrap_or(b"\xFF"));
                    h.u64(u64::from(l.line));
                    out.inlinees.push(h.finish());
                }
            }
            subsection::DEBUG_S_FILECHKSMS => {}
            k => *out.unknown_subsections.entry(k).or_insert(0) += 1,
        }
    }
    out.others.sort_unstable();
    out
}

fn reduce(p: &Linked<'_>) -> Result<Reduced, String> {
    let mut r = Reduced::default();
    let tpi = p.pdb.types().map_err(|e| format!("the TPI: {e}"))?;
    let ipi = p.pdb.ids().map_err(|e| format!("the IPI: {e}"))?;
    let names = p.pdb.names().ok();
    let type_records = types::records(tpi);
    r.types = types::hash(&type_records, None, names.as_ref(), false);
    let records = types::records(ipi);
    r.ids = types::hash(
        &records,
        Some((&r.types, &type_records)),
        names.as_ref(),
        false,
    );
    // The same, blind to translation units' tags in unique names.
    let tagless_types = types::hash(&type_records, None, names.as_ref(), true);
    let tagless_ids = types::hash(
        &records,
        Some((&tagless_types, &type_records)),
        names.as_ref(),
        true,
    );
    r.tagless_types = tagless_types.hashes.clone();
    r.tagless_ids = tagless_ids.hashes.clone();
    for (i, rec) in records.iter().enumerate() {
        if rec.kind == types::LF_UDT_MOD_SRC_LINE && rec.data.len() >= 12 {
            let u = |o: usize| {
                u32::from_le_bytes([
                    rec.data[o],
                    rec.data[o + 1],
                    rec.data[o + 2],
                    rec.data[o + 3],
                ])
            };
            let name = u(0)
                .checked_sub(types::FIRST)
                .and_then(|k| type_records.get(k as usize))
                .and_then(TypeRecord::name)
                .unwrap_or_default();
            let file = names.as_ref().and_then(|n| n.get(u(4))).unwrap_or_default();
            r.udt_lines.push((name.to_vec(), file.to_vec(), u(8)));
        } else {
            r.id_set.push(r.ids.hashes[i]);
        }
    }
    r.id_set.sort_unstable();
    r.id_set.dedup();
    let dbi = p.pdb.dbi().map_err(|e| e.to_string())?;
    let modules: Vec<turbonop_pdb::Module<'_>> = dbi.modules().to_vec();

    // Publics and globals.
    let records = p.pdb.symbol_records();
    let context = Context {
        p,
        types: &r.types,
        ids: &r.ids,
        names: names.as_ref(),
        checksums: &[],
    };
    // The same, blind to translation units' tags (for UDTs and constants).
    let tagless = Context {
        p,
        types: &tagless_types,
        ids: &tagless_ids,
        names: names.as_ref(),
        checksums: &[],
    };
    let mut unknown: Map<u16, u64> = Map::default();
    let mut publics = Vec::new();
    if let Ok(psi) = p.pdb.publics() {
        for off in psi.address_map() {
            let Some(s) = turbonop_pdb::gsi::symbol_at(records, off.get()) else {
                continue;
            };
            let Ok(Symbol::Public {
                name,
                flags,
                segment,
                offset,
            }) = s.parse()
            else {
                continue;
            };
            let mut h = H128::new();
            h.bytes(canonical(name));
            h.u64(u64::from(flags));
            context.address(p.rva_of(segment, offset), &mut h);
            publics.push((
                h.finish(),
                show(name),
                name.starts_with(RENAMED) || name.starts_with(PCH_SYMBOL),
            ));
        }
    }
    let mut globals = Vec::new();
    let mut udts = Vec::new();
    let mut references = Vec::new();
    let (mut root_types, mut root_ids) = (Vec::new(), Vec::new());
    if let Ok(gsi) = p.pdb.globals() {
        for g in gsi.records() {
            let Some(s) =
                turbonop_pdb::gsi::symbol_at(records, (g.offset.get() as u32).wrapping_sub(1))
            else {
                continue;
            };
            roots_of(s.kind, s.data, &mut root_types, &mut root_ids);
            let mut h = H128::new();
            let description = describe(&s);
            match s.parse() {
                Ok(Symbol::Reference {
                    kind, module, name, ..
                }) => {
                    h.u64(u64::from(kind));
                    h.bytes(canonical(name));
                    let m = p
                        .modules
                        .get(usize::from(module).wrapping_sub(1))
                        .map_or_else(Vec::new, |m| m.0.clone());
                    references.push((description.clone(), m));
                }
                _ => {
                    if !context.symbol(s.kind, s.data, &mut h) {
                        *unknown.entry(s.kind).or_insert(0) += 1;
                    }
                }
            }
            if matches!(s.kind, kind::S_UDT | kind::S_CONSTANT) {
                let mut t = H128::new();
                tagless.symbol(s.kind, s.data, &mut t);
                udts.push((h.finish(), t.finish(), description));
                continue;
            }
            globals.push((h.finish(), description));
        }
    }
    r.publics = publics;
    r.globals = globals;
    r.udts = udts;
    r.references = references;

    // Modules, in parallel.
    let reduced = turbonop_par::map_balanced(&modules, 1, |m| {
        module(
            p,
            m,
            (&r.types, &r.ids),
            (&tagless_types, &tagless_ids),
            names.as_ref(),
        )
    });
    for (i, m) in reduced.into_iter().enumerate() {
        r.procedures
            .extend(m.procedures.into_iter().map(|(n, h)| (n, h, i)));
        r.lines.extend(m.lines);
        r.inlinees.extend(m.inlinees);
        root_types.extend(m.root_types);
        root_ids.extend(m.root_ids);
        for (k, n) in m.unknown_records {
            *r.unknown_records.entry(k).or_insert(0) += n;
        }
        for (k, n) in m.unknown_subsections {
            *r.unknown_subsections.entry(k).or_insert(0) += n;
        }
        r.layout.extend(m.layout);
        r.udts.extend(m.udts);
        r.pch |= m.synthesized && m.name.starts_with(b"turbolib-types-");
        r.modules.push((m.name, m.synthesized, m.others));
    }
    for (k, n) in unknown {
        *r.unknown_records.entry(k).or_insert(0) += n;
    }
    // What the symbols reach.
    let id_records = types::records(ipi);
    let (lt, li) = types::live(&type_records, &id_records, &root_types, &root_ids);
    let pick = |flags: &[bool], hashes: &Hashes| -> Vec<u128> {
        let mut v: Vec<u128> = flags
            .iter()
            .zip(&hashes.hashes)
            .filter(|(l, _)| **l)
            .map(|(_, h)| *h)
            .collect();
        v.sort_unstable();
        v.dedup();
        v
    };
    r.live_types = pick(&lt, &r.types);
    r.live_ids = pick(&li, &r.ids);
    // Contributions, but not those of turbolib's renamed copies.
    for c in &p.contributions {
        if p.label_at(c.rva).is_some_and(|l| l.renamed) {
            continue;
        }
        let section = p
            .section_of(c.rva)
            .map_or_else(Vec::new, |s| s.name.clone());
        r.contributions
            .push((section, c.characteristics, c.size, c.data_crc, c.reloc_crc));
    }
    Ok(r)
}

#[cfg(test)]
mod tests;

/// The part of `path` after its last separator (either slash).
fn file_name(path: &[u8]) -> &[u8] {
    path.rsplit(|&c| c == b'/' || c == 0x5C)
        .next()
        .unwrap_or(path)
}
