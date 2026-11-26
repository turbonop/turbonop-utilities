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
//!    folds: its first copy and at least one other foldable, and under
//!    [`Pull::Preserve`] its first copy in the member the archive index
//!    names for each of its symbols, and that member without side effects.
//! 3. **Lay out** the synthesized members: one for each set of members that
//!    gave up copies (and environment), so that a synthesized member is
//!    wanted exactly when one of those members is.
//! 4. **Rewrite** each member that gave up copies: their sections go, the
//!    symbols they defined become undefined externals (or, under
//!    `Preserve`, weak externals with a renamed copy as default), and it
//!    refers to each synthesized member's anchor.
//! 5. **Write** each synthesized member: the surviving copies, their
//!    outside references as undefined externals, an anchor, `@feat.00`,
//!    and the environment's `/FAILIFMISMATCH` directives. Synthesized
//!    members come first in the archive.
//!
//! Every step's result is independent of the number of threads: groups are
//! decided in member order, and the survivor is the first copy.
//!
//! Not yet: folded units' debug information (phase 2.4: a synthesized
//! member holds no `.debug$S` or `.debug$T` yet, so the folded functions
//! have no symbols in the PDB), and the side tables (phase 2.3: a member
//! with any folds nothing).

pub mod units;

use crate::hash::H128;
use std::borrow::Cow;
use std::collections::{BTreeMap, HashMap};
use turbonop_coff::{
    Aux, File, Format, Name, Object, Relocation, Section, SectionData, SectionDefinition,
    SectionId, SectionNumber, Symbol, SymbolId, WeakExternal, comdat_select, storage_class,
    weak_search,
};
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
    /// A link gets the same copy of each name as before, and no member with
    /// side effects loads or stops loading: a group is folded only if its
    /// first copy is in the member the index named for its symbols (the
    /// synthesized members, first in the archive, are named now), and that
    /// member has no side effects.
    Preserve,
    /// Fold regardless.
    Relaxed,
}

/// Options for [`rewrite`].
#[derive(Clone, Copy, Debug)]
pub struct Options {
    pub pull: Pull,
    /// Under [`Pull::Preserve`], fold groups whose first copy's member has
    /// side effects with the weak-external technique. Off: it isn't safe in
    /// link.exe (see [`rewrite`]'s notes in the code).
    pub weak_externals: bool,
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
    /// The member has side tables (phase 2.3).
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
    /// The member the archive index names for a symbol it defines holds
    /// another copy, not this group's (`--pull=preserve`).
    OtherDefinitions,
    /// Its first copy's member has side effects and keeps it, and there
    /// are too few other copies to save anything (`--pull=preserve`).
    SideEffectsTooFewCopies,
    /// Its first copy's member has side effects, so folding it could
    /// change which members a link loads (`--pull=preserve`).
    SideEffects,
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
            Unfoldable::SideTables => "side tables (/guard, .sxdata, FPO), not yet split",
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
                "an earlier member defines its symbols differently (--pull=preserve)"
            }
            Unfoldable::SideEffects => "its first definer has side effects (--pull=preserve)",
            Unfoldable::SideEffectsTooFewCopies => {
                "kept by a member with side effects, too few other copies"
            }
        }
    }
}

/// One output member.
#[derive(Clone, Debug)]
pub struct OutputMember<'a> {
    pub name: Cow<'a, [u8]>,
    pub data: Cow<'a, [u8]>,
    /// Made by turbolib.
    pub synthesized: bool,
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
}

fn parse<'a>(input: &Input<'a>) -> Parsed<'a> {
    let pass = |why| Parsed {
        object: None,
        facts: Facts::default(),
        passed: Some(why),
    };
    match File::parse(input.data) {
        Ok(File::Object(object)) => match units::find(&object) {
            Ok(facts) => Parsed {
                object: Some(object),
                facts,
                passed: None,
            },
            Err(why) => Parsed {
                // Kept for its definitions, which still count.
                object: Some(object),
                facts: Facts::default(),
                passed: Some(why),
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
    let parsed: Vec<Parsed<'a>> = turbonop_par::map_balanced(inputs, 4, parse);
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

    // The member the archive index names for each name: its first definer.
    let mut first_definer: HashMap<&[u8], usize> = HashMap::new();
    let defs: Vec<Vec<Cow<'_, [u8]>>> = inputs
        .iter()
        .zip(&parsed)
        .map(|(i, p)| definitions(i, p))
        .collect();
    for (m, d) in defs.iter().enumerate() {
        for name in d {
            first_definer.entry(name.as_ref()).or_insert(m);
        }
    }

    // Groups of copies, in order of first appearance.
    let mut groups: HashMap<u128, usize> = HashMap::new();
    let mut copies: Vec<Vec<(usize, usize)>> = Vec::new();
    for (m, p) in parsed.iter().enumerate() {
        for (u, unit) in p.facts.units.iter().enumerate() {
            let g = *groups.entry(unit.identity).or_insert_with(|| {
                copies.push(Vec::new());
                copies.len() - 1
            });
            copies[g].push((m, u));
        }
    }

    let mut removals: Vec<Vec<(usize, Removal)>> = vec![Vec::new(); parsed.len()];
    let mut synth_of: HashMap<(u128, Vec<usize>), usize> = HashMap::new();
    let mut synths: Vec<Synth> = Vec::new();
    for c in &copies {
        let unit = |&(m, u): &(usize, usize)| &parsed[m].facts.units[u];
        let first = &c[0];
        // The copies that can move. The first must be one of them: it is
        // the survivor, the copy a link that refers to its names got before.
        // A later copy that can't stays where it is, as it was.
        let movable: Vec<(usize, usize)> = c
            .iter()
            .copied()
            .filter(|x| unit(x).unfoldable.is_none())
            .collect();
        for x in c.iter().filter(|x| unit(x).unfoldable.is_some()) {
            if x != first {
                let w = unit(x).unfoldable.unwrap_or(Unfoldable::CopyUnfoldable);
                *report.not_folded.entry(w).or_default() += 1;
            }
        }
        let mut why = None;
        if let Some(w) = unit(first).unfoldable {
            why = Some(w);
        } else if movable.len() < 2 {
            why = Some(if c.len() < 2 {
                Unfoldable::Single
            } else {
                Unfoldable::CopyUnfoldable
            });
        } else if options.pull == Pull::Preserve
            // Synthesized members come first in the archive, so the index
            // names them for what they define. That keeps what a link gets
            // for each name only if the member the index named was the
            // survivor's.
            && unit(first)
                .externals
                .iter()
                .any(|n| first_definer.get(n.as_slice()) != Some(&first.0))
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
        let side_effects = options.pull == Pull::Preserve && parsed[first.0].facts.side_effects;
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
        let key = (parsed[first.0].facts.environment, givers.clone());
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

    // Rewritten members, then synthesized members, each in parallel.
    let members: Vec<OutputMember<'a>> = turbonop_par::map_range(parsed.len(), 1, |m| {
        let input = &inputs[m];
        if removals[m].is_empty() {
            return OutputMember {
                name: Cow::Borrowed(input.name),
                data: Cow::Borrowed(input.data),
                synthesized: false,
            };
        }
        let anchors: Vec<&[u8]> = anchors_of[m]
            .iter()
            .map(|&n| anchors[n].as_slice())
            .collect();
        let data = strip(&parsed[m], &removals[m], &anchors);
        OutputMember {
            name: Cow::Borrowed(input.name),
            data: Cow::Owned(data),
            synthesized: false,
        }
    });
    let made: Vec<OutputMember<'a>> = turbonop_par::map_range(synths.len(), 1, |n| OutputMember {
        name: Cow::Owned(format!("turbolib{n}.obj").into_bytes()),
        data: Cow::Owned(synthesize(&synths[n], &parsed, &anchors[n])),
        synthesized: true,
    });
    report.rewritten = removals.iter().filter(|r| !r.is_empty()).count() as u64;
    report.synthesized = made.len() as u64;
    // Synthesized members first, so that the index names them for what they
    // define (see the Preserve rule above).
    let mut out = made;
    out.extend(members);
    report.bytes_out = out.iter().map(|m| m.data.len() as u64).sum();
    Output {
        members: out,
        report,
    }
}

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

/// A member with the copies it gives up removed.
fn strip(parsed: &Parsed<'_>, removals: &[(usize, Removal)], anchors: &[&[u8]]) -> Vec<u8> {
    let mut object = parsed
        .object
        .clone()
        .expect("a member that gives up copies is an object");
    let facts = &parsed.facts;
    let mut removed = vec![false; object.sections.len()];
    for &(u, how) in removals {
        let unit = &facts.units[u];
        for s in &unit.sections {
            removed[s.index()] = true;
        }
        for &id in &unit.symbols {
            if !object.symbols[id.index()].is_external() {
                continue;
            }
            let default = match how {
                Removal::Undefined => None,
                Removal::Weak => {
                    let name = renamed(object.symbols[id.index()].name.as_bytes());
                    let kind = object.symbols[id.index()].kind;
                    Some(object.push_symbol(Symbol::new(
                        Name::new(name),
                        0,
                        SectionNumber::Undefined,
                        kind,
                        storage_class::EXTERNAL,
                    )))
                }
            };
            let s = &mut object.symbols[id.index()];
            s.section = SectionNumber::Undefined;
            s.value = 0;
            s.aux = Aux::None;
            if let Some(tag) = default {
                s.storage_class = storage_class::WEAK_EXTERNAL;
                s.aux = Aux::WeakExternal(WeakExternal {
                    tag,
                    search: weak_search::NOLIBRARY,
                });
            }
        }
    }
    // A changed object's checksums would be stale.
    for (i, s) in object.sections.iter().enumerate() {
        if s.name.as_bytes() == b".chks64" {
            removed[i] = true;
        }
    }
    object
        .remove_sections(|id, _| removed[id.index()])
        .expect("a foldable unit is referred to only by external symbols");
    for a in anchors {
        object.push_symbol(Symbol::new(
            Name::new(a.to_vec()),
            0,
            SectionNumber::Undefined,
            0,
            storage_class::EXTERNAL,
        ));
    }
    object.write().expect("a rewritten member fits its format")
}

const IMAGE_SCN_LNK_INFO: u32 = 0x0000_0200;
const IMAGE_SCN_LNK_REMOVE: u32 = 0x0000_0800;
const IMAGE_SCN_ALIGN_1BYTES: u32 = 0x0010_0000;
/// The most sections a regular object can have; past it, big-obj.
const MAX_REGULAR_SECTIONS: usize = 65_279;

/// A synthesized member.
fn synthesize(s: &Synth, parsed: &[Parsed<'_>], anchor: &[u8]) -> Vec<u8> {
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

    // The units: their sections and symbols, with names renamed where the
    // copy is a renamed one. Relocations are filled in after, once every
    // unit's symbols are in.
    struct Placed {
        sections: HashMap<u32, SectionId>,
        symbols: HashMap<u32, SymbolId>,
    }
    let mut placed: Vec<Placed> = Vec::with_capacity(s.units.len());
    let mut defined: HashMap<Vec<u8>, SymbolId> = HashMap::new();
    for &(m, u, renamed_copy) in &s.units {
        let object = parsed[m].object.as_ref().expect("units come from objects");
        let unit = &parsed[m].facts.units[u];
        let mut p = Placed {
            sections: HashMap::new(),
            symbols: HashMap::new(),
        };
        for &id in &unit.sections {
            let sec = &object.sections[id.index()];
            if is_debug(sec.name.as_bytes()) {
                continue;
            }
            let mut copy = sec.clone();
            copy.relocations = Vec::new();
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
    let mut undefined: HashMap<Vec<u8>, SymbolId> = HashMap::new();
    let mut weak: HashMap<Vec<u8>, SymbolId> = HashMap::new();
    // A symbol by name: defined here, or an undefined external.
    fn by_name<'x>(
        o: &mut Object<'x>,
        defined: &HashMap<Vec<u8>, SymbolId>,
        undefined: &mut HashMap<Vec<u8>, SymbolId>,
        name: &[u8],
        kind: u16,
    ) -> SymbolId {
        if let Some(&d) = defined.get(name) {
            return d;
        }
        *undefined.entry(name.to_vec()).or_insert_with(|| {
            o.push_symbol(Symbol::new(
                Name::new(name.to_vec()),
                0,
                SectionNumber::Undefined,
                kind,
                storage_class::EXTERNAL,
            ))
        })
    }
    for (k, &(m, u, _)) in s.units.iter().enumerate() {
        let object = parsed[m].object.as_ref().expect("units come from objects");
        let unit = &parsed[m].facts.units[u];
        for &id in &unit.sections {
            let Some(&new) = placed[k].sections.get(&id.0) else {
                continue;
            };
            let mut relocations = Vec::with_capacity(object.sections[id.index()].relocations.len());
            for r in &object.sections[id.index()].relocations {
                let target = &object.symbols[r.symbol.index()];
                let symbol = match placed[k].symbols.get(&r.symbol.0) {
                    Some(&inside) => inside,
                    None => match units::weak_default(object, target) {
                        // A weak external, made again here, its default by
                        // name.
                        Some((search, default)) => {
                            let name = target.name.as_bytes();
                            match weak.get(name) {
                                Some(&w) => w,
                                None => {
                                    let tag = by_name(&mut o, &defined, &mut undefined, default, 0);
                                    let mut w = Symbol::new(
                                        Name::new(name.to_vec()),
                                        0,
                                        SectionNumber::Undefined,
                                        target.kind,
                                        storage_class::WEAK_EXTERNAL,
                                    );
                                    w.aux = Aux::WeakExternal(WeakExternal { tag, search });
                                    let id = o.push_symbol(w);
                                    weak.insert(name.to_vec(), id);
                                    id
                                }
                            }
                        }
                        None => by_name(
                            &mut o,
                            &defined,
                            &mut undefined,
                            target.name.as_bytes(),
                            target.kind,
                        ),
                    },
                };
                relocations.push(Relocation {
                    offset: r.offset,
                    symbol,
                    kind: r.kind,
                });
            }
            o.sections[new.index()].relocations = relocations;
        }
    }
    o.push_symbol(Symbol::new(
        Name::new(anchor.to_vec()),
        0,
        SectionNumber::Absolute,
        0,
        storage_class::EXTERNAL,
    ));
    if o.sections.len() > MAX_REGULAR_SECTIONS {
        o.set_format(Format::BigObj)
            .expect("a new object converts to big-obj");
    }
    o.sync_section_definitions();
    o.write().expect("a synthesized member fits its format")
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
