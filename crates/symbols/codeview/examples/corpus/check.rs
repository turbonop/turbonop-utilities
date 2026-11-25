//! The checks of one object's CodeView (see the example's documentation):
//! shared by the corpus example and the fixtures' test.

// Offsets in objects and records are 32-bit.
#![allow(clippy::cast_possible_truncation)]

use std::collections::{BTreeMap, HashMap, HashSet};
use std::sync::Arc;
use turbonop_codeview::c13::{self, CV_SIGNATURE_C13, Contents, DebugS};
use turbonop_codeview::lines::subsection;
use turbonop_codeview::remap::{self, Remap};
use turbonop_codeview::symbol_refs::{self, Field};
use turbonop_codeview::symbols::kind as sk;
use turbonop_codeview::type_refs;
use turbonop_codeview::types::{leaf, simple};
use turbonop_codeview::{annotations, write};
use turbonop_pe::{FileKind, PeFile};

/// Up to this many examples of each failure are kept.
pub const EXAMPLES: usize = 5;

#[derive(Default)]
pub struct Stats {
    pub inputs: u64,
    pub objects: u64,
    pub not_objects: u64,
    pub bytes: u64,
    pub debug_t: u64,
    pub debug_p: u64,
    pub debug_s: u64,
    pub precomp_users: u64,
    pub type_servers: u64,
    pub type_records: u64,
    pub type_bytes: u64,
    pub symbol_records: u64,
    pub debug_s_bytes: u64,
    pub type_kinds: BTreeMap<u16, u64>,
    pub symbol_kinds: BTreeMap<u16, u64>,
    pub subsection_kinds: BTreeMap<u32, u64>,
    pub fields: BTreeMap<&'static str, u64>,
    pub forward_refs: u64,
    pub relocations: u64,
    pub unrelocated_addresses: BTreeMap<String, (u64, Vec<String>)>,
    pub nonzero_links: BTreeMap<u16, u64>,
    pub pch_unresolved: u64,
    pub failures: BTreeMap<String, (u64, Vec<String>)>,
}

impl Stats {
    pub fn fail(&mut self, class: String, example: impl FnOnce() -> String) {
        let e = self.failures.entry(class).or_default();
        e.0 += 1;
        if e.1.len() < EXAMPLES {
            e.1.push(example());
        }
    }

    pub fn merge(&mut self, o: Stats) {
        self.inputs += o.inputs;
        self.objects += o.objects;
        self.not_objects += o.not_objects;
        self.bytes += o.bytes;
        self.debug_t += o.debug_t;
        self.debug_p += o.debug_p;
        self.debug_s += o.debug_s;
        self.precomp_users += o.precomp_users;
        self.type_servers += o.type_servers;
        self.type_records += o.type_records;
        self.type_bytes += o.type_bytes;
        self.symbol_records += o.symbol_records;
        self.debug_s_bytes += o.debug_s_bytes;
        self.forward_refs += o.forward_refs;
        self.relocations += o.relocations;
        self.pch_unresolved += o.pch_unresolved;
        for (k, v) in o.type_kinds {
            *self.type_kinds.entry(k).or_default() += v;
        }
        for (k, v) in o.symbol_kinds {
            *self.symbol_kinds.entry(k).or_default() += v;
        }
        for (k, v) in o.subsection_kinds {
            *self.subsection_kinds.entry(k).or_default() += v;
        }
        for (k, v) in o.fields {
            *self.fields.entry(k).or_default() += v;
        }
        for (k, (n, ex)) in o.unrelocated_addresses {
            let e = self.unrelocated_addresses.entry(k).or_default();
            e.0 += n;
            for x in ex {
                if e.1.len() < EXAMPLES {
                    e.1.push(x);
                }
            }
        }
        for (k, v) in o.nonzero_links {
            *self.nonzero_links.entry(k).or_default() += v;
        }
        for (k, (n, ex)) in o.failures {
            let e = self.failures.entry(k).or_default();
            e.0 += n;
            for x in ex {
                if e.1.len() < EXAMPLES {
                    e.1.push(x);
                }
            }
        }
    }
}

fn is_id_kind(kind: u16) -> bool {
    (leaf::LF_FUNC_ID..=leaf::LF_UDT_MOD_SRC_LINE).contains(&kind)
}

pub fn le32(b: &[u8], at: usize) -> u32 {
    u32::from_le_bytes([b[at], b[at + 1], b[at + 2], b[at + 3]])
}

pub fn section_bytes<'d>(pe: &PeFile<'d>, s: &turbonop_pe::raw::SectionHeader) -> Option<&'d [u8]> {
    let at = usize::try_from(s.pointer_to_raw_data.get()).ok()?;
    let len = usize::try_from(s.size_of_raw_data.get()).ok()?;
    pe.data().get(at..at.checked_add(len)?)
}

/// (SECTION, SECREL, ADDR32NB, TOKEN) relocation types by machine.
fn reloc_types(machine: u16) -> Option<[u16; 4]> {
    use turbonop_pe::consts::machine as m;
    Some(match machine {
        m::AMD64 => [0x0A, 0x0B, 0x03, 0x0D],
        m::I386 => [0x0A, 0x0B, 0x07, 0x0C],
        m::ARM64 | m::ARM64EC | m::ARM64X => [0x0D, 0x08, 0x02, 0x0C],
        m::ARM | m::ARMNT | m::THUMB => [0x0E, 0x0F, 0x02, 0x05],
        _ => return None,
    })
}

/// A precompiled header's records: the kind of each, from 0x1000.
pub type Pch = Arc<Vec<u16>>;

/// The types an object's indices name.
struct Space<'a> {
    /// The first of the object's own records.
    base: u32,
    /// Their kinds.
    own: Vec<u16>,
    /// The precompiled header's (for an LF_PRECOMP user), from 0x1000.
    pch: Option<&'a Pch>,
    /// How many of them the object uses (LF_PRECOMP's count).
    pch_count: u32,
    /// Whether the indices can be checked at all (not a type server's, nor
    /// a missing precompiled header's).
    checkable: bool,
}

impl Space<'_> {
    /// The kind of the record at `index` (from 0x1000), if known.
    fn kind(&self, index: u32) -> Result<Option<u16>, ()> {
        if index >= self.base {
            return self
                .own
                .get((index - self.base) as usize)
                .copied()
                .map(Some)
                .ok_or(());
        }
        if index - 0x1000 >= self.pch_count {
            return Err(());
        }
        Ok(self
            .pch
            .and_then(|p| p.get((index - 0x1000) as usize).copied()))
    }
}

/// Checks one index field: in range, and an id exactly when it should be.
#[allow(clippy::too_many_arguments)]
fn check_index(
    st: &mut Stats,
    space: &Space<'_>,
    value: u32,
    id: bool,
    from: &str,
    owner: u16,
    current: Option<u32>,
    where_: &dyn Fn() -> String,
) {
    *st.fields.entry(if id { "id" } else { "type" }).or_default() += 1;
    if value < 0x1000 {
        // A built-in type (or none). Ids have no built-in ones but 0.
        if id && value != 0 {
            st.fail(
                format!("{from} {owner:#06x}: an id field holds simple {value:#x}"),
                where_,
            );
        } else if !id && !simple::is_valid(value) {
            st.fail(
                format!("{from} {owner:#06x}: invalid simple type {value:#x}"),
                where_,
            );
        }
        return;
    }
    if !space.checkable {
        return;
    }
    if let Some(c) = current
        && value >= c
    {
        st.forward_refs += 1;
    }
    match space.kind(value) {
        Err(()) => st.fail(format!("{from} {owner:#06x}: index out of range"), || {
            format!("{} ({value:#x})", where_())
        }),
        Ok(None) => st.pch_unresolved += 1,
        Ok(Some(k)) => {
            if is_id_kind(k) != id {
                let what = if id { "id" } else { "type" };
                st.fail(
                    format!("{from} {owner:#06x}: {what} field names {k:#06x}"),
                    || format!("{} ({value:#x})", where_()),
                );
            }
        }
    }
}

/// The checksum offsets and string offsets an object's subsections define.
#[derive(Default)]
struct Tables {
    strings: Option<Vec<u8>>,
    checksums: Option<HashSet<u32>>,
}

impl Tables {
    fn string_ok(&self, off: u32) -> Option<bool> {
        let s = self.strings.as_ref()?;
        let off = off as usize;
        Some(off < s.len() && (off == 0 || s[off - 1] == 0))
    }

    fn checksum_ok(&self, off: u32) -> Option<bool> {
        Some(self.checksums.as_ref()?.contains(&off))
    }
}

/// An index map with an inverse: indices moved up, and back.
struct Up(u32);

impl Remap for Up {
    fn index(&mut self, index: u32, _: bool) -> Option<u32> {
        Some(index.wrapping_add(self.0))
    }
    fn string(&mut self, offset: u32) -> Option<u32> {
        Some(offset.wrapping_add(self.0))
    }
    fn checksum(&mut self, offset: u32) -> Option<u32> {
        // (Checksum offsets in annotations are compressed: keep them.)
        Some(offset)
    }
}

pub fn check_object(data: &[u8], name: &str, pchs: &HashMap<u32, Pch>, st: &mut Stats) {
    let Ok(pe) = PeFile::parse(data) else {
        st.not_objects += 1;
        return;
    };
    if pe.kind() != FileKind::Object {
        st.not_objects += 1;
        return;
    }
    st.objects += 1;
    st.bytes += data.len() as u64;
    let sections = pe.sections();
    let named = |n: &[u8]| -> Vec<usize> {
        (0..sections.len())
            .filter(|&i| pe.section_name(&sections[i]) == n)
            .collect()
    };
    let t_sections = named(b".debug$T");
    let p_sections = named(b".debug$P");
    let s_sections = named(b".debug$S");
    st.debug_t += t_sections.len() as u64;
    st.debug_p += p_sections.len() as u64;
    st.debug_s += s_sections.len() as u64;
    if t_sections.len() + p_sections.len() > 1 {
        st.fail("more than one type section".into(), || name.to_string());
    }

    // Types.
    let mut space = Space {
        base: 0x1000,
        own: Vec::new(),
        pch: None,
        pch_count: 0,
        checkable: true,
    };
    let types = t_sections.iter().chain(&p_sections).next().copied();
    let mut type_records = Vec::new();
    if let Some(ti) = types {
        let Some(bytes) = section_bytes(&pe, &sections[ti]) else {
            st.fail("type section out of the file".into(), || name.to_string());
            return;
        };
        if bytes.get(..4) != Some(&CV_SIGNATURE_C13.to_le_bytes()[..]) {
            st.fail("type section without the C13 signature".into(), || {
                name.to_string()
            });
            return;
        }
        let stream = &bytes[4..];
        for r in remap::type_records(stream) {
            match r {
                Ok(r) => type_records.push(r),
                Err(at) => {
                    st.fail("type record past the section's end".into(), || {
                        format!("{name} at {at:#x}")
                    });
                    break;
                }
            }
        }
        // An LF_PRECOMP user's own records start after the header's.
        let mut first = 0;
        if let Some((_, r)) = type_records.first() {
            match r.kind {
                leaf::LF_PRECOMP if r.data.len() >= 12 => {
                    st.precomp_users += 1;
                    let start = le32(r.data, 0);
                    let count = le32(r.data, 4);
                    let signature = le32(r.data, 8);
                    if start != 0x1000 {
                        st.fail("LF_PRECOMP not from 0x1000".into(), || name.to_string());
                    }
                    space.base = start + count;
                    space.pch_count = count;
                    space.pch = pchs.get(&signature);
                    if space.pch.is_none() {
                        st.fail("precompiled header not found".into(), || {
                            format!("{name} (signature {signature:#x})")
                        });
                    }
                    first = 1;
                }
                leaf::LF_TYPESERVER2 => {
                    st.type_servers += 1;
                    space.checkable = false;
                }
                _ => {}
            }
        }
        space.own = type_records[first..].iter().map(|(_, r)| r.kind).collect();
        // Every record: its fields, and written back.
        let mut written = CV_SIGNATURE_C13.to_le_bytes().to_vec();
        let mut refs = Vec::new();
        for (i, &(at, r)) in type_records.iter().enumerate() {
            st.type_records += 1;
            *st.type_kinds.entry(r.kind).or_default() += 1;
            let index = (i >= first).then(|| space.base + (i - first) as u32);
            refs.clear();
            if let Err(e) = type_refs::refs(r.kind, r.data, &mut refs) {
                st.fail(format!("type refs: {e}"), || format!("{name} at {at:#x}"));
            }
            for f in &refs {
                let v = le32(r.data, f.offset as usize);
                let w = || format!("{name} type at {at:#x}");
                check_index(st, &space, v, f.id, "type", r.kind, index, &w);
            }
            let before = written.len();
            if let Err(e) = write::push_type(&mut written, r.kind, write::unpadded(r.data)) {
                st.fail(format!("type write: {e}"), || format!("{name} at {at:#x}"));
            }
            if written[before..] != stream[at..at + 4 + r.data.len()] {
                st.fail(format!("type written differs: {:#06x}", r.kind), || {
                    format!("{name} at {at:#x}")
                });
            }
        }
        st.type_bytes += bytes.len() as u64;
        if written != bytes {
            st.fail("type section written differs".into(), || {
                format!("{name}: {} vs {} bytes", written.len(), bytes.len())
            });
        }
        // Renumbered and back.
        let mut copy = bytes.to_vec();
        let ok = remap::debug_t(&mut copy, &mut Up(0x1000_0000))
            .and_then(|()| remap::debug_t(&mut copy, &mut Up(0u32.wrapping_sub(0x1000_0000))));
        if let Err(e) = ok {
            st.fail(format!("remap .debug$T: {e}"), || name.to_string());
        } else if copy != bytes {
            st.fail("remap .debug$T round trip differs".into(), || {
                name.to_string()
            });
        }
    }

    // The string table and checksums: from any .debug$S that has them.
    let mut tables = Tables::default();
    let mut all = Vec::new();
    for &si in &s_sections {
        let Some(bytes) = section_bytes(&pe, &sections[si]) else {
            st.fail(".debug$S out of the file".into(), || name.to_string());
            continue;
        };
        match c13::read_debug_s(bytes) {
            Ok(d) => {
                // As (kind, contents, where they start in the section).
                let subs: Vec<(u32, &[u8], usize)> = match &d {
                    DebugS::C13(subs) => {
                        let mut pos = 4;
                        subs.iter()
                            .map(|s| {
                                let at = pos + 8;
                                pos = at + s.data.len() + (s.data.len().wrapping_neg() & 3);
                                (s.kind, s.data, at)
                            })
                            .collect()
                    }
                    DebugS::Symbols(..) => vec![(subsection::DEBUG_S_SYMBOLS, &bytes[4..], 4)],
                };
                let mut written = Vec::new();
                c13::write_debug_s(&d, &mut written);
                if written != bytes {
                    st.fail(".debug$S written differs".into(), || {
                        format!(
                            "{name} section {}: {} vs {} bytes",
                            si + 1,
                            written.len(),
                            bytes.len()
                        )
                    });
                }
                for s in subs
                    .iter()
                    .map(|&(kind, data, _)| c13::Subsection { kind, data })
                {
                    match s.kind {
                        subsection::DEBUG_S_STRINGTABLE => {
                            if tables.strings.is_some() {
                                st.fail("more than one string table".into(), || name.to_string());
                            }
                            tables.strings = Some(s.data.to_vec());
                        }
                        subsection::DEBUG_S_FILECHKSMS => {
                            if tables.checksums.is_some() {
                                st.fail("more than one checksums subsection".into(), || {
                                    name.to_string()
                                });
                            }
                            match c13::file_checksums(s.data) {
                                Ok(c) => {
                                    tables.checksums = Some(c.iter().map(|e| e.0).collect());
                                }
                                Err(e) => st.fail(format!("checksums: {e}"), || name.to_string()),
                            }
                        }
                        _ => {}
                    }
                }
                all.push((si, bytes, subs));
            }
            Err(e) => st.fail(format!("subsections: {e}"), || name.to_string()),
        }
    }
    let relocs = reloc_types(pe.machine());
    if relocs.is_none() && !all.is_empty() {
        st.fail(format!("machine {:#x}", pe.machine()), || name.to_string());
    }
    for (si, bytes, subs) in all {
        st.debug_s_bytes += bytes.len() as u64;
        // Expected relocations: (offset in the section, type).
        let mut expected: HashMap<usize, (u16, String)> = HashMap::new();
        // Each subsection's (start, end, kind) and each record's, to name
        // where an unexpected relocation falls.
        let mut spans: Vec<(usize, usize, String)> = Vec::new();
        for &(kind, data, data_at) in &subs {
            let s = c13::Subsection { kind, data };
            *st.subsection_kinds.entry(s.kind).or_default() += 1;
            match Contents::parse(s.kind, s.data) {
                Ok(c) => {
                    let mut out = Vec::new();
                    c.write(&mut out);
                    if out != s.data {
                        st.fail(format!("subsection written differs: {:#x}", s.kind), || {
                            format!("{name} at {data_at:#x}")
                        });
                    }
                }
                Err(e) => st.fail(format!("subsection {:#x}: {e}", s.kind), || {
                    format!("{name} at {data_at:#x}")
                }),
            }
            // The records, for symbols.
            let records = if s.kind == subsection::DEBUG_S_SYMBOLS {
                c13::symbol_records(s.data).unwrap_or_default()
            } else {
                Vec::new()
            };
            for r in &records {
                st.symbol_records += 1;
                *st.symbol_kinds.entry(r.kind).or_default() += 1;
                let rec_at = data_at + r.offset as usize;
                spans.push((
                    rec_at,
                    rec_at + 4 + r.data.len(),
                    sk::name(r.kind).to_string(),
                ));
                let body_at = rec_at + 4;
                let mut fields = Vec::new();
                if let Err(e) = symbol_refs::fields(r.kind, r.data, |f| fields.push(f)) {
                    st.fail(format!("symbol fields: {e}"), || {
                        format!("{name} at {rec_at:#x}")
                    });
                    continue;
                }
                let w = || format!("{name} {} at {rec_at:#x}", sk::name(r.kind));
                for f in fields {
                    check_field(st, &space, &tables, f, r.data, sk::name(r.kind), r.kind, &w);
                    match f {
                        Field::Address { offset, section } => {
                            let what = sk::name(r.kind).to_string();
                            expected.insert(body_at + offset as usize, (1, what.clone()));
                            expected.insert(body_at + section as usize, (0, what));
                        }
                        Field::Token(o) => {
                            let what = sk::name(r.kind).to_string();
                            expected.insert(body_at + o as usize, (3, what));
                        }
                        Field::Link(o) if le32(r.data, o as usize) != 0 => {
                            *st.nonzero_links.entry(r.kind).or_default() += 1;
                        }
                        _ => {}
                    }
                }
            }
            if s.kind != subsection::DEBUG_S_SYMBOLS {
                spans.push((
                    data_at,
                    data_at + s.data.len(),
                    format!("subsection {:#x}", s.kind),
                ));
                let mut fields = Vec::new();
                match c13::fields(s.kind, s.data, |f| fields.push(f)) {
                    Ok(()) => {
                        let what = format!("subsection {:#x}", s.kind);
                        let w = || format!("{name} {what} at {data_at:#x}");
                        for f in fields {
                            check_field(st, &space, &tables, f, s.data, &what, 0, &w);
                            match f {
                                Field::Address { offset, section } => {
                                    expected.insert(data_at + offset as usize, (1, what.clone()));
                                    expected.insert(data_at + section as usize, (0, what.clone()));
                                }
                                Field::Rva(o) => {
                                    expected.insert(data_at + o as usize, (2, what.clone()));
                                }
                                _ => {}
                            }
                        }
                    }
                    Err(e) => st.fail(format!("subsection fields {:#x}: {e}", s.kind), || {
                        format!("{name} at {data_at:#x}")
                    }),
                }
            }
        }
        // The relocations: each on a field, as its type.
        if let Some(types) = relocs {
            let mut hit = HashSet::new();
            for r in pe.section_relocations(&sections[si]) {
                st.relocations += 1;
                let at = r.virtual_address.get() as usize;
                let ty = r.kind.get();
                let want = expected
                    .get(&at)
                    .map_or(u16::MAX, |(part, _)| types[usize::from(*part)]);
                if want == ty {
                    hit.insert(at);
                    continue;
                }
                let place = spans
                    .iter()
                    .find(|(s, e, _)| (*s..*e).contains(&at))
                    .map_or("outside any record".to_string(), |(s, _, what)| {
                        format!("{what} +{:#x}", at - s)
                    });
                st.fail(format!("relocation type {ty:#x} at {place}"), || {
                    format!("{name} section {} at {at:#x}", si + 1)
                });
            }
            for (at, (part, what)) in &expected {
                if !hit.contains(at) {
                    let part = ["section", "offset", "rva", "token"][usize::from(*part)];
                    let e = st
                        .unrelocated_addresses
                        .entry(format!("{what} {part}"))
                        .or_default();
                    e.0 += 1;
                    if e.1.len() < EXAMPLES {
                        e.1.push(format!("{name} section {} at {at:#x}", si + 1));
                    }
                }
            }
        }
        // Renumbered and back.
        let mut copy = bytes.to_vec();
        let ok = remap::debug_s(&mut copy, &mut Up(0x1000_0000))
            .and_then(|()| remap::debug_s(&mut copy, &mut Up(0u32.wrapping_sub(0x1000_0000))));
        if let Err(e) = ok {
            st.fail(format!("remap .debug$S: {e}"), || name.to_string());
        } else if copy != bytes {
            st.fail("remap .debug$S round trip differs".into(), || {
                name.to_string()
            });
        }
    }
}

/// Checks a field of a symbol record or subsection (`data` its body or
/// contents).
#[allow(clippy::too_many_arguments)]
fn check_field(
    st: &mut Stats,
    space: &Space<'_>,
    tables: &Tables,
    f: Field,
    data: &[u8],
    what: &str,
    owner: u16,
    w: &dyn Fn() -> String,
) {
    match f {
        Field::Type(o) | Field::Id(o) => {
            let id = matches!(f, Field::Id(_));
            check_index(st, space, le32(data, o as usize), id, what, owner, None, w);
        }
        Field::String(o) => {
            *st.fields.entry("string").or_default() += 1;
            let v = le32(data, o as usize);
            match tables.string_ok(v) {
                Some(true) => {}
                Some(false) => st.fail(format!("{what}: string offset not at a string"), || {
                    format!("{} ({v:#x})", w())
                }),
                None if v != 0 => st.fail(format!("{what}: string offset without a table"), w),
                None => {}
            }
        }
        Field::Checksum(o) => {
            *st.fields.entry("checksum").or_default() += 1;
            let v = le32(data, o as usize);
            if tables.checksum_ok(v) != Some(true) {
                st.fail(format!("{what}: checksum offset not at an entry"), || {
                    format!("{} ({v:#x})", w())
                });
            }
        }
        Field::Annotations(o) => {
            *st.fields.entry("annotations").or_default() += 1;
            match annotations::files(&data[o as usize..]) {
                None => st.fail(format!("{what}: annotations don't decode"), w),
                Some(files) => {
                    for v in files {
                        *st.fields.entry("annotation file").or_default() += 1;
                        if tables.checksum_ok(v) != Some(true) {
                            st.fail(format!("{what}: annotation file not at an entry"), || {
                                format!("{} ({v:#x})", w())
                            });
                        }
                    }
                }
            }
        }
        Field::Address { .. } => *st.fields.entry("address").or_default() += 1,
        Field::Link(_) => *st.fields.entry("link").or_default() += 1,
        Field::Token(_) => *st.fields.entry("token").or_default() += 1,
        Field::Rva(_) => *st.fields.entry("rva").or_default() += 1,
    }
}

/// A precompiled header's signature and records' kinds, if `data` is one
/// (an object with `.debug$P`).
pub fn pch_of(data: &[u8]) -> Option<(u32, Vec<u16>)> {
    let pe = PeFile::parse(data).ok()?;
    if pe.kind() != FileKind::Object {
        return None;
    }
    let s = pe
        .sections()
        .iter()
        .find(|s| pe.section_name(s) == b".debug$P")?;
    let bytes = section_bytes(&pe, s)?;
    let mut kinds = Vec::new();
    let mut signature = None;
    for r in remap::type_records(bytes.get(4..)?) {
        let (_, r) = r.ok()?;
        if r.kind == leaf::LF_ENDPRECOMP && r.data.len() >= 4 && signature.is_none() {
            signature = Some(le32(r.data, 0));
        }
        kinds.push(r.kind);
    }
    Some((signature?, kinds))
}
