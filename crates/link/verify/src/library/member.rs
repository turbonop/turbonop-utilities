//! What one library member holds, as `verify` compares it: the external
//! symbols it defines, refers to, and weakly refers to; its COMDAT units,
//! each reduced to a fingerprint; its other sections; its side-table
//! entries; and its directives.
//!
//! A **unit** is a COMDAT leader's section with its associative sections,
//! transitively (as turbolib folds them). Its **fingerprint** is a 128-bit
//! hash of its canonical form:
//!
//! - its selection (and, for `EXACT_MATCH`, its checksum);
//! - for each of its sections except `.debug$*`, the root first and the
//!   rest in an order of their contents (tools write associative sections
//!   in different orders): the name, characteristics, size, and bytes;
//!   and each relocation's offset, type, and target, where a target inside
//!   the unit is its position (section, value, storage class), and a
//!   target outside is its name (externals) or its name, value, and
//!   section's name (statics);
//! - each symbol it defines (sorted): position, value, storage class, and
//!   (for externals) name.
//!
//! With [`read_with`]'s `structural` (`--locals=copy` and `merge`), a
//! reference to a static symbol outside the unit is to what holds it, by
//! content: a unit (its own fingerprint's form, with the symbol's position
//! in it) or a piece of a section outside units (the bytes from the
//! symbol to the next symbol of the section, without the zeros at their
//! end, and their relocations), each with what it reaches in turn, by
//! contents (so a reference to either of two equal locals is the same,
//! as `--locals=merge` makes them one). So a unit that
//! names its own translation unit's `$SG12345` matches a copy that names
//! the synthesized member's copy of the same bytes.
//!
//! Names are compared without the prefix of turbolib's renamed copies
//! ([`RENAMED`]), so a renamed copy matches the copy it was made from.
//! Debug sections aren't part of a fingerprint: under phase 2.4 the
//! synthesized member's `.debug$S` refers to other type indices, and PDBs
//! are compared by `compare-images`.

use std::borrow::Cow;
use std::cell::RefCell;
use turbonop_coff::{
    Aux, Name, Object, SectionId, SectionNumber, Symbol, comdat_select, storage_class,
};
use turbonop_hash::Map;
use turbonop_pe::consts::section_characteristics as scn;
use turbonop_pe::import_object::{ImportObject, ImportType};

use super::pch;
use crate::hash::H128;

/// The prefix of turbolib's renamed copies under `--pull=preserve` (and
/// of the symbols it makes, such as each synthesized member's anchor): no
/// compiler makes a name that starts with it.
pub const RENAMED: &[u8] = b"__turbolib$";

/// `name` without [`RENAMED`].
pub fn canonical(name: &[u8]) -> &[u8] {
    name.strip_prefix(RENAMED).unwrap_or(name)
}

/// Sections whose entries are symbol table indices (4 bytes each).
pub const INDEX_TABLES: [&[u8]; 5] = [
    b".gfids$y",
    b".giats$y",
    b".gljmp$y",
    b".gehcont$y",
    b".sxdata",
];

/// Sections whose entries are relocations (each entry moves with the
/// section its relocation names).
pub const RELOCATION_TABLES: [&[u8]; 1] = [b".voltbl"];

/// What a definition is.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Class {
    /// In a section of code (`IMAGE_SCN_CNT_CODE`).
    Code,
    /// In a section of initialized data.
    Data,
    /// In an uninitialized section (`.bss`).
    Uninitialized,
    /// An absolute symbol.
    Absolute,
    /// A common symbol (undefined, with a size).
    Common,
    /// A symbol of an import object (`__imp_X`, and `X` for code).
    Import,
    /// A symbol of a member passed through without being read (a `/GL`
    /// object): what the original library's index says it defines.
    Indexed,
}

/// A definition's kind: its class, and its COMDAT selection (0 outside a
/// COMDAT; an associative section's is its leader's).
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Kind {
    pub class: Class,
    pub selection: u8,
}

impl Kind {
    /// The kind in words.
    pub fn describe(self) -> String {
        let class = match self.class {
            Class::Code => "code",
            Class::Data => "data",
            Class::Uninitialized => "uninitialized data",
            Class::Absolute => "absolute",
            Class::Common => "common",
            Class::Import => "an import",
            Class::Indexed => "an indexed symbol of a member not read",
        };
        match self.selection {
            0 => class.to_string(),
            comdat_select::NODUPLICATES => format!("{class}, COMDAT NODUPLICATES"),
            comdat_select::ANY => format!("{class}, COMDAT ANY"),
            comdat_select::SAME_SIZE => format!("{class}, COMDAT SAME_SIZE"),
            comdat_select::EXACT_MATCH => format!("{class}, COMDAT EXACT_MATCH"),
            comdat_select::LARGEST => format!("{class}, COMDAT LARGEST"),
            s => format!("{class}, COMDAT selection {s}"),
        }
    }

    /// Whether a definition of this kind may move to a synthesized member:
    /// a COMDAT that isn't `NODUPLICATES`.
    pub fn foldable(self) -> bool {
        self.selection != 0 && self.selection != comdat_select::NODUPLICATES
    }
}

/// An external symbol a member defines.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Definition<'d> {
    pub name: &'d [u8],
    pub kind: Kind,
    /// The unit it is defined in, if any.
    pub unit: Option<u32>,
}

/// A weak external.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Weak<'d> {
    pub name: &'d [u8],
    /// The default (alternate) symbol's name.
    pub alternate: &'d [u8],
    /// `IMAGE_WEAK_EXTERN_SEARCH_*`.
    pub search: u32,
}

/// A COMDAT unit.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Unit<'d> {
    /// The leader's name, as it is (a renamed copy keeps its prefix).
    pub leader: &'d [u8],
    pub selection: u8,
    /// See the module's documentation.
    pub fingerprint: u128,
    /// Its leader is a static symbol (internal linkage).
    pub local: bool,
    /// Its leader section's size and COMDAT checksum.
    pub size: u32,
    pub checksum: u32,
    /// Its key: its canonical leader name (but a local unit's under
    /// `structural`: its copies keep the name of the translation unit they
    /// came from), fingerprint, and the member's environment.
    pub key: u128,
}

/// A unit's key (see [`Unit::key`]).
pub fn unit_key(
    leader: &[u8],
    local: bool,
    fingerprint: u128,
    environment: u128,
    structural: bool,
) -> u128 {
    let mut h = H128::new();
    if !(structural && local) {
        h.bytes(canonical(leader));
    }
    h.u128(fingerprint);
    h.u128(environment);
    h.finish()
}

/// A section outside every unit, other than debug sections, side tables,
/// `.chks64`, and `.drectve`.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct Piece<'d> {
    pub name: &'d [u8],
    pub fingerprint: u128,
}

/// A side-table entry: (table, the name of what it names).
pub type Entry<'d> = (&'d [u8], Cow<'d, [u8]>);

/// A COFF member.
#[derive(Clone, Debug, Default)]
pub struct Coff<'d> {
    pub machine: u16,
    /// The environment key: machine, `@feat.00`, and the sorted
    /// `/FAILIFMISMATCH` directives.
    pub environment: u128,
    pub definitions: Vec<Definition<'d>>,
    /// Undefined externals that aren't weak or common, in the symbol
    /// table's order (a name with two records is listed twice).
    pub undefined: Vec<&'d [u8]>,
    pub weak: Vec<Weak<'d>>,
    pub units: Vec<Unit<'d>>,
    pub pieces: Vec<Piece<'d>>,
    /// Side-table entries: (table, entry), where an index table's entry is
    /// the name of the symbol it names and a relocation table's is the
    /// name of its relocation's target. Sorted, each once.
    pub side: Vec<Entry<'d>>,
    /// `.drectve`'s directives, in order.
    pub directives: Vec<&'d [u8]>,
    /// Loading the member does something: non-COMDAT initializers,
    /// terminators, or TLS callbacks, or an `/INCLUDE` (other than a PCH
    /// user's of its PCH symbol), as the plan defines side effects.
    pub side_effects: bool,
    /// Whether it has a `.debug$S` or `.debug$T` (not compared here).
    pub debug: bool,
    /// Its `.debug$T`'s `LF_PRECOMP`, if it begins with one.
    pub pch_use: Option<pch::Use>,
    /// Its identity as a PCH object, if it is one.
    pub pch: Option<pch::Pch>,
}

impl<'d> Coff<'d> {
    /// The definition of `name`, if this member has one.
    pub fn definition(&self, name: &[u8]) -> Option<&Definition<'d>> {
        self.definitions.iter().find(|d| d.name == name)
    }
}

/// What a member is.
#[derive(Clone, Debug)]
pub enum Body<'d> {
    Coff(Box<Coff<'d>>),
    /// A short import object: the symbols it puts in the archive index.
    Import(Vec<Box<[u8]>>),
    /// A `/GL` object.
    Ltcg,
    /// Another anonymous object, or something turbonop-coff can't read
    /// (and why).
    Other(String),
}

impl<'d> Body<'d> {
    /// Whether turbolib passes the member through as it is.
    pub fn passed_through(&self) -> bool {
        !matches!(self, Body::Coff(_))
    }

    pub fn coff(&self) -> Option<&Coff<'d>> {
        match self {
            Body::Coff(c) => Some(c),
            _ => None,
        }
    }

    /// What the member is, in words.
    pub fn describe(&self) -> String {
        match self {
            Body::Coff(_) => "a COFF object".into(),
            Body::Import(_) => "an import object".into(),
            Body::Ltcg => "a /GL object".into(),
            Body::Other(why) => format!("unrecognized ({why})"),
        }
    }
}

/// Reads a member's data.
pub fn read(data: &[u8]) -> Body<'_> {
    read_with(data, false)
}

/// Reads a member's data; with `structural`, its units' references to
/// static symbols outside them are by content (see the module's
/// documentation).
pub fn read_with(data: &[u8], structural: bool) -> Body<'_> {
    match turbonop_coff::kind(data) {
        Some(turbonop_coff::Kind::Import) => match ImportObject::parse(data) {
            Ok(i) => {
                let mut names: Vec<Box<[u8]>> = vec![[b"__imp_", i.symbol].concat().into()];
                if matches!(i.kind, ImportType::Code) {
                    names.push(i.symbol.into());
                }
                Body::Import(names)
            }
            Err(e) => Body::Other(format!("an import object: {e}")),
        },
        Some(turbonop_coff::Kind::Ltcg) => Body::Ltcg,
        Some(turbonop_coff::Kind::Anonymous) => Body::Other("an anonymous object".into()),
        None => Body::Other("too short for an object".into()),
        // (Debug sections' relocations aren't read: nothing here looks
        // at them, and under /Z7 they are most of an object's.)
        Some(_) => match Object::parse_with(data, |name| !name.starts_with(b".debug$")) {
            Ok(o) => Body::Coff(Box::new(coff(&o, structural))),
            Err(e) => Body::Other(e.to_string()),
        },
    }
}

fn is_comdat(characteristics: u32) -> bool {
    characteristics & scn::LNK_COMDAT != 0
}

fn class_of(characteristics: u32) -> Class {
    if characteristics & scn::CNT_CODE != 0 {
        Class::Code
    } else if characteristics & scn::CNT_UNINITIALIZED_DATA != 0 {
        Class::Uninitialized
    } else {
        Class::Data
    }
}

/// The directives of a `.drectve` section: separated by spaces, each
/// perhaps quoted in part.
pub fn directives(data: &[u8]) -> Vec<&[u8]> {
    let data = data.strip_prefix(b"\xEF\xBB\xBF").unwrap_or(data);
    let mut out = Vec::new();
    let mut i = 0;
    while i < data.len() {
        while i < data.len() && (data[i] == b' ' || data[i] == 0) {
            i += 1;
        }
        let start = i;
        let mut quoted = false;
        while i < data.len() && (quoted || (data[i] != b' ' && data[i] != 0)) {
            if data[i] == b'"' {
                quoted = !quoted;
            }
            i += 1;
        }
        if i > start {
            out.push(&data[start..i]);
        }
    }
    out
}

pub(super) fn starts_with_ignore_case(s: &[u8], prefix: &[u8]) -> bool {
    s.len() >= prefix.len() && s[..prefix.len()].eq_ignore_ascii_case(prefix)
}

/// Whether a section's name makes it a table of side effects (outside a
/// COMDAT): initializers, terminators, TLS callbacks.
fn side_effect_section(name: &[u8]) -> bool {
    [
        &b".CRT$XC"[..],
        b".CRT$XI",
        b".CRT$XL",
        b".CRT$XP",
        b".CRT$XT",
    ]
    .iter()
    .any(|p| name.starts_with(p))
}

/// A name [`Object::parse`] read, borrowed from the member's bytes (as
/// every name it reads is).
fn borrowed<'d>(name: &Name<'d>) -> &'d [u8] {
    name.borrowed().unwrap_or_default()
}

fn coff<'d>(o: &Object<'d>, structural: bool) -> Coff<'d> {
    let n = o.sections.len();
    let mut c = Coff {
        machine: o.machine,
        ..Coff::default()
    };
    // Each section's definition symbol and COMDAT leader.
    let mut definition: Vec<Option<usize>> = vec![None; n];
    let mut leader: Vec<Option<usize>> = vec![None; n];
    let mut feat00 = None;
    for (i, s) in o.symbols.iter().enumerate() {
        if let SectionNumber::Section(id) = s.section
            && id.index() < n
        {
            let at = id.index();
            if s.section_definition().is_some() {
                if definition[at].is_none() {
                    definition[at] = Some(i);
                }
            } else if definition[at].is_some()
                && leader[at].is_none()
                && is_comdat(o.sections[at].characteristics)
            {
                leader[at] = Some(i);
            }
        }
        if s.section == SectionNumber::Absolute && s.name == b"@feat.00" {
            feat00 = Some(s.value);
        }
    }
    let def = |at: usize| {
        definition[at]
            .and_then(|i| o.symbols[i].section_definition())
            .copied()
    };
    // Each COMDAT section's root: associations followed to a COMDAT that
    // isn't associative.
    let mut root: Vec<Option<usize>> = vec![None; n];
    for (s, slot) in root.iter_mut().enumerate() {
        if !is_comdat(o.sections[s].characteristics) {
            continue;
        }
        let mut at = s;
        let mut steps = 0;
        *slot = loop {
            let Some(d) = def(at) else { break None };
            if d.selection != comdat_select::ASSOCIATIVE {
                break Some(at);
            }
            steps += 1;
            match d.number {
                Some(next)
                    if next.index() < n
                        && is_comdat(o.sections[next.index()].characteristics)
                        && steps <= n =>
                {
                    at = next.index();
                }
                _ => break None,
            }
        };
    }
    // Units: each root with a leader, its sections (root first, then the
    // others in section order), flat: unit u's are
    // `sections_of[starts[u]..starts[u + 1]]`.
    let mut unit_of: Vec<Option<u32>> = vec![None; n];
    let mut roots: Vec<usize> = Vec::new();
    for s in 0..n {
        if root[s] == Some(s) && leader[s].is_some() {
            unit_of[s] = Some(roots.len() as u32);
            roots.push(s);
        }
    }
    let mut starts = vec![0usize; roots.len() + 1];
    for s in 0..n {
        if let Some(r) = root[s]
            && let Some(u) = unit_of[r]
        {
            unit_of[s] = Some(u);
            starts[u as usize + 1] += 1;
        }
    }
    for u in 0..roots.len() {
        starts[u + 1] += starts[u];
    }
    let mut sections_of = vec![0usize; starts[roots.len()]];
    let mut next: Vec<usize> = starts[..roots.len()].iter().map(|&a| a + 1).collect();
    for (u, &r) in roots.iter().enumerate() {
        sections_of[starts[u]] = r;
    }
    for s in 0..n {
        if let Some(u) = unit_of[s]
            && roots[u as usize] != s
        {
            sections_of[next[u as usize]] = s;
            next[u as usize] += 1;
        }
    }
    drop(next);
    let mut position = vec![0u32; n];
    // A section's selection, for its definitions' kinds.
    let selection = |s: usize| -> u8 {
        if !is_comdat(o.sections[s].characteristics) {
            return 0;
        }
        root[s].and_then(def).map_or(0, |d| d.selection)
    };

    // Symbols: definitions, references, weak externals.
    for s in &o.symbols {
        if s.storage_class == storage_class::WEAK_EXTERNAL
            || (s.is_external() && matches!(s.aux, Aux::WeakExternal(_)))
        {
            if let Aux::WeakExternal(w) = s.aux {
                let alternate = o.symbol(w.tag).map_or(&[][..], |t| borrowed(&t.name));
                c.weak.push(Weak {
                    name: borrowed(&s.name),
                    alternate,
                    search: w.search,
                });
            }
            continue;
        }
        if !s.is_external() {
            continue;
        }
        match s.section {
            SectionNumber::Section(id) if id.index() < n => {
                let at = id.index();
                c.definitions.push(Definition {
                    name: borrowed(&s.name),
                    kind: Kind {
                        class: class_of(o.sections[at].characteristics),
                        selection: selection(at),
                    },
                    unit: unit_of[at],
                });
            }
            SectionNumber::Absolute => c.definitions.push(Definition {
                name: borrowed(&s.name),
                kind: Kind {
                    class: Class::Absolute,
                    selection: 0,
                },
                unit: None,
            }),
            SectionNumber::Undefined if s.value != 0 => c.definitions.push(Definition {
                name: borrowed(&s.name),
                kind: Kind {
                    class: Class::Common,
                    selection: 0,
                },
                unit: None,
            }),
            SectionNumber::Undefined => c.undefined.push(borrowed(&s.name)),
            _ => {}
        }
    }

    // Directives, side effects, side tables, debug sections.
    let indices = o.symbol_table_indices();
    let mut mismatches: Vec<&[u8]> = Vec::new();
    for (s, sec) in o.sections.iter().enumerate() {
        let name = borrowed(&sec.name);
        if name == b".drectve" {
            for d in directives(sec.data.borrowed().unwrap_or_default()) {
                if starts_with_ignore_case(d, b"/failifmismatch:")
                    || starts_with_ignore_case(d, b"-failifmismatch:")
                {
                    mismatches.push(d);
                } else if (starts_with_ignore_case(d, b"/include:")
                    || starts_with_ignore_case(d, b"-include:"))
                    && !starts_with_ignore_case(&d[9..], b"__@@_pchsym_")
                {
                    c.side_effects = true;
                }
                c.directives.push(d);
            }
        } else if !is_comdat(sec.characteristics) && side_effect_section(name) && sec.size() > 0 {
            c.side_effects = true;
        }
        if name.starts_with(b".debug$") {
            c.debug = true;
        }
        if INDEX_TABLES.contains(&name) {
            for e in sec.bytes().as_chunks::<4>().0 {
                let index = u32::from_le_bytes([e[0], e[1], e[2], e[3]]);
                let entry = match indices.binary_search(&index) {
                    Ok(i) => entry_name(&o.symbols[i], structural),
                    Err(_) => Cow::Owned(format!("<symbol index {index}>").into_bytes()),
                };
                c.side.push((name, entry));
            }
        } else if RELOCATION_TABLES.contains(&name) && unit_of[s].is_none() {
            for r in &sec.relocations {
                let entry = o
                    .symbol(r.symbol)
                    .map_or(Cow::Borrowed(&b"<no symbol>"[..]), |s| {
                        entry_name(s, structural)
                    });
                c.side.push((name, entry));
            }
        }
    }
    c.side.sort_unstable();
    c.side.dedup();
    mismatches.sort_unstable();
    let mut env = H128::new();
    env.u64(u64::from(o.machine));
    env.u64(feat00.map_or(u64::MAX, u64::from));
    for m in &mismatches {
        env.bytes(m);
    }
    c.environment = env.finish();

    // Units' fingerprints. Each unit's sections in a canonical order: the
    // root, then the others by their contents, since tools write
    // associative sections in different orders; debug sections, which
    // aren't part of a fingerprint, last. Most units' others differ in
    // name, characteristics, or size, which then order them; the others
    // are ordered by their contents' hash (with references inside the unit
    // by section name and value).
    let is_debug = |s: usize| borrowed(&o.sections[s].name).starts_with(b".debug$");
    let mut keyed: Vec<(&[u8], u32, u32, usize)> = Vec::new();
    let mut hashed: Vec<(u128, usize)> = Vec::new();
    for u in 0..roots.len() {
        let list = &mut sections_of[starts[u]..starts[u + 1]];
        // Non-debug sections first (in section order), then debug ones.
        let others = &mut list[1..];
        others.sort_by_key(|&s| is_debug(s));
        let count = others.iter().take_while(|&&s| !is_debug(s)).count();
        let others = &mut others[..count];
        if others.len() > 1 {
            keyed.clear();
            keyed.extend(others.iter().map(|&s| {
                let sec = &o.sections[s];
                (borrowed(&sec.name), sec.characteristics, sec.size(), s)
            }));
            keyed.sort_unstable();
            if keyed
                .windows(2)
                .all(|w| (w[0].0, w[0].1, w[0].2) != (w[1].0, w[1].1, w[1].2))
            {
                for (slot, k) in others.iter_mut().zip(&keyed) {
                    *slot = k.3;
                }
            } else {
                let inside = |t: SectionId| t.index() < n && unit_of[t.index()] == Some(u as u32);
                let shape = Form {
                    o,
                    indices: &indices,
                    inside,
                    position: None,
                    locals: None,
                };
                hashed.clear();
                hashed.extend(others.iter().map(|&s| {
                    let mut k = H128::new();
                    shape.section(s, &mut k);
                    (k.finish(), s)
                }));
                hashed.sort_unstable();
                for (slot, k) in others.iter_mut().zip(&hashed) {
                    *slot = k.1;
                }
            }
        }
        for (p, &s) in list.iter().enumerate() {
            position[s] = p as u32;
        }
    }
    drop((keyed, hashed));
    // The symbols each unit defines outside its debug sections, by unit
    // (each unit's sorted): (unit, position, value, storage class, name of
    // an external).
    let mut defined: Vec<(u32, u32, u32, u8, &[u8])> = Vec::new();
    for s in &o.symbols {
        if let SectionNumber::Section(id) = s.section
            && id.index() < n
            && s.section_definition().is_none()
            && let Some(u) = unit_of[id.index()]
            && !is_debug(id.index())
        {
            let name = if s.is_external() {
                canonical(borrowed(&s.name))
            } else {
                &[][..]
            };
            defined.push((u, position[id.index()], s.value, s.storage_class, name));
        }
    }
    {
        let mut at = vec![0usize; roots.len() + 1];
        for d in &defined {
            at[d.0 as usize + 1] += 1;
        }
        for u in 0..roots.len() {
            at[u + 1] += at[u];
        }
        let mut sorted = vec![(0u32, 0u32, 0u32, 0u8, &[][..]); defined.len()];
        let mut next = at.clone();
        for d in defined {
            sorted[next[d.0 as usize]] = d;
            next[d.0 as usize] += 1;
        }
        for u in 0..roots.len() {
            sorted[at[u]..at[u + 1]].sort_unstable();
        }
        defined = sorted;
    }
    let locals = structural.then(|| {
        RefCell::new(Locals {
            unit_of: &unit_of,
            pieces: Vec::new(),
            by_start: Map::default(),
            bounds: Map::default(),
            edges: Vec::new(),
        })
    });
    let mut forms: Vec<u128> = vec![0; roots.len()];
    let mut unit_edges: Vec<Vec<(Local, u64)>> = vec![Vec::new(); roots.len()];
    c.units.reserve_exact(roots.len());
    let mut symbols = defined.as_slice();
    for (u, &r) in roots.iter().enumerate() {
        let Some(l) = leader[r] else { continue };
        let d = def(r).unwrap_or_default();
        let mut h = H128::new();
        h.u64(u64::from(d.selection));
        if d.selection == comdat_select::EXACT_MATCH {
            h.u64(u64::from(d.checksum));
        }
        let inside = |t: SectionId| t.index() < n && unit_of[t.index()] == Some(u as u32);
        if let Some(cell) = &locals {
            cell.borrow_mut().edges.clear();
        }
        let form = Form {
            o,
            indices: &indices,
            inside,
            position: Some(&position),
            locals: locals.as_ref(),
        };
        for &s in &sections_of[starts[u]..starts[u + 1]] {
            if s != r && is_debug(s) {
                break;
            }
            form.section(s, &mut h);
        }
        let mine = symbols.partition_point(|x| x.0 as usize == u);
        for &(_, p, value, class, name) in &symbols[..mine] {
            h.u64(u64::from(p) | u64::from(value) << 32);
            h.u64(u64::from(class));
            h.bytes(name);
        }
        symbols = &symbols[mine..];
        forms[u] = h.finish();
        if let Some(cell) = &locals {
            unit_edges[u] = std::mem::take(&mut cell.borrow_mut().edges);
        }
        c.units.push(Unit {
            leader: borrowed(&o.symbols[l].name),
            selection: d.selection,
            fingerprint: forms[u],
            local: !o.symbols[l].is_external(),
            size: o.sections[r].size(),
            checksum: d.checksum,
            key: 0,
        });
    }
    // Structural: each piece's form as it is found, then each unit's
    // fingerprint, a walk of what it reaches.
    if let Some(cell) = &locals {
        let mut piece_forms: Vec<u128> = Vec::new();
        let mut piece_edges: Vec<Vec<(Local, u64)>> = Vec::new();
        loop {
            let piece = {
                let l = cell.borrow();
                match l.pieces.get(piece_forms.len()) {
                    Some(&p) => p,
                    None => break,
                }
            };
            cell.borrow_mut().edges.clear();
            let form = Form {
                o,
                indices: &indices,
                inside: |_: SectionId| false,
                position: Some(&position),
                locals: Some(cell),
            };
            let mut h = H128::new();
            form.piece(piece, &mut h);
            piece_forms.push(h.finish());
            piece_edges.push(std::mem::take(&mut cell.borrow_mut().edges));
        }
        let graph = Graph {
            forms: &forms,
            unit_edges: &unit_edges,
            piece_forms: &piece_forms,
            piece_edges: &piece_edges,
        };
        let mut memo = Map::default();
        // (Every unit has a leader, so unit u is `c.units[u]`.)
        for (u, unit) in c.units.iter_mut().enumerate() {
            if !unit_edges[u].is_empty() {
                unit.fingerprint =
                    graph.contents(Local::Unit(u as u32), &mut Vec::new(), &mut memo);
            }
        }
    }

    // Pieces: the other sections.
    for (s, sec) in o.sections.iter().enumerate() {
        let name = borrowed(&sec.name);
        if unit_of[s].is_some()
            || name.starts_with(b".debug$")
            || name == b".drectve"
            || name == b".chks64"
            || INDEX_TABLES.contains(&name)
            || RELOCATION_TABLES.contains(&name)
        {
            continue;
        }
        let mut h = H128::new();
        // A COMDAT outside every unit (associated with a section that isn't
        // a COMDAT, or without a leader) keeps its selection.
        h.u64(u64::from(def(s).map_or(0, |d| d.selection)));
        let form = Form {
            o,
            indices: &indices,
            inside: |t: SectionId| t.index() == s,
            position: Some(&position),
            locals: None,
        };
        form.section(s, &mut h);
        c.pieces.push(Piece {
            name,
            fingerprint: h.finish(),
        });
    }
    c.pieces.sort_unstable();
    for u in &mut c.units {
        u.key = unit_key(u.leader, u.local, u.fingerprint, c.environment, structural);
    }
    c.pch_use = pch::use_in(o);
    c.pch = pch::pch_in(o);
    c
}

/// What a static symbol outside a unit is in (under `structural`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
enum Local {
    Unit(u32),
    Piece(u32),
}

/// The static symbols units reach, as they are found.
struct Locals<'o> {
    unit_of: &'o [Option<u32>],
    /// Each piece: (section, start, end).
    pieces: Vec<(usize, u32, u32)>,
    by_start: Map<(usize, u32), u32>,
    /// Each section's symbols' values, sorted.
    bounds: Map<usize, Vec<u32>>,
    /// The form being hashed's edges: what it reaches, and where in it.
    edges: Vec<(Local, u64)>,
}

impl Locals<'_> {
    /// The place of static `t` (in section `id`) among the edges of the
    /// form being hashed.
    fn place(
        &mut self,
        o: &Object<'_>,
        position: Option<&[u32]>,
        id: SectionId,
        t: &Symbol<'_>,
    ) -> u64 {
        let s = id.index();
        let (node, at) = match self.unit_of.get(s).copied().flatten() {
            Some(v) => {
                let mut h = H128::new();
                h.u64(u64::from(
                    position.and_then(|p| p.get(s).copied()).unwrap_or(0),
                ));
                h.u64(u64::from(t.value));
                h.u64(u64::from(t.storage_class));
                (Local::Unit(v), h.finish() as u64)
            }
            None => {
                let start = t.value;
                let p = match self.by_start.get(&(s, start)) {
                    Some(&p) => p,
                    None => {
                        let bounds = self.bounds.entry(s).or_insert_with(|| {
                            let mut v: Vec<u32> = o
                                .symbols
                                .iter()
                                .filter(|x| x.section == SectionNumber::Section(id))
                                .map(|x| x.value)
                                .collect();
                            v.sort_unstable();
                            v.dedup();
                            v
                        });
                        let size = o.sections.get(s).map_or(0, turbonop_coff::Section::size);
                        let end = bounds
                            .iter()
                            .copied()
                            .find(|&b| b > start)
                            .map_or(size, |b| b.min(size))
                            .max(start);
                        let p = self.pieces.len() as u32;
                        self.pieces.push((s, start.min(end), end));
                        self.by_start.insert((s, start), p);
                        p
                    }
                };
                (Local::Piece(p), 0)
            }
        };
        // One edge for each reference, not for each thing referred to:
        // references to two equal locals and to one (as `merge` leaves
        // them) are then the same.
        self.edges.push((node, at));
        (self.edges.len() - 1) as u64
    }
}

/// The forms and edges of a member's units and pieces, under
/// `structural`.
struct Graph<'g> {
    forms: &'g [u128],
    unit_edges: &'g [Vec<(Local, u64)>],
    piece_forms: &'g [u128],
    piece_edges: &'g [Vec<(Local, u64)>],
}

impl Graph<'_> {
    fn node(&self, n: Local) -> (u128, &[(Local, u64)]) {
        match n {
            Local::Unit(v) => (
                self.forms.get(v as usize).copied().unwrap_or(0),
                self.unit_edges
                    .get(v as usize)
                    .map_or(&[][..], Vec::as_slice),
            ),
            Local::Piece(p) => (
                self.piece_forms.get(p as usize).copied().unwrap_or(0),
                self.piece_edges
                    .get(p as usize)
                    .map_or(&[][..], Vec::as_slice),
            ),
        }
    }

    /// Node `n`'s contents: its form, and each edge's target's contents,
    /// depth first, a reference back to a node being hashed as a marker.
    /// By contents rather than by which node, so that copies of equal
    /// locals `--locals=merge` made one are the same as the originals.
    /// Each node is hashed once (`memo`), so a node in a cycle has the
    /// contents its first walk gave it; the same graph walked in the same
    /// order gives the same results.
    fn contents(&self, n: Local, stack: &mut Vec<Local>, memo: &mut Map<Local, u128>) -> u128 {
        if stack.contains(&n) {
            return u128::MAX;
        }
        if let Some(&h) = memo.get(&n) {
            return h;
        }
        let (form, edges) = self.node(n);
        stack.push(n);
        let mut h = H128::new();
        h.u64(u64::from(matches!(n, Local::Piece(_))));
        h.u128(form);
        h.u64(edges.len() as u64);
        for &(t, at) in edges {
            h.u128(self.contents(t, stack, memo));
            h.u64(at);
        }
        stack.pop();
        let out = h.finish();
        memo.insert(n, out);
        out
    }
}

/// A side-table entry's name for symbol `s`: an external's name, or a
/// static's name marked as one (under `structural`, a static without its
/// name: copies of locals keep the names of the translation unit they
/// were copied from).
fn entry_name<'d>(s: &Symbol<'d>, structural: bool) -> Cow<'d, [u8]> {
    if s.is_external() || s.storage_class == storage_class::WEAK_EXTERNAL {
        Cow::Borrowed(canonical(borrowed(&s.name)))
    } else if structural {
        Cow::Borrowed(b"static")
    } else {
        Cow::Owned([b"static ", s.name.as_bytes()].concat())
    }
}

/// Hashes section `s`'s canonical form (see the module's documentation):
/// `inside` says which sections count as inside the unit, and `position`
/// gives their positions in it.
struct Form<'o, 'd, F> {
    o: &'o Object<'d>,
    /// Each symbol's table index.
    indices: &'o [u32],
    /// Which sections count as inside the unit.
    inside: F,
    /// Their positions in it; `None` names a section inside by its name
    /// instead (for ordering them).
    position: Option<&'o [u32]>,
    /// Under `structural`, where static symbols outside the unit are
    /// found, and the edges to them collected.
    locals: Option<&'o RefCell<Locals<'o>>>,
}

impl<F: Fn(SectionId) -> bool> Form<'_, '_, F> {
    /// Hashes a piece's form: its section's name and characteristics, its
    /// bytes without the zeros at their end (but those of a relocation),
    /// and its relocations, by offset from its start.
    fn piece(&self, (s, start, end): (usize, u32, u32), h: &mut H128) {
        let o = self.o;
        let sec = &o.sections[s];
        let bytes = sec.bytes().get(start as usize..end as usize).unwrap_or(&[]);
        let relocations: Vec<&turbonop_coff::Relocation> = sec
            .relocations
            .iter()
            .filter(|r| r.offset >= start && r.offset < end)
            .collect();
        let mut used = bytes.iter().rposition(|&b| b != 0).map_or(0, |p| p + 1);
        for r in &relocations {
            let width = if o.machine == 0x8664 && r.kind == 1 {
                8
            } else {
                4
            };
            used = used
                .max((r.offset - start) as usize + width)
                .min(bytes.len());
        }
        h.bytes(sec.name.as_bytes());
        h.u64(u64::from(sec.characteristics) | (used as u64) << 32);
        h.bytes(&bytes[..used]);
        for r in relocations {
            self.target(o.symbol(r.symbol), h, relocation(r.offset - start, r.kind));
        }
    }

    /// Hashes a reference to symbol `t`: by position inside the unit, by
    /// name outside it (with a static's value and section name). `lead` is
    /// what comes before it in the same word: a relocation's offset and
    /// type ([`relocation`]), or nothing (an index table's entry).
    fn target(&self, t: Option<&Symbol<'_>>, h: &mut H128, lead: u64) {
        // (The kind of reference in bits 48 to 55, a storage class in 56
        // to 63.)
        let Some(t) = t else {
            h.u64(lead | 0xFF << 48);
            return;
        };
        match t.section {
            SectionNumber::Section(id) if (self.inside)(id) => {
                h.u64(lead | 1 << 48 | u64::from(t.storage_class) << 56);
                match self.position {
                    Some(p) => h.u64(
                        u64::from(p.get(id.index()).copied().unwrap_or(0))
                            | u64::from(t.value) << 32,
                    ),
                    None => {
                        h.bytes(self.o.section(id).map_or(&[][..], |x| x.name.as_bytes()));
                        h.u64(u64::from(t.value));
                    }
                }
            }
            _ if t.is_external() || t.storage_class == storage_class::WEAK_EXTERNAL => {
                h.u64(lead | 2 << 48);
                h.bytes(canonical(t.name.as_bytes()));
            }
            SectionNumber::Section(id)
                if self.locals.is_some() && !matches!(t.aux, Aux::Section(_)) =>
            {
                let cell = self.locals.expect("structural");
                let place = cell.borrow_mut().place(self.o, self.position, id, t);
                h.u64(lead | 4 << 48);
                h.u64(place);
            }
            section => {
                h.u64(lead | 3 << 48);
                h.bytes(t.name.as_bytes());
                h.u64(u64::from(t.value));
                let in_section = section
                    .id()
                    .and_then(|id| self.o.section(id))
                    .map_or(&[][..], |x| x.name.as_bytes());
                h.bytes(in_section);
            }
        }
    }

    /// Hashes section `s`'s canonical form (see the module's
    /// documentation). An index table's entries are hashed as the symbols
    /// they name, since their values are symbol table indices.
    fn section(&self, s: usize, h: &mut H128) {
        let o = self.o;
        let sec = &o.sections[s];
        let name = sec.name.as_bytes();
        h.bytes(name);
        h.u64(u64::from(sec.characteristics) | u64::from(sec.size()) << 32);
        if INDEX_TABLES.contains(&name) {
            let (entries, rest) = sec.bytes().as_chunks::<4>();
            h.bytes(rest);
            for e in entries {
                let index = u32::from_le_bytes([e[0], e[1], e[2], e[3]]);
                let t = self
                    .indices
                    .binary_search(&index)
                    .ok()
                    .map(|i| &o.symbols[i]);
                self.target(t, h, 0);
            }
        } else {
            h.bytes(sec.bytes());
        }
        for r in &sec.relocations {
            self.target(o.symbol(r.symbol), h, relocation(r.offset, r.kind));
        }
    }
}

/// A relocation's offset and type, in the low 48 bits of a word (see
/// [`Form::target`]).
fn relocation(offset: u32, kind: u16) -> u64 {
    u64::from(offset) | u64::from(kind) << 32
}
