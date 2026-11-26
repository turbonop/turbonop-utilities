//! `turbolib verify`: whether a rewritten static library is equivalent to
//! the original, read from the two libraries alone.
//!
//! The rewritten library may differ from the original only as turbolib's
//! folding does (see `docs/turbolib.md`, "How it works"): COMDAT units move
//! out of original members into synthesized members, the symbols they
//! defined become alias weak externals whose default is [`ALIAS_DEFAULT`]
//! (or undefined, or `NOLIBRARY` weak externals whose default is a renamed
//! copy), and each member that gave units away refers to the anchor of each
//! synthesized member holding them. What is checked:
//!
//! - **Members.** Every original member is still there, under its name,
//!   in the same relative order. Every added member is a synthesized one:
//!   a COFF object that defines an anchor (a symbol whose name starts with
//!   [`member::RENAMED`], or a precompiled-types member's [`PCH_SYMBOL`],
//!   and that names no original symbol), wherever it is placed.
//! - **Passed-through members** (import objects, `/GL` objects, anything
//!   unrecognized) are byte-identical.
//! - **Definitions.** Every external the original defines is still
//!   defined by some member, with the same kind (code, data, ...; COMDAT
//!   selection). A weak external, even `NOLIBRARY` with a defined
//!   default, doesn't count: it isn't indexed and loads nothing. The only
//!   new definitions are anchors and renamed copies (each of a symbol the
//!   original defines, of its kind).
//! - **Each original member**, against itself: its directives, its
//!   sections outside COMDATs (except debug sections, side tables, and
//!   `.chks64`), and its environment are unchanged; each COMDAT it keeps is
//!   unchanged; each it lost was foldable (not `NODUPLICATES`) and is held,
//!   in its environment, by a synthesized member whose anchor the member
//!   refers to. Each symbol it defined is still defined, or (moved) now an
//!   alias weak external whose default is [`ALIAS_DEFAULT`] and that a
//!   synthesized member it anchors defines, or undefined and defined
//!   elsewhere, or a `NOLIBRARY` weak external whose default is the
//!   renamed copy. Its undefined symbols are the original's
//!   plus the moved ones and anchors; its weak externals the original's
//!   plus the moved ones. Its side-table entries (`/guard` tables,
//!   `.sxdata`, `.voltbl`) are each still its own or a synthesized
//!   member's it refers to.
//! - **Synthesized members.** Every COMDAT one holds is identical
//!   (fingerprints, see [`member`]) to a copy some original member held in
//!   the same environment, and no COMDAT is in two synthesized members.
//!   Each symbol a synthesized member refers to is one the original
//!   library defines or refers to.
//! - **The archive index**, of both libraries: it names every external
//!   definition and alias weak external (as `lib.exe` indexes them), each
//!   name sending to the first member in archive order that has it, and the
//!   two linker members agree. A synthesized member's names other than its
//!   anchors may go unlisted (as under `--pull=preserve`, so the index
//!   names the original definers), and [`ALIAS_DEFAULT`] is never listed. Under
//!   `--pull=preserve`, every symbol of the original's index sends to the
//!   same member as in the original, or (reported as expected) to a
//!   synthesized member when the original definer has no side effects of
//!   its own (initializers, terminators, TLS callbacks, `/INCLUDE`), as the
//!   engine folds them. That doesn't show that loading the definer had no
//!   effect through what it refers to (which may load members that have
//!   side effects); nothing here follows references.
//!
//! What it can't prove: anything about debug information (see
//! `compare-images`), and anything a linker does beyond loading members
//! by the index and choosing among COMDAT copies, which the plan's spikes
//! measured.

pub mod index;
pub mod member;

/// The default of the alias weak externals that keep moved symbols defined
/// in their original members; each synthesized member defines it, and the
/// index never lists it.
pub const ALIAS_DEFAULT: &[u8] = b"__turbolib$alias";

/// The start of a precompiled-types member's symbol (`--precomp=library`),
/// which the members that share its types refer to as they do to anchors.
pub const PCH_SYMBOL: &[u8] = b"__@@_PchSym_turbolib_";

use std::borrow::Cow;

use turbonop_archive::Archive;
use turbonop_hash::{Map, Set};

use crate::hash::H128;
use crate::report::{Report, show};
use crate::shard::Sharded;
use index::Index;
use member::{Body, Class, Coff, Kind, RENAMED, canonical, starts_with_ignore_case};

/// What `--pull` promised about which members a link loads.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Pull {
    /// Nothing about loading changes: every symbol of the original's index
    /// still sends to the same member.
    #[default]
    Preserve,
    /// Loading may change; definers that changed are reported, not failed.
    Relaxed,
}

/// The options of a check.
#[derive(Clone, Copy, Debug, Default)]
pub struct Options {
    pub pull: Pull,
}

/// One member of a library.
struct Member<'a> {
    name: Cow<'a, [u8]>,
    offset: u64,
    data: &'a [u8],
    body: Body,
}

/// A library, read.
struct Library<'a> {
    members: Vec<Member<'a>>,
    index: Index<'a>,
    /// Each member's position, by its header's offset.
    by_offset: Map<u64, usize>,
}

impl<'a> Library<'a> {
    fn new(
        archive: &Archive<'a>,
        files: Files<'a>,
        bodies: &mut impl Iterator<Item = Body>,
    ) -> Self {
        let members: Vec<Member<'a>> = files
            .into_iter()
            .map(|(name, offset, data)| Member {
                name,
                offset,
                data,
                body: bodies.next().unwrap_or(Body::Other("not read".into())),
            })
            .collect();
        let by_offset = members
            .iter()
            .enumerate()
            .map(|(i, m)| (m.offset, i))
            .collect();
        Library {
            members,
            index: index::read(archive),
            by_offset,
        }
    }

    fn name(&self, i: usize) -> String {
        format!("{} (member {i})", show(&self.members[i].name))
    }
}

/// A library's members: name, header offset, data.
type Files<'a> = Vec<(Cow<'a, [u8]>, u64, &'a [u8])>;

fn open<'a>(data: &'a [u8], what: &str) -> Result<(Archive<'a>, Files<'a>), String> {
    if !Archive::is_archive(data) {
        return Err(format!("{what}: not an archive"));
    }
    let archive = Archive::parse(data).map_err(|e| format!("{what}: {e}"))?;
    if archive.is_thin() {
        return Err(format!("{what}: a thin archive (not read)"));
    }
    let files = archive
        .files()
        .map(|m| (m.name, m.header_offset, m.data))
        .collect();
    Ok((archive, files))
}

/// Checks that `rewritten` is equivalent to `original` (see the module's
/// documentation), reading every member on every core.
pub fn verify(original: &[u8], rewritten: &[u8], options: &Options) -> Result<Report, String> {
    let (oa, of) = open(original, "the original library")?;
    let (ra, rf) = open(rewritten, "the rewritten library")?;
    // Every member of both, read in parallel.
    let all: Vec<&[u8]> = of.iter().chain(&rf).map(|m| m.2).collect();
    let mut bodies = turbonop_par::map_balanced(&all, 4, |d: &&[u8]| member::read(d)).into_iter();
    let o = Library::new(&oa, of, &mut bodies);
    let r = Library::new(&ra, rf, &mut bodies);
    Ok(check(&o, &r, options))
}

/// The pairing of the two libraries' members.
struct Pairing {
    /// For each original member, its rewritten counterpart.
    of_original: Vec<Option<usize>>,
    /// For each rewritten member, its original counterpart.
    of_rewritten: Vec<Option<usize>>,
}

/// Pairs each original member with the next rewritten member of its name,
/// in order: the original's names must be a subsequence of the
/// rewritten's.
fn pair(o: &Library<'_>, r: &Library<'_>) -> Pairing {
    let mut p = Pairing {
        of_original: vec![None; o.members.len()],
        of_rewritten: vec![None; r.members.len()],
    };
    let mut next = 0;
    for (i, m) in o.members.iter().enumerate() {
        if let Some(k) = r.members[next..].iter().position(|x| x.name == m.name) {
            p.of_original[i] = Some(next + k);
            p.of_rewritten[next + k] = Some(i);
            next += k + 1;
        }
    }
    p
}

/// The names each member puts in the archive index, by member: a COFF
/// object's external definitions; an import object's symbols; and, for
/// a member that isn't read, the names `original_index` gives its
/// counterpart.
fn indexed<'s>(
    lib: &'s Library<'_>,
    counterpart: impl Fn(usize) -> Option<usize>,
    original: &'s Library<'_>,
) -> Vec<Indexed<'s>> {
    // The original index's names, by member.
    let mut by_member: Vec<Vec<&[u8]>> = vec![Vec::new(); original.members.len()];
    for &(name, offset) in original.index.entries() {
        if let Some(&i) = original.by_offset.get(&offset) {
            by_member[i].push(name);
        }
    }
    lib.members
        .iter()
        .enumerate()
        .map(|(i, m)| match &m.body {
            Body::Coff(c) => c
                .definitions
                .iter()
                .filter(|d| *d.name != *ALIAS_DEFAULT)
                .map(|d| (Cow::Borrowed(&*d.name), d.kind))
                .collect(),
            Body::Import(names) => names
                .iter()
                .map(|n| {
                    (
                        Cow::Borrowed(&**n),
                        Kind {
                            class: Class::Import,
                            selection: 0,
                        },
                    )
                })
                .collect(),
            Body::Ltcg | Body::Other(_) => counterpart(i)
                .map(|k| {
                    by_member[k]
                        .iter()
                        .map(|n| {
                            (
                                Cow::Borrowed(*n),
                                Kind {
                                    class: Class::Indexed,
                                    selection: 0,
                                },
                            )
                        })
                        .collect()
                })
                .unwrap_or_default(),
        })
        .collect()
}

/// A unit's key: its canonical leader name, fingerprint, and environment.
fn unit_key(leader: &[u8], fingerprint: u128, environment: u128) -> u128 {
    let mut h = H128::new();
    h.bytes(canonical(leader));
    h.u128(fingerprint);
    h.u128(environment);
    h.finish()
}

/// The names a member puts in the index, with their kinds.
type Indexed<'s> = Vec<(Cow<'s, [u8]>, Kind)>;

/// Each name's definitions in a library: (member, kind), in member order.
type Definitions<'s> = Sharded<&'s [u8], Vec<(usize, Kind)>>;

fn definitions<'s>(indexed: &'s [Indexed<'s>]) -> Definitions<'s> {
    Sharded::build(
        indexed,
        |i, list, out| out.extend(list.iter().map(|(n, k)| (&**n, (i, *k)))),
        |m: &mut Map<&'s [u8], Vec<(usize, Kind)>>, name, d| m.entry(name).or_default().push(d),
    )
}

/// What the whole libraries say, for the members' checks.
struct Facts<'s> {
    /// Every name the rewritten library defines, with its definers.
    defined: &'s Definitions<'s>,
    /// The synthesized members that define each anchor.
    anchor_of: Map<&'s [u8], usize>,
    /// The synthesized members holding each unit key.
    holders: Map<u128, Vec<usize>>,
    /// Every original unit key.
    original_units: Sharded<u128, ()>,
    /// Every name an original member defines or refers to.
    original_names: Sharded<&'s [u8], ()>,
    /// The synthesized members that define each name.
    synthesized_definitions: Map<&'s [u8], Vec<usize>>,
}

/// Findings made in parallel, to be reported in a fixed order: (the
/// name they are about, category, example).
type Findings = Vec<(Vec<u8>, &'static str, String)>;

fn report_sorted(mut findings: Findings, report: &mut Report) {
    findings.sort();
    for (_, category, example) in findings {
        report.fail(category, || example);
    }
}

fn check(o: &Library<'_>, r: &Library<'_>, options: &Options) -> Report {
    let mut report = Report::new();
    report.checked("original members", o.members.len() as u64);
    report.checked("rewritten members", r.members.len() as u64);
    for (lib, which) in [(o, "original"), (r, "rewritten")] {
        if lib.index.ec {
            report.unchecked("ARM64EC symbol tables (/<ECSYMBOLS>/)", || {
                format!("the {which} library has one")
            });
        }
        if lib.index.damaged {
            report.fail("archive indexes that can't be read in full", || {
                format!("the {which} library's")
            });
        }
    }
    let p = pair(o, r);
    let oi = indexed(o, Some, o);
    let ri = indexed(r, |j| p.of_rewritten[j], o);
    let (o_defs, r_defs) = turbonop_par::join(|| definitions(&oi), || definitions(&ri));
    report.checked("names the original defines", o_defs.len() as u64);

    // Members: missing, out of order, added.
    let added: Vec<usize> = (0..r.members.len())
        .filter(|&j| p.of_rewritten[j].is_none())
        .collect();
    let added_names: Set<&[u8]> = added.iter().map(|&j| &*r.members[j].name).collect();
    for (i, m) in o.members.iter().enumerate() {
        if p.of_original[i].is_none() {
            if added_names.contains(&*m.name) {
                report.fail("original members out of order", || o.name(i));
            } else {
                report.fail("original members missing", || o.name(i));
            }
        }
    }
    // (A precompiled-types member's symbol, under --precomp=library, is
    // one too: each member that shares its types refers to it.)
    let is_anchor = |name: &[u8]| {
        (name.starts_with(RENAMED) || name.starts_with(PCH_SYMBOL))
            && name != ALIAS_DEFAULT
            && !o_defs.contains(&canonical(name))
    };
    let mut synthesized = Vec::new();
    for &j in &added {
        let anchored = r.members[j]
            .body
            .coff()
            .is_some_and(|c| c.definitions.iter().any(|d| is_anchor(&d.name)));
        if anchored {
            synthesized.push(j);
        } else {
            report.fail("added members that aren't synthesized (no anchor)", || {
                format!("{}: {}", r.name(j), r.members[j].body.describe())
            });
        }
    }
    report.checked("synthesized members", synthesized.len() as u64);

    // Definitions across each library, shard by shard.
    let found = turbonop_par::map_balanced(o_defs.shards(), 1, |shard| {
        let mut f = Findings::new();
        for (name, was) in shard {
            let Some(now) = r_defs.get(name) else {
                f.push((
                    name.to_vec(),
                    "symbols the original defines that no member defines",
                    show(name),
                ));
                continue;
            };
            if let Some((_, k)) = was.iter().find(|(_, k)| !now.iter().any(|(_, n)| n == k)) {
                let kinds: Vec<String> = now.iter().map(|(_, k)| k.describe()).collect();
                f.push((
                    name.to_vec(),
                    "symbols no longer defined with their kind",
                    format!(
                        "{}: was {}, now {}",
                        show(name),
                        k.describe(),
                        kinds.join("; ")
                    ),
                ));
            }
        }
        f
    });
    report_sorted(found.into_iter().flatten().collect(), &mut report);
    let found = turbonop_par::map_balanced(r_defs.shards(), 1, |shard| {
        let mut f = Findings::new();
        for (name, now) in shard {
            // (A precompiled-types member's symbol is new, as anchors are.)
            if o_defs.contains(name) || name.starts_with(PCH_SYMBOL) {
                continue;
            }
            if !name.starts_with(RENAMED) {
                f.push((
                    name.to_vec(),
                    "symbols defined that the original doesn't define",
                    show(name),
                ));
            } else if let Some(was) = o_defs.get(&canonical(name))
                && let Some((_, k)) = now.iter().find(|(_, k)| !was.iter().any(|(_, w)| w == k))
            {
                f.push((
                    name.to_vec(),
                    "renamed copies of another kind than their symbol",
                    format!("{}: {}", show(name), k.describe()),
                ));
            }
        }
        f
    });
    report_sorted(found.into_iter().flatten().collect(), &mut report);

    // Facts for the members' checks.
    let coffs: Vec<Option<&Coff>> = o.members.iter().map(|m| m.body.coff()).collect();
    let (original_units, original_names) = turbonop_par::join(
        || {
            Sharded::build(
                &coffs,
                |_, c, out| {
                    if let Some(c) = c {
                        out.extend(
                            c.units
                                .iter()
                                .map(|u| (unit_key(&u.leader, u.fingerprint, c.environment), ())),
                        );
                    }
                },
                |m, k, ()| {
                    m.insert(k, ());
                },
            )
        },
        || {
            Sharded::build(
                &o.members,
                |i, m, out| {
                    if let Some(c) = m.body.coff() {
                        out.extend(c.undefined.iter().map(|n| (&**n, ())));
                        out.extend(c.weak.iter().map(|w| (&*w.name, ())));
                        // `/INCLUDE` names it refers to too: a PCH user's
                        // PCH object's symbol, which synthesized members
                        // holding its units refer to.
                        out.extend(
                            c.directives
                                .iter()
                                .filter(|d| {
                                    starts_with_ignore_case(d, b"/include:")
                                        || starts_with_ignore_case(d, b"-include:")
                                })
                                .map(|d| (&d[9..], ())),
                        );
                    }
                    out.extend(oi[i].iter().map(|(n, _)| (&**n, ())));
                },
                |m, k, ()| {
                    m.insert(k, ());
                },
            )
        },
    );
    let mut facts = Facts {
        defined: &r_defs,
        anchor_of: Map::default(),
        holders: Map::default(),
        original_units,
        original_names,
        synthesized_definitions: Map::default(),
    };
    for &j in &synthesized {
        let Some(c) = r.members[j].body.coff() else {
            continue;
        };
        for d in &c.definitions {
            if is_anchor(&d.name) {
                facts.anchor_of.insert(&*d.name, j);
            }
            facts
                .synthesized_definitions
                .entry(&*d.name)
                .or_default()
                .push(j);
        }
        for u in &c.units {
            facts
                .holders
                .entry(unit_key(&u.leader, u.fingerprint, c.environment))
                .or_default()
                .push(j);
        }
    }

    // Each original member against its counterpart, and each synthesized
    // member, in parallel.
    let pairs: Vec<(usize, usize)> = p
        .of_original
        .iter()
        .enumerate()
        .filter_map(|(i, j)| j.map(|j| (i, j)))
        .collect();
    let reports = turbonop_par::map_balanced(&pairs, 16, |&(i, j)| {
        let mut rep = Report::new();
        check_member(o, r, i, j, &facts, &mut rep);
        rep
    });
    for rep in reports {
        report.merge(rep);
    }
    let reports = turbonop_par::map_balanced(&synthesized, 4, |&j| {
        let mut rep = Report::new();
        check_synthesized(r, j, &facts, &mut rep);
        rep
    });
    for rep in reports {
        report.merge(rep);
    }
    // No unit in two synthesized members.
    let mut twice: Vec<&Vec<usize>> = facts.holders.values().filter(|h| h.len() > 1).collect();
    twice.sort_unstable();
    for holders in twice {
        report.fail("COMDATs held by more than one synthesized member", || {
            holders
                .iter()
                .map(|&j| r.name(j))
                .collect::<Vec<_>>()
                .join(", ")
        });
    }

    // The archive indexes: what each member puts in it.
    let (o_names, r_names) = turbonop_par::join(|| index_names(o, &oi), || index_names(r, &ri));
    let (o_index, r_index) = turbonop_par::join(|| definitions(&o_names), || definitions(&r_names));
    let is_synthesized = |j: usize| synthesized.binary_search(&j).is_ok();
    let (a, b) = turbonop_par::join(
        || check_index(o, &o_index, &|_| false, "original"),
        || check_index(r, &r_index, &is_synthesized, "rewritten"),
    );
    report.merge(a);
    report.merge(b);
    check_pull(o, r, &p, options, &mut report);
    report
}

/// The names each member puts in the archive index: its [`indexed`] names
/// and its alias weak externals.
fn index_names<'s>(lib: &'s Library<'_>, indexed: &'s [Indexed<'s>]) -> Vec<Indexed<'s>> {
    turbonop_par::map_range(lib.members.len(), 64, |i| {
        let alias = Kind {
            class: Class::Indexed,
            selection: 0,
        };
        let Some(c) = lib.members[i].body.coff() else {
            return indexed[i].clone();
        };
        let mut out = indexed[i].clone();
        out.extend(
            c.weak
                .iter()
                .filter(|w| w.search == turbonop_coff::weak_search::ALIAS)
                .map(|w| (Cow::Borrowed(&*w.name), alias)),
        );
        out
    })
}

/// The synthesized members a member refers to by anchor.
fn anchored(c: &Coff, facts: &Facts<'_>) -> Vec<usize> {
    let mut out: Vec<usize> = c
        .undefined
        .iter()
        .map(|n| &**n)
        .chain(c.weak.iter().map(|w| &*w.name))
        .filter_map(|n| facts.anchor_of.get(n).copied())
        .collect();
    out.sort_unstable();
    out.dedup();
    out
}

fn check_member(
    o: &Library<'_>,
    r: &Library<'_>,
    i: usize,
    j: usize,
    facts: &Facts<'_>,
    rep: &mut Report,
) {
    let (om, rm) = (&o.members[i], &r.members[j]);
    let (oc, rc) = match (&om.body, &rm.body) {
        (Body::Coff(a), Body::Coff(b)) => (a, b),
        (a, _) if a.passed_through() => {
            rep.checked("members passed through", 1);
            if om.data != rm.data {
                rep.fail("passed-through members changed", || {
                    format!("{}: {}", o.name(i), a.describe())
                });
            }
            return;
        }
        (a, b) => {
            rep.fail("members that changed what they are", || {
                format!("{}: was {}, now {}", o.name(i), a.describe(), b.describe())
            });
            return;
        }
    };
    rep.checked("original COFF members compared", 1);
    let name = || o.name(i);
    if oc.directives != rc.directives {
        rep.fail("original members whose directives changed", name);
    }
    if oc.environment != rc.environment || oc.machine != rc.machine {
        rep.fail(
            "original members whose environment (machine, @feat.00, /FAILIFMISMATCH) changed",
            name,
        );
    }
    if oc.pieces != rc.pieces {
        let changed = oc
            .pieces
            .iter()
            .find(|p| !rc.pieces.contains(p))
            .or_else(|| rc.pieces.iter().find(|p| !oc.pieces.contains(p)))
            .map_or_else(String::new, |p| show(&p.name));
        rep.fail(
            "original members whose sections outside COMDATs changed",
            || format!("{}: {changed}", name()),
        );
    }
    let synthesized = anchored(rc, facts);
    // COMDATs kept and lost.
    let kept: Set<(&[u8], u128)> = rc
        .units
        .iter()
        .map(|u| (&*u.leader, u.fingerprint))
        .collect();
    let had: Set<(&[u8], u128)> = oc
        .units
        .iter()
        .map(|u| (&*u.leader, u.fingerprint))
        .collect();
    for u in &rc.units {
        if !had.contains(&(&*u.leader, u.fingerprint)) {
            rep.fail("COMDATs of original members changed or added", || {
                format!("{}: {}", name(), show(&u.leader))
            });
        }
    }
    let mut lost = Set::default();
    for u in &oc.units {
        if kept.contains(&(&*u.leader, u.fingerprint)) {
            continue;
        }
        lost.insert(&*u.leader);
        rep.checked("COMDATs moved out of original members", 1);
        if u.selection == turbonop_coff::comdat_select::NODUPLICATES {
            rep.fail("strong (NODUPLICATES) COMDATs moved", || {
                format!("{}: {}", name(), show(&u.leader))
            });
            continue;
        }
        let key = unit_key(&u.leader, u.fingerprint, oc.environment);
        match facts.holders.get(&key) {
            None => rep.fail(
                "moved COMDATs that no synthesized member holds (in their environment)",
                || format!("{}: {}", name(), show(&u.leader)),
            ),
            Some(holders) => {
                if !holders.iter().any(|h| synthesized.binary_search(h).is_ok()) {
                    rep.fail(
                        "moved COMDATs whose synthesized member the original member doesn't anchor",
                        || format!("{}: {}", name(), show(&u.leader)),
                    );
                }
            }
        }
    }
    // Definitions.
    let lost_units: Set<u32> = oc
        .units
        .iter()
        .enumerate()
        .filter(|(_, u)| lost.contains(&*u.leader))
        .map(|(k, _)| k as u32)
        .collect();
    let r_undefined: Set<&[u8]> = rc.undefined.iter().map(|n| &**n).collect();
    let o_undefined: Set<&[u8]> = oc.undefined.iter().map(|n| &**n).collect();
    let r_definitions: Map<&[u8], &member::Definition> =
        rc.definitions.iter().map(|d| (&*d.name, d)).collect();
    let o_definitions: Set<&[u8]> = oc.definitions.iter().map(|d| &*d.name).collect();
    let r_weak: Map<&[u8], &member::Weak> = rc.weak.iter().map(|w| (&*w.name, w)).collect();
    let o_weak: Map<&[u8], &member::Weak> = oc.weak.iter().map(|w| (&*w.name, w)).collect();
    let mut moved: Set<&[u8]> = Set::default();
    for d in &oc.definitions {
        if let Some(now) = r_definitions.get(&*d.name) {
            if now.kind != d.kind {
                rep.fail("definitions of original members changed kind", || {
                    format!(
                        "{}: {}: was {}, now {}",
                        name(),
                        show(&d.name),
                        d.kind.describe(),
                        now.kind.describe()
                    )
                });
            }
            continue;
        }
        moved.insert(&*d.name);
        if !d.kind.foldable() || !d.unit.is_some_and(|u| lost_units.contains(&u)) {
            rep.fail(
                "definitions that left an original member without their COMDAT moving",
                || format!("{}: {} ({})", name(), show(&d.name), d.kind.describe()),
            );
            continue;
        }
        // An alias first (whether or not the name also has a plain
        // undefined record, as the engine writes it), then undefined.
        if let Some(w) = r_weak.get(&*d.name)
            && w.search == turbonop_coff::weak_search::ALIAS
        {
            if *w.alternate != *ALIAS_DEFAULT {
                rep.fail(
                    "alias weak externals whose default isn't __turbolib$alias",
                    || format!("{}: {}", name(), show(&d.name)),
                );
            } else if !facts
                .synthesized_definitions
                .get(&*d.name)
                .is_some_and(|s| s.iter().any(|s| synthesized.binary_search(s).is_ok()))
            {
                rep.fail(
                    "aliases of moved symbols that no anchored synthesized member defines",
                    || format!("{}: {}", name(), show(&d.name)),
                );
            }
        } else if r_undefined.contains(&*d.name) {
            if !facts.defined.contains(&&*d.name) {
                rep.fail("moved symbols that no member defines", || {
                    format!("{}: {}", name(), show(&d.name))
                });
            }
        } else if let Some(w) = r_weak.get(&*d.name) {
            let renamed = [RENAMED, &d.name].concat();
            if w.search != turbonop_coff::weak_search::NOLIBRARY || *w.alternate != *renamed {
                rep.fail(
                    "weak externals that aren't NOLIBRARY with the renamed copy as default",
                    || format!("{}: {}", name(), show(&d.name)),
                );
            } else if !facts
                .synthesized_definitions
                .get(&*w.alternate)
                .is_some_and(|s| s.iter().any(|s| synthesized.binary_search(s).is_ok()))
            {
                rep.fail(
                    "weak externals whose renamed copy no anchored synthesized member defines",
                    || format!("{}: {}", name(), show(&w.alternate)),
                );
            }
        } else {
            rep.fail(
                "moved definitions that their member no longer names",
                || format!("{}: {}", name(), show(&d.name)),
            );
        }
    }
    for d in &rc.definitions {
        if !o_definitions.contains(&*d.name) {
            rep.fail("definitions added to original members", || {
                format!("{}: {}", name(), show(&d.name))
            });
        }
    }
    // References.
    for n in &oc.undefined {
        if !r_undefined.contains(&**n) {
            rep.fail("undefined references that original members lost", || {
                format!("{}: {}", name(), show(n))
            });
        }
    }
    // The defaults of moved symbols' weak externals (their renamed copies).
    let defaults: Set<&[u8]> = rc
        .weak
        .iter()
        .filter(|w| moved.contains(&*w.name))
        .map(|w| &*w.alternate)
        .collect();
    for n in &rc.undefined {
        if o_undefined.contains(&**n) || moved.contains(&**n) || defaults.contains(&**n) {
            continue;
        }
        if !facts.anchor_of.contains_key(&**n) {
            rep.fail(
                "undefined references added to original members (not moved symbols or anchors)",
                || format!("{}: {}", name(), show(n)),
            );
        }
    }
    for w in &oc.weak {
        if r_weak.get(&*w.name) != Some(&w) {
            rep.fail("weak externals of original members changed or lost", || {
                format!("{}: {}", name(), show(&w.name))
            });
        }
    }
    for w in &rc.weak {
        if o_weak.get(&*w.name) != Some(&w) && !moved.contains(&*w.name) {
            rep.fail("weak externals added to original members", || {
                format!("{}: {}", name(), show(&w.name))
            });
        }
    }
    // Side tables.
    rep.checked("side-table entries", oc.side.len() as u64);
    for e in &oc.side {
        if rc.side.binary_search(e).is_ok() {
            continue;
        }
        let elsewhere = synthesized.iter().any(|&s| {
            r.members[s]
                .body
                .coff()
                .is_some_and(|c| c.side.binary_search(e).is_ok())
        });
        if !elsewhere {
            rep.fail(
                "side-table entries neither in their member nor in a synthesized member it anchors",
                || format!("{}: {} {}", name(), show(&e.0), show(&e.1)),
            );
        }
    }
    if oc.debug {
        rep.unchecked(
            "debug sections (.debug$S, .debug$T) of original members (compare-images compares PDBs)",
            name,
        );
    }
}

fn check_synthesized(r: &Library<'_>, j: usize, facts: &Facts<'_>, rep: &mut Report) {
    let Some(c) = r.members[j].body.coff() else {
        return;
    };
    for u in &c.units {
        rep.checked("COMDATs in synthesized members", 1);
        if !facts
            .original_units
            .contains(&unit_key(&u.leader, u.fingerprint, c.environment))
        {
            rep.fail(
                "synthesized COMDATs that match no original copy (in their environment)",
                || format!("{}: {}", r.name(j), show(&u.leader)),
            );
        }
    }
    for n in c
        .undefined
        .iter()
        .map(|n| &**n)
        .chain(c.weak.iter().map(|w| &*w.name))
    {
        if !facts.original_names.contains(&n) && !facts.anchor_of.contains_key(n) {
            rep.fail(
                "symbols synthesized members refer to that no original member defines or refers to",
                || format!("{}: {}", r.name(j), show(n)),
            );
        }
    }
}

/// Checks `lib`'s index against its members' index names (`defs`). The
/// names of members `optional` says may go unlisted, so an entry may send
/// past them to the next definer, and a name only they have may be missing
/// (but not their anchors).
fn check_index(
    lib: &Library<'_>,
    defs: &Definitions<'_>,
    optional: &(dyn Fn(usize) -> bool + Sync),
    which: &str,
) -> Report {
    let mut rep = Report::new();
    let entries = lib.index.entries();
    rep.checked(&format!("{which} index entries"), entries.len() as u64);
    let no_definer = format!("{which} index entries no member defines");
    let other = format!("{which} index entries naming a member other than the first definer");
    let missing = format!("{which} definitions missing from the index");
    // Each entry, in parallel chunks (in order).
    let chunk = entries
        .len()
        .div_ceil(turbonop_par::threads() * 4)
        .max(1024);
    let found = turbonop_par::map_range(entries.len().div_ceil(chunk), 1, |c| {
        let mut f: Vec<(usize, bool, String)> = Vec::new();
        for &(name, offset) in &entries[c * chunk..entries.len().min((c + 1) * chunk)] {
            let Some(d) = defs.get(&name).filter(|d| !d.is_empty()) else {
                f.push((0, false, show(name)));
                continue;
            };
            let want = d[0].0;
            // The definers an entry may name: the first, and each after
            // optional ones only.
            let allowed = |at: usize| {
                d.iter()
                    .take_while(|&&(m, _)| m == at || optional(m))
                    .any(|&(m, _)| m == at)
            };
            match lib.by_offset.get(&offset) {
                Some(&at) if allowed(at) => {}
                at => f.push((
                    0,
                    true,
                    format!(
                        "{}: {} rather than {}",
                        show(name),
                        at.map_or_else(|| format!("offset {offset}"), |&a| lib.name(a)),
                        lib.name(want)
                    ),
                )),
            }
        }
        f
    });
    for (_, wrong_member, example) in found.into_iter().flatten() {
        rep.fail(if wrong_member { &other } else { &no_definer }, || example);
    }
    let indexed: Sharded<&[u8], ()> = Sharded::build(
        &[()],
        |_, (), out| out.extend(entries.iter().map(|&(n, _)| (n, ()))),
        |m, k, ()| {
            m.insert(k, ());
        },
    );
    let found = turbonop_par::map_balanced(defs.shards(), 1, |shard| {
        let mut f = Findings::new();
        for (name, d) in shard {
            let only_optional = d.iter().all(|&(m, _)| optional(m));
            if !indexed.contains(name) && !(only_optional && !name.starts_with(RENAMED)) {
                f.push((name.to_vec(), "", show(name)));
            }
        }
        f
    });
    let mut found: Findings = found.into_iter().flatten().collect();
    found.sort();
    for (_, _, example) in found {
        rep.fail(&missing, || example);
    }
    if let Some(second) = &lib.index.second {
        if !second.is_sorted_by(|x, y| x.0 <= y.0) {
            rep.fail(&format!("{which} second linker members not sorted"), || {
                "the second linker member".into()
            });
        }
        let mut a = lib.index.first.clone();
        let mut b = second.clone();
        turbonop_par::join(|| a.sort_unstable(), || b.sort_unstable());
        if a != b {
            rep.fail(&format!("{which} linker members that disagree"), || {
                format!("{} and {} entries", a.len(), b.len())
            });
        }
    }
    rep
}

/// Under `--pull=preserve`, every symbol of the original's index sends
/// to the same member in the rewritten's.
fn check_pull(o: &Library<'_>, r: &Library<'_>, p: &Pairing, options: &Options, rep: &mut Report) {
    let entries = r.index.entries();
    let now: Sharded<&[u8], u64> = Sharded::build(
        &[()],
        |_, (), out| out.extend(entries.iter().copied()),
        |m, k, offset| {
            m.entry(k).or_insert(offset);
        },
    );
    let entries = o.index.entries();
    let chunk = entries
        .len()
        .div_ceil(turbonop_par::threads() * 4)
        .max(1024);
    let found = turbonop_par::map_range(entries.len().div_ceil(chunk), 1, |c| {
        // (Whether the original definer has side effects, and the example.)
        let mut f: Vec<(bool, String)> = Vec::new();
        for &(name, offset) in &entries[c * chunk..entries.len().min((c + 1) * chunk)] {
            let Some(&i) = o.by_offset.get(&offset) else {
                continue;
            };
            let want = p.of_original[i];
            let got = now.get(&name).and_then(|off| r.by_offset.get(off)).copied();
            if got.is_none() || got != want {
                // A definer that the original had for a symbol now
                // defined by a synthesized member: allowed under
                // preserve when loading the definer does nothing.
                let to_synthesized = got.is_some_and(|g| p.of_rewritten[g].is_none());
                let hazard =
                    !to_synthesized || o.members[i].body.coff().is_none_or(|c| c.side_effects);
                f.push((
                    hazard,
                    format!(
                        "{}: was {}, now {}",
                        show(name),
                        o.name(i),
                        got.map_or_else(|| "none".into(), |g| r.name(g))
                    ),
                ));
            }
        }
        f
    });
    for (hazard, example) in found.into_iter().flatten() {
        match options.pull {
            Pull::Preserve if !hazard => rep.expect(
                "symbols the index sends to a synthesized member, from a definer without side effects (--pull=preserve allows it)",
                || example,
            ),
            Pull::Preserve => rep.fail(
                "symbols the index sends to another member (--pull=preserve)",
                || example,
            ),
            Pull::Relaxed => rep.expect(
                "symbols the index sends to another member (--pull=relaxed allows it)",
                || example,
            ),
        }
    }
}

#[cfg(test)]
mod tests;
