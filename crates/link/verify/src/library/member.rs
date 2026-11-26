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
//!   rest in section order: the name, characteristics, size, and bytes;
//!   and each relocation's offset, type, and target, where a target inside
//!   the unit is its position (section, value, storage class), and a
//!   target outside is its name (externals) or its name, value, and
//!   section's name (statics);
//! - each symbol it defines: position, value, storage class, and (for
//!   externals) name.
//!
//! Names are compared without the prefix of turbolib's renamed copies
//! ([`RENAMED`]), so a renamed copy matches the copy it was made from.
//! Debug sections aren't part of a fingerprint: under phase 2.4 the
//! synthesized member's `.debug$S` refers to other type indices, and PDBs
//! are compared by `compare-images`.

use turbonop_coff::{Aux, Object, SectionId, SectionNumber, Symbol, comdat_select, storage_class};
use turbonop_pe::consts::section_characteristics as scn;
use turbonop_pe::import_object::{ImportObject, ImportType};

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
pub struct Definition {
    pub name: Box<[u8]>,
    pub kind: Kind,
    /// The unit it is defined in, if any.
    pub unit: Option<u32>,
}

/// A weak external.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Weak {
    pub name: Box<[u8]>,
    /// The default (alternate) symbol's name.
    pub alternate: Box<[u8]>,
    /// `IMAGE_WEAK_EXTERN_SEARCH_*`.
    pub search: u32,
}

/// A COMDAT unit.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Unit {
    /// The leader's name, as it is (a renamed copy keeps its prefix).
    pub leader: Box<[u8]>,
    pub selection: u8,
    /// See the module's documentation.
    pub fingerprint: u128,
}

/// A section outside every unit, other than debug sections, side tables,
/// `.chks64`, and `.drectve`.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct Piece {
    pub name: Box<[u8]>,
    pub fingerprint: u128,
}

/// A side-table entry: (table, the name of what it names).
pub type Entry = (Box<[u8]>, Box<[u8]>);

/// A COFF member.
#[derive(Clone, Debug, Default)]
pub struct Coff {
    pub machine: u16,
    /// The environment key: machine, `@feat.00`, and the sorted
    /// `/FAILIFMISMATCH` directives.
    pub environment: u128,
    pub definitions: Vec<Definition>,
    /// Undefined externals that aren't weak or common (sorted, each once).
    pub undefined: Vec<Box<[u8]>>,
    pub weak: Vec<Weak>,
    pub units: Vec<Unit>,
    pub pieces: Vec<Piece>,
    /// Side-table entries: (table, entry), where an index table's entry is
    /// the name of the symbol it names and a relocation table's is the
    /// name of its relocation's target. Sorted, each once.
    pub side: Vec<Entry>,
    /// `.drectve`'s directives, in order.
    pub directives: Vec<Box<[u8]>>,
    /// Loading the member does something: non-COMDAT initializers,
    /// terminators, or TLS callbacks, or an `/INCLUDE` (other than a PCH
    /// user's of its PCH symbol), as the plan defines side effects.
    pub side_effects: bool,
    /// Whether it has a `.debug$S` or `.debug$T` (not compared here).
    pub debug: bool,
}

impl Coff {
    /// The definition of `name`, if this member has one.
    pub fn definition(&self, name: &[u8]) -> Option<&Definition> {
        self.definitions.iter().find(|d| &*d.name == name)
    }
}

/// What a member is.
#[derive(Clone, Debug)]
pub enum Body {
    Coff(Box<Coff>),
    /// A short import object: the symbols it puts in the archive index.
    Import(Vec<Box<[u8]>>),
    /// A `/GL` object.
    Ltcg,
    /// Another anonymous object, or something turbonop-coff can't read
    /// (and why).
    Other(String),
}

impl Body {
    /// Whether turbolib passes the member through as it is.
    pub fn passed_through(&self) -> bool {
        !matches!(self, Body::Coff(_))
    }

    pub fn coff(&self) -> Option<&Coff> {
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
pub fn read(data: &[u8]) -> Body {
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
        Some(_) => match Object::parse(data) {
            Ok(o) => Body::Coff(Box::new(coff(&o))),
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

fn starts_with_ignore_case(s: &[u8], prefix: &[u8]) -> bool {
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

fn coff(o: &Object<'_>) -> Coff {
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
    // Units: each root with a leader, its sections in order (root first).
    let mut unit_of: Vec<Option<u32>> = vec![None; n];
    let mut unit_sections: Vec<Vec<usize>> = Vec::new();
    for s in 0..n {
        if root[s] == Some(s) && leader[s].is_some() {
            unit_of[s] = Some(unit_sections.len() as u32);
            unit_sections.push(vec![s]);
        }
    }
    for s in 0..n {
        if let Some(r) = root[s]
            && r != s
            && let Some(u) = unit_of[r]
        {
            unit_of[s] = Some(u);
            unit_sections[u as usize].push(s);
        }
    }
    let mut position = vec![0u32; n];
    for list in &unit_sections {
        for (i, &s) in list.iter().enumerate() {
            position[s] = i as u32;
        }
    }
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
                let alternate = o
                    .symbol(w.tag)
                    .map_or_else(Box::default, |t| t.name.as_bytes().into());
                c.weak.push(Weak {
                    name: s.name.as_bytes().into(),
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
                    name: s.name.as_bytes().into(),
                    kind: Kind {
                        class: class_of(o.sections[at].characteristics),
                        selection: selection(at),
                    },
                    unit: unit_of[at],
                });
            }
            SectionNumber::Absolute => c.definitions.push(Definition {
                name: s.name.as_bytes().into(),
                kind: Kind {
                    class: Class::Absolute,
                    selection: 0,
                },
                unit: None,
            }),
            SectionNumber::Undefined if s.value != 0 => c.definitions.push(Definition {
                name: s.name.as_bytes().into(),
                kind: Kind {
                    class: Class::Common,
                    selection: 0,
                },
                unit: None,
            }),
            SectionNumber::Undefined => c.undefined.push(s.name.as_bytes().into()),
            _ => {}
        }
    }
    c.undefined.sort_unstable();
    c.undefined.dedup();

    // Directives, side effects, side tables, debug sections.
    let indices = o.symbol_table_indices();
    let mut mismatches: Vec<&[u8]> = Vec::new();
    for (s, sec) in o.sections.iter().enumerate() {
        let name = sec.name.as_bytes();
        if name == b".drectve" {
            for d in directives(sec.bytes()) {
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
                c.directives.push(d.into());
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
                let entry: Box<[u8]> = match indices.binary_search(&index) {
                    Ok(i) => entry_name(&o.symbols[i]),
                    Err(_) => format!("<symbol index {index}>").into_bytes().into(),
                };
                c.side.push((name.into(), entry));
            }
        } else if RELOCATION_TABLES.contains(&name) && unit_of[s].is_none() {
            for r in &sec.relocations {
                let entry = o
                    .symbol(r.symbol)
                    .map_or_else(|| b"<no symbol>".to_vec().into(), entry_name);
                c.side.push((name.into(), entry));
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

    // Units' fingerprints.
    let mut defined: Vec<Vec<&Symbol<'_>>> = vec![Vec::new(); unit_sections.len()];
    for s in &o.symbols {
        if let SectionNumber::Section(id) = s.section
            && id.index() < n
            && s.section_definition().is_none()
            && let Some(u) = unit_of[id.index()]
        {
            defined[u as usize].push(s);
        }
    }
    for (u, list) in unit_sections.iter().enumerate() {
        let r = list[0];
        let Some(l) = leader[r] else { continue };
        let d = def(r).unwrap_or_default();
        let mut h = H128::new();
        h.u64(u64::from(d.selection));
        if d.selection == comdat_select::EXACT_MATCH {
            h.u64(u64::from(d.checksum));
        }
        let form = Form {
            o,
            indices: &indices,
            inside: |t: SectionId| t.index() < n && unit_of[t.index()] == Some(u as u32),
            position: &position,
        };
        for &s in list {
            if o.sections[s].name.as_bytes().starts_with(b".debug$") {
                continue;
            }
            form.section(s, &mut h);
        }
        for s in &defined[u] {
            let SectionNumber::Section(id) = s.section else {
                continue;
            };
            h.u64(u64::from(position[id.index()]));
            h.u64(u64::from(s.value));
            h.u64(u64::from(s.storage_class));
            if s.is_external() {
                h.bytes(canonical(s.name.as_bytes()));
            }
        }
        c.units.push(Unit {
            leader: o.symbols[l].name.as_bytes().into(),
            selection: d.selection,
            fingerprint: h.finish(),
        });
    }

    // Pieces: the other sections.
    for (s, sec) in o.sections.iter().enumerate() {
        let name = sec.name.as_bytes();
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
            position: &position,
        };
        form.section(s, &mut h);
        c.pieces.push(Piece {
            name: name.into(),
            fingerprint: h.finish(),
        });
    }
    c.pieces.sort_unstable();
    c
}

/// A side-table entry's name for symbol `s`: an external's name, or a
/// static's name marked as one.
fn entry_name(s: &Symbol<'_>) -> Box<[u8]> {
    if s.is_external() || s.storage_class == storage_class::WEAK_EXTERNAL {
        canonical(s.name.as_bytes()).into()
    } else {
        [b"static ", s.name.as_bytes()].concat().into()
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
    /// Their positions in it.
    position: &'o [u32],
}

impl<F: Fn(SectionId) -> bool> Form<'_, '_, F> {
    /// Hashes a reference to symbol `t`: by position inside the unit, by
    /// name outside it (with a static's value and section name).
    fn target(&self, t: Option<&Symbol<'_>>, h: &mut H128) {
        let Some(t) = t else {
            h.u64(u64::MAX);
            return;
        };
        match t.section {
            SectionNumber::Section(id) if (self.inside)(id) => {
                h.u64(1);
                h.u64(u64::from(
                    self.position.get(id.index()).copied().unwrap_or(0),
                ));
                h.u64(u64::from(t.value));
                h.u64(u64::from(t.storage_class));
            }
            _ if t.is_external() || t.storage_class == storage_class::WEAK_EXTERNAL => {
                h.u64(2);
                h.bytes(canonical(t.name.as_bytes()));
            }
            section => {
                h.u64(3);
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
        h.u64(u64::from(sec.characteristics));
        h.u64(u64::from(sec.size()));
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
                self.target(t, h);
            }
        } else {
            h.bytes(sec.bytes());
        }
        for r in &sec.relocations {
            h.u64(u64::from(r.offset));
            h.u64(u64::from(r.kind));
            self.target(o.symbol(r.symbol), h);
        }
    }
}
