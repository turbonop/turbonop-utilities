//! CodeView for synthesized members (phase 2.4): each folded unit keeps its
//! symbols, lines, locals, and inline sites in the PDB.
//!
//! What compilers write, measured on the corpus (MSVC) and the fixtures
//! (MSVC and clang-cl):
//!
//! - **Per function**, a COMDAT `.debug$S` associated with the function's
//!   (or the variable's) COMDAT: its symbols (`S_GPROC32_ID` to
//!   `S_PROC_ID_END`, with its locals and inline sites; `S_GDATA32` for
//!   COMDAT data) and its lines. These are part of the fold unit, and move
//!   with it.
//! - **Per object**, the `.debug$S` that isn't a COMDAT: `S_OBJNAME`,
//!   `S_COMPILE3`, `S_BUILDINFO`, the global `S_UDT`s and `S_CONSTANT`s,
//!   the symbols of non-COMDAT functions, the file checksums, the string
//!   table, and the inlinee lines (`DEBUG_S_INLINEELINES`) of every
//!   function inlined anywhere in the object. And `.debug$T` (or a PCH
//!   object's `.debug$P`): the types and ids of the whole object.
//!
//! A synthesized member gets:
//!
//! - its units' `.debug$S`, with every type and id index renumbered into
//!   its own `.debug$T`, every file checksum offset into its own checksums,
//!   and every string offset into its own string table. Every such field
//!   is 4 bytes, so the sections are patched in place and their
//!   relocations (which name the units' own symbols) stay as they are. An
//!   inline site's file changes are compressed, though: when one's new
//!   offset takes more or fewer bytes, the section is rebuilt, and its
//!   relocations moved with the records they are in.
//! - a `.debug$T` holding the type and id records its units reach,
//!   transitively, each once (records equal once renumbered are one
//!   record), in an order in which each refers only to earlier ones. A
//!   PCH user's indices below its `LF_PRECOMP` count are its PCH object's
//!   records (the library member whose `LF_ENDPRECOMP` has the same
//!   signature), copied like any other: the synthesized member names no
//!   PCH. A `/Zi` member's indices are in its type server, which the
//!   environment key makes common to the units; the synthesized member
//!   names the same type server, and keeps the indices.
//! - a per-object `.debug$S`: `S_OBJNAME` (`turbolib:<member name>`),
//!   `S_COMPILE3` copied from the first unit's member (the environment key
//!   holds it, so the units' members all have the same), the inlinee lines
//!   of the functions inlined in its units, the file checksums its units'
//!   lines and inline sites name, and the string table. No `S_BUILDINFO`:
//!   the member wasn't compiled from any one command line, and a tool that
//!   recompiles a module from it (Edit and Continue, Live++) would compile
//!   the wrong source.
//!
//! The original members keep their per-object `.debug$S` and their
//! `.debug$T` as they were. What's left of them can name a removed
//! function's id (inlinee lines do) but nothing relocates to a removed
//! local symbol: [`units`] refuses a unit whose local symbols anything
//! outside it, debug sections included, relocates to.
//!
//! Before folding, [`read`] checks that every unit's debug information can
//! be carried (every index, offset, and record understood), so that
//! [`Builder`] can't fail. A unit whose debug information can't be carried
//! doesn't fold ([`Unfoldable::DebugInfo`]), nor does a PCH user's whose
//! PCH object isn't in the library ([`Unfoldable::PrecompMissing`]).

use super::{Parsed, Unfoldable, units};
use crate::hash::h64;
use std::borrow::Cow;
use std::collections::{HashMap, HashSet};
use turbonop_codeview::annotations;
use turbonop_codeview::c13::{self, CV_SIGNATURE_C13, InlineeSource, StringTableBuilder};
use turbonop_codeview::lines::subsection as sub;
use turbonop_codeview::remap::{self, Remap};
use turbonop_codeview::symbol_refs::{self, Field};
use turbonop_codeview::symbols::kind as sym;
use turbonop_codeview::type_refs::{self, Ref};
use turbonop_codeview::types::leaf;
use turbonop_codeview::write;
use turbonop_coff::{
    Aux, Object, Section, SectionData, SectionDefinition, SectionId, SectionNumber,
};
use turbonop_coff::{Name, Symbol, storage_class};

/// The first index that isn't a built-in type.
const FIRST: u32 = 0x1000;
const IMAGE_SCN_LNK_COMDAT: u32 = 0x1000;
/// MSVC's per-object `.debug$S` and `.debug$T`: initialized, discardable,
/// read-only, 1-byte aligned.
const DEBUG_CHARACTERISTICS: u32 = 0x4210_0040;

/// A member's types (its `.debug$T`, or a PCH object's `.debug$P`).
#[derive(Clone, Debug, Default)]
pub(super) enum Types {
    /// None: only built-in types can be named.
    #[default]
    None,
    /// Records, in `section`: record `base + i` starts at `offsets[i]`
    /// (its length field), in index order.
    Records {
        section: SectionId,
        base: u32,
        offsets: Vec<u32>,
        /// Its `LF_PRECOMP`, if it uses a PCH.
        precomp: Option<Precomp>,
        /// Its `LF_ENDPRECOMP`'s signature and index, if it is a PCH
        /// object.
        end_precomp: Option<(u32, u32)>,
    },
    /// `/Zi`: the type server record (kind and body), whose PDB holds the
    /// types.
    Server(Vec<u8>),
    /// Unusable, and why.
    Bad(String),
}

/// An `LF_PRECOMP`: indices from `start`, `count` of them, are the PCH
/// object's whose `LF_ENDPRECOMP` has `signature`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct Precomp {
    pub start: u32,
    pub count: u32,
    pub signature: u32,
}

/// What one member's debug information is, for synthesized members.
#[derive(Clone, Debug, Default)]
pub(super) struct Facts {
    pub types: Types,
    /// The PCH object's member, for a PCH user whose PCH is in the
    /// library ([`resolve`]).
    pub pch: Option<usize>,
    /// The per-object file checksums subsection's contents, and where each
    /// entry starts.
    checksums: Vec<u8>,
    entries: Vec<u32>,
    /// The per-object string table.
    strings: Vec<u8>,
    /// The per-object inlinee lines, by function id.
    inlinees: HashMap<u32, InlineeSource>,
    /// The `S_COMPILE3` record (kind and body).
    compile3: Option<Vec<u8>>,
    /// The per-object `.debug$S`'s characteristics.
    characteristics: Option<u32>,
}

fn le32(b: &[u8], at: usize) -> u32 {
    u32::from_le_bytes([b[at], b[at + 1], b[at + 2], b[at + 3]])
}

fn is_comdat(s: &Section<'_>) -> bool {
    s.characteristics & IMAGE_SCN_LNK_COMDAT != 0
}

/// Reads `data` (a `.debug$T` or `.debug$P`, signature first) as a type
/// stream: every record's index fields must name a built-in type, an
/// earlier record, or the PCH's records.
fn read_types(section: SectionId, data: &[u8]) -> Types {
    if data.len() < 4 || le32(data, 0) != CV_SIGNATURE_C13 {
        return Types::Bad("no C13 signature".into());
    }
    let mut offsets = Vec::new();
    let mut precomp = None;
    let mut end_precomp = None;
    let mut base = FIRST;
    let mut refs = Vec::new();
    for r in remap::type_records(&data[4..]) {
        let Ok((at, record)) = r else {
            return Types::Bad("a record overruns the section".into());
        };
        let at = at + 4;
        if offsets.is_empty() && precomp.is_none() {
            if record.kind == leaf::LF_TYPESERVER2 {
                let len = usize::from(u16::from_le_bytes([data[at], data[at + 1]]));
                return Types::Server(data[at + 2..at + 2 + len].to_vec());
            }
            if record.kind == leaf::LF_PRECOMP && record.data.len() >= 12 {
                let p = Precomp {
                    start: le32(record.data, 0),
                    count: le32(record.data, 4),
                    signature: le32(record.data, 8),
                };
                if p.start != FIRST || p.count >= u32::MAX - FIRST {
                    return Types::Bad(format!("LF_PRECOMP from {:#x}", p.start));
                }
                base = p.start + p.count;
                precomp = Some(p);
                continue;
            }
        }
        let index = base + offsets.len() as u32;
        if record.kind == leaf::LF_TYPESERVER2 || record.kind == leaf::LF_PRECOMP {
            return Types::Bad(format!("{:#06x} at {index:#x}", record.kind));
        }
        if record.kind == leaf::LF_ENDPRECOMP && record.data.len() >= 4 && end_precomp.is_none() {
            end_precomp = Some((le32(record.data, 0), index));
        }
        refs.clear();
        if let Err(e) = type_refs::refs(record.kind, record.data, &mut refs) {
            return Types::Bad(format!("record {index:#x}: {e}"));
        }
        for r in &refs {
            let v = le32(record.data, r.offset as usize);
            let earlier = v < FIRST
                || (base..index).contains(&v)
                || precomp.is_some_and(|p: Precomp| v >= p.start && v - p.start < p.count);
            if !earlier {
                return Types::Bad(format!(
                    "record {index:#x} names {v:#x}, not an earlier record"
                ));
            }
        }
        offsets.push(at as u32);
    }
    Types::Records {
        section,
        base,
        offsets,
        precomp,
        end_precomp,
    }
}

/// Reads a member's debug information, and marks each unit whose own
/// debug information can't be carried into a synthesized member
/// ([`Unfoldable::DebugInfo`]).
pub(super) fn read(object: &Object<'_>, facts: &mut units::Facts) -> Facts {
    let mut d = Facts::default();
    let mut bad: Option<String> = None;

    // Types: one .debug$T, or one .debug$P (a PCH object), not COMDATs.
    let mut types = None;
    for (i, s) in object.sections.iter().enumerate() {
        let name = s.name.as_bytes();
        if name == b".debug$T" || name == b".debug$P" {
            if is_comdat(s) || types.is_some() {
                bad.get_or_insert_with(|| "more than one .debug$T, or one in a COMDAT".into());
            }
            types = Some(i);
        }
    }
    if let Some(i) = types {
        d.types = read_types(SectionId(i as u32), object.sections[i].bytes());
        if let Types::Bad(why) = &d.types {
            bad.get_or_insert_with(|| format!("its types: {why}"));
        }
    }

    // The per-object .debug$S: its checksums, strings, inlinee lines, and
    // compiler.
    let mut checksums = 0;
    let mut tables = 0;
    for s in &object.sections {
        if s.name.as_bytes() != b".debug$S" || is_comdat(s) {
            continue;
        }
        d.characteristics.get_or_insert(s.characteristics);
        let Ok(subsections) = c13::section_subsections(s.bytes()) else {
            bad.get_or_insert_with(|| "its .debug$S isn't C13".into());
            continue;
        };
        for x in subsections {
            match x.kind {
                sub::DEBUG_S_FILECHKSMS => {
                    checksums += 1;
                    match c13::file_checksums(x.data) {
                        Ok(entries) => {
                            d.entries = entries.iter().map(|&(at, _)| at).collect();
                            d.checksums = x.data.to_vec();
                        }
                        Err(e) => {
                            bad.get_or_insert_with(|| format!("its file checksums: {e}"));
                        }
                    }
                }
                sub::DEBUG_S_STRINGTABLE => {
                    tables += 1;
                    d.strings = x.data.to_vec();
                }
                sub::DEBUG_S_INLINEELINES => match c13::InlineeLines::parse(x.data) {
                    Ok(lines) => {
                        for e in lines.entries {
                            d.inlinees.entry(e.inlinee).or_insert(e);
                        }
                    }
                    Err(e) => {
                        bad.get_or_insert_with(|| format!("its inlinee lines: {e}"));
                    }
                },
                sub::DEBUG_S_SYMBOLS if d.compile3.is_none() => {
                    for r in c13::symbol_records(x.data).unwrap_or_default() {
                        if r.kind == sym::S_COMPILE3 {
                            let mut v = r.kind.to_le_bytes().to_vec();
                            v.extend_from_slice(r.data);
                            d.compile3 = Some(v);
                            break;
                        }
                    }
                }
                _ => {}
            }
        }
    }
    if checksums > 1 || tables > 1 {
        bad.get_or_insert_with(|| "more than one file checksums or string table".into());
    }

    // Each unit's debug sections.
    for u in &mut facts.units {
        if u.unfoldable.is_some() {
            continue;
        }
        let mut why = None;
        for &id in &u.sections {
            let s = &object.sections[id.index()];
            let name = s.name.as_bytes();
            if !units::is_debug(name) {
                continue;
            }
            why = if let Some(b) = &bad {
                Some(b.clone())
            } else if name != b".debug$S" {
                Some(format!("a {} in a COMDAT", String::from_utf8_lossy(name)))
            } else {
                check_section(&d, s.bytes()).err()
            };
            if why.is_some() {
                break;
            }
        }
        if let Some(w) = why {
            u.unfoldable = Some(Unfoldable::DebugInfo);
            u.detail = Some(w);
        }
    }
    d
}

/// Whether the unit's `.debug$S` section `data` can be carried: C13; only
/// symbols, lines, frame data, inlinee lines, and cross-scope imports and
/// exports (or subsections marked ignored); every field naming something
/// the member has; no links between records set (objects leave them 0).
fn check_section(d: &Facts, data: &[u8]) -> Result<(), String> {
    let subsections = c13::section_subsections(data).map_err(|e| e.to_string())?;
    for x in subsections {
        match x.kind {
            sub::DEBUG_S_SYMBOLS
            | sub::DEBUG_S_LINES
            | sub::DEBUG_S_IL_LINES
            | sub::DEBUG_S_FRAMEDATA
            | sub::DEBUG_S_INLINEELINES
            | sub::DEBUG_S_CROSSSCOPEIMPORTS
            | sub::DEBUG_S_CROSSSCOPEEXPORTS => {}
            k if k & sub::DEBUG_S_IGNORE != 0 => continue,
            k => {
                return Err(format!(
                    "a subsection of kind {k:#x} in a COMDAT's .debug$S"
                ));
            }
        }
        let mut problem: Option<String> = None;
        if x.kind == sub::DEBUG_S_SYMBOLS {
            // Record by record: annotations run to their record's end.
            for r in c13::symbol_records(x.data).map_err(|e| e.to_string())? {
                symbol_refs::fields(r.kind, r.data, |f| {
                    if problem.is_none() {
                        problem = check_field(d, r.data, f).err();
                    }
                })
                .map_err(|e| e.to_string())?;
            }
        } else {
            c13::fields(x.kind, x.data, |f| {
                if problem.is_none() {
                    problem = check_field(d, x.data, f).err();
                }
            })
            .map_err(|e| e.to_string())?;
        }
        if let Some(p) = problem {
            return Err(p);
        }
    }
    Ok(())
}

fn check_field(d: &Facts, data: &[u8], f: Field) -> Result<(), String> {
    match f {
        Field::Type(at) | Field::Id(at) => {
            let v = le32(data, at as usize);
            if !index_ok(&d.types, v) {
                return Err(format!("index {v:#x} isn't in its types"));
            }
        }
        Field::String(at) => {
            let v = le32(data, at as usize);
            if string(&d.strings, v).is_none() {
                return Err(format!("string offset {v:#x} isn't in its string table"));
            }
        }
        Field::Checksum(at) => {
            let v = le32(data, at as usize);
            if d.entries.binary_search(&v).is_err() {
                return Err(format!("file checksum offset {v:#x} isn't an entry"));
            }
        }
        Field::Annotations(at) => {
            let files = annotations::files(&data[at as usize..])
                .ok_or_else(|| "binary annotations that don't decode".to_string())?;
            if let Some(v) = files.iter().find(|v| d.entries.binary_search(v).is_err()) {
                return Err(format!("an inline site's file {v:#x} isn't an entry"));
            }
        }
        Field::Link(at) => {
            if le32(data, at as usize) != 0 {
                return Err("a link between records that isn't 0".into());
            }
        }
        Field::Address { .. } | Field::Rva(_) | Field::Token(_) => {}
    }
    Ok(())
}

fn index_ok(types: &Types, v: u32) -> bool {
    if v < FIRST {
        return true;
    }
    match types {
        Types::Server(_) => true,
        Types::Records {
            base,
            offsets,
            precomp,
            ..
        } => {
            (v >= *base && v - base < offsets.len() as u32)
                || precomp.is_some_and(|p| v >= p.start && v - p.start < p.count)
        }
        Types::None | Types::Bad(_) => false,
    }
}

/// The string at `offset` in `table`, without its zero.
fn string(table: &[u8], offset: u32) -> Option<&[u8]> {
    let rest = table.get(offset as usize..)?;
    let end = rest.iter().position(|&b| b == 0)?;
    Some(&rest[..end])
}

/// Finds each PCH user's PCH object among the members, by the signature
/// of its `LF_PRECOMP` and the PCH's `LF_ENDPRECOMP` (and that the user's
/// count ends where the PCH's records do). A user whose PCH isn't there
/// (or whose types are unusable) can't fold a unit with debug sections
/// ([`Unfoldable::PrecompMissing`]): its types aren't in the library.
pub(super) fn resolve(parsed: &mut [Parsed<'_>]) {
    // PCH objects by signature; a signature two different PCHs have is
    // no one's.
    let mut pchs: HashMap<u32, Option<usize>> = HashMap::new();
    for (m, p) in parsed.iter().enumerate() {
        if let Types::Records {
            end_precomp: Some((signature, _)),
            precomp: None,
            ..
        } = p.debug.types
        {
            pchs.entry(signature)
                .and_modify(|e| {
                    if let Some(first) = *e
                        && !same_types(&parsed[first], p)
                    {
                        *e = None;
                    }
                })
                .or_insert(Some(m));
        }
    }
    for m in 0..parsed.len() {
        let Types::Records {
            precomp: Some(p), ..
        } = parsed[m].debug.types
        else {
            continue;
        };
        let found = pchs.get(&p.signature).copied().flatten().filter(|&pch| {
            matches!(parsed[pch].debug.types,
                Types::Records { end_precomp: Some((_, end)), .. } if end == p.start + p.count)
        });
        parsed[m].debug.pch = found;
        if found.is_some() {
            continue;
        }
        let object = parsed[m].object.as_ref().expect("types come from objects");
        let has_debug: Vec<bool> = parsed[m]
            .facts
            .units
            .iter()
            .map(|u| {
                u.sections
                    .iter()
                    .any(|s| units::is_debug(object.sections[s.index()].name.as_bytes()))
            })
            .collect();
        for (u, has) in parsed[m].facts.units.iter_mut().zip(has_debug) {
            if has && u.unfoldable.is_none() {
                u.unfoldable = Some(Unfoldable::PrecompMissing);
                u.detail = Some(format!(
                    "its PCH (signature {:#010x}) isn't in the library",
                    p.signature
                ));
            }
        }
    }
}

/// The part of a member's debug information its environment key holds:
/// the type server a `/Zi` member names (its record), whose indices its
/// units' `.debug$S` use as they are.
pub(super) fn environment(object: &Object<'_>) -> Option<Vec<u8>> {
    let s = object
        .sections
        .iter()
        .find(|s| s.name.as_bytes() == b".debug$T" && !is_comdat(s))?;
    let data = s.bytes();
    let (_, first) = remap::type_records(data.get(4..)?).next()?.ok()?;
    (le32(data, 0) == CV_SIGNATURE_C13 && first.kind == leaf::LF_TYPESERVER2).then(|| {
        let mut v = first.kind.to_le_bytes().to_vec();
        v.extend_from_slice(first.data);
        // Not the age: each compile into the PDB writes a later one, and a
        // type server only adds records, so an earlier age's indices are
        // the same in a later one.
        if let Some(age) = v.get_mut(SERVER_AGE..SERVER_AGE + 4) {
            age.fill(0);
        }
        v
    })
}

/// Where `LF_TYPESERVER2`'s age is in its record (kind and body): after
/// the kind and the GUID.
const SERVER_AGE: usize = 2 + 16;

/// Whether two PCH objects' type sections are the same bytes.
fn same_types(a: &Parsed<'_>, b: &Parsed<'_>) -> bool {
    let bytes = |p: &Parsed<'_>| match (&p.debug.types, &p.object) {
        (Types::Records { section, .. }, Some(o)) => {
            Some(o.sections[section.index()].bytes().to_vec())
        }
        _ => None,
    };
    bytes(a) == bytes(b)
}

/// Where a record of a rebuilt section went: the bytes from `old` to
/// `old_end` in the original are at `new` in the rebuilt one.
#[derive(Clone, Copy, Debug)]
struct Moved {
    old: u32,
    old_end: u32,
    new: u32,
}

/// A synthesized member's CodeView, built as its units are copied in.
pub(super) struct Builder<'p, 'a> {
    parsed: &'p [Parsed<'a>],
    /// Its type records, from `FIRST`, and where each starts.
    types: Vec<u8>,
    starts: Vec<u32>,
    /// New records by a hash of their bytes (collisions in `collided`).
    by_hash: HashMap<u64, u32>,
    collided: HashMap<Vec<u8>, u32>,
    /// (member, index) to new index, for indices from `FIRST`.
    merged: HashMap<(u32, u32), u32>,
    /// The type server named, for `/Zi` units.
    server: Option<Vec<u8>>,
    /// (member, checksum offset) to new offset.
    files: HashMap<(u32, u32), u32>,
    checksums: Vec<u8>,
    checksum_at: HashMap<(u32, u8, Vec<u8>), u32>,
    strings: StringTableBuilder,
    inlinees: Vec<InlineeSource>,
    inlined: HashSet<u32>,
    /// Rebuilt sections' moved records, by (unit position, section).
    moved: HashMap<(usize, u32), Vec<Moved>>,
    /// The per-object `.debug$S`, if any unit has debug information.
    main: Option<SectionId>,
}

/// Renumbering for one member's records into a [`Builder`].
struct Renumber<'b, 'p, 'a> {
    b: &'b mut Builder<'p, 'a>,
    m: usize,
}

impl Remap for Renumber<'_, '_, '_> {
    fn index(&mut self, index: u32, _id: bool) -> Option<u32> {
        Some(self.b.index(self.m, index))
    }

    fn string(&mut self, offset: u32) -> Option<u32> {
        Some(self.b.string(self.m, offset))
    }

    fn checksum(&mut self, offset: u32) -> Option<u32> {
        Some(self.b.file(self.m, offset))
    }
}

/// A member's index `v` where its record is: in its PCH object, for a PCH
/// user's index below its count.
fn canonical(parsed: &[Parsed<'_>], m: usize, v: u32) -> (usize, u32) {
    if let Types::Records {
        precomp: Some(p), ..
    } = parsed[m].debug.types
        && v >= p.start
        && v - p.start < p.count
    {
        return (parsed[m].debug.pch.expect("a resolved PCH user"), v);
    }
    (m, v)
}

/// Member `m`'s record `v` (its length, kind, body, and padding).
fn record<'p>(parsed: &'p [Parsed<'_>], m: usize, v: u32) -> &'p [u8] {
    let Types::Records {
        section,
        base,
        offsets,
        ..
    } = &parsed[m].debug.types
    else {
        panic!("an index into a member without records");
    };
    let data = parsed[m].object.as_ref().expect("an object").sections[section.index()].bytes();
    let at = offsets[(v - base) as usize] as usize;
    let len = usize::from(u16::from_le_bytes([data[at], data[at + 1]]));
    &data[at..at + 2 + len]
}

impl<'p, 'a> Builder<'p, 'a> {
    pub(super) fn new(parsed: &'p [Parsed<'a>]) -> Self {
        Self {
            parsed,
            types: Vec::new(),
            starts: Vec::new(),
            by_hash: HashMap::new(),
            collided: HashMap::new(),
            merged: HashMap::new(),
            server: None,
            files: HashMap::new(),
            checksums: Vec::new(),
            checksum_at: HashMap::new(),
            strings: StringTableBuilder::new(),
            inlinees: Vec::new(),
            inlined: HashSet::new(),
            moved: HashMap::new(),
            main: None,
        }
    }

    /// Adds the per-object `.debug$S` to `o` (filled in by [`finish`](Self::finish)),
    /// if any of `units` (member, unit, renamed) has debug sections.
    pub(super) fn begin(&mut self, o: &mut Object<'_>, units: &[(usize, usize, bool)]) {
        let parsed = self.parsed;
        let mut m0 = None;
        for &(m, u, _) in units {
            let object = parsed[m].object.as_ref().expect("units come from objects");
            if parsed[m].facts.units[u]
                .sections
                .iter()
                .any(|s| units::is_debug(object.sections[s.index()].name.as_bytes()))
            {
                m0 = Some(m);
                break;
            }
        }
        let Some(m0) = m0 else { return };
        let characteristics = parsed[m0]
            .debug
            .characteristics
            .unwrap_or(DEBUG_CHARACTERISTICS);
        self.main = Some(push(o, b".debug$S", characteristics, Vec::new()));
    }

    /// The new index of member `m`'s index `v`, merging the record and
    /// everything it reaches.
    fn index(&mut self, m: usize, v: u32) -> u32 {
        if v < FIRST {
            return v;
        }
        if let Types::Server(record) = &self.parsed[m].debug.types {
            // The latest age any unit names.
            let age = |r: &[u8]| r.get(SERVER_AGE..SERVER_AGE + 4).map(|a| le32(a, 0));
            if self.server.as_deref().is_none_or(|s| age(s) < age(record)) {
                self.server = Some(record.clone());
            }
            return v;
        }
        let (m, v) = canonical(self.parsed, m, v);
        if let Some(&n) = self.merged.get(&(m as u32, v)) {
            return n;
        }
        self.merge(m, v)
    }

    /// Merges member `m`'s record `v` and what it reaches, depth first: a
    /// record goes in once everything it names has.
    fn merge(&mut self, m: usize, v: u32) -> u32 {
        struct Frame<'p> {
            m: usize,
            v: u32,
            record: &'p [u8],
            refs: std::ops::Range<usize>,
            next: usize,
        }
        let parsed = self.parsed;
        let mut refs: Vec<Ref> = Vec::new();
        let mut stack: Vec<Frame<'p>> = Vec::new();
        let mut bytes = Vec::new();
        let push = |stack: &mut Vec<Frame<'p>>, refs: &mut Vec<Ref>, m: usize, v: u32| {
            let record = record(parsed, m, v);
            let kind = u16::from_le_bytes([record[2], record[3]]);
            let start = refs.len();
            type_refs::refs(kind, &record[4..], refs).expect("records checked when read");
            stack.push(Frame {
                m,
                v,
                record,
                refs: start..refs.len(),
                next: start,
            });
        };
        push(&mut stack, &mut refs, m, v);
        while let Some(top) = stack.last_mut() {
            let mut child = None;
            while top.next < top.refs.end {
                let r = refs[top.next];
                let value = le32(top.record, 4 + r.offset as usize);
                if value >= FIRST {
                    let c = canonical(parsed, top.m, value);
                    if !self.merged.contains_key(&(c.0 as u32, c.1)) {
                        child = Some(c);
                        break;
                    }
                }
                top.next += 1;
            }
            if let Some((cm, cv)) = child {
                push(&mut stack, &mut refs, cm, cv);
                continue;
            }
            let f = stack.pop().expect("the top");
            bytes.clear();
            bytes.extend_from_slice(f.record);
            for r in &refs[f.refs.clone()] {
                let at = 4 + r.offset as usize;
                let value = le32(&bytes, at);
                if value >= FIRST {
                    let c = canonical(parsed, f.m, value);
                    let n = self.merged[&(c.0 as u32, c.1)];
                    bytes[at..at + 4].copy_from_slice(&n.to_le_bytes());
                }
            }
            refs.truncate(f.refs.start);
            let n = self.intern(&bytes);
            self.merged.insert((f.m as u32, f.v), n);
        }
        self.merged[&(m as u32, v)]
    }

    /// The index of a record with these bytes: an equal one's, or a new
    /// one.
    fn intern(&mut self, bytes: &[u8]) -> u32 {
        let h = h64(bytes);
        if let Some(&n) = self.by_hash.get(&h) {
            let i = (n - FIRST) as usize;
            let start = self.starts[i] as usize;
            let end = self
                .starts
                .get(i + 1)
                .map_or(self.types.len(), |&e| e as usize);
            if &self.types[start..end] == bytes {
                return n;
            }
            if let Some(&n) = self.collided.get(bytes) {
                return n;
            }
            let n = self.push_record(bytes);
            self.collided.insert(bytes.to_vec(), n);
            return n;
        }
        let n = self.push_record(bytes);
        self.by_hash.insert(h, n);
        n
    }

    fn push_record(&mut self, bytes: &[u8]) -> u32 {
        let n = FIRST + self.starts.len() as u32;
        self.starts.push(self.types.len() as u32);
        self.types.extend_from_slice(bytes);
        n
    }

    /// The new offset of member `m`'s string at `offset`.
    fn string(&mut self, m: usize, offset: u32) -> u32 {
        let s = string(&self.parsed[m].debug.strings, offset).expect("strings checked when read");
        self.strings.insert(s)
    }

    /// The new offset of member `m`'s file checksum entry at `offset`.
    fn file(&mut self, m: usize, offset: u32) -> u32 {
        if let Some(&n) = self.files.get(&(m as u32, offset)) {
            return n;
        }
        let d = &self.parsed[m].debug;
        let at = offset as usize;
        let name = le32(&d.checksums, at);
        let len = usize::from(d.checksums[at + 4]);
        let kind = d.checksums[at + 5];
        let checksum = &d.checksums[at + 6..at + 6 + len];
        let name = self.string(m, name);
        let key = (name, kind, checksum.to_vec());
        let n = match self.checksum_at.get(&key) {
            Some(&n) => n,
            None => {
                let n = c13::push_file_checksum(
                    &mut self.checksums,
                    &turbonop_codeview::lines::FileChecksum {
                        name,
                        kind,
                        checksum,
                    },
                );
                self.checksum_at.insert(key, n);
                n
            }
        };
        self.files.insert((m as u32, offset), n);
        n
    }

    /// Member `m`'s inlinee lines entry for its function id `id`, if it
    /// has one, carried (once per function).
    fn inlinee(&mut self, m: usize, id: u32) {
        let Some(e) = self.parsed[m].debug.inlinees.get(&id) else {
            return;
        };
        let e = e.clone();
        let inlinee = self.index(m, e.inlinee);
        if !self.inlined.insert(inlinee) {
            return;
        }
        let file = self.file(m, e.file);
        let extra_files = e.extra_files.iter().map(|&f| self.file(m, f)).collect();
        self.inlinees.push(InlineeSource {
            inlinee,
            file,
            line: e.line,
            extra_files,
        });
    }

    /// A copy of member `m`'s debug section `section` (its `id`), for the
    /// unit at position `k`, renumbered into this member.
    pub(super) fn section(
        &mut self,
        k: usize,
        m: usize,
        id: SectionId,
        section: &Section<'_>,
    ) -> Section<'static> {
        let src = section.bytes();
        let mut data = src.to_vec();
        match remap::debug_s(&mut data, &mut Renumber { b: self, m }) {
            Ok(()) => {}
            Err(remap::Error::AnnotationLength { .. }) => {
                let (rebuilt, moved) = self.rebuild(m, src);
                data = rebuilt;
                self.moved.insert((k, id.0), moved);
            }
            Err(e) => panic!("a unit's .debug$S checked when read: {e}"),
        }
        // The inline sites' functions' inlinee lines.
        for x in c13::section_subsections(src).expect("checked when read") {
            if x.kind != sub::DEBUG_S_SYMBOLS {
                continue;
            }
            for r in c13::symbol_records(x.data).expect("checked when read") {
                if (r.kind == sym::S_INLINESITE || r.kind == sym::S_INLINESITE2)
                    && r.data.len() >= 12
                {
                    self.inlinee(m, le32(r.data, 8));
                }
            }
        }
        let mut copy = Section::new(
            Name::new(section.name.as_bytes().to_vec()),
            section.characteristics,
            SectionData::Bytes(Cow::Owned(data)),
        );
        copy.virtual_size = section.virtual_size;
        copy.virtual_address = section.virtual_address;
        copy
    }

    /// Where a relocation at `offset` in the unit at position `k`'s section
    /// `id` goes in its copy.
    pub(super) fn offset(&self, k: usize, id: SectionId, offset: u32) -> u32 {
        let Some(moved) = self.moved.get(&(k, id.0)) else {
            return offset;
        };
        let i = moved.partition_point(|m| m.old_end <= offset);
        match moved.get(i) {
            Some(m) if m.old <= offset => m.new + (offset - m.old),
            _ => panic!("a relocation outside the records of a rebuilt .debug$S"),
        }
    }

    /// Member `m`'s `.debug$S` `src` renumbered record by record, for when
    /// an inline site's annotations change length: the new bytes, and
    /// where each record (and each other subsection's contents) moved.
    fn rebuild(&mut self, m: usize, src: &[u8]) -> (Vec<u8>, Vec<Moved>) {
        let mut out = CV_SIGNATURE_C13.to_le_bytes().to_vec();
        let mut moved = Vec::new();
        let mut fields = Vec::new();
        let mut at = 4;
        while at < src.len() {
            let kind = le32(src, at);
            let len = le32(src, at + 4) as usize;
            let data = &src[at + 8..at + 8 + len];
            let header = out.len();
            out.extend_from_slice(&kind.to_le_bytes());
            out.extend_from_slice(&0u32.to_le_bytes());
            let start = out.len();
            if kind == sub::DEBUG_S_SYMBOLS {
                for r in c13::symbol_records(data).expect("checked when read") {
                    let old = (at + 8) as u32 + r.offset;
                    let new = out.len() as u32;
                    let body = self.symbol(m, r.kind, r.data, &mut fields);
                    out.extend_from_slice(&((body.len() + 2) as u16).to_le_bytes());
                    out.extend_from_slice(&r.kind.to_le_bytes());
                    out.extend_from_slice(&body);
                    moved.push(Moved {
                        old,
                        old_end: old + 4 + r.data.len() as u32,
                        new,
                    });
                }
            } else {
                let mut one = Vec::with_capacity(8 + len + 3);
                one.extend_from_slice(&src[at..at + 8 + len]);
                one.resize(8 + len + (len.wrapping_neg() & 3), 0);
                remap::subsections(&mut one, &mut Renumber { b: self, m })
                    .expect("checked when read");
                moved.push(Moved {
                    old: (at + 8) as u32,
                    old_end: (at + 8 + len) as u32,
                    new: start as u32,
                });
                out.extend_from_slice(&one[8..8 + len]);
            }
            let n = (out.len() - start) as u32;
            out[header + 4..header + 8].copy_from_slice(&n.to_le_bytes());
            out.resize(out.len() + ((n as usize).wrapping_neg() & 3), 0);
            at += 8 + len + (len.wrapping_neg() & 3);
        }
        (out, moved)
    }

    /// A symbol record's body renumbered; one whose annotations change
    /// length is written again, as long as it was 4-byte aligned if it
    /// was.
    fn symbol(&mut self, m: usize, kind: u16, data: &[u8], fields: &mut Vec<Field>) -> Vec<u8> {
        let mut body = data.to_vec();
        match remap::symbol_record(kind, &mut body, fields, &mut Renumber { b: self, m }) {
            Ok(()) => return body,
            Err(remap::Error::AnnotationLength { .. }) => {}
            Err(e) => panic!("a symbol record checked when read: {e}"),
        }
        let mut annotations_at = None;
        symbol_refs::fields(kind, data, |f| {
            if let Field::Annotations(a) = f {
                annotations_at = Some(a as usize);
            }
        })
        .expect("checked when read");
        let a = annotations_at.expect("annotations changed length");
        let mut head = data[..a].to_vec();
        remap::symbol_record(kind, &mut head, fields, &mut Renumber { b: self, m })
            .expect("the fields before the annotations");
        let mut files = Vec::new();
        for v in annotations::files(&data[a..]).expect("checked when read") {
            files.push((v, self.file(m, v)));
        }
        annotations::rewrite_files(&data[a..], &mut head, |v| {
            files
                .iter()
                .find(|&&(old, _)| old == v)
                .map_or(v, |&(_, new)| new)
        })
        .expect("a new file offset fits");
        if (4 + data.len()).is_multiple_of(4) {
            head.resize(head.len() + (4 + head.len()).wrapping_neg() % 4, 0);
        }
        head
    }

    /// Adds the per-object `.debug$S` and the `.debug$T` to `o`, if any
    /// unit had debug information: `name` is the member's, and `m0` the
    /// member whose compiler it names.
    pub(super) fn finish(mut self, o: &mut Object<'_>, m0: usize, name: &[u8]) {
        let Some(main) = self.main else {
            return;
        };
        let parsed = self.parsed;
        let d0 = &parsed[m0].debug;
        let mut symbols = Vec::new();
        let mut objname = b"turbolib:".to_vec();
        objname.extend_from_slice(name);
        write::obj_name(0, &objname)
            .write_symbol(&mut symbols, 1)
            .expect("a short record");
        if let Some(c) = &d0.compile3 {
            symbols.extend_from_slice(&((c.len()) as u16).to_le_bytes());
            symbols.extend_from_slice(c);
        }
        let mut s = CV_SIGNATURE_C13.to_le_bytes().to_vec();
        c13::push_subsection(&mut s, sub::DEBUG_S_SYMBOLS, &symbols);
        if !self.inlinees.is_empty() {
            let lines = c13::InlineeLines {
                extended: self.inlinees.iter().any(|e| !e.extra_files.is_empty()),
                entries: std::mem::take(&mut self.inlinees),
            };
            let mut v = Vec::new();
            lines.write(&mut v);
            c13::push_subsection(&mut s, sub::DEBUG_S_INLINEELINES, &v);
        }
        if !self.checksums.is_empty() {
            c13::push_subsection(&mut s, sub::DEBUG_S_FILECHKSMS, &self.checksums);
        }
        if !self.checksums.is_empty() || self.strings.bytes().len() > 1 {
            c13::push_subsection(&mut s, sub::DEBUG_S_STRINGTABLE, self.strings.bytes());
        }
        let characteristics = o.sections[main.index()].characteristics;
        o.sections[main.index()].data = SectionData::Bytes(Cow::Owned(s));

        let types = match self.server.take() {
            Some(record) => {
                let mut t = CV_SIGNATURE_C13.to_le_bytes().to_vec();
                t.extend_from_slice(&(record.len() as u16).to_le_bytes());
                t.extend_from_slice(&record);
                Some(t)
            }
            None if !self.types.is_empty() => {
                let mut t = CV_SIGNATURE_C13.to_le_bytes().to_vec();
                t.append(&mut self.types);
                Some(t)
            }
            None => None,
        };
        if let Some(t) = types {
            push(o, b".debug$T", characteristics, t);
        }
    }
}

/// Adds a section and its section symbol.
fn push(o: &mut Object<'_>, name: &[u8], characteristics: u32, data: Vec<u8>) -> SectionId {
    let id = o.push_section(Section::new(
        Name::new(name.to_vec()),
        characteristics,
        SectionData::Bytes(Cow::Owned(data)),
    ));
    let mut s = Symbol::new(
        Name::new(name.to_vec()),
        0,
        SectionNumber::Section(id),
        0,
        storage_class::STATIC,
    );
    s.aux = Aux::Section(SectionDefinition::default());
    o.push_symbol(s);
    id
}

#[cfg(test)]
mod tests;
