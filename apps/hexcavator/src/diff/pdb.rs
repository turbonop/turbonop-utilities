//! Comparing two PDBs by what they say (see `diff`'s module
//! documentation).
//!
//! Types and ids are compared as records whose index fields name other
//! records: each record's canonical hash mixes its kind and bytes with the
//! canonical hashes of the records its index fields name (indices only
//! ever name earlier records), so the same records numbered differently
//! hash the same. Symbols are rendered with their type and id indices as
//! those hashes, and without what only says where they are (their
//! offsets, their parents' and ends' offsets).

use std::collections::BTreeMap;
use std::hash::{BuildHasher, Hasher};

use turbonop_codeview::types::{TypeIndex, TypeRecord};
use turbonop_codeview::{Symbol, SymbolIter, SymbolRecord};
use turbonop_pdb::Pdb;

use super::{Options, Part, Report};

/// A record's canonical hash.
type Canon = u64;

/// The canonical hashes of a type or id stream's records, by index.
struct Canonical {
    begin: u32,
    hashes: Vec<Canon>,
    /// A short description of each (its name, or its kind).
    names: Vec<String>,
}

impl Canonical {
    fn get(&self, ti: u32) -> Option<Canon> {
        ti.checked_sub(self.begin)
            .and_then(|i| self.hashes.get(i as usize))
            .copied()
    }
}

fn hasher() -> turbonop_hash::Hash {
    turbonop_hash::State::default().build_hasher()
}

/// An index as canonical text: a simple type's own number, else its
/// record's hash.
fn index_text(ti: u32, stream: Option<&Canonical>) -> String {
    if ti < 0x1000 {
        return format!("T{ti:#x}");
    }
    match stream.and_then(|s| s.get(ti)) {
        Some(h) => format!("#{h:016x}"),
        None => format!("?{ti:#x}"),
    }
}

/// The canonical hashes of `records` (a TPI or IPI stream from `begin`),
/// whose id fields name `ids` (the IPI's records name TPI records as
/// types; `types` is then the TPI's canonical hashes).
fn canonicalize<'r>(
    begin: u32,
    records: impl Iterator<Item = TypeRecord<'r>>,
    types: Option<&Canonical>,
    names: Option<&turbonop_pdb::StringTable<'_>>,
    modules: &[String],
) -> Canonical {
    let mut c = Canonical {
        begin,
        hashes: Vec::new(),
        names: Vec::new(),
    };
    let mut refs = Vec::new();
    for record in records {
        let mut h = hasher();
        h.write_u16(record.kind);
        refs.clear();
        let ok = turbonop_codeview::type_refs::refs(record.kind, record.data, &mut refs).is_ok();
        let mut at = 0usize;
        if ok {
            for r in &refs {
                let o = r.offset as usize;
                if o < at || o + 4 > record.data.len() {
                    continue;
                }
                h.write(&record.data[at..o]);
                let ti = u32::from_le_bytes([
                    record.data[o],
                    record.data[o + 1],
                    record.data[o + 2],
                    record.data[o + 3],
                ]);
                // A type field of an id record names the TPI; anything else
                // names this stream (earlier records only).
                let target = if r.id || types.is_none() {
                    c.get(ti)
                } else {
                    types.and_then(|t| t.get(ti))
                };
                match (ti < 0x1000, target) {
                    (true, _) => {
                        h.write_u8(0);
                        h.write_u32(ti);
                    }
                    (false, Some(t)) => {
                        h.write_u8(1);
                        h.write_u64(t);
                    }
                    (false, None) => {
                        h.write_u8(2);
                        h.write_u32(ti);
                    }
                }
                at = o + 4;
            }
        }
        // LF_UDT_MOD_SRC_LINE: (udt, source file, line, module); the file is
        // an offset into the PDB's string table, whose layout is the
        // PDB's own: hashed as the string.
        let rest = record.data.get(at..).unwrap_or(&[]);
        if record.kind == 0x1607 && rest.len() >= 4 {
            let off = u32::from_le_bytes([rest[0], rest[1], rest[2], rest[3]]);
            match names.and_then(|n| n.get(off)) {
                Some(s) => h.write(s),
                None => h.write_u32(off),
            }
            // (line, then the module: by its name, not its number)
            h.write(rest.get(4..8).unwrap_or(&[]));
            let m = rest
                .get(8..10)
                .map_or(u16::MAX, |b| u16::from_le_bytes([b[0], b[1]]));
            match modules
                .get(usize::from(m).wrapping_sub(1))
                .or_else(|| modules.get(usize::from(m)))
            {
                Some(name) => h.write(name.as_bytes()),
                None => h.write_u16(m),
            }
            h.write(rest.get(10..).unwrap_or(&[]));
        } else {
            h.write(rest);
        }
        c.hashes.push(h.finish());
        let name = if record.kind == 0x1607 && record.data.len() >= 14 {
            // (udt, file, line, module)
            let u32_at = |o: usize| {
                u32::from_le_bytes([
                    record.data[o],
                    record.data[o + 1],
                    record.data[o + 2],
                    record.data[o + 3],
                ])
            };
            let udt = types
                .and_then(|t| {
                    t.get(u32_at(0))
                        .and(t.names.get((u32_at(0).wrapping_sub(t.begin)) as usize))
                })
                .cloned()
                .unwrap_or_default();
            let file = names
                .and_then(|n| n.get(u32_at(4)))
                .map(|s| String::from_utf8_lossy(s).into_owned())
                .unwrap_or_default();
            format!(
                "{udt} at {file}:{} (module {})",
                u32_at(8),
                u16::from_le_bytes([record.data[12], record.data[13]])
            )
        } else {
            record
                .name()
                .map(|n| String::from_utf8_lossy(n).into_owned())
                .unwrap_or_default()
        };
        c.names.push(format!(
            "{} {}",
            turbonop_codeview::types::leaf::name(record.kind),
            name
        ));
    }
    c
}

/// A symbol record as canonical text: its kind and fields, its indices as
/// canonical hashes, without its offsets in the stream.
fn symbol_text(
    rec: &SymbolRecord<'_>,
    types: Option<&Canonical>,
    ids: Option<&Canonical>,
) -> String {
    let n = |s: &[u8]| String::from_utf8_lossy(s).into_owned();
    let t = |ti: TypeIndex| index_text(ti.0, types);
    let id = |ti: TypeIndex| index_text(ti.0, ids);
    let kind = turbonop_codeview::symbols::kind::name(rec.kind);
    match rec.parse() {
        Ok(Symbol::Procedure {
            global,
            id: is_id,
            len,
            debug_start,
            debug_end,
            ty,
            offset,
            segment,
            flags,
            name,
            ..
        }) => format!(
            "{kind} {} {} type={} at {segment}:{offset:#x} len={len:#x} debug={debug_start:#x}..{debug_end:#x} flags={flags:#x}",
            if global { "global" } else { "local" },
            n(name),
            if is_id { id(ty) } else { t(ty) },
        ),
        Ok(Symbol::Data {
            global,
            thread,
            ty,
            offset,
            segment,
            name,
        }) => format!(
            "{kind} {}{} {} type={} at {segment}:{offset:#x}",
            if global { "global" } else { "local" },
            if thread { " thread" } else { "" },
            n(name),
            t(ty)
        ),
        Ok(Symbol::Udt { ty, name }) => format!("{kind} {} type={}", n(name), t(ty)),
        Ok(Symbol::Constant { ty, value, name }) => {
            format!(
                "{kind} {} type={} value={:#x}",
                n(name),
                t(ty),
                value.as_u64()
            )
        }
        Ok(Symbol::RegisterRelative {
            offset,
            ty,
            register,
            name,
        }) => format!("{kind} {} type={} [r{register}{offset:+}]", n(name), t(ty)),
        Ok(Symbol::BpRelative { offset, ty, name }) => {
            format!("{kind} {} type={} [bp{offset:+}]", n(name), t(ty))
        }
        Ok(Symbol::Register { ty, register, name }) => {
            format!("{kind} {} type={} r{register}", n(name), t(ty))
        }
        Ok(Symbol::Local { ty, flags, name }) => {
            format!("{kind} {} type={} flags={flags:#x}", n(name), t(ty))
        }
        Ok(Symbol::Block {
            len,
            offset,
            segment,
            name,
            ..
        }) => format!("{kind} {} at {segment}:{offset:#x} len={len:#x}", n(name)),
        Ok(Symbol::Thunk {
            offset,
            segment,
            len,
            ordinal,
            name,
            ..
        }) => format!(
            "{kind} {} at {segment}:{offset:#x} len={len:#x} ordinal={ordinal}",
            n(name)
        ),
        Ok(Symbol::InlineSite {
            inlinee,
            annotations,
            ..
        }) => format!(
            "{kind} inlinee={} annotations={}",
            id(inlinee),
            annotations
                .iter()
                .map(|b| format!("{b:02x}"))
                .collect::<String>()
        ),
        Ok(Symbol::SeparatedCode {
            len,
            flags,
            offset,
            parent_offset,
            segment,
            parent_segment,
            ..
        }) => format!(
            "{kind} at {segment}:{offset:#x} len={len:#x} flags={flags:#x} parent at {parent_segment}:{parent_offset:#x}"
        ),
        Ok(Symbol::BuildInfo { id: b }) => format!("{kind} {}", id(b)),
        Ok(Symbol::Reference {
            kind: k,
            module,
            name,
            ..
        }) => format!(
            "{} {} module={module}",
            turbonop_codeview::symbols::kind::name(k),
            n(name)
        ),
        Ok(Symbol::End { kind: k }) => turbonop_codeview::symbols::kind::name(k).to_owned(),
        Ok(Symbol::Other { kind: k, data }) => other_symbol(k, data, types, ids),
        Ok(other) => format!("{kind} {other:?}"),
        Err(_) => format!(
            "{kind} (unreadable) {}",
            rec.data
                .iter()
                .map(|b| format!("{b:02x}"))
                .collect::<String>()
        ),
    }
}

/// A symbol the decoder doesn't model, as canonical text: those that hold
/// type or id indices with them mapped (S_CALLEES, S_CALLERS, S_INLINEES:
/// a count and ids; S_HEAPALLOCSITE, S_CALLSITEINFO: an address and a
/// type; S_FILESTATIC: a type first), the rest as bytes.
fn other_symbol(
    kind: u16,
    data: &[u8],
    types: Option<&Canonical>,
    ids: Option<&Canonical>,
) -> String {
    let name = turbonop_codeview::symbols::kind::name(kind);
    let u32_at = |o: usize| {
        data.get(o..o + 4)
            .map(|b| u32::from_le_bytes([b[0], b[1], b[2], b[3]]))
    };
    let hex = |b: &[u8]| b.iter().map(|x| format!("{x:02x}")).collect::<String>();
    match kind {
        // S_CALLEES, S_CALLERS, S_INLINEES
        0x115A | 0x115B | 0x1168 => {
            let n = u32_at(0).unwrap_or(0) as usize;
            let list: Vec<String> = (0..n.min(data.len() / 4))
                .filter_map(|i| u32_at(4 + 4 * i))
                .map(|ti| index_text(ti, ids))
                .collect();
            format!("{name} {}", list.join(" "))
        }
        // S_HEAPALLOCSITE (offset, section, length, type); S_CALLSITEINFO
        // (offset, section, padding, type)
        0x115E | 0x1139 => format!(
            "{name} {} type={}",
            hex(data.get(..8).unwrap_or(data)),
            u32_at(8).map_or_else(String::new, |t| index_text(t, types))
        ),
        // S_FILESTATIC (type, module's file name offset, flags, name)
        0x1153 => format!(
            "{name} type={} {}",
            u32_at(0).map_or_else(String::new, |t| index_text(t, types)),
            hex(data.get(4..).unwrap_or(&[]))
        ),
        _ => format!("{name} {}", hex(data)),
    }
}
/// Compares two lists as multisets: what one has more of than the other.
fn multiset_diff(p: &mut Part, what: &str, a: &[String], b: &[String]) {
    let mut count: BTreeMap<&str, (usize, usize)> = BTreeMap::new();
    for x in a {
        count.entry(x).or_default().0 += 1;
    }
    for y in b {
        count.entry(y).or_default().1 += 1;
    }
    for (k, (x, y)) in count {
        if x != y {
            p.add_with(|| {
                let side = if x > y { "a" } else { "b" };
                let n = x.abs_diff(y);
                if n == 1 {
                    format!("{what} only in {side}: {k}")
                } else {
                    format!("{what} {n} more times in {side}: {k}")
                }
            });
        }
    }
}

/// Compares two sequences: as multisets, and if those agree, their order.
fn sequence_diff(p: &mut Part, what: &str, a: &[String], b: &[String]) {
    let before = p.differences;
    multiset_diff(p, what, a, b);
    if p.differences == before && a != b {
        let first = a.iter().zip(b).position(|(x, y)| x != y).unwrap_or(0);
        p.add(format!(
            "{what}: the same, in another order (first at {first}: {} | {})",
            a.get(first).map_or("", String::as_str),
            b.get(first).map_or("", String::as_str)
        ));
    }
}

/// What one PDB says, read for comparing.
struct Side<'d> {
    pdb: Pdb<'d>,
    types: Option<Canonical>,
    ids: Option<Canonical>,
}

impl<'d> Side<'d> {
    fn open(data: &'d [u8], path: &std::path::Path) -> Result<Side<'d>, String> {
        let pdb = Pdb::parse(data).map_err(|e| format!("{}: {e}", path.display()))?;
        let names = pdb.names().ok();
        let modules = module_keys(&pdb);
        let types = pdb
            .types()
            .ok()
            .map(|t| canonicalize(t.begin().0, t.iter().map(|(_, r)| r), None, None, &[]));
        let ids = pdb.ids().ok().map(|t| {
            canonicalize(
                t.begin().0,
                t.iter().map(|(_, r)| r),
                types.as_ref(),
                names.as_ref(),
                &modules,
            )
        });
        Ok(Side { pdb, types, ids })
    }
}

fn info(a: &Side<'_>, b: &Side<'_>, o: &Options) -> Part {
    let mut p = Part::new("info", "PDB information", o.rows);
    let (ia, ib) = (a.pdb.info(), b.pdb.info());
    p.field("version", ia.version(), ib.version());
    let mut fa = ia.features.clone();
    let mut fb = ib.features.clone();
    fa.sort_unstable();
    fb.sort_unstable();
    if fa != fb {
        p.add(format!("features: {fa:x?} | {fb:x?}"));
    }
    let named = |i: &turbonop_pdb::InfoStream<'_>| {
        let mut v: Vec<String> = i
            .named_streams()
            .iter()
            .map(|(n, _)| String::from_utf8_lossy(n).into_owned())
            .collect();
        v.sort();
        v
    };
    let (na, nb) = (named(&ia), named(&ib));
    if na != nb {
        p.add(format!(
            "named streams: {} | {}",
            na.join(", "),
            nb.join(", ")
        ));
    }
    // (The GUID and age identify a build; they differ between any two.)
    let (ga, gb) = (ia.guid(), ib.guid());
    p.note(format!(
        "GUID {} age {} | GUID {} age {}{}",
        turbonop_pdb::info::format_guid(&ga),
        ia.age(),
        turbonop_pdb::info::format_guid(&gb),
        ib.age(),
        if ga == gb && ia.age() == ib.age() {
            " (the same)"
        } else {
            ""
        }
    ));
    p
}

fn records(
    key: &'static str,
    title: &str,
    a: Option<&Canonical>,
    b: Option<&Canonical>,
    o: &Options,
) -> Part {
    let mut p = Part::new(key, title, o.rows);
    let (Some(a), Some(b)) = (a, b) else {
        if a.is_some() != b.is_some() {
            p.add(format!("present: {} | {}", a.is_some(), b.is_some()));
        }
        return p;
    };
    p.note(format!("{} | {} records", a.hashes.len(), b.hashes.len()));
    let mut count: turbonop_hash::Map<Canon, (usize, usize, usize, usize)> =
        turbonop_hash::Map::default();
    for (i, &h) in a.hashes.iter().enumerate() {
        let e = count.entry(h).or_insert((0, 0, i, usize::MAX));
        e.0 += 1;
    }
    for (i, &h) in b.hashes.iter().enumerate() {
        let e = count.entry(h).or_insert((0, 0, usize::MAX, i));
        e.1 += 1;
        if e.3 == usize::MAX {
            e.3 = i;
        }
    }
    let mut diffs: Vec<(usize, String)> = Vec::new();
    for (_, (x, y, ia, ib)) in count {
        // (Duplicates are differences only where the counts' presence
        // differs: a merger keeps one of each, so extra copies are noise
        // unless one side has none.)
        if (x == 0) == (y == 0) {
            continue;
        }
        if x > 0 {
            diffs.push((
                ia,
                format!("only in a: {:#x} {}", a.begin as usize + ia, a.names[ia]),
            ));
        } else {
            diffs.push((
                ib,
                format!("only in b: {:#x} {}", b.begin as usize + ib, b.names[ib]),
            ));
        }
    }
    diffs.sort();
    if !diffs.is_empty() && diffs.iter().all(|(_, d)| d.contains("LF_UDT_MOD_SRC_LINE")) {
        p.note(
            "only LF_UDT_MOD_SRC_LINE records differ: which module's copy of a type a link names (link.exe's vary from one link of the same inputs to the next)",
        );
    }
    for (_, d) in diffs {
        p.add(d);
    }
    p
}

/// A module's key: its name and object name (and which copy of the pair).
fn module_keys(pdb: &Pdb<'_>) -> Vec<String> {
    let Ok(dbi) = pdb.dbi() else {
        return Vec::new();
    };
    let mut seen: turbonop_hash::Map<String, usize> = turbonop_hash::Map::default();
    dbi.modules()
        .iter()
        .map(|m| {
            let k = format!(
                "{} ({})",
                String::from_utf8_lossy(m.name),
                String::from_utf8_lossy(m.object_name)
            );
            let n = seen.entry(k.clone()).or_insert(0);
            *n += 1;
            if *n > 1 { format!("{k} #{n}") } else { k }
        })
        .collect()
}

fn modules(a: &Side<'_>, b: &Side<'_>, o: &Options) -> Part {
    let mut p = Part::new("modules", "Modules", o.rows);
    let (ka, kb) = (module_keys(&a.pdb), module_keys(&b.pdb));
    p.note(format!("{} | {} modules", ka.len(), kb.len()));
    sequence_diff(&mut p, "module", &ka, &kb);
    if let (Ok(da), Ok(db)) = (a.pdb.dbi(), b.pdb.dbi()) {
        p.field(
            "machine",
            format!("{:#x}", da.machine()),
            format!("{:#x}", db.machine()),
        );
        // Each module's source files.
        let files =
            |d: &turbonop_pdb::DbiStream<'_>, keys: &[String]| -> BTreeMap<String, Vec<String>> {
                (0..d.modules().len())
                    .map(|i| {
                        (
                            keys.get(i).cloned().unwrap_or_default(),
                            d.module_files(i)
                                .unwrap_or_default()
                                .iter()
                                .map(|f| String::from_utf8_lossy(f).into_owned())
                                .collect(),
                        )
                    })
                    .collect()
            };
        let (fa, fb) = (files(&da, &ka), files(&db, &kb));
        for (k, x) in &fa {
            if let Some(y) = fb.get(k)
                && x != y
            {
                p.add(format!(
                    "{k}: source files {} | {}",
                    x.join(", "),
                    y.join(", ")
                ));
            }
        }
    }
    p
}

fn contributions(a: &Side<'_>, b: &Side<'_>, o: &Options) -> Part {
    let mut p = Part::new("contributions", "Section contributions", o.rows);
    let read = |s: &Side<'_>| -> Vec<String> {
        let keys = module_keys(&s.pdb);
        let Ok(dbi) = s.pdb.dbi() else {
            return Vec::new();
        };
        let Ok(c) = dbi.section_contribs() else {
            return Vec::new();
        };
        let mut v: Vec<String> = c
            .iter()
            .map(|e| {
                format!(
                    "{}:{:#x} size {:#x} chars {:#x} data crc {:#x} module {}",
                    e.section.get(),
                    e.offset.get(),
                    e.size.get(),
                    e.characteristics.get(),
                    e.data_crc.get(),
                    keys.get(usize::from(e.module_index.get()))
                        .map_or("?", String::as_str)
                )
            })
            .collect();
        v.sort();
        v
    };
    let (ca, cb) = (read(a), read(b));
    p.note(format!("{} | {} contributions", ca.len(), cb.len()));
    multiset_diff(&mut p, "contribution", &ca, &cb);
    p
}

fn section_map(a: &Side<'_>, b: &Side<'_>, o: &Options) -> Part {
    let mut p = Part::new("sectionmap", "Section map", o.rows);
    let read = |s: &Side<'_>| -> Vec<String> {
        s.pdb
            .dbi()
            .ok()
            .and_then(|d| {
                d.section_map()
                    .ok()
                    .map(|m| m.iter().map(|e| format!("{e:?}")).collect())
            })
            .unwrap_or_default()
    };
    sequence_diff(&mut p, "section map entry", &read(a), &read(b));
    // And the image's section headers, from the debug header's stream.
    let sh = |s: &Side<'_>| {
        s.pdb
            .sections()
            .map(|v| v.iter().map(|x| format!("{x:?}")).collect::<Vec<_>>())
            .unwrap_or_default()
    };
    sequence_diff(&mut p, "section header", &sh(a), &sh(b));
    p
}

/// One module's symbols, lines, checksums, and inlinee lines, canonical.
#[derive(Default, PartialEq, Eq)]
struct ModuleText {
    symbols: Vec<String>,
    lines: Vec<String>,
    checksums: Vec<String>,
    inlinees: Vec<String>,
}

fn module_text(s: &Side<'_>, index: usize) -> ModuleText {
    let mut out = ModuleText::default();
    let Ok(dbi) = s.pdb.dbi() else { return out };
    let Some(m) = dbi.modules().get(index) else {
        return out;
    };
    let Ok(Some(stream)) = s.pdb.module_stream(m) else {
        return out;
    };
    for rec in stream.symbols() {
        out.symbols
            .push(symbol_text(&rec, s.types.as_ref(), s.ids.as_ref()));
    }
    let names = s.pdb.names().ok();
    let file = |off: u32| -> String {
        stream
            .file_checksum(off)
            .and_then(|c| {
                names
                    .as_ref()?
                    .get(c.name)
                    .map(|n| String::from_utf8_lossy(n).into_owned())
            })
            .unwrap_or_else(|| format!("file {off:#x}"))
    };
    for b in stream.line_blocks() {
        let lines: Vec<String> = b
            .lines
            .iter()
            .map(|l| {
                format!(
                    "{:#x}:{}{}",
                    l.offset.get(),
                    l.line(),
                    if l.is_statement() { "" } else { "e" }
                )
            })
            .collect();
        out.lines.push(format!(
            "{}:{:#x} size {:#x} {}: {}",
            b.segment,
            b.offset,
            b.code_size,
            file(b.file),
            lines.join(" ")
        ));
    }
    for (kind, body) in stream.subsections() {
        match kind {
            // DEBUG_S_FILECHKSMS
            0xF4 => {
                let mut at = 0u32;
                while let Some(c) = turbonop_codeview::lines::file_checksum(body, at) {
                    let name = names
                        .as_ref()
                        .and_then(|n| n.get(c.name))
                        .map(|n| String::from_utf8_lossy(n).into_owned())
                        .unwrap_or_default();
                    out.checksums.push(format!(
                        "{name} kind {} {}",
                        c.kind,
                        c.checksum
                            .iter()
                            .map(|b| format!("{b:02x}"))
                            .collect::<String>()
                    ));
                    let len = 4 + 2 + u32::try_from(c.checksum.len()).unwrap_or(u32::MAX);
                    at = (at + len + 3) & !3;
                    if at as usize >= body.len() {
                        break;
                    }
                }
            }
            // DEBUG_S_INLINEELINES
            0xF6 => {
                for l in turbonop_codeview::lines::inlinee_lines(body) {
                    out.inlinees.push(format!(
                        "{} {}:{}",
                        index_text(l.inlinee, s.ids.as_ref()),
                        file(l.file),
                        l.line
                    ));
                }
            }
            _ => {}
        }
    }
    out.lines.sort();
    out.checksums.sort();
    out.inlinees.sort();
    out
}

fn module_parts(a: &Side<'_>, b: &Side<'_>, o: &Options) -> (Part, Part) {
    let mut symbols = Part::new("symbols", "Modules' symbols", o.rows);
    let mut lines = Part::new(
        "lines",
        "Modules' lines, checksums, and inlinee lines",
        o.rows,
    );
    let (ka, kb) = (module_keys(&a.pdb), module_keys(&b.pdb));
    let index_b: turbonop_hash::Map<&str, usize> = kb
        .iter()
        .enumerate()
        .map(|(i, k)| (k.as_str(), i))
        .collect();
    let pairs: Vec<(usize, usize)> = ka
        .iter()
        .enumerate()
        .filter_map(|(i, k)| index_b.get(k.as_str()).map(|&j| (i, j)))
        .collect();
    let texts =
        turbonop_par::map_balanced(&pairs, 4, |&(i, j)| (module_text(a, i), module_text(b, j)));
    let (mut sym_total, mut sym_same) = (0, 0);
    for (&(i, _), (x, y)) in pairs.iter().zip(&texts) {
        sym_total += 1;
        let k = &ka[i];
        if x.symbols == y.symbols {
            sym_same += 1;
        } else {
            sequence_diff(
                &mut symbols,
                &format!("{k}: symbol"),
                &x.symbols,
                &y.symbols,
            );
        }
        if x.lines != y.lines {
            multiset_diff(&mut lines, &format!("{k}: lines"), &x.lines, &y.lines);
        }
        if x.checksums != y.checksums {
            multiset_diff(
                &mut lines,
                &format!("{k}: checksum"),
                &x.checksums,
                &y.checksums,
            );
        }
        if x.inlinees != y.inlinees {
            multiset_diff(
                &mut lines,
                &format!("{k}: inlinee line"),
                &x.inlinees,
                &y.inlinees,
            );
        }
    }
    symbols.note(format!(
        "{sym_same} of {sym_total} modules in both have the same symbols"
    ));
    (symbols, lines)
}

/// The global symbols (the symbol records the globals' hash table holds).
fn globals_text(s: &Side<'_>) -> Vec<String> {
    let Ok(g) = s.pdb.globals() else {
        return Vec::new();
    };
    let records = s.pdb.symbol_records();
    let mut v: Vec<String> = g
        .records()
        .iter()
        .filter_map(|r| {
            let off = u32::try_from(r.offset.get()).ok()?.checked_sub(1)?;
            let rec = SymbolIter::new(records.get(off as usize..)?, off).next()?;
            Some(symbol_text(&rec, s.types.as_ref(), s.ids.as_ref()))
        })
        .collect();
    v.sort();
    v
}

fn publics_text(s: &Side<'_>) -> Vec<String> {
    let Ok(p) = s.pdb.publics() else {
        return Vec::new();
    };
    let records = s.pdb.symbol_records();
    let mut v: Vec<String> = p
        .hash
        .records()
        .iter()
        .filter_map(|r| {
            let off = u32::try_from(r.offset.get()).ok()?.checked_sub(1)?;
            let rec = SymbolIter::new(records.get(off as usize..)?, off).next()?;
            match rec.parse().ok()? {
                Symbol::Public {
                    flags,
                    offset,
                    segment,
                    name,
                } => Some(format!(
                    "{} at {segment}:{offset:#x} flags {flags:#x}",
                    String::from_utf8_lossy(name)
                )),
                _ => Some(symbol_text(&rec, None, None)),
            }
        })
        .collect();
    v.sort();
    v
}

fn names_text(s: &Side<'_>) -> Vec<String> {
    let mut v: Vec<String> = s
        .pdb
        .names()
        .map(|n| {
            n.iter()
                .map(|(_, s)| String::from_utf8_lossy(s).into_owned())
                .collect()
        })
        .unwrap_or_default();
    v.sort();
    v.dedup();
    v
}

/// Compares the PDBs `a` and `b`.
pub(crate) fn compare(a: &[u8], b: &[u8], o: &Options) -> Result<Report, String> {
    let (sa, sb) = turbonop_par::join(|| Side::open(a, &o.a), || Side::open(b, &o.b));
    let (sa, sb) = (sa?, sb?);
    o.check_parts(&[
        "info",
        "types",
        "ids",
        "modules",
        "contributions",
        "sectionmap",
        "symbols",
        "lines",
        "globals",
        "publics",
        "names",
    ])?;
    let mut parts = Vec::new();
    if o.wants("info") {
        parts.push(info(&sa, &sb, o));
    }
    if o.wants("types") {
        parts.push(records(
            "types",
            "Types (TPI)",
            sa.types.as_ref(),
            sb.types.as_ref(),
            o,
        ));
    }
    if o.wants("ids") {
        parts.push(records(
            "ids",
            "Ids (IPI)",
            sa.ids.as_ref(),
            sb.ids.as_ref(),
            o,
        ));
    }
    if o.wants("modules") {
        parts.push(modules(&sa, &sb, o));
    }
    if o.wants("contributions") {
        parts.push(contributions(&sa, &sb, o));
    }
    if o.wants("sectionmap") {
        parts.push(section_map(&sa, &sb, o));
    }
    if o.wants("symbols") || o.wants("lines") {
        let (s, l) = module_parts(&sa, &sb, o);
        if o.wants("symbols") {
            parts.push(s);
        }
        if o.wants("lines") {
            parts.push(l);
        }
    }
    if o.wants("globals") {
        let mut p = Part::new("globals", "Global symbols", o.rows);
        let (ga, gb) = (globals_text(&sa), globals_text(&sb));
        p.note(format!("{} | {} globals", ga.len(), gb.len()));
        multiset_diff(&mut p, "global", &ga, &gb);
        parts.push(p);
    }
    if o.wants("publics") {
        let mut p = Part::new("publics", "Public symbols", o.rows);
        let (pa, pb) = (publics_text(&sa), publics_text(&sb));
        p.note(format!("{} | {} publics", pa.len(), pb.len()));
        multiset_diff(&mut p, "public", &pa, &pb);
        parts.push(p);
    }
    if o.wants("names") {
        let mut p = Part::new("names", "String table (/names)", o.rows);
        multiset_diff(&mut p, "string", &names_text(&sa), &names_text(&sb));
        parts.push(p);
    }
    Ok(Report { kind: "pdb", parts })
}

/// What the unit tests reach.
#[cfg(test)]
pub(super) mod tests_api {
    use super::*;

    pub fn multiset(p: &mut Part, what: &str, a: &[String], b: &[String]) {
        multiset_diff(p, what, a, b);
    }

    pub fn sequence(p: &mut Part, what: &str, a: &[String], b: &[String]) {
        sequence_diff(p, what, a, b);
    }

    /// The canonical hashes of records (kind, body) numbered from 0x1000.
    pub fn hashes(records: &[(u16, &[u8])]) -> Vec<Canon> {
        canonicalize(
            0x1000,
            records
                .iter()
                .map(|&(kind, data)| TypeRecord { kind, data }),
            None,
            None,
            &[],
        )
        .hashes
    }
}
