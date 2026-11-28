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
    /// (name, hash, hash without translation units' tags, module)
    /// (and without which definition of a class of a name it names)
    procedures: Vec<(Vec<u8>, u128, u128, u128, usize)>,
    /// Each module's name, whether it is synthesized, and its other
    /// records' hashes (sorted).
    /// (name, synthesized, its own records' hashes, and their descriptions)
    modules: Vec<(Vec<u8>, bool, Vec<u128>, Vec<String>, Vec<u128>)>,
    /// (what the block's start names, hash).
    /// (what its start names, hash, and its code size if it is a
    /// function's one line at its translation unit's end: see
    /// [`at_end_of_file`])
    /// and a description: its size, and its first file and line)
    lines: Vec<(Vec<u8>, u128, Option<(u32, bool)>, String)>,
    inlinees: Vec<u128>,
    /// Records whose fields aren't known, by kind; subsections not
    /// compared, by kind.
    unknown_records: Map<u16, u64>,
    unknown_subsections: Map<u32, u64>,
    contributions: Vec<ContributionKey>,
    /// For each contribution holding a compiler temporary's name (`/RTC1`
    /// data): its key and a hash of its bytes with the names' numbers and
    /// the base-relocated fields left out.
    temporaries: Vec<(ContributionKey, u128)>,
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
                        "{what} only in the first PDB, the same but for translation units' tags in unique names and constructors' attribute 0x08 (another translation unit's copy kept)"
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
                        "{what} only in the second PDB, the same but for translation units' tags in unique names and constructors' attribute 0x08 (another translation unit's copy kept)"
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

/// Whether line `line` of the file at `path` (read from this machine's
/// disk) names the UDT `name` or a class template it is nested in: the
/// last component of its name, or one with template arguments, without
/// them (not a namespace).
fn names_type(path: &[u8], line: u32, name: &[u8]) -> bool {
    let Some(text) = std::str::from_utf8(path)
        .ok()
        .and_then(|p| std::fs::read(p).ok())
    else {
        return false;
    };
    let Some(l) = text
        .split(|&c| c == b'\n')
        .nth((line as usize).wrapping_sub(1))
    else {
        return false;
    };
    let l = String::from_utf8_lossy(l);
    // Each component of the name outside template arguments, and whether
    // it had them.
    let name = String::from_utf8_lossy(name);
    let mut parts: Vec<(String, bool)> = Vec::new();
    let (mut depth, mut cur, mut templated) = (0usize, String::new(), false);
    let mut chars = name.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '<' => {
                depth += 1;
                templated = true;
            }
            '>' => depth = depth.saturating_sub(1),
            ':' if depth == 0 && chars.peek() == Some(&':') => {
                chars.next();
                parts.push((std::mem::take(&mut cur), templated));
                templated = false;
            }
            _ if depth == 0 => cur.push(c),
            _ => {}
        }
    }
    parts.push((cur, true));
    let ident = |c: Option<char>| c.is_some_and(|c| c.is_alphanumeric() || c == '_');
    parts
        .iter()
        .filter(|p| p.1 && !p.0.is_empty())
        .any(|(p, _)| {
            l.match_indices(p.as_str()).any(|(i, _)| {
                !ident(l[..i].chars().next_back()) && !ident(l[i + p.len()..].chars().next())
            })
        })
}

/// Whether `path` is a translation unit's own source (not a header): a
/// class a macro defines there (`REGISTER_SET_FACTORY_WITH_PROGRAMSTATE`)
/// has that unit's line.
fn source_file(path: &[u8]) -> bool {
    let name = file_name(path).to_ascii_lowercase();
    [&b".cpp"[..], b".cc", b".cxx", b".c"]
        .iter()
        .any(|e| name.ends_with(e))
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
            (Some(x), Some(y))
                if x != y
                    && x.iter()
                        .chain(y)
                        .all(|(f, l)| names_type(f, *l, n) || source_file(f)) =>
            {
                // Each translation unit gives a template's class the line
                // it was instantiated from: its definition's, or an
                // explicit instantiation's (`template class
                // RegionBase<...>;`); the linker keeps the first it reads.
                rep.expect(
                    "UDTs whose source lines differ, each a line naming the type or a class template it is in, or a translation unit's own source (another translation unit's line kept)",
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
    type Procs = Map<Vec<u8>, Vec<(u128, usize, u128, u128)>>;
    let by_name = |r: &Reduced| -> Procs {
        let mut m: Procs = Map::default();
        for (n, h, t, b, module) in &r.procedures {
            m.entry(n.clone()).or_default().push((*h, *module, *t, *b));
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
                    // The same but for translation units' tags in the unique
                    // names of the types they use (a lambda's class, say):
                    // the copy of an inline function the link kept came
                    // from another translation unit (see the types'
                    // comparison).
                    let mut tx: Vec<u128> = x.iter().map(|p| p.2).collect();
                    let mut ty: Vec<u128> = y.iter().map(|p| p.2).collect();
                    tx.sort_unstable();
                    ty.sort_unstable();
                    if tx == ty {
                        rep.expect(
                            "procedures the same but for translation units' tags in unique names and constructors' attribute 0x08 (another translation unit's copy kept)",
                            || show(n),
                        );
                    } else {
                        let mut bx: Vec<u128> = x.iter().map(|p| p.3).collect();
                        let mut by: Vec<u128> = y.iter().map(|p| p.3).collect();
                        bx.sort_unstable();
                        by.sort_unstable();
                        if bx == by {
                            rep.expect(
                                "procedures the same but for which definition of a class of a name they name, and local typedefs (another translation unit's copy kept, whose classes list other implicit members)",
                                || show(n),
                            );
                        } else {
                            rep.fail(
                                "procedures whose symbols differ (signature, frame, locals, ranges, inline sites, ...)",
                                || show(n),
                            );
                        }
                    }
                }
            }
            (None, None) => {}
        }
    }

    // Modules.
    rep.checked("modules", ra.modules.len() as u64);
    type Own<'r> = (
        bool,
        Vec<Vec<u128>>,
        Vec<(&'r [u128], &'r [String])>,
        Vec<Vec<u128>>,
    );
    fn mods(r: &Reduced) -> Map<Vec<u8>, Own<'_>> {
        let mut m: Map<Vec<u8>, Own<'_>> = Map::default();
        for (n, synthesized, h, d, t) in &r.modules {
            let e =
                m.entry(n.clone())
                    .or_insert((*synthesized, Vec::new(), Vec::new(), Vec::new()));
            e.2.push((h.as_slice(), d.as_slice()));
            let mut h = h.clone();
            h.sort_unstable();
            e.1.push(h);
            let mut t = t.clone();
            t.sort_unstable();
            e.3.push(t);
        }
        for v in m.values_mut() {
            v.1.sort();
            v.3.sort();
        }
        m
    }
    let (xa, xb) = (mods(ra), mods(rb));
    let mut names: Vec<&Vec<u8>> = xa.keys().chain(xb.keys()).collect();
    names.sort();
    names.dedup();
    for n in names {
        match (xa.get(n), xb.get(n)) {
            (Some(_), None) => {
                rep.fail("modules (loaded members) only in the first PDB", || show(n));
            }
            (None, Some((true, _, _, _))) => {
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
            (Some(x), Some(y)) if x.1 != y.1 && x.3 == y.3 => {
                rep.expect(
                    "modules whose own records are the same but for translation units' tags in unique names and constructors' attribute 0x08 (the types of another translation unit's copy kept)",
                    || show(n),
                );
            }
            (Some(x), Some(y)) if x.1 != y.1 => {
                if n.as_slice() == b"* Linker *" {
                    rep.fail(
                        "the linker's own records differ (its command line, sections, ...)",
                        || show(n),
                    );
                } else {
                    // (What differs, for a module loaded once in each.)
                    let detail = match (x.2.as_slice(), y.2.as_slice()) {
                        ([(ha, da)], [(hb, db)]) => {
                            let only = |h: &[u128], d: &[String], other: &[u128]| {
                                h.iter()
                                    .zip(d)
                                    .filter(|(k, _)| !other.contains(k))
                                    .map(|(_, d)| d.clone())
                                    .take(3)
                                    .collect::<Vec<_>>()
                                    .join("; ")
                            };
                            format!(
                                ": {} and {} records; first only {}; second only {}",
                                ha.len(),
                                hb.len(),
                                only(ha, da, hb),
                                only(hb, db, ha)
                            )
                        }
                        _ => String::new(),
                    };
                    rep.fail(
                        "modules whose own records differ (S_OBJNAME, S_COMPILE3, UDTs, data, ...)",
                        || format!("{}{detail}", show(n)),
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
    lines_compared(ra, rb, rep);
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
    let show_contribution = |c: &ContributionKey| {
        format!(
            "{} {:#x}, {:#x} bytes, CRCs {:#x} {:#x}",
            show(&c.0),
            c.1,
            c.2,
            c.3,
            c.4
        )
    };
    // (As multisets: both sorted.)
    let (mut i, mut j) = (0, 0);
    let (mut only_a, mut only_b) = (Vec::new(), Vec::new());
    while i < ca.len() || j < cb.len() {
        match (ca.get(i), cb.get(j)) {
            (Some(x), Some(y)) if x == y => {
                i += 1;
                j += 1;
            }
            (Some(x), Some(y)) if x < y => {
                only_a.push(x);
                i += 1;
            }
            (Some(x), None) => {
                only_a.push(x);
                i += 1;
            }
            (_, Some(y)) => {
                only_b.push(y);
                j += 1;
            }
            (None, None) => break,
        }
    }
    // link.exe leaves some contributions' CRCs 0 (most of the data of
    // turbolib's synthesized members, and of some objects): those pair
    // with one of the same section, characteristics, and size. (Their
    // bytes are compared in the images.)
    let mut unpaired: Map<(&[u8], u32, u32), Vec<&ContributionKey>> = Map::default();
    for &c in &only_b {
        unpaired.entry((&c.0, c.1, c.2)).or_default().push(c);
    }
    let zero = |c: &ContributionKey| c.3 == 0 && c.4 == 0;
    let mut left_a = Vec::new();
    for &c in &only_a {
        let list = unpaired.entry((&c.0, c.1, c.2)).or_default();
        let at = list.iter().position(|&d| zero(d) || zero(c));
        match at {
            Some(k) => {
                list.swap_remove(k);
                rep.expect(
                    "section contributions whose CRCs the linker left 0 in one PDB",
                    || show_contribution(c),
                );
            }
            None => left_a.push(c),
        }
    }
    let mut left_b: Vec<&ContributionKey> = unpaired.into_values().flatten().collect();
    left_b.sort();
    // Then those whose bytes differ only in compiler temporaries' names
    // (another translation unit's copy of a function's `/RTC1` data).
    let temps =
        |r: &Reduced| -> Map<ContributionKey, u128> { r.temporaries.iter().cloned().collect() };
    let (ta, tb) = (temps(ra), temps(rb));
    left_a.retain(|&c| {
        let Some(h) = ta.get(c) else { return true };
        match left_b
            .iter()
            .position(|d| (&d.0, d.1, d.2) == (&c.0, c.1, c.2) && tb.get(*d) == Some(h))
        {
            Some(k) => {
                left_b.remove(k);
                rep.expect(
                    "section contributions whose bytes differ only in compiler temporaries' names (another translation unit's copy kept)",
                    || show_contribution(c),
                );
                false
            }
            None => true,
        }
    });
    for (list, which) in [(left_a, "first"), (left_b, "second")] {
        for c in list {
            rep.fail(
                "section contributions that differ (section, characteristics, size, CRCs)",
                || format!("only in the {which}: {}", show_contribution(c)),
            );
        }
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

/// Compares the line blocks as multisets. MSVC gives a function the
/// compiler generates (an implicit constructor, say) one line: its
/// translation unit's end (the line after its file's last). Copies of
/// that function from two translation units are the same code with other
/// lines, and folding may keep another's: such blocks, one line at the
/// end of a file in both, at what the same name starts (with the same
/// code size), pair as expected.
fn lines_compared(ra: &Reduced, rb: &Reduced, rep: &mut Report) {
    fn sorted(r: &Reduced) -> Vec<(u128, &[u8], Option<(u32, bool)>, &str)> {
        let mut v: Vec<(u128, &[u8], Option<(u32, bool)>, &str)> = r
            .lines
            .iter()
            .map(|(n, h, e, d)| (*h, n.as_slice(), *e, d.as_str()))
            .collect();
        v.sort_unstable();
        v
    }
    let (a, b) = (sorted(ra), sorted(rb));
    let (mut i, mut j) = (0, 0);
    let (mut only_a, mut only_b) = (Vec::new(), Vec::new());
    while i < a.len() || j < b.len() {
        match (a.get(i), b.get(j)) {
            (Some(x), Some(y)) if x.0 == y.0 => {
                i += 1;
                j += 1;
            }
            (Some(x), Some(y)) if x.0 < y.0 => {
                only_a.push(*x);
                i += 1;
            }
            (Some(x), None) => {
                only_a.push(*x);
                i += 1;
            }
            (_, Some(y)) => {
                only_b.push(*y);
                j += 1;
            }
            (None, None) => break,
        }
    }
    // A function's one line at a translation unit's end in one, and one
    // line (that one, or another file's) in the other: paired by name and
    // code size, then (identical code folded into groups another layout
    // labels otherwise) by code size alone.
    fn pair<'x, K: std::hash::Hash + Eq>(
        only_a: Vec<(u128, &'x [u8], Option<(u32, bool)>, &'x str)>,
        only_b: Vec<(u128, &'x [u8], Option<(u32, bool)>, &'x str)>,
        key: impl Fn(&(u128, &'x [u8], Option<(u32, bool)>, &'x str)) -> Option<K>,
        what: &str,
        rep: &mut Report,
    ) -> (
        Vec<(u128, &'x [u8], Option<(u32, bool)>, &'x str)>,
        Vec<(u128, &'x [u8], Option<(u32, bool)>, &'x str)>,
    ) {
        // The second's, by key: those at an end, and those not.
        let mut b: Map<K, (Vec<usize>, Vec<usize>)> = Map::default();
        for (k, y) in only_b.iter().enumerate() {
            if let (Some(key), Some((_, eof))) = (key(y), y.2) {
                let e = b.entry(key).or_default();
                if eof { e.0.push(k) } else { e.1.push(k) }
            }
        }
        let mut taken = vec![false; only_b.len()];
        let mut left_a = Vec::new();
        for x in only_a {
            let found = match (key(&x), x.2) {
                (Some(k), Some((_, eof))) => b.get_mut(&k).and_then(|(ends, others)| {
                    // (An end pairs with any one line; another line only
                    // with an end.)
                    ends.pop().or_else(|| if eof { others.pop() } else { None })
                }),
                _ => None,
            };
            match found {
                Some(k) => {
                    taken[k] = true;
                    rep.expect(what, || format!("{} and {}", x.3, only_b[k].3));
                }
                None => left_a.push(x),
            }
        }
        let left_b = only_b
            .into_iter()
            .zip(taken)
            .filter(|(_, t)| !t)
            .map(|(y, _)| y)
            .collect();
        (left_a, left_b)
    }
    let (left_a, left_b) = pair(
        only_a,
        only_b,
        |x| x.2.map(|(size, _)| (x.1, size)),
        "line blocks of a function the compiler generated, at a translation unit's end in one (another translation unit's copy kept)",
        rep,
    );
    let (still_a, still_b) = pair(
        left_a,
        left_b,
        |x| x.2.map(|(size, _)| size),
        "line blocks of a function the compiler generated, at a translation unit's end in one, at what another name starts (identical code folded otherwise)",
        rep,
    );
    let mut left_a: Vec<String> = still_a.iter().map(|x| x.3.to_string()).collect();
    let mut left_b: Vec<String> = still_b.iter().map(|y| y.3.to_string()).collect();
    left_a.sort();
    left_b.sort();
    for d in left_a {
        rep.fail("line blocks only in the first PDB (or different)", || d);
    }
    for d in left_b {
        rep.fail("line blocks only in the second PDB (or different)", || d);
    }
}

/// Whether `line` is the line after the last of the file at `path` (where
/// MSVC puts a function the compiler generates), the file read from this
/// machine's disk (once each); false if it can't be read.
fn at_end_of_file(path: &[u8], line: u32) -> bool {
    use std::sync::{Mutex, OnceLock};
    static LINES: OnceLock<Mutex<Map<Vec<u8>, Option<u32>>>> = OnceLock::new();
    let cache = LINES.get_or_init(|| Mutex::new(Map::default()));
    let known = cache
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .get(path)
        .copied();
    let count = known.unwrap_or_else(|| {
        let n = std::str::from_utf8(path)
            .ok()
            .and_then(|p| std::fs::read(p).ok())
            .map(|d| {
                let newlines = d.iter().filter(|&&c| c == b'\n').count();
                // (A last line without its newline is a line too.)
                let partial = usize::from(d.last().is_some_and(|&c| c != b'\n'));
                u32::try_from(newlines + partial).unwrap_or(u32::MAX)
            });
        cache
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .insert(path.to_vec(), n);
        n
    });
    count.is_some_and(|n| line == n.saturating_add(1))
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
                    let offset = u32_at(offset as usize);
                    // (A thread-local's offset is from the TLS template's
                    // start.)
                    let rva = if matches!(
                        kind,
                        turbonop_codeview::symbols::kind::S_GTHREAD32
                            | turbonop_codeview::symbols::kind::S_LTHREAD32
                    ) {
                        self.p.tls.and_then(|(start, _)| start.checked_add(offset))
                    } else {
                        self.p.rva_of(seg, offset)
                    };
                    self.address(rva, h);
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
    /// What each of `others` is (kind and name).
    others_desc: Vec<String>,
    /// `others` without translation units' tags (see the types).
    others_tagless: Vec<u128>,
    procedures: Vec<(Vec<u8>, u128, u128, u128)>,
    /// (what its start names, hash, and its code size if it is a
    /// function's one line at its translation unit's end: see
    /// [`at_end_of_file`])
    /// and a description: its size, and its first file and line)
    lines: Vec<(Vec<u8>, u128, Option<(u32, bool)>, String)>,
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
    (named_types, named_ids): (&Hashes, &Hashes),
    names: Option<&StringTable<'_>>,
) -> ModuleFacts {
    let mut out = ModuleFacts {
        name: m.name.to_vec(),
        synthesized: false,
        others: Vec::new(),
        others_desc: Vec::new(),
        others_tagless: Vec::new(),
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
    let named = Context {
        p,
        types: named_types,
        ids: named_ids,
        names,
        checksums,
    };
    // Symbols: procedures (and other top-level scopes) as one hash each;
    // the rest one each.
    let mut depth = 0usize;
    let mut scope: Option<(Vec<u8>, H128, H128, H128, bool)> = None;
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
        // The same without translation units' tags (only in a scope).
        let tagless_record = || {
            if layout.is_some() {
                return record;
            }
            let mut t = H128::new();
            tagless.symbol(s.kind, s.data, &mut t);
            t.finish()
        };
        let named_record = || {
            if layout.is_some() {
                return record;
            }
            let mut t = H128::new();
            named.symbol(s.kind, s.data, &mut t);
            t.finish()
        };
        if let Some((_, sh, th, nh, _)) = &mut scope {
            sh.u128(record);
            th.u128(tagless_record());
            // (A local typedef some translation units' copies list and
            // others don't: `_Mypair` in `std::make_pair`.)
            if s.kind != kind::S_UDT {
                nh.u128(named_record());
            }
            if opens_scope(s.kind) {
                depth += 1;
            } else if closes_scope(s.kind) {
                depth = depth.saturating_sub(1);
            }
            if depth == 0 {
                let (name, sh, th, nh, procedure) = scope.take().expect("in a scope");
                if procedure {
                    out.procedures
                        .push((name, sh.finish(), th.finish(), nh.finish()));
                } else {
                    let mut x = H128::new();
                    x.bytes(&name);
                    x.u128(sh.finish());
                    out.others.push(x.finish());
                    out.others_desc.push(format!("scope {}", show(&name)));
                    let mut t = H128::new();
                    t.bytes(&name);
                    t.u128(th.finish());
                    out.others_tagless.push(t.finish());
                }
            }
            continue;
        }
        if opens_scope(s.kind) {
            let procedure = matches!(
                s.kind,
                kind::S_GPROC32
                    | kind::S_LPROC32
                    | kind::S_GPROC32_ID
                    | kind::S_LPROC32_ID
                    // (A thunk is code too: an adjustor's, a COMDAT, moves
                    // with folding.)
                    | kind::S_THUNK32
            );
            let name = s.name().map_or_else(Vec::new, |n| canonical(n).to_vec());
            let mut sh = H128::new();
            sh.u128(record);
            let mut th = H128::new();
            th.u128(tagless_record());
            let mut nh = H128::new();
            nh.u128(named_record());
            scope = Some((name, sh, th, nh, procedure));
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
        out.others_tagless.push(tagless_record());
        out.others.push(record);
        out.others_desc.push(describe(&s));
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
                // One line for the whole function: (its size, whether the
                // line is its translation unit's end).
                let end = (blocks.len() == 1
                    && first.lines.len() == 1
                    && first.lines[0].offset.get() == 0)
                    .then(|| {
                        let eof = cx
                            .file(first.file)
                            .is_some_and(|f| at_end_of_file(f, first.lines[0].line()));
                        (first.code_size, eof)
                    });
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
                let desc = format!(
                    "{} ({:#x} bytes, {} lines, {}:{})",
                    show(&name),
                    first.code_size,
                    blocks.iter().map(|b| b.lines.len()).sum::<usize>(),
                    cx.file(first.file)
                        .map_or_else(|| "?".into(), |f| show(file_name(f))),
                    first.lines.first().map_or(0, |l| l.line())
                );
                out.lines.push((name, h.finish(), end, desc));
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
    r.types = types::hash(&type_records, None, names.as_ref(), false, false);
    let records = types::records(ipi);
    r.ids = types::hash(
        &records,
        Some((&r.types, &type_records)),
        names.as_ref(),
        false,
        false,
    );
    // The same, blind to translation units' tags in unique names.
    let tagless_types = types::hash(&type_records, None, names.as_ref(), true, false);
    let tagless_ids = types::hash(
        &records,
        Some((&tagless_types, &type_records)),
        names.as_ref(),
        true,
        false,
    );
    // And blind to which definition of a class of a name (see
    // `types::hash`), for procedures only.
    let named_types = types::hash(&type_records, None, names.as_ref(), true, true);
    let named_ids = types::hash(
        &records,
        Some((&named_types, &type_records)),
        names.as_ref(),
        true,
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
            (&named_types, &named_ids),
            names.as_ref(),
        )
    });
    for (i, m) in reduced.into_iter().enumerate() {
        r.procedures
            .extend(m.procedures.into_iter().map(|(n, h, t, b)| (n, h, t, b, i)));
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
        r.modules.push((
            m.name,
            m.synthesized,
            m.others,
            m.others_desc,
            m.others_tagless,
        ));
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
        // (Nor those of the tables compared as tables: `.pdata`, say, whose
        // contributions' sizes follow the functions each object kept.)
        if p.in_tables(c.rva, c.size) {
            continue;
        }
        let section = p
            .section_of(c.rva)
            .map_or_else(Vec::new, |s| s.name.clone());
        let key = (section, c.characteristics, c.size, c.data_crc, c.reloc_crc);
        if c.characteristics & 0x2000_0000 == 0 {
            let data = p.bytes(c.rva, c.size);
            if data.windows(2).any(|w| w == b"$S") {
                let mut h = H128::new();
                let relocs = p.relocations_in(c.rva, c.size);
                let mut at = 0;
                while at < data.len() {
                    if let Some(&(_, size)) = relocs.iter().find(|r| r.0 - c.rva == at as u32) {
                        at += usize::from(size);
                        h.u64(u64::MAX);
                        continue;
                    }
                    if let Some(n) = crate::image::bytes::temporary(&data, at) {
                        h.bytes(b"$S#");
                        at += n;
                        while data.get(at) == Some(&0) {
                            at += 1;
                        }
                        continue;
                    }
                    h.bytes(&data[at..at + 1]);
                    at += 1;
                }
                r.temporaries.push((key.clone(), h.finish()));
            }
        }
        r.contributions.push(key);
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
