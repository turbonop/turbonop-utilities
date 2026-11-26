//! Rewriting a library's members: each group of identical COMDATs is kept
//! once, in a synthesized member, and the members that held copies refer
//! to it.
//!
//! The steps, as `docs/turbolib.md` ("Rewriting a library") describes
//! them:
//!
//! 1. **Read** every member in parallel into the object model, and find its
//!    fold units ([`units`]). Import objects, `/GL` objects, and members
//!    with sections turbolib doesn't know are passed through.
//! 2. **Group** units by identity, and decide for each group whether it
//!    folds: at least two copies foldable, and under [`Pull::Preserve`]
//!    every definition of its names in the library a copy with the same
//!    code, and the member the archive index names for them without side
//!    effects.
//! 3. **Lay out** the synthesized members: one for each set of members that
//!    gave up copies (and environment), so that a synthesized member is
//!    wanted exactly when one of those members is.
//! 4. **Rewrite** each member that gave up copies: their sections go, the
//!    symbols they defined become alias weak externals (see
//!    [`ALIAS_DEFAULT`]), so the archive index names the same members for
//!    them, and the member refers to each synthesized member's anchor.
//! 5. **Write** each synthesized member: the surviving copies, their
//!    outside references as undefined externals, an anchor (which alone the
//!    index lists it for, under `Preserve`), `@feat.00`, and the
//!    environment's `/FAILIFMISMATCH` directives. Synthesized members come
//!    last in the archive under `Preserve`, first under `Relaxed`.
//!
//! Every step's result is independent of the number of threads: groups are
//! decided in member order, and the survivor is the first copy.
//!
//! Side tables (the `/guard` tables, `.sxdata`, FPO, `.voltbl`, and
//! `.retplne`) are split as [`tables`] describes: entries go with their
//! code.
//!
//! Each synthesized member carries its units' CodeView, renumbered into
//! types, checksums, and strings of its own ([`debug`]).
//!
//! Under [`Precomp::Library`], the records members share go into one
//! precompiled-types member, which every member holding some of them (and
//! every synthesized member) names ([`precomp`]).

pub mod debug;
pub mod precomp;
pub mod tables;
pub mod units;

use crate::hash::H128;
use std::borrow::Cow;
use std::collections::BTreeMap;
use turbonop_coff::{
    Aux, File, Format, Name, Object, Relocation, Renumbering, Section, SectionData,
    SectionDefinition, SectionId, SectionNumber, Skeleton, Symbol, SymbolId, WeakExternal,
    comdat_select, storage_class, weak_search,
};
use turbonop_hash::Map as HashMap;
use units::{Facts, is_debug};

/// One input member: its name in the archive and its bytes.
#[derive(Clone, Copy, Debug)]
pub struct Input<'a> {
    pub name: &'a [u8],
    pub data: &'a [u8],
}

/// Whether which members a link loads may change (`--pull`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Pull {
    /// A link gets the same code for each name as before, and no member with
    /// side effects loads or stops loading: a group is folded only if every
    /// definition of its symbols in the library is a unit with the same code
    /// (environments may differ), and the member the index named for them
    /// (the synthesized members, first in the archive, are named now) has no
    /// side effects.
    Preserve,
    /// Fold regardless.
    Relaxed,
}

/// Shared types (`--precomp`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Precomp {
    /// Each member keeps its own types.
    None,
    /// One precompiled-types member for the library, holding the records
    /// at least `min` members hold.
    Library { min: u32 },
}

/// Options for [`rewrite`].
#[derive(Clone, Copy, Debug)]
pub struct Options {
    pub pull: Pull,
    /// Under [`Pull::Preserve`], fold groups whose first copy's member has
    /// side effects with the weak-external technique. Off: it isn't safe in
    /// link.exe (see [`rewrite`]'s notes in the code).
    pub weak_externals: bool,
    /// Fold identical COMDATs (`--fold=identical`); off, only types are
    /// shared (if `precomp` says so).
    pub fold: bool,
    pub precomp: Precomp,
}

/// Why a unit (or a whole member) isn't folded.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Unfoldable {
    /// An import object, passed through.
    Import,
    /// A `/GL` object, passed through.
    Ltcg,
    /// Another anonymous object, passed through.
    Anonymous,
    /// Not an object turbolib can read, passed through.
    Unreadable,
    /// The member has a section of a kind turbolib doesn't know.
    UnknownSection,
    /// The member has a side table turbolib can't read.
    SideTables,
    /// The unit has COFF line numbers.
    LineNumbers,
    /// A relocation names no symbol.
    BadRelocation,
    /// The unit refers to a weak external.
    WeakExternalReference,
    /// The unit refers to a local symbol outside it (`--locals=keep`).
    LocalReference,
    /// The COMDAT's own symbol is local: internal linkage, never shared
    /// between objects by the linker (`--locals=keep`).
    InternalLinkage,
    /// Something outside the unit refers to one of its local symbols.
    ReferencedLocally,
    /// An auxiliary record kept as bytes.
    RawAuxiliary,
    /// Only one copy.
    Single,
    /// Another copy can't be folded.
    CopyUnfoldable,
    /// A symbol it defines is defined elsewhere in the library with other
    /// code, or not by a unit (`--pull=preserve`).
    OtherDefinitions,
    /// Its first copy's member has side effects and keeps it, and there
    /// are too few other copies to save anything (`--pull=preserve`).
    SideEffectsTooFewCopies,
    /// Its first copy's member has side effects, so folding it could
    /// change which members a link loads (`--pull=preserve`).
    SideEffects,
    /// Its debug information can't be carried into a synthesized member
    /// (see [`debug`]).
    DebugInfo,
    /// Its object uses a PCH, but names neither the PCH object's symbol
    /// (so a synthesized member can't name the PCH) nor is the PCH object
    /// in the library (so its types can't be copied).
    PrecompMissing,
}

impl Unfoldable {
    /// A short description, for reports.
    pub fn describe(self) -> &'static str {
        match self {
            Unfoldable::Import => "import object",
            Unfoldable::Ltcg => "/GL object",
            Unfoldable::Anonymous => "anonymous object",
            Unfoldable::Unreadable => "not a readable object",
            Unfoldable::UnknownSection => "a section of an unknown kind",
            Unfoldable::SideTables => "a side table turbolib can't read",
            Unfoldable::LineNumbers => "COFF line numbers",
            Unfoldable::BadRelocation => "a relocation naming no symbol",
            Unfoldable::WeakExternalReference => "refers to a weak external",
            Unfoldable::LocalReference => "refers to a local symbol outside it",
            Unfoldable::InternalLinkage => "internal linkage (a local COMDAT)",
            Unfoldable::ReferencedLocally => "a local symbol of it is referred to from outside",
            Unfoldable::RawAuxiliary => "an auxiliary record turbolib can't read",
            Unfoldable::Single => "only one copy",
            Unfoldable::CopyUnfoldable => "another copy can't be folded",
            Unfoldable::OtherDefinitions => {
                "its symbols are defined elsewhere with other code (--pull=preserve)"
            }
            Unfoldable::SideEffects => "its first definer has side effects (--pull=preserve)",
            Unfoldable::SideEffectsTooFewCopies => {
                "kept by a member with side effects, too few other copies"
            }
            Unfoldable::DebugInfo => "debug information turbolib can't carry",
            Unfoldable::PrecompMissing => "its PCH is neither named nor in the library",
        }
    }
}

/// Section data at least this large isn't copied into a rewritten or
/// synthesized member's bytes: it is left as a hole, filled from the input
/// when the library is written.
const HOLE: usize = 4096;

/// One output member.
#[derive(Clone, Debug)]
pub struct OutputMember<'a> {
    pub name: Cow<'a, [u8]>,
    /// Its bytes, with zeros in the holes.
    pub data: Cow<'a, [u8]>,
    /// The pieces of the member that `data` leaves out (see
    /// [`turbonop_coff::Skeleton`]): each offset in it, and the bytes to put
    /// there. Everything but section contents is in `data`, so it can be
    /// read for its headers and symbols as it is.
    pub holes: Vec<(usize, &'a [u8])>,
    /// Made by turbolib.
    pub synthesized: bool,
    /// The names the archive index lists for this member, if not the ones
    /// it defines: under [`Pull::Preserve`], a synthesized member is listed
    /// for its anchor alone, so that each folded name stays listed for the
    /// member it was listed for (see [`Pull::Preserve`]).
    pub index_only: Option<Vec<Vec<u8>>>,
}

/// What a rewrite did.
#[derive(Clone, Debug, Default)]
pub struct Report {
    /// Members read, and passed through whole (with why).
    pub members: u64,
    pub passed_through: BTreeMap<Unfoldable, u64>,
    /// Fold units found, and those that didn't fold (with why).
    pub units: u64,
    pub not_folded: BTreeMap<Unfoldable, u64>,
    /// The bytes a group's copies but one would save, for the groups not
    /// folded (with why): what each reason costs.
    pub not_folded_bytes: BTreeMap<Unfoldable, u64>,
    /// The bytes the removed copies held (their sections, as stored).
    pub removed_bytes: u64,
    /// Groups of copies folded, and copies removed from members.
    pub groups: u64,
    pub copies_removed: u64,
    /// Groups whose first copy stayed in its member for its side effects.
    pub kept_for_side_effects: u64,
    /// Members rewritten, and members synthesized.
    pub rewritten: u64,
    pub synthesized: u64,
    /// Bytes of all members, before and after.
    pub bytes_in: u64,
    pub bytes_out: u64,
    /// How long each step took: read and find units, definers, group and
    /// decide, rewrite members, synthesize members.
    pub times: [std::time::Duration; 5],
    /// Shared types (`--precomp=library`).
    pub types: TypesReport,
}

/// What sharing types did.
#[derive(Clone, Debug, Default)]
pub struct TypesReport {
    /// Precompiled-types members made, the records they hold, and their
    /// bytes (their `.debug$P`).
    pub pchs: u64,
    pub shared_records: u64,
    pub shared_bytes: u64,
    /// Members whose types could be shared; of those, the members that
    /// share (rewritten), that hold no shared record, and whose debug
    /// information couldn't be read through (with the first reason).
    pub candidates: u64,
    pub sharing: u64,
    pub nothing_shared: u64,
    pub unreadable: u64,
    pub first_unreadable: Option<String>,
    /// PCH users whose PCH object is in the library, absorbed.
    pub absorbed: u64,
    /// The sharing members' `.debug$T` bytes, before and after.
    pub types_before: u64,
    pub types_after: u64,
    /// How long finding the shared types took.
    pub time: std::time::Duration,
}

/// How a member gives up one copy.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Removal {
    /// Its symbols become undefined externals.
    Undefined,
    /// Its symbols become weak externals (no library search) whose default
    /// is the renamed copy.
    Weak,
}

/// The name a renamed copy's symbol has: no compiler makes names with `$`
/// after `__turbolib`.
pub fn renamed(name: &[u8]) -> Vec<u8> {
    let mut v = b"__turbolib$".to_vec();
    v.extend_from_slice(name);
    v
}

struct Parsed<'a> {
    object: Option<Object<'a>>,
    facts: Facts,
    passed: Option<Unfoldable>,
    debug: debug::Facts,
}

fn parse<'a>(input: &Input<'a>) -> Parsed<'a> {
    let pass = |why| Parsed {
        object: None,
        facts: Facts::default(),
        passed: Some(why),
        debug: debug::Facts::default(),
    };
    match File::parse(input.data) {
        Ok(File::Object(object)) => match units::find(&object) {
            Ok(mut facts) => {
                let debug = debug::read(&object, &mut facts);
                Parsed {
                    object: Some(object),
                    facts,
                    passed: None,
                    debug,
                }
            }
            Err(why) => Parsed {
                // Kept for its definitions, which still count.
                object: Some(object),
                facts: Facts::default(),
                passed: Some(why),
                debug: debug::Facts::default(),
            },
        },
        Ok(File::Opaque { kind, .. }) => pass(match kind {
            turbonop_coff::Kind::Import => Unfoldable::Import,
            turbonop_coff::Kind::Ltcg => Unfoldable::Ltcg,
            _ => Unfoldable::Anonymous,
        }),
        Err(_) => pass(Unfoldable::Unreadable),
    }
}

/// The external symbols a member defines, as the archive index would list
/// them (for counting definers). Import objects define their symbol and
/// `__imp_` it; `/GL` objects aren't read.
fn definitions<'a>(input: &Input<'a>, parsed: &'a Parsed<'a>) -> Vec<Cow<'a, [u8]>> {
    if let Some(o) = &parsed.object {
        return o
            .symbols
            .iter()
            .filter(|s| s.is_external() && s.section.id().is_some())
            .map(|s| Cow::Borrowed(s.name.as_bytes()))
            .collect();
    }
    if parsed.passed == Some(Unfoldable::Import)
        && let Ok(i) = turbonop_pe::import_object::ImportObject::parse(input.data)
    {
        let mut imp = b"__imp_".to_vec();
        imp.extend_from_slice(i.symbol);
        return vec![Cow::Borrowed(i.symbol), Cow::Owned(imp)];
    }
    Vec::new()
}

/// The member the archive index names for each name (its first definer),
/// and each name's code, if every definition of it in the library is a
/// unit with that code (whatever its environment); `None` if any differs,
/// or isn't a unit's.
type Definers<'p> = (HashMap<&'p [u8], usize>, HashMap<&'p [u8], Option<u128>>);

fn definers<'p>(parsed: &'p [Parsed<'_>], defs: &'p [Vec<Cow<'_, [u8]>>]) -> Definers<'p> {
    let mut first_definer: HashMap<&[u8], usize> = HashMap::default();
    for (m, d) in defs.iter().enumerate() {
        for name in d {
            first_definer.entry(name.as_ref()).or_insert(m);
        }
    }
    let mut one_code: HashMap<&[u8], Option<u128>> = HashMap::default();
    for (p, d) in parsed.iter().zip(defs) {
        if p.passed.is_some() {
            for name in d {
                one_code.insert(name.as_ref(), None);
            }
            continue;
        }
        for unit in &p.facts.units {
            for n in &unit.externals {
                let e = one_code.entry(n.as_slice()).or_insert(Some(unit.code));
                if *e != Some(unit.code) {
                    *e = None;
                }
            }
        }
        for n in &p.facts.other_definitions {
            one_code.insert(n.as_slice(), None);
        }
    }
    (first_definer, one_code)
}

/// A synthesized member's contents: the surviving copies, and whether each
/// is a renamed copy.
struct Synth {
    environment: u128,
    /// The members that gave copies, in order.
    givers: Vec<usize>,
    /// (member, unit, renamed), in member and unit order.
    units: Vec<(usize, usize, bool)>,
}

/// Rewrites `inputs` (a library's members, in archive order). The output is
/// the synthesized members, then the members in the same order, rewritten
/// where they gave up copies.
pub fn rewrite<'a>(inputs: &[Input<'a>], options: Options) -> Output<'a> {
    let mut clock = std::time::Instant::now();
    let mut times = [std::time::Duration::ZERO; 5];
    let mut lap = |k: usize| {
        let now = std::time::Instant::now();
        times[k] = now - clock;
        clock = now;
    };
    let mut parsed: Vec<Parsed<'a>> = turbonop_par::map_balanced(inputs, 4, parse);
    // PCH users' PCH objects (their units' types are there).
    let library = matches!(options.precomp, Precomp::Library { .. });
    debug::resolve(&mut parsed, library);
    let mut report = Report {
        members: inputs.len() as u64,
        bytes_in: inputs.iter().map(|i| i.data.len() as u64).sum(),
        ..Default::default()
    };
    for p in &parsed {
        if let Some(why) = p.passed {
            *report.passed_through.entry(why).or_default() += 1;
        }
        report.units += p.facts.units.len() as u64;
    }

    lap(0);
    let defs: Vec<Vec<Cow<'_, [u8]>>> = inputs
        .iter()
        .zip(&parsed)
        .map(|(i, p)| definitions(i, p))
        .collect();
    // The shared types, while the definers are found (on one thread).
    let types_clock = std::time::Instant::now();
    let (plan, (first_definer, one_code)) = turbonop_par::join(
        || match options.precomp {
            Precomp::Library { min } => {
                Some((precomp::plan(&parsed, min.max(1)), types_clock.elapsed()))
            }
            Precomp::None => None,
        },
        || definers(&parsed, &defs),
    );
    let plan = plan.map(|(plan, time)| {
        let t = &mut report.types;
        t.pchs = plan.pchs.len() as u64;
        t.shared_records = plan.pchs.iter().map(|p| u64::from(p.count)).sum();
        t.shared_bytes = plan.pchs.iter().map(|p| p.records.len() as u64).sum();
        t.candidates = plan
            .members
            .iter()
            .filter(|m| m.role == precomp::Role::Sharer)
            .count() as u64
            + plan.unreadable;
        t.unreadable = plan.unreadable;
        t.first_unreadable = plan.first_unreadable.clone();
        t.absorbed = parsed.iter().filter(|p| p.debug.absorbed).count() as u64;
        t.time = time;
        plan
    });

    lap(1);
    // Groups of copies, in order of first appearance.
    let mut groups: HashMap<u128, usize> = HashMap::default();
    let mut copies: Vec<Vec<(usize, usize)>> = Vec::new();
    for (m, p) in parsed.iter().enumerate().filter(|_| options.fold) {
        for (u, unit) in p.facts.units.iter().enumerate() {
            let g = *groups.entry(unit.identity).or_insert_with(|| {
                copies.push(Vec::new());
                copies.len() - 1
            });
            copies[g].push((m, u));
        }
    }

    let mut removals: Vec<Vec<(usize, Removal)>> = vec![Vec::new(); parsed.len()];
    let mut synth_of: HashMap<(u128, u64, Vec<usize>), usize> = HashMap::default();
    let mut synths: Vec<Synth> = Vec::new();
    for c in &copies {
        let unit = |&(m, u): &(usize, usize)| &parsed[m].facts.units[u];
        // The copies that can move; the first of them survives. A copy that
        // can't stays where it is.
        let movable: Vec<(usize, usize)> = c
            .iter()
            .copied()
            .filter(|x| unit(x).unfoldable.is_none())
            .collect();
        for x in c.iter().filter(|x| unit(x).unfoldable.is_some()) {
            let w = unit(x).unfoldable.unwrap_or(Unfoldable::CopyUnfoldable);
            *report.not_folded.entry(w).or_default() += 1;
        }
        if movable.len() < 2 {
            if movable.len() == 1 {
                let w = if c.len() < 2 {
                    Unfoldable::Single
                } else {
                    Unfoldable::CopyUnfoldable
                };
                *report.not_folded.entry(w).or_default() += 1;
            }
            continue;
        }
        let first = &movable[0];
        let mut why = None;
        // Under Preserve, a link must get the same code for each name, from
        // the same members. Which copy a link keeps depends on which members
        // it loads and in what order, so a name folds only if every
        // definition of it in the library is a unit with the same code;
        // then any copy is the same. And the member the index named for it
        // (now the synthesized member, first in the archive) must have had
        // no side effects, so that no longer loading it changes nothing.
        if options.pull == Pull::Preserve
            && unit(first)
                .externals
                .iter()
                .any(|n| one_code.get(n.as_slice()).copied().flatten() != Some(unit(first).code))
        {
            why = Some(Unfoldable::OtherDefinitions);
        }
        let c = &movable;
        // Under Preserve, a group whose first copy's member has side effects
        // isn't folded: the weak-external technique (that member keeps its
        // copy, the others get NOLIBRARY weak externals whose default is a
        // renamed copy) fails in link.exe 14.52 when a strong reference to
        // the name reaches the link from a library member after the weak
        // externals (measured; see docs/turbolib.md). `weak_externals`
        // turns it on, for study.
        let side_effects = options.pull == Pull::Preserve
            && unit(first).externals.iter().any(|n| {
                first_definer
                    .get(n.as_slice())
                    .is_some_and(|&m| parsed[m].facts.side_effects)
            });
        let keep_first = side_effects && options.weak_externals;
        if why.is_none() && side_effects && !options.weak_externals {
            why = Some(Unfoldable::SideEffects);
        }
        if why.is_none() && keep_first && c.len() < 3 {
            why = Some(Unfoldable::SideEffectsTooFewCopies);
        }
        if let Some(w) = why {
            *report.not_folded.entry(w).or_default() += c.len().max(1) as u64;
            if c.len() > 1 {
                *report.not_folded_bytes.entry(w).or_default() +=
                    unit(first).size * (c.len() as u64 - 1);
            }
            continue;
        }
        report.groups += 1;
        let removed: &[(usize, usize)] = if keep_first {
            report.kept_for_side_effects += 1;
            &c[1..]
        } else {
            c
        };
        let how = if keep_first {
            Removal::Weak
        } else {
            Removal::Undefined
        };
        for &(m, u) in removed {
            removals[m].push((u, how));
        }

        report.copies_removed += removed.len() as u64;
        report.removed_bytes += removed.iter().map(|x| unit(x).size).sum::<u64>();
        let mut givers: Vec<usize> = removed.iter().map(|&(m, _)| m).collect();
        givers.dedup();
        // (And the PCH its CodeView names, if any: see debug::key.)
        let key = (
            parsed[first.0].facts.environment,
            debug::key(&parsed[first.0]),
            givers.clone(),
        );
        let s = *synth_of.entry(key).or_insert_with(|| {
            synths.push(Synth {
                environment: parsed[first.0].facts.environment,
                givers,
                units: Vec::new(),
            });
            synths.len() - 1
        });
        synths[s].units.push((first.0, first.1, keep_first));
    }
    for s in &mut synths {
        s.units.sort_unstable();
    }

    // Names and anchors.
    let anchors: Vec<Vec<u8>> = synths
        .iter()
        .enumerate()
        .map(|(n, s)| anchor_name(n, s, inputs, &parsed))
        .collect();
    let mut anchors_of: Vec<Vec<usize>> = vec![Vec::new(); parsed.len()];
    for (n, s) in synths.iter().enumerate() {
        for &m in &s.givers {
            anchors_of[m].push(n);
        }
    }

    lap(2);
    // Rewritten members, then synthesized members, each in parallel.
    // Largest first, so the threads finish together.
    let mut order: Vec<usize> = (0..parsed.len()).collect();
    order.sort_by_key(|&m| std::cmp::Reverse(inputs[m].data.len()));
    let mut members: Vec<Rewritten<'a>> = turbonop_par::map_balanced(&order, 1, |&m| {
        let input = &inputs[m];
        // Its shared types, if any (not of the sections it gives up).
        let edit = plan.as_ref().and_then(|plan| {
            let mut removed =
                vec![false; parsed[m].object.as_ref().map_or(0, |o| o.sections.len())];
            for &(u, _) in &removals[m] {
                for s in &parsed[m].facts.units[u].sections {
                    removed[s.index()] = true;
                }
            }
            plan.edit(&parsed, m, &removed)
        });
        let sizes = edit.as_ref().map(|e| (e.before, e.after));
        if removals[m].is_empty() && edit.is_none() {
            return (
                m,
                OutputMember {
                    name: Cow::Borrowed(input.name),
                    data: Cow::Borrowed(input.data),
                    holes: Vec::new(),
                    synthesized: false,
                    index_only: None,
                },
                None,
            );
        }
        let refers: Vec<&[u8]> = anchors_of[m]
            .iter()
            .map(|&n| anchors[n].as_slice())
            .collect();
        let skeleton = strip(&parsed[m], &removals[m], &refers, edit.as_ref());
        (
            m,
            OutputMember {
                name: Cow::Borrowed(input.name),
                data: Cow::Owned(skeleton.bytes),
                holes: skeleton.holes,
                synthesized: false,
                index_only: None,
            },
            sizes,
        )
    });
    members.sort_unstable_by_key(|(m, _, _)| *m);
    let mut rewritten = 0;
    for (m, _, sizes) in &members {
        if let Some((before, after)) = sizes {
            report.types.sharing += 1;
            report.types.types_before += before;
            report.types.types_after += after;
        }
        rewritten += u64::from(!removals[*m].is_empty() || sizes.is_some());
    }
    report.types.nothing_shared =
        report.types.candidates - report.types.unreadable - report.types.sharing;
    let members: Vec<OutputMember<'a>> = members.into_iter().map(|(_, o, _)| o).collect();
    lap(3);
    let made: Vec<OutputMember<'a>> = turbonop_par::map_range(synths.len(), 1, |n| {
        let name = format!("turbolib{n}.obj").into_bytes();
        let skeleton = synthesize(&synths[n], &parsed, plan.as_ref(), &anchors[n], &name);
        OutputMember {
            name: Cow::Owned(name),
            data: Cow::Owned(skeleton.bytes),
            holes: skeleton.holes,
            synthesized: true,
            index_only: (options.pull == Pull::Preserve).then(|| vec![anchors[n].clone()]),
        }
    });
    // The precompiled-types members, after them.
    let pchs: Vec<OutputMember<'a>> = plan
        .as_ref()
        .map(|plan| {
            plan.pchs
                .iter()
                .map(|pch| (pch, precomp::Plan::member(pch).bytes))
                // One the library already has (compacted again): it stays
                // where it is.
                .filter(|(pch, data)| {
                    !inputs
                        .iter()
                        .any(|i| i.name == pch.name.as_slice() && i.data == data.as_slice())
                })
                .map(|(pch, data)| OutputMember {
                    name: Cow::Owned(pch.name.clone()),
                    data: Cow::Owned(data),
                    holes: Vec::new(),
                    synthesized: true,
                    index_only: Some(vec![pch.symbol.clone()]),
                })
                .collect()
        })
        .unwrap_or_default();
    lap(4);
    report.times = times;
    report.rewritten = rewritten;
    report.synthesized = (made.len() + pchs.len()) as u64;
    // Under Preserve, synthesized members (and precompiled-types members)
    // last: they are indexed for their anchors (and PCH symbols) alone, so
    // nothing needs them first, and the original members keep their places
    // at the front of the archive. (It didn't change the order in which
    // link.exe loads the original members, measured; see TODO.md.) Under
    // Relaxed, first, so that the index names them for what they define.
    let out = if options.pull == Pull::Preserve {
        let mut out = members;
        out.extend(made);
        out.extend(pchs);
        out
    } else {
        let mut out = made;
        out.extend(pchs);
        out.extend(members);
        out
    };
    report.bytes_out = out.iter().map(|m| m.data.len() as u64).sum();
    Output {
        members: out,
        report,
    }
}

/// A member as rewritten: its position, the member, and its `.debug$T`'s
/// bytes before and after sharing types (if it shares).
type Rewritten<'a> = (usize, OutputMember<'a>, Option<(u64, u64)>);

/// The output of [`rewrite`].
#[derive(Clone, Debug)]
pub struct Output<'a> {
    pub members: Vec<OutputMember<'a>>,
    pub report: Report,
}

/// A synthesized member's anchor: a name no other library's anchor has,
/// from the members that refer to it and what it holds.
fn anchor_name(n: usize, s: &Synth, inputs: &[Input<'_>], parsed: &[Parsed<'_>]) -> Vec<u8> {
    let mut h = H128::new();
    h.u64(n as u64);
    h.u128(s.environment);
    for &m in &s.givers {
        h.u64(inputs[m].name.len() as u64);
        h.bytes(inputs[m].name);
    }
    for &(m, u, _) in &s.units {
        h.u128(parsed[m].facts.units[u].identity);
    }
    format!("__turbolib$anchor${:032x}", h.finish()).into_bytes()
}

/// The default of the alias weak externals that keep each moved copy's names
/// defined in the member that gave it up.
///
/// As an alias, the name counts as defined: the archive index still names
/// the members it named (`lib.exe` indexes aliases), and a member that gave
/// up a copy never sends the linker to the library for it. Were the name
/// undefined there, a link could resolve it through the index, loading the
/// first definer, before the member's anchor brought the copy, and so load
/// members the original library's link didn't (measured with link.exe).
///
/// The default itself never matters: each such member refers to the anchor
/// of the synthesized member holding the copy, whose strong definition
/// overrides the alias. But it must be the same everywhere, since link.exe
/// refuses two weak externals for one name with different defaults
/// (LNK1227), and both linkers require it resolved: each synthesized member
/// defines it, as the absolute 0.
pub const ALIAS_DEFAULT: &[u8] = b"__turbolib$alias";

/// A member with the copies it gives up removed.
fn strip<'a>(
    parsed: &Parsed<'a>,
    removals: &[(usize, Removal)],
    anchors: &[&[u8]],
    edit: Option<&precomp::Edit>,
) -> Skeleton<'a> {
    let mut object = parsed
        .object
        .clone()
        .expect("a member that gives up copies is an object");
    let facts = &parsed.facts;
    let mut removed = vec![false; object.sections.len()];
    // The symbols that become weak externals, and their defaults' names
    // and search: made after the edit, since an auxiliary record moves the
    // symbol table indices the side tables hold.
    let mut weak: Vec<(SymbolId, Vec<u8>, u32)> = Vec::new();
    for &(u, how) in removals {
        let unit = &facts.units[u];
        for s in &unit.sections {
            removed[s.index()] = true;
        }
        for &id in &unit.symbols {
            if !object.symbols[id.index()].is_external() {
                continue;
            }
            let name = object.symbols[id.index()].name.as_bytes();
            // A moved copy's name stays defined here, as an alias (see
            // `ALIAS_DEFAULT`).
            if how == Removal::Weak {
                weak.push((id, renamed(name), weak_search::NOLIBRARY));
            } else {
                weak.push((id, ALIAS_DEFAULT.to_vec(), weak_search::ALIAS));
            }
            let s = &mut object.symbols[id.index()];
            s.section = SectionNumber::Undefined;
            s.value = 0;
            s.aux = Aux::None;
        }
    }
    // With its types shared, its precomputed type hashes (lld's .debug$H)
    // would be stale. Its section checksums (.chks64) are kept for the
    // sections that stay as they were (see `checksums`), or, in a form
    // not known, dropped.
    let sections = object.sections.len();
    for (i, s) in object.sections.iter().enumerate() {
        let name = s.name.as_bytes();
        if (name == b".chks64" && s.size() as usize != 8 * sections)
            || (edit.is_some() && name == b".debug$H")
        {
            removed[i] = true;
        }
    }
    let original = parsed.object.as_ref().expect("an object");
    let fields = tables::strip(original, &facts.tables, &removed, &mut object);
    let renumbering = object
        .remove_sections(|id, _| removed[id.index()])
        .expect("a foldable unit is referred to only by external symbols");
    let before = object.symbol_table_indices();
    let mut defaults: HashMap<Vec<u8>, SymbolId> = HashMap::default();
    // (Alias record, the plain record the relocations now name.)
    let mut retarget: Vec<(SymbolId, SymbolId)> = Vec::new();
    for (id, default, search) in weak {
        let id = renumbering
            .symbol(id)
            .expect("a unit's external symbols stay, undefined");
        let kind = if search == weak_search::ALIAS {
            0
        } else {
            object.symbols[id.index()].kind
        };
        let tag = *defaults.entry(default.clone()).or_insert_with(|| {
            object.push_symbol(Symbol::new(
                Name::new(default),
                0,
                SectionNumber::Undefined,
                kind,
                storage_class::EXTERNAL,
            ))
        });
        if search == weak_search::ALIAS {
            // Two records of the name: the alias first, then a plain
            // undefined external, which the relocations name. link.exe's ICF
            // takes a reference through a weak external for another target
            // than one to the name, so with the alias alone, code calling a
            // moved function stopped folding with identical code elsewhere
            // (measured: 4,870 more functions in llvm-nm under /OPT:REF).
            // And lld-link reads the records in order: a plain undefined
            // name first loads the member the index names for it (95 more
            // in llvm-nm), while after the alias it loads nothing.
            let s = &object.symbols[id.index()];
            let plain = Symbol::new(
                s.name.clone(),
                0,
                SectionNumber::Undefined,
                s.kind,
                storage_class::EXTERNAL,
            );
            retarget.push((id, object.push_symbol(plain)));
        }
        let s = &mut object.symbols[id.index()];
        s.storage_class = storage_class::WEAK_EXTERNAL;
        s.aux = Aux::WeakExternal(WeakExternal { tag, search });
    }
    if !retarget.is_empty() {
        let mut to: Vec<Option<SymbolId>> = vec![None; object.symbols.len()];
        for &(from, plain) in &retarget {
            to[from.index()] = Some(plain);
        }
        for sec in &mut object.sections {
            for r in &mut sec.relocations {
                if let Some(p) = to.get(r.symbol.index()).copied().flatten() {
                    r.symbol = p;
                }
            }
        }
    }
    let now = object.symbol_table_indices();
    tables::renumber(&mut object, &fields, &renumbering, &|i| {
        let id = before
            .binary_search(&i)
            .expect("a table's entry names a symbol's primary record");
        now[id]
    });
    for a in anchors {
        if !defaults.contains_key(*a) {
            object.push_symbol(Symbol::new(
                Name::new(a.to_vec()),
                0,
                SectionNumber::Undefined,
                0,
                storage_class::EXTERNAL,
            ));
        }
    }
    if let Some(edit) = edit {
        // Its types and symbols, renumbered into the shared types, and a
        // reference to the precompiled-types member, as a /Yu object's.
        let mut changed = vec![None; object.sections.len()];
        for (id, data) in &edit.sections {
            let id = renumbering
                .section(*id)
                .expect("a shared member's debug sections stay");
            object.sections[id.index()].data = SectionData::Bytes(Cow::Owned(data.clone()));
            changed[id.index()] = Some(object.sections[id.index()].size());
        }
        for s in &mut object.symbols {
            if let (SectionNumber::Section(id), Aux::Section(d)) = (s.section, &mut s.aux)
                && let Some(Some(size)) = changed.get(id.index())
            {
                d.length = *size;
            }
        }
        object.forget_layout();
        object.push_symbol(Symbol::new(
            Name::new(edit.symbol.clone()),
            0,
            SectionNumber::Undefined,
            0,
            storage_class::EXTERNAL,
        ));
    }
    checksums(original, &mut object, &renumbering);
    object
        .write_skeleton(HOLE)
        .expect("a rewritten member fits its format")
}

/// Rewrites a changed member's `.chks64` (one 8-byte checksum for each
/// section, in section order, as cl writes it): the checksums of the
/// sections that stay as they were (the same bytes as in `original`), and
/// 0 (as cl gives `.bss`) for those that changed. link.exe takes the
/// contributions' CRCs in its PDB from them (likely: without them, every
/// contribution of a changed member had none), and neither linker checks
/// them in a full link (phase 0).
fn checksums(original: &Object<'_>, object: &mut Object<'_>, renumbering: &Renumbering) {
    let Some(c) = object
        .sections
        .iter()
        .position(|s| s.name.as_bytes() == b".chks64")
    else {
        return;
    };
    let Some(old_c) = original
        .sections
        .iter()
        .position(|s| s.name.as_bytes() == b".chks64")
    else {
        return;
    };
    let old = original.sections[old_c].bytes();
    let mut of_new = vec![None; object.sections.len()];
    for i in 0..original.sections.len() {
        if let Some(n) = renumbering.section(SectionId(i as u32)) {
            of_new[n.index()] = Some(i);
        }
    }
    let mut data = Vec::with_capacity(8 * object.sections.len());
    for (j, s) in object.sections.iter().enumerate() {
        let entry = match of_new[j] {
            Some(i) if j != c && std::ptr::eq(s.bytes(), original.sections[i].bytes()) => {
                &old[8 * i..8 * i + 8]
            }
            _ => &[0; 8][..],
        };
        data.extend_from_slice(entry);
    }
    let size = data.len() as u32;
    object.sections[c].data = SectionData::Bytes(Cow::Owned(data));
    for s in &mut object.symbols {
        if s.section == SectionNumber::Section(SectionId(c as u32))
            && let Aux::Section(d) = &mut s.aux
        {
            d.length = size;
        }
    }
    object.forget_layout();
}

const IMAGE_SCN_LNK_INFO: u32 = 0x0000_0200;
const IMAGE_SCN_LNK_REMOVE: u32 = 0x0000_0800;
const IMAGE_SCN_ALIGN_1BYTES: u32 = 0x0010_0000;
/// The most sections a regular object can have; past it, big-obj.
const MAX_REGULAR_SECTIONS: usize = 65_279;

/// A synthesized member, named `name`.
fn synthesize<'a>(
    s: &Synth,
    parsed: &[Parsed<'a>],
    plan: Option<&precomp::Plan>,
    anchor: &[u8],
    name: &[u8],
) -> Skeleton<'a> {
    let (m0, _, _) = s.units[0];
    let first = parsed[m0].object.as_ref().expect("units come from objects");
    let facts0 = &parsed[m0].facts;
    let mut o = Object::new(Format::Regular, first.machine);

    // Directives and the absolute symbols first, as compilers write them.
    if !facts0.mismatches.is_empty() {
        let mut text = Vec::new();
        for d in &facts0.mismatches {
            text.push(b' ');
            text.extend_from_slice(d);
        }
        let id = o.push_section(Section::new(
            Name::new(b".drectve".to_vec()),
            IMAGE_SCN_LNK_INFO | IMAGE_SCN_LNK_REMOVE | IMAGE_SCN_ALIGN_1BYTES,
            SectionData::Bytes(Cow::Owned(text)),
        ));
        o.push_symbol(section_symbol(
            b".drectve",
            id,
            SectionDefinition::default(),
        ));
    }
    if let Some(v) = facts0.comp_id {
        o.push_symbol(Symbol::new(
            Name::new(b"@comp.id".to_vec()),
            v,
            SectionNumber::Absolute,
            0,
            storage_class::STATIC,
        ));
    }
    if let Some(v) = facts0.feat00 {
        o.push_symbol(Symbol::new(
            Name::new(b"@feat.00".to_vec()),
            v,
            SectionNumber::Absolute,
            0,
            storage_class::STATIC,
        ));
    }
    for (name, v) in &facts0.markers {
        o.push_symbol(Symbol::new(
            Name::new(name.clone()),
            *v,
            SectionNumber::Absolute,
            0,
            storage_class::STATIC,
        ));
    }

    // The units' CodeView, renumbered into the member's own (phase 2.4);
    // its per-object .debug$S first, as compilers write it.
    let mut debug = debug::Builder::new(parsed, plan);
    debug.begin(&mut o, &s.units);

    // The units: their sections and symbols, with names renamed where the
    // copy is a renamed one. Relocations are filled in after, once every
    // unit's symbols are in.
    let mut placed: Vec<Placed> = Vec::with_capacity(s.units.len());
    let mut defined: HashMap<Vec<u8>, SymbolId> = HashMap::default();
    for (k, &(m, u, renamed_copy)) in s.units.iter().enumerate() {
        let object = parsed[m].object.as_ref().expect("units come from objects");
        let unit = &parsed[m].facts.units[u];
        let mut p = Placed {
            sections: HashMap::default(),
            symbols: HashMap::default(),
        };
        for &id in &unit.sections {
            let sec = &object.sections[id.index()];
            let copy = if tables::moves_with_unit(sec.name.as_bytes()) {
                let mut copy = sec.clone();
                copy.relocations = Vec::new();
                copy
            } else if is_debug(sec.name.as_bytes()) {
                debug.section(k, m, id, sec)
            } else {
                let mut copy = sec.clone();
                copy.relocations = Vec::new();
                copy
            };
            p.sections.insert(id.0, o.push_section(copy));
        }
        for &id in &unit.symbols {
            let sym = &object.symbols[id.index()];
            let Some(old) = sym.section.id() else {
                continue;
            };
            let Some(&new) = p.sections.get(&old.0) else {
                continue;
            };
            let mut copy = sym.clone();
            copy.section = SectionNumber::Section(new);
            copy.aux = match &sym.aux {
                Aux::Section(d) => {
                    let mut d = *d;
                    d.number = if d.selection == comdat_select::ASSOCIATIVE {
                        d.number.and_then(|n| p.sections.get(&n.0).copied())
                    } else {
                        None
                    };
                    Aux::Section(d)
                }
                // Function definitions' links to .bf and line numbers don't
                // survive a move; MSVC and LLVM write none.
                Aux::Function(_) => Aux::None,
                other => other.clone(),
            };
            if renamed_copy && sym.is_external() {
                copy.name = Name::new(renamed(sym.name.as_bytes()));
            }
            let new_id = o.push_symbol(copy);
            if sym.is_external() {
                defined.insert(o.symbols[new_id.index()].name.as_bytes().to_vec(), new_id);
            }
            p.symbols.insert(id.0, new_id);
        }
        placed.push(p);
    }

    // Relocations: inside a unit, to its own symbols; outside, by name, to
    // a symbol defined here or to an undefined external.
    let mut names = Names {
        defined,
        undefined: HashMap::default(),
        weak: HashMap::default(),
    };
    for (k, &(m, u, _)) in s.units.iter().enumerate() {
        let object = parsed[m].object.as_ref().expect("units come from objects");
        let unit = &parsed[m].facts.units[u];
        for &id in &unit.sections {
            let Some(&new) = placed[k].sections.get(&id.0) else {
                continue;
            };
            let mut relocations = Vec::with_capacity(object.sections[id.index()].relocations.len());
            for r in &object.sections[id.index()].relocations {
                let symbol = match placed[k].symbols.get(&r.symbol.0) {
                    Some(&inside) => inside,
                    None => names.outside(&mut o, object, &object.symbols[r.symbol.index()]),
                };
                relocations.push(Relocation {
                    offset: debug.offset(k, id, r.offset),
                    symbol,
                    kind: r.kind,
                });
            }
            o.sections[new.index()].relocations = relocations;
        }
    }
    let table_fields = tables::place(&mut o, s, parsed, &placed, &mut |o, object, target| {
        names.outside(o, object, target)
    });
    debug.finish(&mut o, m0, name);
    o.push_symbol(Symbol::new(
        Name::new(anchor.to_vec()),
        0,
        SectionNumber::Absolute,
        0,
        storage_class::EXTERNAL,
    ));
    // Each member with aliases refers to an anchor, so the synthesized
    // member that resolves their default is loaded with them; the index
    // doesn't list the default, so it never loads a member itself. Every
    // synthesized member defines it alike, which both linkers accept.
    o.push_symbol(Symbol::new(
        Name::new(ALIAS_DEFAULT.to_vec()),
        0,
        SectionNumber::Absolute,
        0,
        storage_class::EXTERNAL,
    ));
    if o.sections.len() > MAX_REGULAR_SECTIONS {
        o.set_format(Format::BigObj)
            .expect("a new object converts to big-obj");
    }
    tables::finish(&mut o, &table_fields);
    o.sync_section_definitions();
    o.write_skeleton(HOLE)
        .expect("a synthesized member fits its format")
}

/// Where one unit's sections and symbols went in a synthesized member, by
/// their numbers in the member they came from.
struct Placed {
    sections: HashMap<u32, SectionId>,
    symbols: HashMap<u32, SymbolId>,
}

/// The symbols a synthesized member names things outside its units by.
struct Names {
    /// External symbols defined here, by name.
    defined: HashMap<Vec<u8>, SymbolId>,
    /// Undefined externals and weak externals made here, by name.
    undefined: HashMap<Vec<u8>, SymbolId>,
    weak: HashMap<Vec<u8>, SymbolId>,
}

impl Names {
    /// A symbol by name: defined here, or an undefined external.
    fn by_name(&mut self, o: &mut Object<'_>, name: &[u8], kind: u16) -> SymbolId {
        if let Some(&d) = self.defined.get(name) {
            return d;
        }
        *self.undefined.entry(name.to_vec()).or_insert_with(|| {
            o.push_symbol(Symbol::new(
                Name::new(name.to_vec()),
                0,
                SectionNumber::Undefined,
                kind,
                storage_class::EXTERNAL,
            ))
        })
    }

    /// The symbol here for `target`, a symbol of `object` outside the
    /// units: by name, or a weak external made again here, its default by
    /// name.
    fn outside(
        &mut self,
        o: &mut Object<'_>,
        object: &Object<'_>,
        target: &Symbol<'_>,
    ) -> SymbolId {
        let Some((search, default)) = units::weak_default(object, target) else {
            return self.by_name(o, target.name.as_bytes(), target.kind);
        };
        let name = target.name.as_bytes();
        if let Some(&w) = self.weak.get(name) {
            return w;
        }
        let tag = self.by_name(o, default, 0);
        let mut w = Symbol::new(
            Name::new(name.to_vec()),
            0,
            SectionNumber::Undefined,
            target.kind,
            storage_class::WEAK_EXTERNAL,
        );
        w.aux = Aux::WeakExternal(WeakExternal { tag, search });
        let id = o.push_symbol(w);
        self.weak.insert(name.to_vec(), id);
        id
    }
}

fn section_symbol(name: &[u8], id: SectionId, def: SectionDefinition) -> Symbol<'static> {
    let mut s = Symbol::new(
        Name::new(name.to_vec()),
        0,
        SectionNumber::Section(id),
        0,
        storage_class::STATIC,
    );
    s.aux = Aux::Section(def);
    s
}

#[cfg(test)]
mod tests;
