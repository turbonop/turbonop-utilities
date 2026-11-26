//! Side tables: sections whose entries name code elsewhere in the member,
//! split when fold units move to a synthesized member.
//!
//! What each holds, as cl 14.52 and clang-cl 24 write them (see
//! `docs/turbolib.md`, "Side tables"):
//!
//! - `.gfids$y` (`/guard:cf`): the functions whose address is taken, as
//!   32-bit symbol table indices. `.giats$y`: the `__imp_` symbols whose
//!   IAT entry's address is taken. `.gljmp$y`: the labels `longjmp` can
//!   return to. `.gehcont$y` (`/guard:ehcont`): the labels an exception
//!   can continue at.
//! - `.sxdata` (x86 SafeSEH): the exception handlers the member's
//!   functions register, as symbol table indices.
//! - `.debug$F` (x86 FPO): 16-byte `FPO_DATA` records, each with a
//!   relocation at its start naming the function it describes.
//! - `.voltbl` (`/volatileMetadata`, x64): in a COMDAT associated with a
//!   function, its volatile accesses as bytes, which name nothing. Outside
//!   COMDATs (`/Gy-`), records: `0xFFFFFFFF`, a function's symbol index,
//!   and 32-bit offsets from it.
//! - `.retplne` (`/d2guardretpoline`): `RetpolineV1\0`, then records of a
//!   32-bit size (of the whole record), a section number, and pairs of a
//!   kind and an offset: the indirect branches in that section.
//! - `.llvm_addrsig` (clang-cl, always): the symbols whose address is
//!   significant (which lld-link's ICF mustn't fold), as ULEB128 symbol
//!   table indices.
//!
//! cl with `/Gy` puts a function's entries in a COMDAT associated with it
//! (and a vftable's `.gfids$y` with the vftable), so they are in its fold
//! unit. Code outside COMDATs (`/Gy-`) and clang-cl use one table of each
//! kind per object, and `.sxdata` and `.retplne` are always one table.
//!
//! How each is split:
//!
//! - A table in a unit moves with it, and its entries are renumbered for
//!   the synthesized member: a target outside the unit is named by its
//!   name, as relocations are.
//! - A table outside the units stays, and its entries are renumbered after
//!   the edit. An entry naming a local symbol or a section of a unit that
//!   moves goes with the unit, into the synthesized member's own table of
//!   that name; so does an FPO, volatile, or retpoline record describing a
//!   unit's code. An entry of the other tables (sets of addresses) naming
//!   an external symbol stays: the symbol is now an undefined external,
//!   which both linkers resolve to its definition (measured, for
//!   `.gfids$y` and `.sxdata`). It is also copied to the synthesized member
//!   when the unit defines it or refers to it, so that a synthesized member
//!   loaded without the member still lists what its code needs. A superset
//!   of a set is harmless; a missing entry fails at run time.
//! - A table in a COMDAT associated with code that stays can't give an
//!   entry away, so a unit whose local symbol it names doesn't fold.
//!
//! A table turbolib can't read makes its member pass through
//! ([`Unfoldable::SideTables`]).

use super::units::{Unit, weak_default};
use super::{Parsed, Placed, Synth, Unfoldable, section_symbol};
use crate::hash::H128;
use std::borrow::Cow;
use std::collections::{HashMap, HashSet};
use turbonop_coff::{
    Object, Relocation, Renumbering, Section, SectionData, SectionDefinition, SectionId, Symbol,
    SymbolId, storage_class,
};

/// The side tables' section names.
pub const NAMES: &[&[u8]] = &[
    b".gfids$y",
    b".giats$y",
    b".gljmp$y",
    b".gehcont$y",
    b".sxdata",
    b".debug$F",
    b".voltbl",
    b".retplne",
    b".llvm_addrsig",
];

/// The start of a `.retplne` section.
pub const RETPOLINE_MAGIC: &[u8] = b"RetpolineV1\0";

const IMAGE_SCN_LNK_COMDAT: u32 = 0x1000;

/// How a table's entries are laid out.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    /// 32-bit symbol table indices: `.gfids$y`, `.giats$y`, `.gljmp$y`,
    /// `.gehcont$y`, `.sxdata`.
    Indices,
    /// 16-byte FPO records, each named by a relocation at its start
    /// (`.debug$F`).
    Fpo,
    /// Volatile-access records: `0xFFFFFFFF`, a symbol table index, and
    /// offsets (`.voltbl` outside COMDATs).
    Volatile,
    /// Retpoline records: a size, a section number, and (kind, offset)
    /// pairs, after [`RETPOLINE_MAGIC`] (`.retplne`).
    Retpoline,
    /// ULEB128 symbol table indices (`.llvm_addrsig`).
    Addrsig,
}

impl Kind {
    /// The table's entries describe their target's code (rather than list
    /// it in a set), so an entry goes wherever that code goes, and never
    /// stays behind as well.
    pub fn describes_code(self) -> bool {
        !self.is_set()
    }

    /// The table is a set of symbols (an extra entry is harmless).
    pub fn is_set(self) -> bool {
        matches!(self, Kind::Indices | Kind::Addrsig)
    }

    /// Where, in an entry, the field naming its target is (none for FPO,
    /// whose entries are named by relocations).
    fn field(self) -> Option<usize> {
        match self {
            Kind::Indices => Some(0),
            Kind::Volatile | Kind::Retpoline => Some(4),
            Kind::Fpo | Kind::Addrsig => None,
        }
    }
}

/// What an entry names.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Target {
    Symbol(SymbolId),
    Section(SectionId),
}

/// One entry of a table.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Entry {
    /// Its bytes in the section.
    pub start: u32,
    pub end: u32,
    /// What it names (`None` for an FPO record without a relocation).
    pub target: Option<Target>,
    /// For an FPO record, its relocation's type.
    pub relocation: u16,
}

/// One side table.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Table {
    pub section: SectionId,
    pub kind: Kind,
    /// The section is a COMDAT (associated with what it describes).
    pub comdat: bool,
    /// The bytes before the first entry.
    pub header: u32,
    pub entries: Vec<Entry>,
}

/// A member's side tables.
#[derive(Clone, Debug, Default)]
pub struct Tables {
    pub tables: Vec<Table>,
    /// Each table's position in `tables`, by section.
    of_section: HashMap<u32, u32>,
    /// Entries of tables outside COMDATs whose target is in a unit, by
    /// unit, as (table, entry).
    by_unit: HashMap<usize, Vec<(u32, u32)>>,
    /// Entries of sets ([`Kind::is_set`]) outside COMDATs that name an
    /// external or weak external symbol, by its name, as (table, entry).
    by_name: HashMap<Vec<u8>, Vec<(u32, u32)>>,
}

fn u32_at(data: &[u8], at: usize) -> Option<u32> {
    data.get(at..at + 4)
        .map(|b| u32::from_le_bytes([b[0], b[1], b[2], b[3]]))
}

fn show(b: &[u8]) -> String {
    String::from_utf8_lossy(b).into_owned()
}

/// Whether `name` is a side table's.
pub fn is_table(name: &[u8]) -> bool {
    NAMES.contains(&name)
}

/// Whether a debug section of a unit goes with it to the synthesized
/// member as a side table does (FPO data, which describes the unit's code).
pub fn moves_with_unit(name: &[u8]) -> bool {
    name == b".debug$F"
}

/// Reads a member's side tables. `Err` says what turbolib can't read, and
/// the member passes through.
pub fn find(object: &Object<'_>) -> Result<Tables, String> {
    let mut tables = Tables::default();
    let mut indices: Option<Vec<u32>> = None;
    for (i, sec) in object.sections.iter().enumerate() {
        let name = sec.name.as_bytes();
        if !NAMES.iter().any(|n| name.starts_with(n)) {
            continue;
        }
        let what = |why: &str| format!("{} (section {}): {why}", show(name), i + 1);
        let kind = match name {
            b".gfids$y" | b".giats$y" | b".gljmp$y" | b".gehcont$y" | b".sxdata" => Kind::Indices,
            b".debug$F" => Kind::Fpo,
            b".voltbl" => Kind::Volatile,
            b".retplne" => Kind::Retpoline,
            b".llvm_addrsig" => Kind::Addrsig,
            _ => return Err(what("a side table turbolib doesn't know")),
        };
        if matches!(sec.data, SectionData::Uninitialized(n) if n > 0) {
            return Err(what("uninitialized"));
        }
        if kind != Kind::Fpo && !sec.relocations.is_empty() {
            return Err(what("relocations"));
        }
        let comdat = sec.characteristics & IMAGE_SCN_LNK_COMDAT != 0;
        if kind == Kind::Addrsig && comdat {
            return Err(what("a COMDAT"));
        }
        let data = sec.bytes();
        // A COMDAT's volatile table holds bytes that name nothing.
        if kind == Kind::Volatile && comdat && !data.starts_with(&[0xFF; 4]) {
            continue;
        }
        let indices = indices.get_or_insert_with(|| object.symbol_table_indices());
        let symbol = |index: u32| {
            indices
                .binary_search(&index)
                .ok()
                .map(|s| Target::Symbol(SymbolId(s as u32)))
        };
        let mut table = Table {
            section: SectionId(i as u32),
            kind,
            comdat,
            header: 0,
            entries: Vec::new(),
        };
        let entry = |start: usize, end: usize, target| Entry {
            start: start as u32,
            end: end as u32,
            target,
            relocation: 0,
        };
        match kind {
            Kind::Indices => {
                if !data.len().is_multiple_of(4) {
                    return Err(what("not a whole number of indices"));
                }
                for at in (0..data.len()).step_by(4) {
                    let index = u32_at(data, at).unwrap_or(u32::MAX);
                    let Some(t) = symbol(index) else {
                        return Err(what(&format!("index {index:#x} names no symbol")));
                    };
                    table.entries.push(entry(at, at + 4, Some(t)));
                }
            }
            Kind::Volatile => {
                if !data.len().is_multiple_of(4) {
                    return Err(what("not a whole number of words"));
                }
                let words: Vec<u32> = (0..data.len() / 4)
                    .map(|w| u32_at(data, 4 * w).unwrap_or(0))
                    .collect();
                let mut w = 0;
                while w < words.len() {
                    if words[w] != u32::MAX || w + 1 >= words.len() {
                        return Err(what("not a volatile-access record"));
                    }
                    let Some(t) = symbol(words[w + 1]) else {
                        return Err(what(&format!("index {:#x} names no symbol", words[w + 1])));
                    };
                    let mut end = w + 2;
                    while end < words.len() && words[end] != u32::MAX {
                        end += 1;
                    }
                    table.entries.push(entry(4 * w, 4 * end, Some(t)));
                    w = end;
                }
            }
            Kind::Retpoline => {
                if !data.starts_with(RETPOLINE_MAGIC) {
                    return Err(what("no RetpolineV1 header"));
                }
                table.header = RETPOLINE_MAGIC.len() as u32;
                let mut at = RETPOLINE_MAGIC.len();
                while at < data.len() {
                    let size = u32_at(data, at).unwrap_or(0) as usize;
                    if size < 8 || !size.is_multiple_of(4) || at + size > data.len() {
                        return Err(what(&format!("a record of size {size:#x} at {at:#x}")));
                    }
                    let number = u32_at(data, at + 4).unwrap_or(0);
                    let Some(s) = SectionId::from_number(number)
                        .filter(|s| s.index() < object.sections.len())
                    else {
                        return Err(what(&format!("section number {number} at {at:#x}")));
                    };
                    table
                        .entries
                        .push(entry(at, at + size, Some(Target::Section(s))));
                    at += size;
                }
            }
            Kind::Addrsig => {
                let mut at = 0;
                while at < data.len() {
                    let start = at;
                    let mut value = 0u64;
                    let mut shift = 0;
                    loop {
                        let Some(&b) = data.get(at) else {
                            return Err(what("a truncated ULEB128 index"));
                        };
                        at += 1;
                        if shift > 28 {
                            return Err(what("an index over 32 bits"));
                        }
                        value |= u64::from(b & 0x7F) << shift;
                        shift += 7;
                        if b & 0x80 == 0 {
                            break;
                        }
                    }
                    let Some(t) = u32::try_from(value).ok().and_then(symbol) else {
                        return Err(what(&format!("index {value:#x} names no symbol")));
                    };
                    table.entries.push(entry(start, at, Some(t)));
                }
            }
            Kind::Fpo => {
                if !data.len().is_multiple_of(16) {
                    return Err(what("not a whole number of FPO records"));
                }
                let mut named: Vec<Option<(SymbolId, u16)>> = vec![None; data.len() / 16];
                for r in &sec.relocations {
                    let at = r.offset as usize;
                    if !at.is_multiple_of(16) || at >= data.len() || named[at / 16].is_some() {
                        return Err(what(&format!("a relocation at {at:#x}")));
                    }
                    if object.symbol(r.symbol).is_none() {
                        return Err(what("a relocation naming no symbol"));
                    }
                    named[at / 16] = Some((r.symbol, r.kind));
                }
                for (n, r) in named.into_iter().enumerate() {
                    let mut e = entry(16 * n, 16 * n + 16, r.map(|(s, _)| Target::Symbol(s)));
                    e.relocation = r.map_or(0, |(_, k)| k);
                    table.entries.push(e);
                }
            }
        }
        tables
            .of_section
            .insert(i as u32, tables.tables.len() as u32);
        tables.tables.push(table);
    }
    Ok(tables)
}

impl Tables {
    /// The table in section `id`, if it is one.
    pub fn get(&self, id: SectionId) -> Option<&Table> {
        self.of_section
            .get(&id.0)
            .map(|&t| &self.tables[t as usize])
    }

    /// Whether section `id`'s relocations are a table's entries that are
    /// split here (an FPO table outside COMDATs), rather than references
    /// that decide foldability.
    pub fn splits_relocations(&self, id: SectionId) -> bool {
        self.get(id)
            .is_some_and(|t| t.kind == Kind::Fpo && !t.comdat)
    }

    /// Decides what the tables' entries mean for foldability (as
    /// relocations do: see the module's notes), and indexes the entries
    /// that go with each unit. `unit_of` is each section's unit and
    /// `symbol_unit` each symbol's.
    pub fn check(
        &mut self,
        object: &Object<'_>,
        unit_of: &[Option<usize>],
        symbol_unit: &[Option<usize>],
        reason: &mut [Option<Unfoldable>],
        detail: &mut [Option<String>],
    ) {
        let mut flag = |u: usize, why: Unfoldable, what: &dyn Fn() -> String| {
            if reason[u].is_none() {
                reason[u] = Some(why);
                detail[u] = Some(what());
            }
        };
        for (ti, t) in self.tables.iter().enumerate() {
            let here = unit_of.get(t.section.index()).copied().flatten();
            let table_name = object.sections[t.section.index()].name.as_bytes();
            for (ei, e) in t.entries.iter().enumerate() {
                let what = || {
                    let target = match e.target {
                        Some(Target::Symbol(s)) => object
                            .symbol(s)
                            .map_or_else(String::new, |s| show(s.name.as_bytes())),
                        Some(Target::Section(s)) => format!("section {}", s.number()),
                        None => String::new(),
                    };
                    format!(
                        "{} (section {}) names {target}",
                        show(table_name),
                        t.section.number()
                    )
                };
                let key = (ti as u32, ei as u32);
                match e.target {
                    None => {}
                    Some(Target::Symbol(s)) => {
                        let sym = &object.symbols[s.index()];
                        let there = symbol_unit.get(s.index()).copied().flatten();
                        match here {
                            Some(a) if there == Some(a) => {}
                            Some(a) => {
                                if sym.storage_class == storage_class::WEAK_EXTERNAL {
                                    if weak_default(object, sym).is_none() {
                                        flag(a, Unfoldable::WeakExternalReference, &what);
                                    }
                                } else if !sym.is_external() {
                                    flag(a, Unfoldable::LocalReference, &what);
                                    if let Some(b) = there {
                                        flag(b, Unfoldable::ReferencedLocally, &what);
                                    }
                                }
                            }
                            None if t.comdat => {
                                if let Some(b) = there
                                    && (!sym.is_external() || t.kind.describes_code())
                                {
                                    flag(b, Unfoldable::ReferencedLocally, &what);
                                }
                            }
                            None => {
                                if let Some(b) = there {
                                    self.by_unit.entry(b).or_default().push(key);
                                }
                                if t.kind.is_set()
                                    && (sym.is_external()
                                        || sym.storage_class == storage_class::WEAK_EXTERNAL)
                                {
                                    let name = sym.name.as_bytes().to_vec();
                                    self.by_name.entry(name).or_default().push(key);
                                }
                            }
                        }
                    }
                    Some(Target::Section(s)) => {
                        let there = unit_of.get(s.index()).copied().flatten();
                        match here {
                            Some(a) if there == Some(a) => {}
                            Some(a) => {
                                flag(a, Unfoldable::LocalReference, &what);
                                if let Some(b) = there {
                                    flag(b, Unfoldable::ReferencedLocally, &what);
                                }
                            }
                            None if t.comdat => {
                                if let Some(b) = there {
                                    flag(b, Unfoldable::ReferencedLocally, &what);
                                }
                            }
                            None => {
                                if let Some(b) = there {
                                    self.by_unit.entry(b).or_default().push(key);
                                }
                            }
                        }
                    }
                }
            }
        }
    }

    /// The entries of tables outside COMDATs that go to (or are copied to)
    /// the synthesized member that receives unit `u`: those naming
    /// something in it, and set entries naming an external symbol its
    /// relocations name (or, for `__imp_X`, naming `X`, which clang-cl
    /// lists when an import's address is taken). In table and entry order.
    fn entries_for(&self, object: &Object<'_>, unit: &Unit, u: usize) -> Vec<(u32, u32)> {
        let mut out: Vec<(u32, u32)> = self.by_unit.get(&u).cloned().unwrap_or_default();
        if !self.by_name.is_empty() {
            for s in &unit.sections {
                for r in &object.sections[s.index()].relocations {
                    let Some(t) = object.symbol(r.symbol) else {
                        continue;
                    };
                    if !t.is_external() && t.storage_class != storage_class::WEAK_EXTERNAL {
                        continue;
                    }
                    let name = t.name.as_bytes();
                    for n in [Some(name), name.strip_prefix(b"__imp_")]
                        .into_iter()
                        .flatten()
                    {
                        if let Some(e) = self.by_name.get(n) {
                            out.extend_from_slice(e);
                        }
                    }
                }
            }
        }
        out.sort_unstable();
        out.dedup();
        out
    }
}

impl Table {
    /// Adds the table's contents to a unit's identity: its bytes, with
    /// each target (in place of the field that names it) by position in
    /// the unit, or by name outside it. Unit `k`'s symbols are `unit`'s.
    pub(super) fn hash(
        &self,
        object: &Object<'_>,
        unit: &Unit,
        k: usize,
        symbol_unit: &[Option<usize>],
        h: &mut H128,
    ) {
        let data = object.sections[self.section.index()].bytes();
        h.u64(data.len() as u64 | 1 << 62);
        h.bytes(&data[..self.header as usize]);
        for e in &self.entries {
            let bytes = &data[e.start as usize..e.end as usize];
            match self.kind.field() {
                Some(f) => {
                    h.bytes(&bytes[..f]);
                    h.bytes(&bytes[f + 4..]);
                }
                None => h.bytes(bytes),
            }
            match e.target {
                None => h.u64(u64::MAX),
                Some(Target::Symbol(s)) => {
                    let inside = symbol_unit.get(s.index()).copied().flatten() == Some(k);
                    match (inside, unit.symbols.binary_search(&s)) {
                        (true, Ok(p)) => h.u64(p as u64),
                        _ => {
                            let t = &object.symbols[s.index()];
                            let name = t.name.as_bytes();
                            h.u64(u64::MAX - name.len() as u64);
                            h.bytes(name);
                            if let Some((search, default)) = weak_default(object, t) {
                                h.u64(u64::from(search) | 1 << 40);
                                h.u64(default.len() as u64);
                                h.bytes(default);
                            }
                        }
                    }
                }
                Some(Target::Section(s)) => {
                    let p = unit.sections.iter().position(|&x| x == s);
                    h.u64(p.map_or(u64::MAX - 1, |p| p as u64 | 1 << 61));
                }
            }
        }
    }
}

/// A value to write into a table once the final numbering is known.
#[derive(Clone, Copy, Debug)]
enum Value {
    /// A symbol table index of the edited member, from its old index.
    OldIndex(u32),
    /// A section number of the edited member, from its old section.
    OldSection(SectionId),
    /// A symbol's table index in a new object.
    Index(SymbolId),
    /// A section's number in a new object.
    Number(SectionId),
}

/// The values still to write: 32-bit fields, as (section, offset, value),
/// and whole ULEB128 tables, as (section, values).
#[derive(Debug, Default)]
pub(super) struct Fields {
    fields: Vec<(SectionId, u32, Value)>,
    uleb: Vec<(SectionId, Vec<Value>)>,
}

impl Fields {
    fn write(
        &self,
        object: &mut Object<'_>,
        section: impl Fn(SectionId) -> SectionId,
        value: impl Fn(Value) -> u32,
    ) {
        for &(s, at, v) in &self.fields {
            let v = value(v);
            let sec = &mut object.sections[section(s).index()];
            if let SectionData::Bytes(b) = &mut sec.data {
                b.to_mut()[at as usize..at as usize + 4].copy_from_slice(&v.to_le_bytes());
            }
        }
        for (s, values) in &self.uleb {
            let mut data = Vec::with_capacity(values.len() * 2);
            for &v in values {
                let mut v = value(v);
                loop {
                    let b = (v & 0x7F) as u8;
                    v >>= 7;
                    if v == 0 {
                        data.push(b);
                        break;
                    }
                    data.push(b | 0x80);
                }
            }
            object.sections[section(*s).index()].data = SectionData::Bytes(Cow::Owned(data));
        }
    }
}

/// Splits a member's tables before the edit that removes its units'
/// sections (`removed`, by section): entries that go with a removed unit
/// are taken out (see the module's notes). `original` is the member as it
/// was read; `object` is the member being edited, whose sections are still
/// numbered as `original`'s. The rest is renumbered by [`renumber`], once
/// the edit is done.
pub(super) fn strip(
    original: &Object<'_>,
    tables: &Tables,
    removed: &[bool],
    object: &mut Object<'_>,
) -> Fields {
    let mut fields = Fields::default();
    let mut indices: Option<Vec<u32>> = None;
    let gone = |s: SymbolId| {
        original.symbols[s.index()]
            .section
            .id()
            .is_some_and(|id| removed[id.index()])
    };
    for t in &tables.tables {
        if removed[t.section.index()] {
            continue;
        }
        let old = original.sections[t.section.index()].bytes();
        let mut data = old[..t.header as usize].to_vec();
        let mut relocations = Vec::new();
        let mut uleb = Vec::new();
        for e in &t.entries {
            let keep = match e.target {
                None => true,
                Some(Target::Symbol(s)) => {
                    !(gone(s)
                        && (!original.symbols[s.index()].is_external() || t.kind.describes_code()))
                }
                Some(Target::Section(s)) => !removed[s.index()],
            };
            if !keep {
                continue;
            }
            if t.kind == Kind::Addrsig {
                if let Some(Target::Symbol(s)) = e.target {
                    let indices = indices.get_or_insert_with(|| original.symbol_table_indices());
                    uleb.push(Value::OldIndex(indices[s.index()]));
                }
                continue;
            }
            let at = data.len() as u32;
            let bytes = &old[e.start as usize..e.end as usize];
            data.extend_from_slice(bytes);
            match (t.kind.field(), e.target) {
                (Some(f), Some(Target::Section(s))) => {
                    let v = Value::OldSection(s);
                    fields.fields.push((t.section, at + f as u32, v));
                }
                (Some(f), Some(Target::Symbol(_))) => {
                    let v = Value::OldIndex(u32_at(bytes, f).unwrap_or(0));
                    fields.fields.push((t.section, at + f as u32, v));
                }
                (None, Some(Target::Symbol(s))) => relocations.push(Relocation {
                    offset: at,
                    symbol: s,
                    kind: e.relocation,
                }),
                _ => {}
            }
        }
        let sec = &mut object.sections[t.section.index()];
        sec.data = SectionData::Bytes(Cow::Owned(data));
        if t.kind == Kind::Fpo {
            sec.relocations = relocations;
        }
        if t.kind == Kind::Addrsig {
            fields.uleb.push((t.section, uleb));
        }
    }
    fields
}

/// Renumbers the tables [`strip`] kept, after the edit.
pub(super) fn renumber(object: &mut Object<'_>, fields: &Fields, renumbering: &Renumbering) {
    fields.write(
        object,
        |s| {
            renumbering
                .section(s)
                .expect("a kept table's section stays")
        },
        |v| match v {
            Value::OldIndex(i) => renumbering
                .table_index(i)
                .expect("a kept entry names a symbol that stays"),
            Value::OldSection(s) => renumbering
                .section(s)
                .expect("a kept entry names a section that stays")
                .number(),
            Value::Index(_) | Value::Number(_) => unreachable!("not an edited member's"),
        },
    );
}

/// One table being made for a synthesized member.
struct Building {
    name: Vec<u8>,
    characteristics: u32,
    data: Vec<u8>,
    relocations: Vec<Relocation>,
    /// For a set, the symbols it lists already.
    listed: HashSet<SymbolId>,
    /// The fields to fill in: (offset, symbol or section in the new
    /// object).
    fields: Vec<(u32, Value)>,
    /// For a ULEB128 table, its values.
    uleb: Option<Vec<Value>>,
}

/// Puts a synthesized member's table entries in: renumbers the tables its
/// units brought, and makes its own tables for the entries of the givers'
/// tables outside COMDATs that go with its units. `placed` is where each
/// unit's sections and symbols went, and `outside` gives the symbol for a
/// target outside a unit (as relocations get one). Indices are written by
/// [`finish`], once the symbol table is complete.
pub(super) fn place<'d>(
    o: &mut Object<'d>,
    s: &Synth,
    parsed: &[Parsed<'d>],
    placed: &[Placed],
    outside: &mut dyn FnMut(&mut Object<'d>, &Object<'d>, &Symbol<'d>) -> SymbolId,
) -> Fields {
    let mut fields = Fields::default();
    let mut building: Vec<Building> = Vec::new();
    let mut by_name: HashMap<Vec<u8>, usize> = HashMap::new();
    for (k, &(m, u, _)) in s.units.iter().enumerate() {
        let object = parsed[m].object.as_ref().expect("units come from objects");
        let facts = &parsed[m].facts;
        let unit = &facts.units[u];
        let tables = &facts.tables;
        let p = &placed[k];
        let mut symbol = |o: &mut Object<'d>, old: SymbolId| match p.symbols.get(&old.0) {
            Some(&new) => new,
            None => outside(o, object, &object.symbols[old.index()]),
        };

        // The unit's own tables, copied with its sections.
        for &id in &unit.sections {
            let (Some(t), Some(&new)) = (tables.get(id), p.sections.get(&id.0)) else {
                continue;
            };
            let Some(f) = t.kind.field() else { continue };
            for e in &t.entries {
                let value = match e.target {
                    Some(Target::Symbol(old)) => Value::Index(symbol(o, old)),
                    Some(Target::Section(old)) => Value::Number(
                        *p.sections
                            .get(&old.0)
                            .expect("a unit's table names its own sections"),
                    ),
                    None => continue,
                };
                fields.fields.push((new, e.start + f as u32, value));
            }
        }

        // Entries of the member's other tables that go with the unit.
        for (ti, ei) in tables.entries_for(object, unit, u) {
            let t = &tables.tables[ti as usize];
            let e = &t.entries[ei as usize];
            let source = &object.sections[t.section.index()];
            let name = source.name.as_bytes();
            let b = *by_name.entry(name.to_vec()).or_insert_with(|| {
                building.push(Building {
                    name: name.to_vec(),
                    characteristics: source.characteristics,
                    data: source.bytes()[..t.header as usize].to_vec(),
                    relocations: Vec::new(),
                    listed: HashSet::new(),
                    fields: Vec::new(),
                    uleb: (t.kind == Kind::Addrsig).then(Vec::new),
                });
                building.len() - 1
            });
            let value = match e.target {
                Some(Target::Symbol(old)) => Value::Index(symbol(o, old)),
                Some(Target::Section(old)) => Value::Number(
                    *p.sections
                        .get(&old.0)
                        .expect("an entry that goes with a unit names its sections"),
                ),
                None => continue,
            };
            let b = &mut building[b];
            if t.kind.is_set()
                && let Value::Index(id) = value
                && !b.listed.insert(id)
            {
                continue;
            }
            if let Some(uleb) = &mut b.uleb {
                uleb.push(value);
                continue;
            }
            let at = b.data.len() as u32;
            b.data
                .extend_from_slice(&source.bytes()[e.start as usize..e.end as usize]);
            match (t.kind.field(), value) {
                (Some(f), v) => b.fields.push((at + f as u32, v)),
                (None, Value::Index(id)) => b.relocations.push(Relocation {
                    offset: at,
                    symbol: id,
                    kind: e.relocation,
                }),
                (None, _) => {}
            }
        }
    }
    for b in building {
        let mut section = Section::new(
            turbonop_coff::Name::new(b.name.clone()),
            b.characteristics,
            SectionData::Bytes(Cow::Owned(b.data)),
        );
        section.relocations = b.relocations;
        let id = o.push_section(section);
        o.push_symbol(section_symbol(&b.name, id, SectionDefinition::default()));
        fields
            .fields
            .extend(b.fields.into_iter().map(|(at, v)| (id, at, v)));
        if let Some(values) = b.uleb {
            fields.uleb.push((id, values));
        }
    }
    fields
}

/// Writes a synthesized member's table fields, once its symbols (and its
/// format, which sets the size of auxiliary records) are final.
pub(super) fn finish(o: &mut Object<'_>, fields: &Fields) {
    let indices = o.symbol_table_indices();
    fields.write(
        o,
        |s| s,
        |v| match v {
            Value::Index(id) => indices[id.index()],
            Value::Number(s) => s.number(),
            Value::OldIndex(_) | Value::OldSection(_) => unreachable!("not a new object's"),
        },
    );
}

#[cfg(test)]
mod tests;
