//! `--locals=copy` and `merge`: units that reach internal-linkage symbols
//! outside them, and what they reach.
//!
//! A unit that refers to a local symbol outside it can't be moved alone:
//! another object can't name that symbol. Under `--locals=copy`, it is
//! moved with a copy of everything local it reaches, transitively, which
//! the synthesized member holds as its own locals. Two kinds of thing are
//! reached:
//!
//! - **Local units**: a COMDAT whose symbols are all local (a `static`
//!   inline function, an anonymous namespace's), copied whole with its
//!   associated sections (`.pdata`, `.xdata`, `.debug$S`). Its original
//!   stays in the member unless only units that move refer to it (it is
//!   then removed too, as a moved unit is: rewrite's "absorbed").
//! - **Pieces** of a section outside every unit: the bytes from a local
//!   symbol to the next symbol of its section (or the section's end), with
//!   their relocations. These are `$SG` string literals and constant
//!   tables with internal linkage. The original always stays.
//!
//! Only what nobody can tell apart from the original is copied: code and
//! read-only data, where a copy differs only in its address (comparing the
//! addresses of a translation unit's internal-linkage entities with
//! another's is already meaningless, and C++ calls an inline function that
//! names them in two translation units an ODR violation). Never:
//!
//! - writable data ([`Unfoldable::WritableLocal`]): a copy would split one
//!   variable in two;
//! - registration sections (`.CRT$*`, `.rtc$*`, `.tls*`), whose entries
//!   run once each;
//! - anything a side table names, a section symbol (a reference to a place
//!   in a section with no symbol of its own), or code outside COMDATs
//!   (whose symbols don't bound functions);
//! - a reference whose addend (x64 and x86 relocations, read in place)
//!   points outside the piece, or a relocation of another machine;
//! - a local symbol of a unit that has external symbols too (copying it
//!   would define them twice);
//! - a unit whose debug information refers to a local outside it.
//!
//! A piece's contents are its bytes without the zeros at its end (or after
//! its last relocation): those are the alignment padding before the next
//! symbol, which depends on where the compiler put the piece. Its copy has
//! the bytes it has. (So `{1, 2, 0}` and `{1, 2}` are the same; an inline
//! function that names either in two translation units is the same code
//! only if they are the same object, as C++'s ODR requires.)
//!
//! A unit's identity is then structural: its code, with each local it
//! reaches named by its place in the order its code names them (see
//! [`PLACE`]), followed by everything it reaches, numbered in the order a
//! breadth-first walk finds them. Two copies have the same identity when
//! they are the same but for which translation unit's locals they name:
//! the name of `$SG12345` and its place in the section don't count, its
//! bytes do.

use super::Unfoldable;
use super::units::{Facts, Unit, is_debug, weak_default};
use crate::hash::H128;
use turbonop_coff::{Aux, Object, SectionId, SectionNumber, SymbolId, storage_class};
use turbonop_hash::Map as HashMap;

/// A relocation that refers to a local: its section, offset, and type.
type Via = (usize, u32, u16);

/// The tag of a local's place in a unit's code hash, or'd with its place.
pub const PLACE: u64 = 0x4C4F_4341_4C00_0000;

/// Something local a unit can reach.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Node {
    /// A unit of the member (a local one, or the unit itself).
    Unit(u32),
    /// A piece ([`Facts::pieces`]).
    Piece(u32),
}

/// A reference to a local symbol outside the referrer: the symbol, what
/// holds it, and where in that: its position among a unit's symbols, or 0
/// for a piece (whose symbols are all at its start).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Edge {
    pub symbol: SymbolId,
    pub node: Node,
    pub position: u32,
}

/// The place of `symbol` among `edges`, if it is one of them.
pub fn place(edges: &[Edge], symbol: SymbolId) -> Option<u64> {
    edges
        .iter()
        .position(|e| e.symbol == symbol)
        .map(|p| p as u64)
}

/// A piece of a section outside every unit.
#[derive(Clone, Debug)]
pub struct Piece {
    pub section: SectionId,
    pub start: u32,
    pub end: u32,
    /// The symbols at its start, in symbol order; the first names the
    /// copy.
    pub labels: Vec<SymbolId>,
    /// Its relocations: their positions in the section's, in order.
    pub relocations: Vec<u32>,
    /// The locals its relocations reach, in their order.
    pub edges: Vec<Edge>,
    /// Its contents, with the locals it reaches by place.
    pub code: u128,
    /// Its contents with what it reaches: equal for pieces `merge` makes
    /// one.
    pub closure: u128,
    /// Why it can't be copied, if it can't.
    pub blocked: Option<(Unfoldable, String)>,
}

/// What a member's units reach (see [`analyze`]).
pub(super) struct Analysis {
    /// Each unit's edges, in the order its code names them.
    pub edges: Vec<Vec<Edge>>,
    /// Each unit is a local unit: all of its symbols are local.
    pub pure: Vec<bool>,
    /// Each local unit is referred to by something that never moves.
    pub pinned: Vec<bool>,
    /// Each local unit's referrers among the units.
    pub referrers: Vec<Vec<u32>>,
    /// What about internal linkage keeps each unit from folding (or, for a
    /// local unit, from being copied), beyond what it reaches.
    pub hard: Vec<Option<(Unfoldable, String)>>,
    pub pieces: Vec<Piece>,
}

const IMAGE_SCN_CNT_CODE: u32 = 0x0000_0020;
const IMAGE_SCN_CNT_INITIALIZED_DATA: u32 = 0x0000_0040;
const IMAGE_SCN_CNT_UNINITIALIZED_DATA: u32 = 0x0000_0080;
const IMAGE_SCN_LNK_INFO: u32 = 0x0000_0200;
const IMAGE_SCN_LNK_REMOVE: u32 = 0x0000_0800;
const IMAGE_SCN_LNK_COMDAT: u32 = 0x0000_1000;
const IMAGE_SCN_MEM_WRITE: u32 = 0x8000_0000;

const AMD64: u16 = 0x8664;
const I386: u16 = 0x014C;

/// Sections whose entries each run, or are each a thing: never copied.
const REGISTRATIONS: &[&[u8]] = &[b".CRT$", b".rtc$", b".tls", b".00cfg"];

fn show(b: &[u8]) -> String {
    String::from_utf8_lossy(b).into_owned()
}

/// The width and in-place addend of a relocation of `kind` at `at` in
/// `data`, for the machines whose relocations are read: x64 and x86.
fn addend(machine: u16, kind: u16, data: &[u8], at: usize) -> Option<(usize, i64)> {
    let width = match (machine, kind) {
        // ADDR64.
        (AMD64, 1) => 8,
        // ADDR32, ADDR32NB, REL32 to REL32_5, SECREL.
        (AMD64, 2..=9 | 0xB) => 4,
        // DIR32, DIR32NB, SECREL, REL32.
        (I386, 6 | 7 | 0xB | 0x14) => 4,
        _ => return None,
    };
    let b = data.get(at..at + width)?;
    let v = if width == 8 {
        i64::from_le_bytes(b.try_into().ok()?)
    } else {
        i64::from(i32::from_le_bytes(b.try_into().ok()?))
    };
    Some((width, v))
}

/// Finds what each unit of `object` reaches of its locals (see the
/// module's notes). `units` are its units, `local_leader` says which have
/// a local COMDAT symbol, and `unit_of` and `symbol_unit` give each
/// section's and symbol's unit.
pub(super) fn analyze(
    object: &Object<'_>,
    units: &[Unit],
    local_leader: &[bool],
    unit_of: &[Option<usize>],
    symbol_unit: &[Option<usize>],
    tables: &super::tables::Tables,
) -> Analysis {
    let n = units.len();
    let mut a = Analysis {
        edges: vec![Vec::new(); n],
        pure: (0..n)
            .map(|k| local_leader[k] && units[k].externals.is_empty())
            .collect(),
        pinned: vec![false; n],
        referrers: vec![Vec::new(); n],
        hard: vec![None; n],
        pieces: Vec::new(),
    };
    let set = |hard: &mut Vec<Option<(Unfoldable, String)>>, k: usize, why, what: String| {
        hard[k].get_or_insert((why, what));
    };
    let is_local = |s: SymbolId| {
        object
            .symbol(s)
            .is_some_and(|t| !t.is_external() && t.storage_class != storage_class::WEAK_EXTERNAL)
    };
    let name_of = |s: SymbolId| {
        object
            .symbol(s)
            .map_or_else(String::new, |t| show(t.name.as_bytes()))
    };

    // Each local unit: can it be copied at all?
    for k in 0..n {
        if local_leader[k] && !a.pure[k] {
            set(
                &mut a.hard,
                k,
                Unfoldable::InternalLinkage,
                "a local COMDAT that defines external symbols too".into(),
            );
        }
        if !a.pure[k] {
            continue;
        }
        for &s in &units[k].sections {
            let sec = &object.sections[s.index()];
            let name = sec.name.as_bytes();
            if is_debug(name) {
                continue;
            }
            if sec.characteristics & IMAGE_SCN_MEM_WRITE != 0 {
                set(
                    &mut a.hard,
                    k,
                    Unfoldable::WritableLocal,
                    format!("{} is writable", show(name)),
                );
            } else if REGISTRATIONS.iter().any(|p| name.starts_with(p)) {
                set(
                    &mut a.hard,
                    k,
                    Unfoldable::LocalReference,
                    format!("{} is a registration section", show(name)),
                );
            }
        }
    }

    // Who refers to each unit's locals from outside it.
    let refer = |a: &mut Analysis, from: Option<usize>, never_moves: bool, target: SymbolId| {
        let Some(b) = symbol_unit.get(target.index()).copied().flatten() else {
            return;
        };
        if from == Some(b) || !is_local(target) {
            return;
        }
        if !a.pure[b] {
            set(
                &mut a.hard,
                b,
                Unfoldable::ReferencedLocally,
                format!(
                    "its local {} is referred to from outside it",
                    name_of(target)
                ),
            );
        } else if never_moves || from.is_none() {
            a.pinned[b] = true;
        } else if let Some(f) = from
            && !a.referrers[b].contains(&(f as u32))
        {
            a.referrers[b].push(f as u32);
        }
    };
    for (i, sec) in object.sections.iter().enumerate() {
        let here = unit_of[i];
        let id = SectionId(i as u32);
        if let Some(t) = tables.get(id) {
            for e in &t.entries {
                if let Some(super::tables::Target::Symbol(s)) = e.target {
                    // A table outside COMDATs goes with what it names; one
                    // in a COMDAT that isn't a unit's is the table of code
                    // that stays.
                    if here.is_some() || t.comdat {
                        refer(&mut a, here, here.is_none(), s);
                    }
                }
            }
        }
        if tables.splits_relocations(id) {
            continue;
        }
        for r in &sec.relocations {
            refer(&mut a, here, false, r.symbol);
        }
    }
    for (i, s) in object.symbols.iter().enumerate() {
        let named = match &s.aux {
            Aux::WeakExternal(w) => Some(w.tag),
            Aux::Function(f) => f.tag,
            Aux::ClrToken(c) => Some(c.symbol),
            _ => None,
        };
        if let Some(t) = named {
            refer(&mut a, symbol_unit[i], true, t);
        }
    }

    // Each unit's edges, in the order `code` names them.
    let mut finder = Pieces::new(object, unit_of, tables);
    for k in 0..n {
        let unit = &units[k];
        let inside = |s: SymbolId| symbol_unit.get(s.index()).copied().flatten() == Some(k);
        let mut edges: Vec<Edge> = Vec::new();
        let mut why: Option<(Unfoldable, String)> = None;
        for &s in &unit.sections {
            let sec = &object.sections[s.index()];
            let name = sec.name.as_bytes();
            if is_debug(name) {
                for r in &sec.relocations {
                    if !inside(r.symbol) && is_local(r.symbol) && why.is_none() {
                        why = Some((
                            Unfoldable::LocalDebug,
                            format!("{} refers to {}", show(name), name_of(r.symbol)),
                        ));
                    }
                }
                continue;
            }
            let mut targets: Vec<(SymbolId, Option<Via>)> = Vec::new();
            if let Some(t) = tables.get(s) {
                for e in &t.entries {
                    if let Some(super::tables::Target::Symbol(x)) = e.target
                        && !inside(x)
                        && is_local(x)
                    {
                        if t.kind.is_set() {
                            targets.push((x, None));
                        } else if why.is_none() {
                            why = Some((
                                Unfoldable::LocalReference,
                                format!("{} describes {}", show(name), name_of(x)),
                            ));
                        }
                    }
                }
            }
            for r in &sec.relocations {
                if !inside(r.symbol) && is_local(r.symbol) {
                    targets.push((r.symbol, Some((s.index(), r.offset, r.kind))));
                }
            }
            for (x, via) in targets {
                if edges.iter().any(|e| e.symbol == x) {
                    if let Some(via) = via
                        && let Some(&Edge {
                            node: Node::Piece(p),
                            ..
                        }) = edges.iter().find(|e| e.symbol == x)
                        && let Err(w) = finder.check_addend(p, via)
                    {
                        why.get_or_insert(w);
                    }
                    continue;
                }
                match symbol_unit.get(x.index()).copied().flatten() {
                    Some(b) if a.pure[b] => {
                        let position = units[b].symbols.binary_search(&x).unwrap_or(0) as u32;
                        edges.push(Edge {
                            symbol: x,
                            node: Node::Unit(b as u32),
                            position,
                        });
                    }
                    Some(_) => {
                        why.get_or_insert((
                            Unfoldable::LocalReference,
                            format!(
                                "refers to {}, a local of a unit with external symbols",
                                name_of(x)
                            ),
                        ));
                    }
                    None => match finder.piece(x, via) {
                        Ok(p) => edges.push(Edge {
                            symbol: x,
                            node: Node::Piece(p),
                            position: 0,
                        }),
                        Err(w) => {
                            why.get_or_insert(w);
                        }
                    },
                }
            }
        }
        a.edges[k] = edges;
        if let Some(w) = why {
            set(&mut a.hard, k, w.0, w.1);
        }
    }

    // The pieces' own edges, as each is found.
    let mut next = 0;
    while next < finder.pieces.len() {
        let p = next;
        next += 1;
        let (section, start, end) = {
            let x = &finder.pieces[p];
            (x.section, x.start, x.end)
        };
        let sec = &object.sections[section.index()];
        let mut edges: Vec<Edge> = Vec::new();
        let mut blocked = None;
        let mut list = Vec::new();
        for (ri, r) in sec.relocations.iter().enumerate() {
            if r.offset < start || r.offset >= end {
                continue;
            }
            list.push(ri as u32);
            let Some((width, _)) = addend(object.machine, r.kind, sec.bytes(), r.offset as usize)
            else {
                blocked.get_or_insert((
                    Unfoldable::LocalReference,
                    format!(
                        "a relocation of type {:#x} in {}",
                        r.kind,
                        show(sec.name.as_bytes())
                    ),
                ));
                continue;
            };
            if r.offset as usize + width > end as usize {
                blocked.get_or_insert((
                    Unfoldable::LocalReference,
                    format!(
                        "a relocation across the end of {}",
                        name_of(finder.pieces[p].labels[0])
                    ),
                ));
            }
            if !is_local(r.symbol) || edges.iter().any(|e| e.symbol == r.symbol) {
                continue;
            }
            match symbol_unit.get(r.symbol.index()).copied().flatten() {
                Some(b) if a.pure[b] => {
                    let position = units[b].symbols.binary_search(&r.symbol).unwrap_or(0) as u32;
                    edges.push(Edge {
                        symbol: r.symbol,
                        node: Node::Unit(b as u32),
                        position,
                    });
                }
                Some(_) => {
                    blocked.get_or_insert((
                        Unfoldable::LocalReference,
                        format!(
                            "refers to {}, a local of a unit with external symbols",
                            name_of(r.symbol)
                        ),
                    ));
                }
                None => match finder.piece(r.symbol, Some((section.index(), r.offset, r.kind))) {
                    Ok(q) => edges.push(Edge {
                        symbol: r.symbol,
                        node: Node::Piece(q),
                        position: 0,
                    }),
                    Err(w) => {
                        blocked.get_or_insert(w);
                    }
                },
            }
        }
        // Its contents, with the locals it reaches by place, and without
        // the zeros after its last byte that isn't one (or its last
        // relocation): the alignment padding before the next symbol, which
        // depends on where the compiler put it.
        let bytes = &sec.bytes()[start as usize..end as usize];
        let mut used = bytes.iter().rposition(|&b| b != 0).map_or(0, |p| p + 1);
        for &ri in &list {
            let r = &sec.relocations[ri as usize];
            let width = addend(object.machine, r.kind, sec.bytes(), r.offset as usize)
                .map_or(4, |(w, _)| w);
            used = used
                .max((r.offset - start) as usize + width)
                .min(bytes.len());
        }
        let mut h = H128::new();
        let name = sec.name.as_bytes();
        h.u64(name.len() as u64);
        h.bytes(name);
        h.u64(u64::from(sec.characteristics));
        h.u64(used as u64);
        h.bytes(&bytes[..used]);
        h.u64(list.len() as u64);
        for &ri in &list {
            let r = &sec.relocations[ri as usize];
            h.u64(u64::from(r.offset - start) | u64::from(r.kind) << 32);
            match (place(&edges, r.symbol), object.symbol(r.symbol)) {
                (Some(pl), _) => h.u64(PLACE | pl),
                (None, Some(t)) => {
                    let name = t.name.as_bytes();
                    h.u64(u64::MAX - name.len() as u64);
                    h.bytes(name);
                    if let Some((search, default)) = weak_default(object, t) {
                        h.u64(u64::from(search) | 1 << 40);
                        h.u64(default.len() as u64);
                        h.bytes(default);
                    }
                }
                (None, None) => h.u64(u64::MAX),
            }
        }
        let x = &mut finder.pieces[p];
        x.relocations = list;
        x.edges = edges;
        x.code = h.finish();
        if x.blocked.is_none() {
            x.blocked = blocked;
        }
    }
    a.pieces = finder.pieces;
    a
}

/// Finds pieces as units reach them.
struct Pieces<'o, 'd> {
    object: &'o Object<'d>,
    unit_of: &'o [Option<usize>],
    /// Each section's symbols' values, sorted (made as needed).
    bounds: HashMap<u32, Vec<u32>>,
    /// The symbols any side table names.
    named: Vec<bool>,
    by_start: HashMap<(u32, u32), u32>,
    pieces: Vec<Piece>,
}

impl<'o, 'd> Pieces<'o, 'd> {
    fn new(
        object: &'o Object<'d>,
        unit_of: &'o [Option<usize>],
        tables: &super::tables::Tables,
    ) -> Self {
        let mut named = vec![false; object.symbols.len()];
        for t in &tables.tables {
            for e in &t.entries {
                if let Some(super::tables::Target::Symbol(s)) = e.target
                    && let Some(x) = named.get_mut(s.index())
                {
                    *x = true;
                }
            }
        }
        Pieces {
            object,
            unit_of,
            bounds: HashMap::default(),
            named,
            by_start: HashMap::default(),
            pieces: Vec::new(),
        }
    }

    /// The piece that local `symbol` starts, referred to by the relocation
    /// `via` (section, offset, type), if it has one (a side table's entry
    /// has none).
    fn piece(&mut self, symbol: SymbolId, via: Option<Via>) -> Result<u32, (Unfoldable, String)> {
        let object = self.object;
        let t = &object.symbols[symbol.index()];
        let refuse = |what: String| Err((Unfoldable::LocalReference, what));
        let name = show(t.name.as_bytes());
        let SectionNumber::Section(x) = t.section else {
            return refuse(format!("refers to {name}, a local in no section"));
        };
        let Some(sec) = object.sections.get(x.index()) else {
            return refuse(format!("refers to {name}, in no section"));
        };
        if matches!(t.aux, Aux::Section(_)) {
            return refuse(format!("refers to a place in {name} by its section symbol"));
        }
        let sname = sec.name.as_bytes();
        let c = sec.characteristics;
        if c & IMAGE_SCN_MEM_WRITE != 0 || c & IMAGE_SCN_CNT_UNINITIALIZED_DATA != 0 {
            return Err((
                Unfoldable::WritableLocal,
                format!("refers to {name}, in {}, which is writable", show(sname)),
            ));
        }
        if c & IMAGE_SCN_CNT_CODE != 0
            || c & IMAGE_SCN_CNT_INITIALIZED_DATA == 0
            || c & (IMAGE_SCN_LNK_INFO | IMAGE_SCN_LNK_REMOVE | IMAGE_SCN_LNK_COMDAT) != 0
            || is_debug(sname)
            || super::tables::is_table(sname)
            || REGISTRATIONS.iter().any(|p| sname.starts_with(p))
            || !sec.line_numbers.is_empty()
            || self.unit_of.get(x.index()).copied().flatten().is_some()
        {
            return refuse(format!(
                "refers to {name}, in {}, which isn't read-only data outside COMDATs",
                show(sname)
            ));
        }
        if self.named.get(symbol.index()).copied().unwrap_or(false) {
            return refuse(format!("refers to {name}, which a side table names"));
        }
        let bounds = self.bounds.entry(x.0).or_insert_with(|| {
            let mut v: Vec<u32> = object
                .symbols
                .iter()
                .filter(|s| s.section == SectionNumber::Section(x))
                .map(|s| s.value)
                .collect();
            v.sort_unstable();
            v.dedup();
            v
        });
        let start = t.value;
        let size = sec.size();
        if start >= size {
            return refuse(format!("refers to {name}, past its section's end"));
        }
        let end = bounds
            .iter()
            .copied()
            .find(|&b| b > start)
            .map_or(size, |b| b.min(size));
        let p = match self.by_start.get(&(x.0, start)) {
            Some(&p) => p,
            None => {
                let mut labels: Vec<SymbolId> = object
                    .symbols
                    .iter()
                    .enumerate()
                    .filter(|(_, s)| {
                        s.section == SectionNumber::Section(x)
                            && s.value == start
                            && !matches!(s.aux, Aux::Section(_))
                    })
                    .map(|(i, _)| SymbolId(i as u32))
                    .collect();
                labels.sort_unstable();
                let blocked = labels
                    .iter()
                    .find(|l| self.named.get(l.index()).copied().unwrap_or(false))
                    .map(|_| {
                        (
                            Unfoldable::LocalReference,
                            format!("a side table names {name}"),
                        )
                    });
                let p = self.pieces.len() as u32;
                self.pieces.push(Piece {
                    section: x,
                    start,
                    end,
                    labels,
                    relocations: Vec::new(),
                    edges: Vec::new(),
                    code: 0,
                    closure: 0,
                    blocked,
                });
                self.by_start.insert((x.0, start), p);
                p
            }
        };
        if let Some(via) = via {
            self.check_addend(p, via)?;
        }
        Ok(p)
    }

    /// Whether the relocation `via` (section, offset, type) points into
    /// piece `p`: its addend (read in place) is within it.
    fn check_addend(&self, p: u32, via: (usize, u32, u16)) -> Result<(), (Unfoldable, String)> {
        let object = self.object;
        let (s, at, kind) = via;
        let piece = &self.pieces[p as usize];
        let label = object.symbols[piece.labels.first().map_or(0, |l| l.index())]
            .name
            .as_bytes();
        let Some((_, v)) = addend(
            object.machine,
            kind,
            object.sections[s].bytes(),
            at as usize,
        ) else {
            return Err((
                Unfoldable::LocalReference,
                format!(
                    "refers to {} by a relocation of type {kind:#x} (machine {:#x})",
                    show(label),
                    object.machine
                ),
            ));
        };
        if v < 0 || v > i64::from(piece.end - piece.start) {
            return Err((
                Unfoldable::LocalReference,
                format!("refers to {}{v:+}, outside it", show(label)),
            ));
        }
        Ok(())
    }
}

/// A unit's closure: its structural hash, what it reaches (itself first),
/// and why something it reaches can't be copied, if anything can't.
#[derive(Clone, Debug, Default)]
pub(super) struct Closure {
    pub hash: u128,
    pub nodes: Vec<Node>,
    pub blocked: Option<(Unfoldable, String)>,
}

/// What a closure's walk reads: each node's code, edges, and why it
/// can't be copied.
struct Graph<'g> {
    a: &'g Analysis,
    codes: &'g [u128],
    hard: &'g [Option<(Unfoldable, String)>],
}

impl Graph<'_> {
    fn code(&self, node: Node) -> u128 {
        match node {
            Node::Unit(u) => self.codes[u as usize],
            Node::Piece(p) => self.a.pieces[p as usize].code,
        }
    }

    fn edges(&self, node: Node) -> &[Edge] {
        match node {
            Node::Unit(u) => &self.a.edges[u as usize],
            Node::Piece(p) => &self.a.pieces[p as usize].edges,
        }
    }

    fn blocked(&self, node: Node) -> Option<(Unfoldable, String)> {
        match node {
            Node::Unit(u) => self.hard[u as usize]
                .clone()
                .or_else(|| self.a.hard[u as usize].clone()),
            Node::Piece(p) => self.a.pieces[p as usize].blocked.clone(),
        }
    }

    /// Walks what `root` reaches, breadth first, hashing each node's code
    /// and edges with its targets numbered in the order found.
    fn walk(&self, root: Node) -> Closure {
        let mut nodes = vec![root];
        let mut number: HashMap<Node, u32> = HashMap::default();
        number.insert(root, 0);
        let mut h = H128::new();
        let mut first_blocked = None;
        let mut i = 0;
        while i < nodes.len() {
            let node = nodes[i];
            if i > 0 && first_blocked.is_none() {
                first_blocked = self.blocked(node);
            }
            h.u64(match node {
                Node::Unit(_) => 1,
                Node::Piece(_) => 2,
            });
            h.u128(self.code(node));
            let list = self.edges(node);
            h.u64(list.len() as u64);
            for e in list {
                let j = *number.entry(e.node).or_insert_with(|| {
                    nodes.push(e.node);
                    (nodes.len() - 1) as u32
                });
                h.u64(u64::from(j) | u64::from(e.position) << 32);
            }
            i += 1;
        }
        Closure {
            hash: h.finish(),
            nodes,
            blocked: first_blocked,
        }
    }
}

/// Each unit's closure (see [`Closure`]), and each piece's
/// ([`Piece::closure`]), from the units' code hashes `codes` and what keeps
/// each unit from folding in any mode (`hard`).
pub(super) fn closures(
    a: &mut Analysis,
    codes: &[u128],
    hard: &[Option<(Unfoldable, String)>],
) -> Vec<Closure> {
    let pieces: Vec<u128> = {
        let g = Graph { a, codes, hard };
        (0..a.pieces.len())
            .map(|p| g.walk(Node::Piece(p as u32)).hash)
            .collect()
    };
    for (p, c) in pieces.into_iter().enumerate() {
        a.pieces[p].closure = c;
    }
    let g = Graph { a, codes, hard };
    (0..codes.len())
        .map(|k| {
            if a.edges[k].is_empty() {
                Closure {
                    hash: codes[k],
                    nodes: vec![Node::Unit(k as u32)],
                    blocked: None,
                }
            } else {
                g.walk(Node::Unit(k as u32))
            }
        })
        .collect()
}

/// A hash of the external names unit `k` of `object` and what it reaches
/// refer to (by relocations and side-table entries), each once: the names
/// a link must resolve when it keeps that copy.
pub(super) fn references(object: &Object<'_>, facts: &Facts, k: usize) -> u128 {
    let mut names: Vec<&[u8]> = Vec::new();
    let mut add = |s: SymbolId| {
        if let Some(t) = object.symbol(s)
            && (t.is_external() || t.storage_class == storage_class::WEAK_EXTERNAL)
        {
            names.push(t.name.as_bytes());
        }
    };
    let closure = &facts.units[k].closure;
    let nodes: Vec<Node> = if closure.is_empty() {
        vec![Node::Unit(k as u32)]
    } else {
        closure.clone()
    };
    for node in nodes {
        match node {
            Node::Unit(u) => {
                let unit = &facts.units[u as usize];
                let inside = |s: SymbolId| unit.symbols.binary_search(&s).is_ok();
                for &s in &unit.sections {
                    let sec = &object.sections[s.index()];
                    if is_debug(sec.name.as_bytes()) {
                        continue;
                    }
                    if let Some(t) = facts.tables.get(s) {
                        for e in &t.entries {
                            if let Some(super::tables::Target::Symbol(x)) = e.target
                                && !inside(x)
                            {
                                add(x);
                            }
                        }
                    }
                    for r in &sec.relocations {
                        if !inside(r.symbol) {
                            add(r.symbol);
                        }
                    }
                }
            }
            Node::Piece(p) => {
                let piece = &facts.pieces[p as usize];
                let sec = &object.sections[piece.section.index()];
                for &ri in &piece.relocations {
                    add(sec.relocations[ri as usize].symbol);
                }
            }
        }
    }
    names.sort_unstable();
    names.dedup();
    let mut h = H128::new();
    for n in names {
        h.u64(n.len() as u64);
        h.bytes(n);
    }
    h.finish()
}

/// After the debug checks (which may find a local unit's debug information
/// can't be carried): a unit that reaches a local unit that can't be
/// copied doesn't fold.
pub(super) fn settle(facts: &mut Facts) {
    let mut found: Vec<(usize, Unfoldable, String)> = Vec::new();
    for (k, u) in facts.units.iter().enumerate() {
        if u.unfoldable.is_some() || u.closure.len() < 2 {
            continue;
        }
        for node in &u.closure[1..] {
            if let Node::Unit(v) = *node
                && let Some((why, what)) = &facts.units[v as usize].blocked
            {
                found.push((k, *why, format!("a local unit it reaches: {what}")));
                break;
            }
        }
    }
    for (k, why, what) in found {
        facts.units[k].unfoldable = Some(why);
        facts.units[k].detail = Some(what);
    }
}

#[cfg(test)]
pub(crate) mod tests;
