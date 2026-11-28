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
//!    [`alias_default`]), so the archive index names the same members for
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
//!
//! Under [`Fold::SelectAny`], copies of one name that the linker could
//! choose among fold though their code differs, and under
//! [`Locals::Copy`], units that reach internal-linkage symbols fold with
//! copies of what they reach ([`locals`]).

pub mod debug;
pub mod locals;
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
    /// Up to `max` precompiled-types members (by default, as many as the
    /// cost model chooses, up to 16), each for a group of members, holding
    /// the records at least `min` members of the group hold
    /// ([`precomp::cluster`]); `weight` is what a byte an average link
    /// reads weighs against a byte of the library
    /// ([`precomp::cluster::WEIGHT`] by default).
    Clustered {
        min: u32,
        max: Option<u32>,
        weight: u64,
    },
}

impl Precomp {
    /// The numbers of groups `Clustered` tries: the powers of two below
    /// its `max`, and `max`.
    pub fn tries(max: Option<u32>) -> Vec<u32> {
        let max = max.unwrap_or(16).max(1);
        let mut v: Vec<u32> = (0..32)
            .map(|i| 1u32 << i)
            .take_while(|&t| t < max)
            .collect();
        v.push(max);
        v
    }
}

/// Which copies count as the same (`--fold`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Fold {
    /// None: only types are shared (if `precomp` says so).
    None,
    /// Copies whose identities are equal.
    Identical,
    /// Copies the linker could substitute for each other, by their
    /// selection (see `docs/turbolib.md`, "`--fold`"), the survivor chosen
    /// by the [`Pick`]: `ANY` copies of one name whatever their code;
    /// `SAME_SIZE` and `EXACT_MATCH` ones when their sizes (and checksums)
    /// agree, identical ones otherwise; `LARGEST` ones keeping the largest.
    /// Only in one environment, as for `Identical`.
    SelectAny(Pick),
}

/// Which copy survives under [`Fold::SelectAny`] (`--pick`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Pick {
    /// The first in member order, which the archive index names.
    First,
    /// The one whose COMDAT section is largest (the first of those).
    Largest,
}

/// Units that reach internal-linkage symbols outside them (`--locals`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Locals {
    /// They don't fold.
    Keep,
    /// They fold, with copies of what they reach ([`locals`]) in the
    /// synthesized member, one for each member a copy comes from.
    Copy,
    /// As `Copy`, but a synthesized member holds one copy of each local
    /// with given contents.
    Merge,
}

/// Options for [`rewrite`].
#[derive(Clone, Copy, Debug)]
pub struct Options {
    pub pull: Pull,
    /// Under [`Pull::Preserve`], fold groups whose first copy's member has
    /// side effects with the weak-external technique. Off: it isn't safe in
    /// link.exe (see [`rewrite`]'s notes in the code).
    pub weak_externals: bool,
    pub fold: Fold,
    pub locals: Locals,
    pub precomp: Precomp,
    /// List the names whose definitions differ (`--report odr`).
    pub odr: bool,
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
    /// The unit holds an initializer, terminator, or TLS callback entry
    /// (`.CRT$X*`: an inline variable's or a template's static member's
    /// dynamic initializer). Moved into a synthesized member, it would run
    /// at that member's place in the order, not its own object's, and
    /// inline variables' initialization is ordered within each translation
    /// unit.
    Initializer,
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
    /// It reaches a writable local (`--locals=copy`): a copy would be
    /// another variable.
    WritableLocal,
    /// Its debug information refers to a local symbol outside it
    /// (`--locals=copy`).
    LocalDebug,
    /// `SAME_SIZE` or `EXACT_MATCH` copies whose sizes or checksums differ,
    /// and no identical copy (`--fold=select-any`).
    SelectAnyMismatch,
    /// Copies with other code refer to other names, so keeping one for all
    /// could change which members a link loads (`--fold=select-any`,
    /// `--pull=preserve`).
    DifferingReferences,
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
            Unfoldable::Initializer => "an initializer, terminator, or TLS callback (.CRT$X*)",
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
            Unfoldable::WritableLocal => "reaches a writable local (--locals=copy)",
            Unfoldable::LocalDebug => {
                "its debug information refers to a local outside it (--locals=copy)"
            }
            Unfoldable::SelectAnyMismatch => {
                "SAME_SIZE or EXACT_MATCH copies that disagree (--fold=select-any)"
            }
            Unfoldable::DifferingReferences => {
                "copies with other code refer to other names (--fold=select-any, --pull=preserve)"
            }
        }
    }
}

/// Section data at least this large isn't copied into a rewritten or
/// synthesized member's bytes: it is left as a hole, filled from the input
/// when the library is written. Each hole costs its place in a list (24
/// bytes) and a piece for the writer to copy, so smaller data is copied.
const HOLE: usize = 32;

/// One output member.
#[derive(Clone, Debug)]
pub struct OutputMember<'a> {
    pub name: Cow<'a, [u8]>,
    /// Its bytes, but for the holes'.
    pub data: Cow<'a, [u8]>,
    /// The pieces of the member that `data` leaves out (see
    /// [`turbonop_coff::Skeleton`]): each offset in the member, and the
    /// bytes to put there, in order. Only section contents are left out:
    /// the headers are where the member has them.
    pub holes: Vec<(usize, &'a [u8])>,
    /// Where `data` has the symbol table (and the string table after it),
    /// for a member with holes (see [`turbonop_coff::Skeleton`]).
    pub symbol_table: Option<u32>,
    /// Made by turbolib.
    pub synthesized: bool,
    /// The names the archive index lists for this member, if not the ones
    /// it defines: under [`Pull::Preserve`], a synthesized member is listed
    /// for its anchor alone, so that each folded name stays listed for the
    /// member it was listed for (see [`Pull::Preserve`]).
    pub index_only: Option<Vec<Vec<u8>>>,
}

impl OutputMember<'_> {
    /// The member's length.
    pub fn len(&self) -> usize {
        self.data.len() + self.holes.iter().map(|h| h.1.len()).sum::<usize>()
    }

    /// Whether the member is empty.
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// The member's bytes, gathered if it has holes.
    pub fn file(&self) -> Cow<'_, [u8]> {
        if self.holes.is_empty() {
            Cow::Borrowed(&self.data)
        } else {
            Cow::Owned(turbonop_coff::skeleton_parts(&self.data, &self.holes).concat())
        }
    }
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
    /// Groups folded whose copies' code differs (`--fold=select-any`).
    pub differing_groups: u64,
    /// `SAME_SIZE` and `EXACT_MATCH` copies of one name that disagree, left
    /// as they are (but for identical ones).
    pub mismatches: Vec<Mismatch>,
    /// Copies of locals (`--locals=copy`).
    pub locals: LocalsReport,
    /// Names whose definitions differ (`--report odr`).
    pub odr: Vec<OdrFinding>,
}

/// `SAME_SIZE` or `EXACT_MATCH` copies of one name that disagree.
#[derive(Clone, Debug)]
pub struct Mismatch {
    pub names: Vec<Vec<u8>>,
    pub selection: u8,
    /// Each copy's member, COMDAT section size, and checksum.
    pub copies: Vec<(usize, u32, u32)>,
}

/// What copying locals did.
#[derive(Clone, Debug, Default)]
pub struct LocalsReport {
    /// Copies made of local units and of pieces, and their bytes.
    pub units_copied: u64,
    pub pieces_copied: u64,
    pub bytes_copied: u64,
    /// Local units removed from their members with what reached them, and
    /// their bytes.
    pub absorbed: u64,
    pub absorbed_bytes: u64,
}

/// A name whose definitions in the library differ (`--report odr`).
#[derive(Clone, Debug)]
pub struct OdrFinding {
    /// The external names the definitions define.
    pub names: Vec<Vec<u8>>,
    /// The definitions, in order of first appearance.
    pub variants: Vec<OdrVariant>,
    /// Whether two of them are in one environment (rather than in members
    /// compiled with other options).
    pub same_environment: bool,
}

/// One of an [`OdrFinding`]'s definitions.
#[derive(Clone, Debug)]
pub struct OdrVariant {
    pub code: u128,
    /// Its unit's bytes and selection.
    pub size: u64,
    pub selection: u8,
    /// The members holding it, and their environments.
    pub members: Vec<Vec<u8>>,
    pub environments: Vec<u128>,
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
    /// Under `--precomp=clustered`, each machine's grouping.
    pub clusters: Vec<precomp::Clusters>,
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
    /// The object (`None` for a member that isn't one, and, when the
    /// members are made one at a time, once read: see [`Objects`]).
    object: Option<Object<'a>>,
    /// Whether the member is a COFF object.
    is_object: bool,
    /// The object's machine (0 if it isn't one).
    machine: u16,
    facts: Facts<'a>,
    passed: Option<Unfoldable>,
    debug: debug::Facts,
    /// Its types' section data (`.debug$T` or `.debug$P`), if it has
    /// records ([`debug::Types::Records`]).
    types: &'a [u8],
    /// The external symbols it defines (see [`definitions`]); taken once
    /// the definers are found.
    definitions: Vec<Cow<'a, [u8]>>,
    /// What shared types need of its object (under `--precomp`).
    scan: precomp::Scan<'a>,
    /// Under select-any with `--pull=preserve`, each unit's references
    /// ([`locals::references`]; 0 for a unit without external names).
    references: Vec<u128>,
}

/// What reading a member computes beyond its facts, for the options.
#[derive(Clone, Copy, Debug, Default)]
struct Needs {
    mode: units::Mode,
    /// Shared types' [`precomp::Scan`], and its undefined names
    /// (`clustered`).
    precomp: bool,
    clustered: bool,
    /// Each unit's references (select-any, `--pull=preserve`).
    references: bool,
    /// Keep the object (tests).
    keep: bool,
}

impl Needs {
    fn of(options: &Options) -> Needs {
        Needs {
            mode: units::Mode {
                copy_locals: options.locals != Locals::Keep && options.fold != Fold::None,
                structural: options.odr,
            },
            precomp: !matches!(options.precomp, Precomp::None),
            clustered: matches!(options.precomp, Precomp::Clustered { .. }),
            references: matches!(options.fold, Fold::SelectAny(_))
                && options.pull == Pull::Preserve,
            keep: false,
        }
    }
}

/// Where the members' objects are found while members are made: the ones
/// being worked on (the members are made one at a time, each object read
/// again and let go once its work is done, so that they aren't all in
/// memory at once), or (tests) those kept in `parsed`.
#[derive(Clone, Copy)]
struct Objects<'p, 'a> {
    parsed: &'p [Parsed<'a>],
    current: &'p [(usize, &'p Object<'a>)],
}

impl<'p, 'a> Objects<'p, 'a> {
    /// The objects kept in `parsed`.
    #[cfg(test)]
    fn kept(parsed: &'p [Parsed<'a>]) -> Self {
        Objects {
            parsed,
            current: &[],
        }
    }

    /// Member `m`'s object.
    fn get(&self, m: usize) -> &'p Object<'a> {
        match self.current.iter().find(|c| c.0 == m) {
            Some(&(_, o)) => o,
            None => self.parsed[m]
                .object
                .as_ref()
                .expect("units come from objects"),
        }
    }
}

/// Reads a member: its facts, and what `needs` asks for of its object,
/// which isn't kept (unless `needs.keep`): the members are made one at a
/// time, each object read again (see [`Objects`]).
fn parse<'a>(input: &Input<'a>, needs: Needs) -> Parsed<'a> {
    let pass = |why| Parsed {
        object: None,
        is_object: false,
        machine: 0,
        facts: Facts::default(),
        passed: Some(why),
        debug: debug::Facts::default(),
        types: &[],
        definitions: Vec::new(),
        scan: precomp::Scan::default(),
        references: Vec::new(),
    };
    let mut p = match File::parse(input.data) {
        Ok(File::Object(object)) => match units::find_with(&object, needs.mode) {
            Ok(mut facts) => {
                let debug = debug::read(&object, &mut facts);
                let types = match &debug.types {
                    debug::Types::Records { section, .. } => object.sections[section.index()]
                        .data
                        .borrowed()
                        .unwrap_or_default(),
                    _ => &[],
                };
                let references = if needs.references {
                    (0..facts.units.len())
                        .map(|k| {
                            if facts.units[k].name_key == 0 {
                                0
                            } else {
                                locals::references(&object, &facts, k)
                            }
                        })
                        .collect()
                } else {
                    Vec::new()
                };
                Parsed {
                    machine: object.machine,
                    object: Some(object),
                    is_object: true,
                    facts,
                    passed: None,
                    debug,
                    types,
                    definitions: Vec::new(),
                    scan: precomp::Scan::default(),
                    references,
                }
            }
            Err(why) => Parsed {
                // Read for its definitions, which still count.
                machine: object.machine,
                object: Some(object),
                is_object: true,
                facts: Facts::default(),
                passed: Some(why),
                debug: debug::Facts::default(),
                types: &[],
                definitions: Vec::new(),
                scan: precomp::Scan::default(),
                references: Vec::new(),
            },
        },
        Ok(File::Opaque { kind, .. }) => pass(match kind {
            turbonop_coff::Kind::Import => Unfoldable::Import,
            turbonop_coff::Kind::Ltcg => Unfoldable::Ltcg,
            _ => Unfoldable::Anonymous,
        }),
        Err(_) => pass(Unfoldable::Unreadable),
    };
    p.definitions = definitions(input, &p);
    if needs.precomp
        && let Some(o) = &p.object
    {
        p.scan = precomp::scan(o, &p.debug, needs.clustered);
    }
    if !needs.keep {
        p.object = None;
    }
    p
}

/// The external symbols a member defines, as the archive index would list
/// them (for counting definers). Import objects define their symbol and
/// `__imp_` it; `/GL` objects aren't read.
fn definitions<'a>(input: &Input<'a>, parsed: &Parsed<'a>) -> Vec<Cow<'a, [u8]>> {
    if let Some(o) = &parsed.object {
        return o
            .symbols
            .iter()
            .filter(|s| s.is_external() && s.section.id().is_some())
            .map(|s| s.name.data())
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
///
/// Under select-any (`keys`), also each name's [`units::Unit::name_key`],
/// if every definition of it is a unit with that key; `None` if any
/// differs, or isn't a unit's.
type Definers<'p> = (
    HashMap<&'p [u8], usize>,
    HashMap<&'p [u8], Option<u128>>,
    HashMap<&'p [u8], Option<u128>>,
);

fn definers<'p>(
    parsed: &'p [Parsed<'_>],
    defs: &'p [Vec<Cow<'_, [u8]>>],
    keys: bool,
) -> Definers<'p> {
    let mut first_definer: HashMap<&[u8], usize> = HashMap::default();
    for (m, d) in defs.iter().enumerate() {
        for name in d {
            first_definer.entry(name.as_ref()).or_insert(m);
        }
    }
    let mut one_code: HashMap<&[u8], Option<u128>> = HashMap::default();
    let mut one_key: HashMap<&[u8], Option<u128>> = HashMap::default();
    for (p, d) in parsed.iter().zip(defs) {
        if p.passed.is_some() {
            for name in d {
                one_code.insert(name.as_ref(), None);
                if keys {
                    one_key.insert(name.as_ref(), None);
                }
            }
            continue;
        }
        for unit in &p.facts.units {
            for n in &unit.externals {
                let e = one_code.entry(n.as_ref()).or_insert(Some(unit.code));
                if *e != Some(unit.code) {
                    *e = None;
                }
                if keys {
                    let e = one_key.entry(n.as_ref()).or_insert(Some(unit.name_key));
                    if *e != Some(unit.name_key) {
                        *e = None;
                    }
                }
            }
        }
        for n in &p.facts.other_definitions {
            one_code.insert(n.as_ref(), None);
            if keys {
                one_key.insert(n.as_ref(), None);
            }
        }
    }
    (first_definer, one_code, one_key)
}

/// A synthesized member's contents: the surviving copies, and whether each
/// is a renamed copy.
struct Synth {
    environment: u128,
    /// The members that gave copies, in order.
    givers: Vec<usize>,
    /// (member, unit, renamed), in member and unit order.
    units: Vec<(usize, usize, bool)>,
    /// The locals its units reach, copied (`--locals=copy`).
    copies: Vec<LocalCopy>,
}

/// A local a synthesized member holds a copy of: a member's local unit or
/// piece.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct LocalCopy {
    member: usize,
    node: locals::Node,
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
    let needs = Needs::of(&options);
    let mode = needs.mode;
    let mut parsed: Vec<Parsed<'a>> =
        turbonop_par::map_balanced(inputs, 4, |i: &Input<'a>| parse(i, needs));
    // PCH users' PCH objects (their units' types are there).
    let library = !matches!(options.precomp, Precomp::None);
    debug::resolve(&mut parsed, library);
    if mode.copy_locals {
        // A unit that reaches a local unit whose debug information can't be
        // carried doesn't fold.
        for p in &mut parsed {
            locals::settle(&mut p.facts);
        }
    }
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
    let defs: Vec<Vec<Cow<'a, [u8]>>> = parsed
        .iter_mut()
        .map(|p| std::mem::take(&mut p.definitions))
        .collect();
    // The shared types, while the definers are found (on one thread).
    let types_clock = std::time::Instant::now();
    let (plan, (first_definer, one_code, one_key)) = turbonop_par::join(
        || match options.precomp {
            Precomp::Library { min } => Some((
                precomp::plan(&parsed, min.max(1), None),
                types_clock.elapsed(),
            )),
            Precomp::Clustered { min, max, weight } => {
                let c = precomp::Clustering {
                    tries: Precomp::tries(max),
                    weight,
                    definitions: &defs,
                };
                Some((
                    precomp::plan(&parsed, min.max(2), Some(&c)),
                    types_clock.elapsed(),
                ))
            }
            Precomp::None => None,
        },
        || definers(&parsed, &defs, matches!(options.fold, Fold::SelectAny(_))),
    );
    let plan = plan.map(|(plan, time)| {
        let t = &mut report.types;
        // (Two groups with the same shared records have one member.)
        let mut seen: turbonop_hash::Set<&[u8]> = turbonop_hash::Set::default();
        for p in plan.pchs.iter().filter(|p| seen.insert(&p.name)) {
            t.pchs += 1;
            t.shared_records += u64::from(p.count);
            t.shared_bytes += p.records.len() as u64;
        }
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
        t.clusters = plan.clusters.clone();
        plan
    });

    lap(1);
    // Groups of copies, in order of first appearance: by identity, or, under
    // select-any, by environment, selection, and names (for units that have
    // external names).
    let select_any = matches!(options.fold, Fold::SelectAny(_));
    let mut groups: HashMap<u128, usize> = HashMap::default();
    let mut copies: Vec<Vec<(usize, usize)>> = Vec::new();
    let mut named: Vec<bool> = Vec::new();
    for (m, p) in parsed
        .iter()
        .enumerate()
        .filter(|_| options.fold != Fold::None)
    {
        for (u, unit) in p.facts.units.iter().enumerate() {
            let by_name = select_any && unit.name_key != 0;
            let key = if by_name {
                let mut h = H128::new();
                h.u64(0x5345_4C45_4354);
                h.u128(p.facts.environment);
                h.u128(unit.name_key);
                h.finish()
            } else {
                unit.identity
            };
            let g = *groups.entry(key).or_insert_with(|| {
                copies.push(Vec::new());
                named.push(by_name);
                copies.len() - 1
            });
            copies[g].push((m, u));
        }
    }

    let mut d = Decider {
        parsed: &parsed,
        options,
        first_definer: &first_definer,
        one_code: &one_code,
        one_key: &one_key,
        report: &mut report,
        removals: vec![Vec::new(); parsed.len()],
        same: vec![Vec::new(); parsed.len()],
        synth_of: HashMap::default(),
        synths: Vec::new(),
    };
    for (c, &by_name) in copies.iter().zip(&named) {
        d.group(c, by_name);
    }
    let Decider {
        mut removals,
        same,
        mut synths,
        ..
    } = d;
    // What deciding needed, let go before the members are made.
    drop((first_definer, one_code, one_key, groups, copies, named));
    drop(defs);

    // Under --locals=copy, a member's local units that only moved units
    // refer to go too (their copies are in the synthesized members).
    if options.locals != Locals::Keep {
        let absorbed: Vec<Vec<usize>> =
            turbonop_par::map_range(parsed.len(), 16, |m| absorb(&parsed[m].facts, &same[m]));
        for (m, list) in absorbed.into_iter().enumerate() {
            report.locals.absorbed += list.len() as u64;
            for v in list {
                report.locals.absorbed_bytes += parsed[m].facts.units[v].size;
                removals[m].push((v, Removal::Undefined));
            }
        }
    }
    for s in &mut synths {
        s.units.sort_unstable();
        if options.locals != Locals::Keep {
            s.copies = local_copies(s, &parsed, options.locals);
            for c in &s.copies {
                let facts = &parsed[c.member].facts;
                match c.node {
                    locals::Node::Unit(v) => {
                        report.locals.units_copied += 1;
                        report.locals.bytes_copied += facts.units[v as usize].size;
                    }
                    locals::Node::Piece(p) => {
                        let piece = &facts.pieces[p as usize];
                        report.locals.pieces_copied += 1;
                        report.locals.bytes_copied += u64::from(piece.end - piece.start)
                            + 10 * piece.relocations.len() as u64;
                    }
                }
            }
        }
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
    // `turbolib<n>-<the end of its anchor's hash>.obj`: a name no other
    // library's synthesized members have, so that a library made of folded
    // libraries (or lib.exe's combination of them) has no two members of
    // one name, which is one too many (lib.exe replaces one with the other
    // when it archives the library again with an object of that name).
    // Should an input member have the name anyway, the next `n` is taken.
    let names: Vec<Vec<u8>> = {
        let taken: std::collections::HashSet<Vec<u8>> =
            inputs.iter().map(|i| i.name.to_ascii_lowercase()).collect();
        let mut k = 0usize;
        (0..synths.len())
            .map(|n| {
                let tag = &anchors[n][anchors[n].len().saturating_sub(12)..];
                loop {
                    let mut name = format!("turbolib{k}-").into_bytes();
                    name.extend_from_slice(tag);
                    name.extend_from_slice(b".obj");
                    k += 1;
                    if !taken.contains(&name) {
                        break name;
                    }
                }
            })
            .collect()
    };
    let name_of = |n: usize| names[n].clone();
    let synthesized = |n: usize, objects: Objects<'_, 'a>| {
        let name = name_of(n);
        let skeleton = synthesize(
            &synths[n],
            objects,
            &parsed,
            plan.as_ref(),
            &anchors[n],
            &name,
        );
        OutputMember {
            name: Cow::Owned(name),
            data: Cow::Owned(skeleton.bytes),
            holes: skeleton.holes,
            symbol_table: Some(skeleton.symbol_table),
            synthesized: true,
            index_only: (options.pull == Pull::Preserve).then(|| vec![anchors[n].clone()]),
        }
    };
    // A member that gives up copies or shares types, rewritten from its
    // object (read again).
    let rewrite_member = |m: usize, mut object: Object<'a>| -> Rewritten<'a> {
        let input = &inputs[m];
        // Its shared types, if any (not of the sections it gives up).
        let edit = plan.as_ref().and_then(|plan| {
            let mut removed = vec![false; object.sections.len()];
            for &(u, _) in &removals[m] {
                for s in &parsed[m].facts.units[u].sections {
                    removed[s.index()] = true;
                }
            }
            plan.edit(&parsed, m, &object, &removed)
        });
        let sizes = edit.as_ref().map(|e| (e.before, e.after));
        if removals[m].is_empty() && edit.is_none() {
            // A member of a library folded before keeps nothing of the old
            // form of the aliases' default (see `LEGACY_ALIAS_DEFAULT`).
            if !renew_alias_default(&mut object) {
                return (m, passed_through(input), None);
            }
            let skeleton = object
                .write_skeleton(HOLE)
                .expect("a member renamed in place fits its format");
            return (
                m,
                OutputMember {
                    name: Cow::Borrowed(input.name),
                    data: Cow::Owned(skeleton.bytes),
                    holes: skeleton.holes,
                    symbol_table: Some(skeleton.symbol_table),
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
        let skeleton = strip(object, &parsed[m], &removals[m], &refers, edit.as_ref());
        (
            m,
            OutputMember {
                name: Cow::Borrowed(input.name),
                data: Cow::Owned(skeleton.bytes),
                holes: skeleton.holes,
                symbol_table: Some(skeleton.symbol_table),
                synthesized: false,
                index_only: None,
            },
            sizes,
        )
    };
    let rewrites: Vec<bool> = turbonop_par::map_range(parsed.len(), 64, |m| {
        !removals[m].is_empty()
            || plan.as_ref().is_some_and(|p| p.may_edit(m))
            || (parsed[m].is_object
                && parsed[m].passed.is_none()
                && may_have_legacy_alias_default(inputs[m].data))
    });
    let (members, made) = one_at_a_time_members(
        inputs,
        &parsed,
        &rewrites,
        &synths,
        &rewrite_member,
        &synthesized,
    );
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
    // The precompiled-types members, after them.
    let pchs: Vec<OutputMember<'a>> = plan
        .as_ref()
        .map(|plan| {
            let mut seen: turbonop_hash::Set<&[u8]> = turbonop_hash::Set::default();
            plan.pchs
                .iter()
                // Two groups whose shared records are the same have the same
                // member (name, signature, and symbol): written once.
                .filter(|pch| seen.insert(pch.name.as_slice()))
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
                    symbol_table: None,
                    synthesized: true,
                    index_only: Some(vec![pch.symbol.clone()]),
                })
                .collect()
        })
        .unwrap_or_default();
    if options.odr {
        report.odr = odr(inputs, &parsed);
    }
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
    report.bytes_out = out.iter().map(|m| m.len() as u64).sum();
    Output {
        members: out,
        report,
    }
}

/// Decides, group by group, which copies fold, and lays out the
/// synthesized members that hold the survivors.
struct Decider<'d, 'p, 'a> {
    parsed: &'p [Parsed<'a>],
    options: Options,
    first_definer: &'d HashMap<&'p [u8], usize>,
    one_code: &'d HashMap<&'p [u8], Option<u128>>,
    one_key: &'d HashMap<&'p [u8], Option<u128>>,
    report: &'d mut Report,
    /// Each member's units that move, and how.
    removals: Vec<Vec<(usize, Removal)>>,
    /// Each member's units that move whose copy is the survivor's code (so
    /// the synthesized member holds the same locals they reach).
    same: Vec<Vec<usize>>,
    synth_of: HashMap<(u128, u64, Vec<usize>), usize>,
    synths: Vec<Synth>,
}

impl<'a> Decider<'_, '_, 'a> {
    fn unit(&self, (m, u): (usize, usize)) -> &units::Unit<'a> {
        &self.parsed[m].facts.units[u]
    }

    /// One group of copies: by identity, or (`by_name`) by name under
    /// select-any.
    fn group(&mut self, c: &[(usize, usize)], by_name: bool) {
        // The copies that can move; the first of them survives. A copy that
        // can't stays where it is.
        let movable: Vec<(usize, usize)> = c
            .iter()
            .copied()
            .filter(|&x| self.unit(x).unfoldable.is_none())
            .collect();
        for &x in c {
            if let Some(w) = self.unit(x).unfoldable {
                *self.report.not_folded.entry(w).or_default() += 1;
            }
        }
        if movable.len() < 2 {
            if movable.len() == 1 {
                let w = if c.len() < 2 {
                    Unfoldable::Single
                } else {
                    Unfoldable::CopyUnfoldable
                };
                *self.report.not_folded.entry(w).or_default() += 1;
            }
            return;
        }
        if by_name {
            self.select_any(&movable);
        } else {
            self.identical(&movable);
        }
    }

    fn not_folded(&mut self, c: &[(usize, usize)], why: Unfoldable) {
        *self.report.not_folded.entry(why).or_default() += c.len().max(1) as u64;
        if c.len() > 1 {
            *self.report.not_folded_bytes.entry(why).or_default() +=
                self.unit(c[0]).size * (c.len() as u64 - 1);
        }
    }

    /// Whether member `m`'s name `n`'s first definer (which the archive
    /// index names) has side effects.
    fn definer_has_side_effects(&self, x: (usize, usize)) -> bool {
        self.unit(x).externals.iter().any(|n| {
            self.first_definer
                .get(n.as_ref())
                .is_some_and(|&m| self.parsed[m].facts.side_effects)
        })
    }

    /// Copies with the same identity (`--fold=identical`, and what
    /// select-any leaves).
    fn identical(&mut self, c: &[(usize, usize)]) {
        let first = c[0];
        let mut why = None;
        // Under Preserve, a link must get the same code for each name, from
        // the same members. Which copy a link keeps depends on which members
        // it loads and in what order, so a name folds only if every
        // definition of it in the library is a unit with the same code;
        // then any copy is the same. And the member the index named for it
        // must have had no side effects (see `Pull::Preserve`).
        if self.options.pull == Pull::Preserve
            && self.unit(first).externals.iter().any(|n| {
                self.one_code.get(n.as_ref()).copied().flatten() != Some(self.unit(first).code)
            })
        {
            why = Some(Unfoldable::OtherDefinitions);
        }
        // Under Preserve, a group whose first copy's member has side effects
        // isn't folded: the weak-external technique (that member keeps its
        // copy, the others get NOLIBRARY weak externals whose default is a
        // renamed copy) fails in link.exe 14.52 when a strong reference to
        // the name reaches the link from a library member after the weak
        // externals (measured; see docs/turbolib.md). `weak_externals`
        // turns it on, for study.
        let side_effects =
            self.options.pull == Pull::Preserve && self.definer_has_side_effects(first);
        let keep_first = side_effects && self.options.weak_externals;
        if why.is_none() && side_effects && !self.options.weak_externals {
            why = Some(Unfoldable::SideEffects);
        }
        if why.is_none() && keep_first && c.len() < 3 {
            why = Some(Unfoldable::SideEffectsTooFewCopies);
        }
        if let Some(w) = why {
            self.not_folded(c, w);
            return;
        }
        if keep_first {
            self.report.kept_for_side_effects += 1;
            self.fold(first, &c[1..], true);
        } else {
            self.fold(first, c, false);
        }
    }

    /// Copies of one name, selection, and environment, under select-any:
    /// what the linker could choose among (see `docs/turbolib.md`,
    /// "`--fold`").
    fn select_any(&mut self, c: &[(usize, usize)]) {
        let first = c[0];
        let all_same = c
            .iter()
            .all(|&x| self.unit(x).identity == self.unit(first).identity);
        let selection = self.unit(first).selection;
        // SAME_SIZE and EXACT_MATCH copies that disagree would fail a link
        // that has two of them: they stay, and only identical ones fold.
        let agree = match selection {
            comdat_select::SAME_SIZE => c
                .iter()
                .all(|&x| self.unit(x).leader_size == self.unit(first).leader_size),
            comdat_select::EXACT_MATCH => c.iter().all(|&x| {
                self.unit(x).checksum == self.unit(first).checksum
                    && self.unit(x).leader_size == self.unit(first).leader_size
            }),
            _ => true,
        };
        if !agree {
            self.report.mismatches.push(Mismatch {
                names: self
                    .unit(first)
                    .externals
                    .iter()
                    .map(|n| n.to_vec())
                    .collect(),
                selection,
                copies: c
                    .iter()
                    .map(|&x| (x.0, self.unit(x).leader_size, self.unit(x).checksum))
                    .collect(),
            });
            // By identity, in order of first appearance.
            let mut subgroups: Vec<Vec<(usize, usize)>> = Vec::new();
            let mut of: HashMap<u128, usize> = HashMap::default();
            for &x in c {
                let g = *of.entry(self.unit(x).identity).or_insert_with(|| {
                    subgroups.push(Vec::new());
                    subgroups.len() - 1
                });
                subgroups[g].push(x);
            }
            for g in subgroups {
                if g.len() < 2 {
                    *self
                        .report
                        .not_folded
                        .entry(Unfoldable::SelectAnyMismatch)
                        .or_default() += 1;
                } else {
                    self.identical(&g);
                }
            }
            return;
        }
        // The survivor: the first, or the largest (as LARGEST keeps it).
        let pick_largest = selection == comdat_select::LARGEST
            || matches!(self.options.fold, Fold::SelectAny(Pick::Largest));
        let survivor = if pick_largest {
            let mut best = first;
            for &x in c {
                if self.unit(x).leader_size > self.unit(best).leader_size {
                    best = x;
                }
            }
            best
        } else {
            first
        };
        let mut why = None;
        if self.options.pull == Pull::Preserve {
            // Every definition of its names in the library is a copy of
            // this group's kind, so whichever copy a link would have kept,
            // it could have kept the survivor.
            if self.unit(first).externals.iter().any(|n| {
                self.one_key.get(n.as_ref()).copied().flatten() != Some(self.unit(first).name_key)
            }) {
                why = Some(Unfoldable::OtherDefinitions);
            }
            // The survivor refers to no name a copy it stands for doesn't:
            // each member that gave a copy keeps the copy's references, so
            // a link that loads it needs nothing new, and loads no other
            // member.
            if why.is_none() && !all_same {
                // (Read with the members' objects.)
                let refs = |x: (usize, usize)| self.parsed[x.0].references[x.1];
                let r0 = refs(survivor);
                if c.iter().any(|&x| refs(x) != r0) {
                    why = Some(Unfoldable::DifferingReferences);
                }
            }
            if why.is_none() && self.definer_has_side_effects(first) {
                why = Some(Unfoldable::SideEffects);
            }
        }
        if let Some(w) = why {
            self.not_folded(c, w);
            return;
        }
        if !all_same {
            self.report.differing_groups += 1;
        }
        self.fold(survivor, c, false);
    }

    /// Folds `removed` (the copies that move, in member order) into the
    /// synthesized member for their members, holding `survivor`.
    fn fold(&mut self, survivor: (usize, usize), removed: &[(usize, usize)], renamed: bool) {
        self.report.groups += 1;
        let how = if renamed {
            Removal::Weak
        } else {
            Removal::Undefined
        };
        let identity = self.unit(survivor).identity;
        for &(m, u) in removed {
            self.removals[m].push((u, how));
            if self.parsed[m].facts.units[u].identity == identity {
                self.same[m].push(u);
            }
        }
        self.report.copies_removed += removed.len() as u64;
        self.report.removed_bytes += removed.iter().map(|&x| self.unit(x).size).sum::<u64>();
        let mut givers: Vec<usize> = removed.iter().map(|&(m, _)| m).collect();
        givers.dedup();
        // (And the PCH its CodeView names, if any: see debug::key.)
        let environment = self.parsed[survivor.0].facts.environment;
        let key = (
            environment,
            debug::key(&self.parsed[survivor.0]),
            givers.clone(),
        );
        let synths = &mut self.synths;
        let s = *self.synth_of.entry(key).or_insert_with(|| {
            synths.push(Synth {
                environment,
                givers,
                units: Vec::new(),
                copies: Vec::new(),
            });
            synths.len() - 1
        });
        self.synths[s].units.push((survivor.0, survivor.1, renamed));
    }
}

/// The local units of a member (`facts`) that go with the units it gives
/// up whose copies are the survivors' (`same`): each local unit those
/// reach that nothing else refers to (nothing that stays, and no local
/// unit that stays), and that can be copied.
fn absorb(facts: &units::Facts, same: &[usize]) -> Vec<usize> {
    if same.is_empty() {
        return Vec::new();
    }
    let n = facts.units.len();
    let mut removed = vec![false; n];
    for &u in same {
        removed[u] = true;
    }
    let mut going = vec![false; n];
    for &u in same {
        for node in facts.units[u].closure().iter().skip(1) {
            if let locals::Node::Unit(v) = *node {
                let v = v as usize;
                let unit = &facts.units[v];
                if unit.pure_local() && !unit.pinned() && unit.blocked().is_none() {
                    going[v] = true;
                }
            }
        }
    }
    loop {
        let mut changed = false;
        for v in 0..n {
            if going[v]
                && facts.units[v]
                    .referrers()
                    .iter()
                    .any(|&r| !removed[r as usize] && !going[r as usize])
            {
                going[v] = false;
                changed = true;
            }
        }
        if !changed {
            break;
        }
    }
    (0..n).filter(|&v| going[v]).collect()
}

/// The locals a synthesized member holds copies of: what its units reach,
/// each once for each member it comes from (`copy`), or each once by its
/// contents (`merge`), in the order its units reach them.
fn local_copies(s: &Synth, parsed: &[Parsed<'_>], mode: Locals) -> Vec<LocalCopy> {
    let mut out: Vec<LocalCopy> = Vec::new();
    let mut seen: HashMap<(u8, u128), ()> = HashMap::default();
    for &(m, u, _) in &s.units {
        let facts = &parsed[m].facts;
        for &node in facts.units[u].closure().iter().skip(1) {
            let key = match (mode, node) {
                (Locals::Merge, locals::Node::Unit(v)) => (1, facts.units[v as usize].structural()),
                (Locals::Merge, locals::Node::Piece(p)) => (2, facts.pieces[p as usize].closure),
                (_, locals::Node::Unit(v)) => (3, (m as u128) << 64 | u128::from(v)),
                (_, locals::Node::Piece(p)) => (4, (m as u128) << 64 | u128::from(p)),
            };
            if seen.insert(key, ()).is_none() {
                out.push(LocalCopy { member: m, node });
            }
        }
    }
    out
}

/// Names whose definitions in the library differ (`--report odr`): units
/// defining the same external names whose code differs (their locals by
/// content), in order of first appearance.
fn odr(inputs: &[Input<'_>], parsed: &[Parsed<'_>]) -> Vec<OdrFinding> {
    let mut of: HashMap<u128, usize> = HashMap::default();
    let mut found: Vec<Vec<(usize, usize)>> = Vec::new();
    for (m, p) in parsed.iter().enumerate() {
        for (u, unit) in p.facts.units.iter().enumerate() {
            if unit.externals.is_empty() {
                continue;
            }
            let mut names: Vec<&[u8]> = unit.externals.iter().map(AsRef::as_ref).collect();
            names.sort_unstable();
            let mut h = H128::new();
            for n in names {
                h.u64(n.len() as u64);
                h.bytes(n);
            }
            let g = *of.entry(h.finish()).or_insert_with(|| {
                found.push(Vec::new());
                found.len() - 1
            });
            found[g].push((m, u));
        }
    }
    let mut out = Vec::new();
    for copies in found {
        let unit = |(m, u): (usize, usize)| &parsed[m].facts.units[u];
        let first = unit(copies[0]).structural();
        if copies.iter().all(|&x| unit(x).structural() == first) {
            continue;
        }
        let mut variants: Vec<OdrVariant> = Vec::new();
        for &(m, u) in &copies {
            let x = unit((m, u));
            let name = inputs[m].name.to_vec();
            match variants.iter_mut().find(|v| v.code == x.structural()) {
                Some(v) => {
                    if v.members.last() != Some(&name) {
                        v.members.push(name);
                    }
                    if !v.environments.contains(&parsed[m].facts.environment) {
                        v.environments.push(parsed[m].facts.environment);
                    }
                }
                None => variants.push(OdrVariant {
                    code: x.structural(),
                    size: x.size,
                    selection: x.selection,
                    members: vec![name],
                    environments: vec![parsed[m].facts.environment],
                }),
            }
        }
        // Whether one environment has two of them (not merely other
        // compiler options in other members).
        let mut envs: Vec<u128> = variants
            .iter()
            .flat_map(|v| v.environments.iter().copied())
            .collect();
        let total = envs.len();
        envs.sort_unstable();
        envs.dedup();
        out.push(OdrFinding {
            names: unit(copies[0])
                .externals
                .iter()
                .map(|n| n.to_vec())
                .collect(),
            same_environment: envs.len() < total,
            variants,
        });
    }
    out
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

/// (Tests) [`rewrite`], with each member's bytes gathered, holes and all,
/// so that tests read the members as files.
#[cfg(test)]
pub(crate) fn gathered<'a>(inputs: &[Input<'a>], options: Options) -> Output<'a> {
    let mut out = rewrite(inputs, options);
    for m in &mut out.members {
        if !m.holes.is_empty() {
            m.data = Cow::Owned(m.file().into_owned());
            m.holes.clear();
            m.symbol_table = None;
        }
    }
    out
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
/// (LNK1227), and both linkers require it resolved. It is `__ImageBase`
/// (`___ImageBase` for x86, whose C names are decorated), which both
/// linkers define themselves in every image, so that no member defines it:
/// `lib.exe` warns LNK4006 for a name two members define (an error under
/// `/WX`), which the absolute definition each synthesized member had
/// ([`LEGACY_ALIAS_DEFAULT`]) made it do whenever it combined folded
/// libraries. No relocation names the default, so nothing refers to the
/// image base that didn't.
pub fn alias_default(machine: u16) -> &'static [u8] {
    const IMAGE_FILE_MACHINE_I386: u16 = 0x014C;
    if machine == IMAGE_FILE_MACHINE_I386 {
        b"___ImageBase"
    } else {
        b"__ImageBase"
    }
}

/// The aliases' default as turbolib wrote it before [`alias_default`]: a
/// name every synthesized member defined, as the absolute 0. A library
/// folded then and folded again comes out in the new form: each record of
/// the name, an alias's default or a definition, becomes an undefined
/// reference to [`alias_default`] ([`renew_alias_default`]), so that one
/// library never has aliases of one name with two defaults (LNK1227).
pub const LEGACY_ALIAS_DEFAULT: &[u8] = b"__turbolib$alias";

/// Turns each external record of [`LEGACY_ALIAS_DEFAULT`] in `object` (an
/// alias's default, or a synthesized member's absolute definition) into an
/// undefined [`alias_default`], in place, so that no symbol table index
/// moves. Whether there was one.
fn renew_alias_default(object: &mut Object<'_>) -> bool {
    let new = alias_default(object.machine);
    let mut changed = false;
    for s in &mut object.symbols {
        if s.is_external() && s.name.as_bytes() == LEGACY_ALIAS_DEFAULT {
            s.name = Name::new(new.to_vec());
            s.section = SectionNumber::Undefined;
            s.value = 0;
            s.kind = 0;
            s.aux = Aux::None;
            changed = true;
        }
    }
    changed
}

/// Whether a member may hold [`LEGACY_ALIAS_DEFAULT`] (a quick look at its
/// bytes, for the name, which only turbolib writes).
fn may_have_legacy_alias_default(data: &[u8]) -> bool {
    memchr::memmem::find(data, LEGACY_ALIAS_DEFAULT).is_some()
}

/// A member (its `object`, as read, edited in place) with the copies it
/// gives up removed.
fn strip<'a>(
    mut object: Object<'a>,
    parsed: &Parsed<'a>,
    removals: &[(usize, Removal)],
    anchors: &[&[u8]],
    edit: Option<&precomp::Edit>,
) -> Skeleton<'a> {
    let as_read = AsRead::of(&object);
    let facts = &parsed.facts;
    let mut removed = vec![false; object.sections.len()];
    for &(u, _) in removals {
        for s in &facts.units[u].sections {
            removed[s.index()] = true;
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
    // The side tables, from the symbols as read.
    let fields = tables::strip(&mut object, &facts.tables, &removed);
    // The symbols that become weak externals, and their defaults' names
    // and search: made after the edit, since an auxiliary record moves the
    // symbol table indices the side tables hold.
    let mut weak: Vec<(SymbolId, Vec<u8>, u32)> = Vec::new();
    // (And the aliases a library folded before has, in the new form.)
    renew_alias_default(&mut object);
    let default = alias_default(object.machine);
    for &(u, how) in removals {
        let unit = &facts.units[u];
        for &id in &unit.symbols {
            if !object.symbols[id.index()].is_external() {
                continue;
            }
            let name = object.symbols[id.index()].name.as_bytes();
            // A moved copy's name stays defined here, as an alias (see
            // `alias_default`).
            if how == Removal::Weak {
                weak.push((id, renamed(name), weak_search::NOLIBRARY));
            } else {
                weak.push((id, default.to_vec(), weak_search::ALIAS));
            }
            let s = &mut object.symbols[id.index()];
            s.section = SectionNumber::Undefined;
            s.value = 0;
            s.aux = Aux::None;
        }
    }
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
    // The anchors come before every external and weak external record (see
    // `anchors_first`).
    let first = anchors_first(&object);
    // All at once: inserting each renumbers every reference in the object
    // (on clangSema.lib, with hundreds of anchors in a member, rewriting
    // took 11 to 15 s instead of 0.15 s).
    let new: Vec<Symbol<'_>> = anchors
        .iter()
        .map(|a| {
            Symbol::new(
                Name::new(a.to_vec()),
                0,
                SectionNumber::Undefined,
                0,
                storage_class::EXTERNAL,
            )
        })
        .collect();
    object.insert_symbols(SymbolId(first as u32), new);
    let now = object.symbol_table_indices();
    tables::renumber(&mut object, &fields, &renumbering, &|i| {
        let id = before
            .binary_search(&i)
            .expect("a table's entry names a symbol's primary record");
        now[if id >= first { id + anchors.len() } else { id }]
    });
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
    checksums(&as_read, &mut object, &renumbering);
    object
        .write_skeleton(HOLE)
        .expect("a rewritten member fits its format")
}

/// The rewritten members (in member order, each as [`Rewritten`]) and the
/// synthesized members (in their order), made one member's object at a
/// time (see [`Objects`]): each member's work is the synthesized members
/// whose units are its own (in chunks, so that a member holding many isn't
/// one thread's work) and its rewriting (if `rewrites` says so), and its
/// object is read again for them and let go when the last is done. Its
/// rewriting comes last, and edits the object in place when no other work
/// holds it (it is copied otherwise). A synthesized member whose units come
/// from several members gets those members' objects read on its own.
fn one_at_a_time_members<'a>(
    inputs: &[Input<'a>],
    parsed: &[Parsed<'a>],
    rewrites: &[bool],
    synths: &[Synth],
    rewrite: &(dyn Fn(usize, Object<'a>) -> Rewritten<'a> + Sync),
    synthesized: &(dyn Fn(usize, Objects<'_, 'a>) -> OutputMember<'a> + Sync),
) -> (Vec<Rewritten<'a>>, Vec<OutputMember<'a>>) {
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::{Arc, Mutex};
    /// The units' bytes a chunk of synthesized members holds, at least.
    const CHUNK: u64 = 4 << 20;
    let read = |m: usize| match File::parse(inputs[m].data) {
        Ok(File::Object(o)) => o,
        _ => unreachable!("a member read as an object before"),
    };
    // The work: (member, Some(synthesized members)) or (member, None) for
    // its rewriting, with its cost.
    let mut home: Vec<Vec<usize>> = vec![Vec::new(); parsed.len()];
    let mut several: Vec<usize> = Vec::new();
    for (n, s) in synths.iter().enumerate() {
        let m0 = s.units[0].0;
        if s.units.iter().all(|&(m, _, _)| m == m0) {
            home[m0].push(n);
        } else {
            several.push(n);
        }
    }
    let mut work: Vec<(usize, Option<Vec<usize>>, u64)> = Vec::new();
    for m in 0..parsed.len() {
        let mut chunk = Vec::new();
        let mut bytes = 0;
        for &n in &home[m] {
            chunk.push(n);
            bytes += synths[n]
                .units
                .iter()
                .map(|&(m, u, _)| parsed[m].facts.units[u].size)
                .sum::<u64>();
            if bytes >= CHUNK {
                work.push((m, Some(std::mem::take(&mut chunk)), bytes));
                bytes = 0;
            }
        }
        if !chunk.is_empty() {
            work.push((m, Some(chunk), bytes));
        }
        if rewrites[m] {
            work.push((m, None, inputs[m].data.len() as u64));
        }
    }
    // Each member's object while its work is being done, and how much of
    // its work is left.
    let objects: Vec<Mutex<Option<Arc<Object<'a>>>>> =
        (0..parsed.len()).map(|_| Mutex::new(None)).collect();
    let left: Vec<AtomicUsize> = (0..parsed.len()).map(|_| AtomicUsize::new(0)).collect();
    let mut jobs = vec![0usize; parsed.len()];
    for (m, _, _) in &work {
        left[*m].fetch_add(1, Ordering::Relaxed);
        jobs[*m] += 1;
    }
    let lock = |m: usize| {
        objects[m]
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    };
    // The member's object, shared with its other work, and whether this
    // was its last work to start (which lets the shared object go).
    let shared = |m: usize| {
        let object = lock(m).get_or_insert_with(|| Arc::new(read(m))).clone();
        let last = left[m].fetch_sub(1, Ordering::AcqRel) == 1;
        if last {
            lock(m).take();
        }
        object
    };
    // A member's work all at once, the dearest members' first (and within
    // a member, in order: its rewriting last): its object is read once,
    // and held no longer than it takes.
    let mut member_cost = vec![0u64; parsed.len()];
    for (m, _, c) in &work {
        member_cost[*m] += c;
    }
    enum Done<'a> {
        Member(Rewritten<'a>),
        Made(Vec<OutputMember<'a>>),
    }
    let done = turbonop_par::map_costed(
        work.len(),
        |i| member_cost[work[i].0],
        |i| {
            let (m, ref job, _) = work[i];
            match job {
                // Its only work: an object of its own, edited in place.
                None if jobs[m] == 1 => Done::Member(rewrite(m, read(m))),
                None => {
                    let object = shared(m);
                    let object = Arc::try_unwrap(object).unwrap_or_else(|o| (*o).clone());
                    Done::Member(rewrite(m, object))
                }
                Some(chunk) => {
                    let object = shared(m);
                    let current = [(m, &*object)];
                    let objects = Objects {
                        parsed,
                        current: &current,
                    };
                    Done::Made(chunk.iter().map(|&n| synthesized(n, objects)).collect())
                }
            }
        },
    );
    let mut members: Vec<Option<Rewritten<'a>>> = (0..parsed.len()).map(|_| None).collect();
    let mut made: Vec<Option<OutputMember<'a>>> = (0..synths.len()).map(|_| None).collect();
    for ((m, job, _), out) in work.iter().zip(done) {
        match (job, out) {
            (Some(chunk), Done::Made(out)) => {
                for (&n, o) in chunk.iter().zip(out) {
                    made[n] = Some(o);
                }
            }
            (None, Done::Member(r)) => members[*m] = Some(r),
            _ => unreachable!("each job's own result"),
        }
    }
    // Those whose units are several members' (folding identical copies,
    // only those kept for side effects: each one's other survivors are the
    // first copies, all its first giver's), each with its members' objects.
    let rest = turbonop_par::map_balanced(&several, 1, |&n| {
        let mut held: Vec<(usize, Object<'a>)> = Vec::new();
        for &(m, _, _) in &synths[n].units {
            if !held.iter().any(|h| h.0 == m) {
                held.push((m, read(m)));
            }
        }
        let current: Vec<(usize, &Object<'a>)> = held.iter().map(|(m, o)| (*m, o)).collect();
        synthesized(
            n,
            Objects {
                parsed,
                current: &current,
            },
        )
    });
    for (&n, o) in several.iter().zip(rest) {
        made[n] = Some(o);
    }
    let members = members
        .into_iter()
        .enumerate()
        .map(|(m, o)| o.unwrap_or_else(|| (m, passed_through(&inputs[m]), None)))
        .collect();
    let made = made
        .into_iter()
        .map(|o| o.expect("every synthesized member made"))
        .collect();
    (members, made)
}

/// A member as it is.
fn passed_through<'a>(input: &Input<'a>) -> OutputMember<'a> {
    OutputMember {
        name: Cow::Borrowed(input.name),
        data: Cow::Borrowed(input.data),
        holes: Vec::new(),
        symbol_table: None,
        synthesized: false,
        index_only: None,
    }
}

/// Where a member that gave up copies gets its references to the anchors:
/// before its first external or weak external record, so after only the
/// static records (`.file`, `@comp.id`, `@feat.00`, section symbols) that
/// precede it.
///
/// This keeps link.exe's load order. A library sweep goes through the
/// undefined names in the order they became undefined, a loaded member's
/// in the order of its records (measured: a member's anchors were found in
/// its symbol table's order, and then the names of the members loaded after
/// it). With the anchors last, a sweep reached a member's moved names (still
/// undefined, the synthesized members not yet loaded) before its anchors,
/// and some sweeps ended early once an anchored member loaded: the members
/// they would have gone on to load came in a later pass, in another order,
/// and two of `llvm-nm`'s 55 C++ initializers moved. (Reproduced with five
/// of `LLVMObject.lib`'s members and one synthesized member; which records
/// end the sweep, and why, isn't known: hand-made objects of the same
/// shape didn't.) With the anchors first, the synthesized members load
/// before the sweep reaches the moved names, and `llvm-nm` loads its 637
/// members in the original order.
fn anchors_first(object: &Object<'_>) -> usize {
    object
        .symbols
        .iter()
        .position(|s| s.is_external() || s.storage_class == storage_class::WEAK_EXTERNAL)
        .unwrap_or(object.symbols.len())
}

/// What [`checksums`] needs of a member as read, before it is edited in
/// place: where each section's bytes were (a section that still has them
/// is as it was), and its `.chks64`'s bytes.
struct AsRead<'a> {
    sections: Vec<(*const u8, usize)>,
    checksums: Option<Cow<'a, [u8]>>,
}

impl<'a> AsRead<'a> {
    fn of(object: &Object<'a>) -> Self {
        AsRead {
            sections: object
                .sections
                .iter()
                .map(|s| (s.bytes().as_ptr(), s.bytes().len()))
                .collect(),
            checksums: object
                .sections
                .iter()
                .find(|s| s.name.as_bytes() == b".chks64")
                .map(|s| match s.data.borrowed() {
                    Some(b) => Cow::Borrowed(b),
                    None => Cow::Owned(s.bytes().to_vec()),
                }),
        }
    }
}

/// Rewrites a changed member's `.chks64` (one 8-byte checksum for each
/// section, in section order, as cl writes it): the checksums of the
/// sections that stay as they were (the same bytes as in `original`), and
/// 0 (as cl gives `.bss`) for those that changed. link.exe takes the
/// contributions' CRCs in its PDB from them (likely: without them, every
/// contribution of a changed member had none), and neither linker checks
/// them in a full link (phase 0).
fn checksums(original: &AsRead<'_>, object: &mut Object<'_>, renumbering: &Renumbering) {
    let Some(c) = object
        .sections
        .iter()
        .position(|s| s.name.as_bytes() == b".chks64")
    else {
        return;
    };
    let Some(old) = &original.checksums else {
        return;
    };
    let mut of_new = vec![None; object.sections.len()];
    for i in 0..original.sections.len() {
        if let Some(n) = renumbering.section(SectionId(i as u32)) {
            of_new[n.index()] = Some(i);
        }
    }
    let mut data = Vec::with_capacity(8 * object.sections.len());
    for (j, s) in object.sections.iter().enumerate() {
        let b = s.bytes();
        let entry = match of_new[j] {
            Some(i) if j != c && original.sections[i] == (b.as_ptr(), b.len()) => {
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
    objects: Objects<'_, 'a>,
    parsed: &[Parsed<'a>],
    plan: Option<&precomp::Plan>,
    anchor: &[u8],
    name: &[u8],
) -> Skeleton<'a> {
    let (m0, _, _) = s.units[0];
    let first = objects.get(m0);
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

    // What is placed as a unit: the folded units, then the copies of the
    // local units they reach (`--locals=copy`).
    let mut placements: Vec<(usize, usize, bool)> = s.units.clone();
    // Each copy's placement, or its piece's symbol here once made.
    let mut copy_at: Vec<usize> = vec![usize::MAX; s.copies.len()];
    for (i, c) in s.copies.iter().enumerate() {
        if let locals::Node::Unit(v) = c.node {
            copy_at[i] = placements.len();
            placements.push((c.member, v as usize, false));
        }
    }
    let folded = s.units.len();

    // The units' CodeView, renumbered into the member's own (phase 2.4);
    // its per-object .debug$S first, as compilers write it.
    let mut debug = debug::Builder::new(parsed, objects, plan);
    debug.begin(&mut o, &placements);

    // The units: their sections and symbols, with names renamed where the
    // copy is a renamed one. Relocations are filled in after, once every
    // unit's symbols are in.
    let mut placed: Vec<Placed> = Vec::with_capacity(placements.len());
    let mut defined: HashMap<Vec<u8>, SymbolId> = HashMap::default();
    // The local COMDAT symbols' names of the local units copied, which two
    // copies mustn't share.
    let mut local_comdats: HashMap<Vec<u8>, u32> = HashMap::default();
    for (k, &(m, u, renamed_copy)) in placements.iter().enumerate() {
        let object = objects.get(m);
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
        let mut leader_named = false;
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
            // A copied local unit's COMDAT symbol: another copy's name gets
            // a suffix, as one object can't have two COMDATs of one name.
            if k >= folded
                && !leader_named
                && old == unit.leader
                && !matches!(sym.aux, Aux::Section(_))
            {
                leader_named = true;
                let n = local_comdats
                    .entry(sym.name.as_bytes().to_vec())
                    .or_insert(0);
                if *n > 0 {
                    let mut v = sym.name.as_bytes().to_vec();
                    v.extend_from_slice(format!("$turbolib{n}").as_bytes());
                    copy.name = Name::new(v);
                }
                *n += 1;
            }
            let new_id = o.push_symbol(copy);
            if sym.is_external() {
                defined.insert(o.symbols[new_id.index()].name.as_bytes().to_vec(), new_id);
            }
            p.symbols.insert(id.0, new_id);
        }
        placed.push(p);
    }

    // The pieces copied: each a section of its own, with one symbol.
    let mut pieces: Vec<(usize, SectionId, SymbolId)> = Vec::new();
    for (i, c) in s.copies.iter().enumerate() {
        let locals::Node::Piece(p) = c.node else {
            continue;
        };
        let object = objects.get(c.member);
        let piece = &parsed[c.member].facts.pieces[p as usize];
        let source = &object.sections[piece.section.index()];
        let bytes = &source.bytes()[piece.start as usize..piece.end as usize];
        let id = o.push_section(Section::new(
            Name::new(source.name.as_bytes().to_vec()),
            source.characteristics,
            SectionData::Bytes(Cow::Owned(bytes.to_vec())),
        ));
        o.push_symbol(section_symbol(
            source.name.as_bytes(),
            id,
            SectionDefinition::default(),
        ));
        let label = &object.symbols[piece.labels[0].index()];
        let symbol = o.push_symbol(Symbol::new(
            label.name.clone(),
            0,
            SectionNumber::Section(id),
            label.kind,
            storage_class::STATIC,
        ));
        copy_at[i] = pieces.len();
        pieces.push((i, id, symbol));
    }
    let mut exact = HashMap::default();
    let mut by_contents = HashMap::default();
    for (i, c) in s.copies.iter().enumerate() {
        exact.insert((c.member, c.node), i);
        by_contents
            .entry(contents(parsed, c.member, c.node))
            .or_insert(i);
    }
    let copies = Copies {
        parsed,
        placements: &placements,
        copy_at: &copy_at,
        pieces: &pieces,
        exact,
        by_contents,
    };

    // Relocations: inside a unit, to its own symbols; to a local it
    // reaches, to the copy's; outside, by name, to a symbol defined here or
    // to an undefined external.
    let mut names = Names {
        defined,
        undefined: HashMap::default(),
        weak: HashMap::default(),
    };
    for (k, &(m, u, _)) in placements.iter().enumerate() {
        let object = objects.get(m);
        let unit = &parsed[m].facts.units[u];
        for &id in &unit.sections {
            let Some(&new) = placed[k].sections.get(&id.0) else {
                continue;
            };
            let mut relocations = Vec::with_capacity(object.sections[id.index()].relocations.len());
            for r in &object.sections[id.index()].relocations {
                let symbol = match placed[k].symbols.get(&r.symbol.0) {
                    Some(&inside) => inside,
                    None => match copies.local(m, unit.edges(), r.symbol, &placed) {
                        Some(copy) => copy,
                        None => names.outside(&mut o, object, &object.symbols[r.symbol.index()]),
                    },
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
    for &(i, id, _) in &pieces {
        let c = &s.copies[i];
        let locals::Node::Piece(p) = c.node else {
            continue;
        };
        let object = objects.get(c.member);
        let piece = &parsed[c.member].facts.pieces[p as usize];
        let source = &object.sections[piece.section.index()];
        let mut relocations = Vec::with_capacity(piece.relocations.len());
        for &ri in &piece.relocations {
            let r = &source.relocations[ri as usize];
            let symbol = match copies.local(c.member, &piece.edges, r.symbol, &placed) {
                Some(copy) => copy,
                None => names.outside(&mut o, object, &object.symbols[r.symbol.index()]),
            };
            relocations.push(Relocation {
                offset: r.offset - piece.start,
                symbol,
                kind: r.kind,
            });
        }
        o.sections[id.index()].relocations = relocations;
    }
    let table_fields = tables::place(
        &mut o,
        &placements,
        parsed,
        objects,
        &placed,
        &mut |o, k, old| {
            let (m, u, _) = placements[k];
            let object = objects.get(m);
            match copies.local(m, parsed[m].facts.units[u].edges(), old, &placed) {
                Some(copy) => copy,
                None => names.outside(o, object, &object.symbols[old.index()]),
            }
        },
    );
    debug.finish(&mut o, m0, name);
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
    tables::finish(&mut o, &table_fields);
    o.sync_section_definitions();
    o.write_skeleton(HOLE)
        .expect("a synthesized member fits its format")
}

/// Where a synthesized member's copies of locals went.
struct Copies<'c, 'p, 'a> {
    parsed: &'p [Parsed<'a>],
    /// What is placed as a unit (member, unit, renamed).
    placements: &'c [(usize, usize, bool)],
    /// Each copy's placement (a local unit) or position in `pieces`.
    copy_at: &'c [usize],
    /// The pieces made: (copy, section, symbol).
    pieces: &'c [(usize, SectionId, SymbolId)],
    /// Each copy by (member, node), and by contents.
    exact: HashMap<(usize, locals::Node), usize>,
    by_contents: HashMap<(u8, u128), usize>,
}

impl Copies<'_, '_, '_> {
    /// The symbol here for member `m`'s local `target`, which a placed
    /// unit or a piece copied reaches through `edges`, if it is one.
    fn local(
        &self,
        m: usize,
        edges: &[locals::Edge],
        target: SymbolId,
        placed: &[Placed],
    ) -> Option<SymbolId> {
        let e = edges.iter().find(|e| e.symbol == target)?;
        let i = self.find(m, e.node)?;
        match e.node {
            locals::Node::Unit(_) => {
                let k = self.copy_at[i];
                let (cm, cu, _) = self.placements[k];
                let source = *self.parsed[cm].facts.units[cu]
                    .symbols
                    .get(e.position as usize)?;
                placed[k].symbols.get(&source.0).copied()
            }
            locals::Node::Piece(_) => Some(self.pieces[self.copy_at[i]].2),
        }
    }

    /// The copy that stands for member `m`'s `node`: its own, or (under
    /// `merge`) one with the same contents.
    fn find(&self, m: usize, node: locals::Node) -> Option<usize> {
        if let Some(&i) = self.exact.get(&(m, node)) {
            return Some(i);
        }
        self.by_contents
            .get(&contents(self.parsed, m, node))
            .copied()
    }
}

/// What `merge` makes one copy of: a local's kind and contents.
fn contents(parsed: &[Parsed<'_>], m: usize, node: locals::Node) -> (u8, u128) {
    let f = &parsed[m].facts;
    match node {
        locals::Node::Unit(v) => (1, f.units[v as usize].structural()),
        locals::Node::Piece(p) => (2, f.pieces[p as usize].closure),
    }
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
        // (A library folded before: its aliases' default, in the new form.)
        let default = if default == LEGACY_ALIAS_DEFAULT {
            alias_default(object.machine)
        } else {
            default
        };
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
mod loose_tests;
#[cfg(test)]
mod tests;
