//! `/DEBUG`: the PDB, from the objects' debug information (`.debug$S`
//! symbols and lines, `.debug$T` types, type servers' PDBs for `/Zi`,
//! precompiled headers' `.debug$P` for `/Yu`), the image's layout, and its
//! public symbols. Each object's debug sections are relocated and its
//! module built on its own core; the types are merged on every core
//! (`turbonop_pdb_write::merge`).

use std::path::{Path, PathBuf};

use turbonop_pdb_write::dbi::{Contrib, ModuleInfo};
use turbonop_pdb_write::gsi::Public;
use turbonop_pdb_write::merge::{self, IndexMap, Source};
use turbonop_pdb_write::module::ModuleStream;
use turbonop_pdb_write::{Input, StringTable};

use crate::layout::{ChunkKind, Layout};
use crate::object::{Object, Selection, SymbolKind, scn};
use crate::resolve::{Def, FileKind, Resolved};
use crate::write::Image;

/// The PDB's path: `/PDB`, or the output's with `.pdb`.
pub fn pdb_path(r: &Resolved, out: &Path) -> PathBuf {
    r.config
        .pdb
        .clone()
        .unwrap_or_else(|| out.with_extension("pdb"))
}

/// The path the image names: `/PDBALTPATH` (with `%_PDB%` and `%_EXT%`
/// expanded), or the PDB's absolute path.
pub fn image_pdb_name(r: &Resolved, pdb: &Path) -> Vec<u8> {
    let abs = std::path::absolute(pdb).unwrap_or_else(|_| pdb.to_path_buf());
    match &r.config.pdb_alt_path {
        Some(alt) => {
            let file = pdb
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_default();
            let ext = pdb
                .extension()
                .map(|n| format!(".{}", n.to_string_lossy()))
                .unwrap_or_default();
            alt.replace("%_PDB%", &file)
                .replace("%_EXT%", &ext)
                .into_bytes()
        }
        None => abs.to_string_lossy().into_owned().into_bytes(),
    }
}

/// Whether an object's section is debug information that stays: a
/// `.debug$S` not a COMDAT's, or one whose COMDAT is in the image.
fn kept_debug(object: &Object, chunks: &[u32], s: usize) -> bool {
    match object.sections[s].comdat {
        Some((Selection::Associative(parent), _)) => {
            chunks.get(parent as usize).is_some_and(|&c| c != u32::MAX)
        }
        Some(_) => false,
        None => true,
    }
}

/// `DEBUG_S_*` subsection kinds.
mod sub {
    pub const SYMBOLS: u32 = 0xF1;
    pub const LINES: u32 = 0xF2;
    pub const STRINGS: u32 = 0xF3;
    pub const CHECKSUMS: u32 = 0xF4;
    pub const FRAMEDATA: u32 = 0xF5;
    pub const INLINEE_LINES: u32 = 0xF6;
    pub const CROSS_IMPORTS: u32 = 0xF7;
    pub const CROSS_EXPORTS: u32 = 0xF8;
    pub const IGNORE: u32 = 0x8000_0000;
}

/// An object's debug subsections, relocated: in order, (kind, data).
fn subsections(data: &[u8]) -> Vec<(u32, Vec<u8>)> {
    let mut out = Vec::new();
    if data.len() < 4 || u32::from_le_bytes([data[0], data[1], data[2], data[3]]) != 4 {
        return out;
    }
    let mut at = 4usize;
    while at + 8 <= data.len() {
        let kind = u32::from_le_bytes(data[at..at + 4].try_into().expect("4"));
        let len = u32::from_le_bytes(data[at + 4..at + 8].try_into().expect("4")) as usize;
        let start = at + 8;
        let end = (start + len).min(data.len());
        if kind & sub::IGNORE == 0 {
            out.push((kind, data[start..end].to_vec()));
        }
        at = (start + len).next_multiple_of(4);
    }
    out
}

/// The whole records of a `.debug$T` or `.debug$P` section.
fn type_records(data: &'static [u8]) -> Vec<&'static [u8]> {
    let mut out = Vec::new();
    if data.len() < 4 || u32::from_le_bytes([data[0], data[1], data[2], data[3]]) != 4 {
        return out;
    }
    let mut at = 4usize;
    while at + 4 <= data.len() {
        let len = usize::from(u16::from_le_bytes([data[at], data[at + 1]]));
        let end = at + 2 + len;
        if len < 2 || end > data.len() {
            break;
        }
        out.push(&data[at..end]);
        at = end;
    }
    out
}

fn kind_of(rec: &[u8]) -> u16 {
    u16::from_le_bytes([rec[2], rec[3]])
}

/// Whole type records, borrowed from an input.
type Records = Vec<&'static [u8]>;

/// Each object's types: where they come from.
enum Types {
    /// No types.
    None,
    /// Its own records (and a precompiled header's), as source `n`.
    Source(usize),
    /// A type server's, source `n`.
    Server(usize),
}

/// Reads a type server (a `/Zi` object's `LF_TYPESERVER2` names it) as a
/// source: its TPI and IPI records, copied.
fn read_server(name: &[u8], object_path: &str) -> Result<Source<'static>, String> {
    let name = String::from_utf8_lossy(name).into_owned();
    let mut candidates = vec![PathBuf::from(&name)];
    if let Some(dir) = Path::new(object_path).parent()
        && let Some(file) = Path::new(&name).file_name()
    {
        candidates.push(dir.join(file));
    }
    let path = candidates
        .into_iter()
        .find(|p| p.is_file())
        .ok_or_else(|| format!("type server '{name}' not found"))?;
    let data = std::fs::read(&path).map_err(|e| format!("{}: {e}", path.display()))?;
    let data: &'static [u8] = Box::leak(data.into_boxed_slice());
    let pdb = turbonop_pdb::Pdb::parse(data).map_err(|e| format!("{}: {e}", path.display()))?;
    let pdb: &'static turbonop_pdb::Pdb<'static> = Box::leak(Box::new(pdb));
    let whole = |s: &turbonop_pdb::TypeStream<'static>| -> Vec<&'static [u8]> {
        s.iter()
            .map(|(_, r)| {
                let mut v = Vec::with_capacity(4 + r.data.len());
                v.extend_from_slice(
                    &u16::try_from(2 + r.data.len())
                        .unwrap_or(u16::MAX)
                        .to_le_bytes(),
                );
                v.extend_from_slice(&r.kind.to_le_bytes());
                v.extend_from_slice(r.data);
                &*Box::leak(v.into_boxed_slice())
            })
            .collect()
    };
    let types = whole(pdb.types().map_err(|e| e.to_string())?);
    let ids = whole(pdb.ids().map_err(|e| e.to_string())?);
    Ok(Source::Pdb {
        types,
        ids,
        names: pdb.names().ok(),
    })
}

/// What one object gives the PDB, read in parallel: its relocated
/// subsections.
struct ObjectDebug {
    subsections: Vec<(u32, Vec<u8>)>,
}

/// A whole symbol record of `kind` and `body` (as a subsection holds it).
fn record(kind: u16, body: &[u8]) -> Vec<u8> {
    let mut v = Vec::with_capacity(4 + body.len());
    v.extend_from_slice(
        &u16::try_from(2 + body.len())
            .unwrap_or(u16::MAX)
            .to_le_bytes(),
    );
    v.extend_from_slice(&kind.to_le_bytes());
    v.extend_from_slice(body);
    v
}

/// The `S_SECTION` record of `* Linker *` for output section `n`.
fn section_record(n: u16, sec: &crate::layout::OutputSection) -> Vec<u8> {
    let mut b = Vec::new();
    b.extend_from_slice(&n.to_le_bytes());
    b.push(12); // alignment: 4 KB, as a power of two
    b.push(0);
    b.extend_from_slice(&sec.rva.to_le_bytes());
    b.extend_from_slice(&sec.virtual_size.to_le_bytes());
    b.extend_from_slice(&sec.characteristics.to_le_bytes());
    b.extend_from_slice(&sec.name);
    b.push(0);
    b
}

/// Builds the PDB for the image `image` (laid out as `l`). Returns the
/// file, its GUID and age, and warnings.
pub fn build(
    r: &Resolved,
    l: &Layout,
    image: &Image<'_>,
    pdb_path: &Path,
) -> Result<(turbonop_pdb_write::Written, Vec<String>), String> {
    let mut warnings = Vec::new();
    let n = r.files.len();
    // Files by index, for work split by size as threads finish theirs.
    let files: Vec<usize> = (0..n).collect();
    let timing = std::env::var_os("TURBOLINK_PDB_TIME").is_some();
    let mut last = std::time::Instant::now();
    let mut lap = |what: &str| {
        if timing {
            eprintln!(
                "pdb: {what}: {:.1} ms",
                last.elapsed().as_secs_f64() * 1000.0
            );
        }
        last = std::time::Instant::now();
    };
    // 1. Each object's debug sections, relocated, in parallel.
    let debug: Vec<Option<ObjectDebug>> = turbonop_par::map_balanced(&files, 1, |&f| {
        let FileKind::Object { object, .. } = &r.files[f].kind else {
            return None;
        };
        let chunks = &l.section_chunks[f];
        let mut subs = Vec::new();
        for (s, sec) in object.sections.iter().enumerate() {
            if sec.name == b".debug$S" && kept_debug(object, chunks, s) {
                subs.extend(subsections(&image.relocated(crate::n32(f), crate::n32(s))));
            }
        }
        Some(ObjectDebug { subsections: subs })
    });

    lap("relocate debug sections");
    // 2. The type sources: objects' records (a /Yu object's after its
    // precompiled header's), and type servers, each read once.
    let mut sources: Vec<Source<'static>> = Vec::new();
    let mut types: Vec<Types> = Vec::with_capacity(n);
    let mut servers: turbonop_hash::Map<Vec<u8>, usize> = turbonop_hash::Map::default();
    // Precompiled headers by signature: their .debug$P records.
    let mut pch: turbonop_hash::Map<u32, Vec<&'static [u8]>> = turbonop_hash::Map::default();
    // Each object's .debug$T and .debug$P records, found on every core.
    let records: Vec<(Records, Records)> = turbonop_par::map_balanced(&files, 1, |&f| {
        let FileKind::Object { object, .. } = &r.files[f].kind else {
            return (Vec::new(), Vec::new());
        };
        let find = |name: &[u8]| {
            object
                .sections
                .iter()
                .find(|s| s.name == name)
                .map(|s| type_records(s.data))
                .unwrap_or_default()
        };
        (find(b".debug$T"), find(b".debug$P"))
    });
    for (_, p) in &records {
        if !p.is_empty() {
            let recs = p.clone();
            if let Some(end) = recs.iter().find(|x| kind_of(x) == 0x0014) {
                // LF_ENDPRECOMP: its signature.
                if end.len() >= 8 {
                    let sig = u32::from_le_bytes([end[4], end[5], end[6], end[7]]);
                    pch.insert(sig, recs);
                }
            }
        }
    }
    for (file, (t, p)) in r.files.iter().zip(records) {
        let FileKind::Object { .. } = &file.kind else {
            types.push(Types::None);
            continue;
        };
        match t.first().map(|x| kind_of(x)) {
            // LF_TYPESERVER2: GUID, age, name.
            Some(0x1515) => {
                let rec = t[0];
                let name = rec
                    .get(24..)
                    .map(|b| &b[..b.iter().position(|&c| c == 0).unwrap_or(b.len())])
                    .unwrap_or_default();
                let key = name.to_ascii_lowercase();
                let s = match servers.get(&key) {
                    Some(&s) => Some(s),
                    None => match read_server(name, &file.name) {
                        Ok(src) => {
                            sources.push(src);
                            servers.insert(key, sources.len() - 1);
                            Some(sources.len() - 1)
                        }
                        Err(e) => {
                            warnings.push(format!(
                                "{}: {e}; its types are not in the PDB (LNK4099)",
                                file.name
                            ));
                            None
                        }
                    },
                };
                types.push(s.map_or(Types::None, Types::Server));
            }
            // LF_PRECOMP: start, count, signature, name: the header's
            // records first.
            Some(0x1509) => {
                let rec = t[0];
                let count = rec
                    .get(8..12)
                    .map_or(0, |b| u32::from_le_bytes(b.try_into().expect("4")));
                let sig = rec
                    .get(12..16)
                    .map_or(0, |b| u32::from_le_bytes(b.try_into().expect("4")));
                match pch.get(&sig) {
                    Some(h) => {
                        let mut recs: Vec<&'static [u8]> =
                            h.iter().take(count as usize).copied().collect();
                        recs.extend_from_slice(&t[1..]);
                        sources.push(Source::Object(recs));
                        types.push(Types::Source(sources.len() - 1));
                    }
                    None => {
                        warnings.push(format!(
                            "{}: its precompiled header's object (signature {sig:#x}) isn't in the link; its types are not in the PDB (LNK2011)",
                            file.name
                        ));
                        types.push(Types::None);
                    }
                }
            }
            _ if !p.is_empty() || !t.is_empty() => {
                let mut recs = p;
                recs.extend(t);
                sources.push(Source::Object(recs));
                types.push(Types::Source(sources.len() - 1));
            }
            _ => types.push(Types::None),
        }
    }
    lap("type sources");
    // Modules: one per object (ids from 0), then each DLL's two, then
    // `* Linker *`.
    let module_of: Vec<Option<u16>> = {
        let mut next = 0u16;
        r.files
            .iter()
            .map(|f| match f.kind {
                FileKind::Object { .. } => {
                    let m = next;
                    next = next.saturating_add(1);
                    Some(m)
                }
                FileKind::Import { .. } => None,
            })
            .collect()
    };
    // The merge keeps only what the image's symbols refer to (as link.exe
    // prunes): the indices each object's symbols and inlinee lines hold,
    // found on every core once the sources' maps exist.
    let roots = |maps: &[IndexMap]| -> (Vec<u32>, Vec<u32>) {
        let none = IndexMap::default();
        let found: Vec<(Vec<u32>, Vec<u32>)> = turbonop_par::map_balanced(&files, 1, |&f| {
            let mut t = Vec::new();
            let mut i = Vec::new();
            let Some(d) = debug[f].as_ref() else {
                return (t, i);
            };
            let map = match types[f] {
                Types::Source(s) | Types::Server(s) => &maps[s],
                Types::None => &none,
            };
            let mut refs = Vec::new();
            for (kind, data) in &d.subsections {
                match *kind {
                    sub::SYMBOLS => {
                        let mut at = 0usize;
                        while at + 4 <= data.len() {
                            let len = usize::from(u16::from_le_bytes([data[at], data[at + 1]]));
                            let end = (at + 2 + len).min(data.len());
                            if len < 2 {
                                break;
                            }
                            let k = u16::from_le_bytes([data[at + 2], data[at + 3]]);
                            let body = &data[at + 4..end];
                            refs.clear();
                            let _ = turbonop_codeview::symbol_refs::refs(k, body, &mut refs);
                            for r in &refs {
                                let o = r.offset as usize;
                                let ti = u32::from_le_bytes([
                                    body[o],
                                    body[o + 1],
                                    body[o + 2],
                                    body[o + 3],
                                ]);
                                let g = map.map(ti, r.id);
                                if r.id { i.push(g) } else { t.push(g) }
                            }
                            at = end;
                        }
                    }
                    sub::INLINEE_LINES if data.len() >= 4 => {
                        let extra = u32::from_le_bytes(data[0..4].try_into().expect("4")) == 1;
                        let mut at = 4usize;
                        while at + 12 <= data.len() {
                            let ti = u32::from_le_bytes(data[at..at + 4].try_into().expect("4"));
                            i.push(map.map(ti, true));
                            at += 12;
                            if extra && at + 4 <= data.len() {
                                let k = u32::from_le_bytes(data[at..at + 4].try_into().expect("4"))
                                    as usize;
                                at += 4 + 4 * k;
                            }
                        }
                    }
                    _ => {}
                }
            }
            (t, i)
        });
        let mut type_roots = Vec::new();
        let mut id_roots = Vec::new();
        for (t, i) in found {
            type_roots.extend(t);
            id_roots.extend(i);
        }
        (type_roots, id_roots)
    };
    let mut merged = merge::merge(&sources, roots);
    lap("merge");
    for (i, m) in merged.maps.iter().enumerate() {
        if let Some(e) = &m.error {
            warnings.push(format!(
                "type source {i}: {e}; its types are not in the PDB"
            ));
        }
    }
    // The kept UDTs' source lines, as link.exe writes them:
    // LF_UDT_MOD_SRC_LINE, the file's name in /names, and the module (the
    // first object of the record's source).
    let mut names = StringTable::default();
    {
        let mut source_module = vec![u16::MAX; sources.len()];
        for (f, t) in types.iter().enumerate() {
            if let (Types::Source(s) | Types::Server(s), Some(m)) = (t, module_of[f])
                && source_module[*s] == u16::MAX
            {
                source_module[*s] = m;
            }
        }
        let lines = std::mem::take(&mut merged.src_lines);
        let ids = &mut merged.ids;
        for (udt, file, line, source) in lines {
            let module = source_module.get(source as usize).copied().unwrap_or(0);
            let name = names.insert(&file);
            let mut n = Vec::with_capacity(20);
            n.extend_from_slice(&18u16.to_le_bytes());
            n.extend_from_slice(&0x1607u16.to_le_bytes());
            n.extend_from_slice(&udt.to_le_bytes());
            n.extend_from_slice(&name.to_le_bytes());
            n.extend_from_slice(&line.to_le_bytes());
            n.extend_from_slice(&module.wrapping_add(1).to_le_bytes());
            n.extend_from_slice(&[0xF2, 0xF1]);
            ids.offsets.push(crate::n32(ids.records.len()));
            ids.hashes.push(turbonop_pdb_write::streams::type_hash(&n));
            ids.sources.push(source);
            ids.records.extend_from_slice(&n);
        }
    } // Each id's function type (LF_FUNC_ID's and LF_MFUNC_ID's: at 8).
    let func_types: Vec<u32> = merged
        .ids
        .offsets
        .iter()
        .map(|&o| {
            let r = &merged.ids.records[o as usize..];
            let kind = u16::from_le_bytes([r[2], r[3]]);
            if matches!(kind, 0x1601 | 0x1602) && r.len() >= 12 {
                u32::from_le_bytes([r[8], r[9], r[10], r[11]])
            } else {
                0
            }
        })
        .collect();
    let func_type = |id: u32| -> u32 {
        id.checked_sub(0x1000)
            .and_then(|i| func_types.get(i as usize))
            .copied()
            .unwrap_or(0)
    };
    let none = IndexMap::default();
    let map_of = |f: usize| -> &IndexMap {
        match types[f] {
            Types::Source(s) | Types::Server(s) => &merged.maps[s],
            Types::None => &none,
        }
    };

    struct Built {
        stream: ModuleStream,
        strings: Vec<u8>,
        checksums: Vec<u8>,
        frames: Vec<u8>,
    }
    let built: Vec<Option<Built>> = turbonop_par::map_balanced(&files, 1, |&f| {
        let d = debug[f].as_ref()?;
        let m = module_of[f]?;
        let map = map_of(f);
        let mut stream = ModuleStream::default();
        let mut strings = Vec::new();
        let mut checksums = Vec::new();
        let mut frames = Vec::new();
        let remap = |ti: u32, id: bool| map.map(ti, id);
        for (kind, data) in &d.subsections {
            match *kind {
                sub::SYMBOLS => stream.add_symbols(data, &remap, &func_type, m),
                sub::STRINGS if strings.is_empty() => strings.clone_from(data),
                sub::CHECKSUMS if checksums.is_empty() => checksums.clone_from(data),
                sub::LINES | sub::CROSS_IMPORTS | sub::CROSS_EXPORTS => {
                    stream.add_subsection(*kind, data);
                }
                sub::INLINEE_LINES => {
                    let mut v = data.clone();
                    if v.len() >= 4 {
                        let extra = u32::from_le_bytes(v[0..4].try_into().expect("4")) == 1;
                        let mut at = 4usize;
                        while at + 12 <= v.len() {
                            let ti = u32::from_le_bytes(v[at..at + 4].try_into().expect("4"));
                            v[at..at + 4].copy_from_slice(&map.map(ti, true).to_le_bytes());
                            at += 12;
                            if extra && at + 4 <= v.len() {
                                let k = u32::from_le_bytes(v[at..at + 4].try_into().expect("4"))
                                    as usize;
                                at += 4 + 4 * k;
                            }
                        }
                    }
                    stream.add_subsection(*kind, &v);
                }
                sub::FRAMEDATA => frames.extend_from_slice(data),
                _ => {}
            }
        }
        Some(Built {
            stream,
            strings,
            checksums,
            frames,
        })
    });

    lap("modules");
    // 4. File checksums' names into /names (in order), and each module's
    // source files.
    let mut modules: Vec<(ModuleInfo, Option<Vec<u8>>)> = Vec::new();
    let mut new_fpo = Vec::new();
    let mut globals: Vec<Vec<u8>> = Vec::new();
    let mut unknown = Vec::new();
    for (f, b) in built.into_iter().enumerate() {
        let Some(m) = module_of[f] else { continue };
        let file = &r.files[f];
        let (module_name, obj_name) = match file.name.rfind('(') {
            Some(p) if file.name.ends_with(')') => (
                file.name[p + 1..file.name.len() - 1].to_owned(),
                file.name[..p].to_owned(),
            ),
            _ => {
                let abs = std::path::absolute(&file.name)
                    .map_or_else(|_| file.name.clone(), |p| p.to_string_lossy().into_owned());
                (abs.clone(), abs)
            }
        };
        let mut info = ModuleInfo {
            name: module_name.into_bytes(),
            obj: obj_name.into_bytes(),
            first: Contrib {
                section: 0xFFFF,
                module: 0xFFFF,
                ..Contrib::default()
            },
            ..ModuleInfo::default()
        };
        let _ = m;
        let Some(mut b) = b else {
            modules.push((info, None));
            continue;
        };
        if !b.checksums.is_empty() {
            // Each entry: name offset (in the object's string table), size,
            // kind, the checksum, padded to 4.
            let mut c = b.checksums.clone();
            let mut at = 0usize;
            while at + 6 <= c.len() {
                let off = u32::from_le_bytes(c[at..at + 4].try_into().expect("4")) as usize;
                let len = usize::from(c[at + 4]);
                let name = b
                    .strings
                    .get(off..)
                    .map(|s| &s[..s.iter().position(|&x| x == 0).unwrap_or(s.len())])
                    .unwrap_or_default();
                let new = names.insert(name);
                info.files.push(name.to_vec());
                c[at..at + 4].copy_from_slice(&new.to_le_bytes());
                at = (at + 6 + len).next_multiple_of(4);
            }
            b.stream.add_subsection(sub::CHECKSUMS, &c);
        }
        if b.frames.len() >= 4 {
            // A relocated base (the function's address), then FRAMEDATA
            // records (32 bytes) whose start is relative to it.
            let base = u32::from_le_bytes(b.frames[0..4].try_into().expect("4"));
            for rec in b.frames[4..].as_chunks::<32>().0 {
                let mut rec = rec.to_vec();
                let start = u32::from_le_bytes(rec[0..4].try_into().expect("4"));
                rec[0..4].copy_from_slice(&start.wrapping_add(base).to_le_bytes());
                new_fpo.extend_from_slice(&rec);
            }
        }
        for k in &b.stream.unknown_kinds {
            if !unknown.contains(k) {
                unknown.push(*k);
            }
        }
        globals.append(&mut b.stream.globals);
        if b.stream.is_empty() {
            modules.push((info, None));
            continue;
        }
        info.sym_bytes = b.stream.sym_bytes();
        info.c13_bytes = b.stream.c13_bytes();
        modules.push((info, Some(b.stream.finish())));
    }
    for k in unknown {
        warnings.push(format!("symbol records of kind {k:#06x} kept as they were (their index fields, if any, aren't renumbered)"));
    }

    // Typedefs of a tag to its own name (`typedef enum X X`) aren't
    // globals in link.exe's PDBs: the type has the name.
    globals.retain(|g| {
        if g.len() < 12 || u16::from_le_bytes([g[2], g[3]]) != 0x1108 {
            return true;
        }
        let ti = u32::from_le_bytes([g[4], g[5], g[6], g[7]]);
        let name = &g[8..];
        let name = &name[..name.iter().position(|&b| b == 0).unwrap_or(name.len())];
        let Some(&o) = ti
            .checked_sub(0x1000)
            .and_then(|i| merged.types.offsets.get(i as usize))
        else {
            return true;
        };
        turbonop_pdb_write::streams::tag(&merged.types.records[o as usize..])
            .is_none_or(|(_, n, _)| n != name)
    });
    // 5. Each imported DLL's modules, as link.exe makes them: one named
    // for the DLL (the import library's descriptor), and `Import:` with a
    // thunk for each function imported by name.
    let objects = modules.len();
    let dlls = l.dlls.len();
    let linker_module = objects + 2 * dlls;
    let compile3 = |link: &mut ModuleStream| {
        let mut c = Vec::new();
        c.extend_from_slice(&7u32.to_le_bytes()); // language: link
        // The CodeView CPU (CV_CFL_*), not the image's machine.
        let cpu: u16 = match r.config.machine.unwrap_or(crate::config::Machine::X64) {
            crate::config::Machine::X86 => 0x03,
            crate::config::Machine::Arm => 0xF4,
            crate::config::Machine::X64 => 0xD0,
            _ => 0xF6,
        };
        c.extend_from_slice(&cpu.to_le_bytes());
        for v in [0u16, 0, 0, 0, 14, 52, 0, 0] {
            c.extend_from_slice(&v.to_le_bytes());
        }
        c.extend_from_slice(b"turbolink\0");
        link.add_record(0x113C, &c); // S_COMPILE3
    };
    let objname = |link: &mut ModuleStream, name: &[u8]| {
        let mut b = 0u32.to_le_bytes().to_vec();
        b.extend_from_slice(name);
        b.push(0);
        link.add_record(0x1101, &b); // S_OBJNAME
    };
    let library_of = |f: u32| -> Vec<u8> {
        let n = &r.files[f as usize].name;
        n.rfind('(')
            .map_or(n.as_str(), |p| &n[..p])
            .as_bytes()
            .to_vec()
    };
    let mut import_modules = Vec::with_capacity(2 * dlls);
    for dll in &l.dlls {
        let lib = dll
            .imports
            .first()
            .map(|&f| library_of(f))
            .unwrap_or_default();
        let mut s = ModuleStream::default();
        objname(&mut s, dll.name);
        compile3(&mut s);
        let info = ModuleInfo {
            name: dll.name.to_vec(),
            obj: lib,
            sym_bytes: s.sym_bytes(),
            first: Contrib {
                section: 0xFFFF,
                module: 0xFFFF,
                ..Contrib::default()
            },
            ..ModuleInfo::default()
        };
        import_modules.push((info, Some(s.finish())));
    }
    for (d, dll) in l.dlls.iter().enumerate() {
        let lib = dll
            .imports
            .first()
            .map(|&f| library_of(f))
            .unwrap_or_default();
        let mut s = ModuleStream::default();
        for &f in &dll.imports {
            let Some(&c) = l.thunk_chunks.get(&f) else {
                continue;
            };
            let chunk = &l.chunks[c as usize];
            let FileKind::Import { thunk: Some(g), .. } = r.files[f as usize].kind else {
                continue;
            };
            objname(&mut s, dll.name);
            compile3(&mut s);
            let (seg, off) = image.section_offset(chunk.rva);
            let mut b = Vec::new();
            for _ in 0..3 {
                b.extend_from_slice(&0u32.to_le_bytes()); // parent, end, next
            }
            b.extend_from_slice(&off.to_le_bytes());
            b.extend_from_slice(&seg.to_le_bytes());
            b.extend_from_slice(&u16::try_from(chunk.size).unwrap_or(0).to_le_bytes());
            b.push(0); // ordinal: none
            b.extend_from_slice(r.globals[g as usize].name);
            b.push(0);
            s.add_symbols(
                &record(0x1102, &b),
                &|t, _| t,
                &|t| t,
                crate::n16(objects + dlls + d),
            );
            s.add_symbols(
                &record(0x0006, &[]),
                &|t, _| t,
                &|t| t,
                crate::n16(objects + dlls + d),
            );
        }
        let mut name = b"Import:".to_vec();
        name.extend_from_slice(dll.name);
        let info = ModuleInfo {
            name,
            obj: lib,
            sym_bytes: s.sym_bytes(),
            first: Contrib {
                section: 0xFFFF,
                module: 0xFFFF,
                ..Contrib::default()
            },
            ..ModuleInfo::default()
        };
        let stream = (!s.is_empty()).then(|| s.finish());
        let mut info = info;
        if stream.is_none() {
            info.sym_bytes = 0;
        }
        import_modules.push((info, stream));
    }
    modules.extend(import_modules);

    // 6. Section contributions, by section and offset; each module's
    // first.
    let mut contribs = Vec::new();
    for (i, sec) in l.sections.iter().enumerate() {
        let section = u16::try_from(i + 1).unwrap_or(u16::MAX);
        for &c in &sec.chunks {
            let chunk = &l.chunks[c as usize];
            let (module, characteristics) = match chunk.kind {
                ChunkKind::Input(f, s) => {
                    let FileKind::Object { object, .. } = &r.files[f as usize].kind else {
                        continue;
                    };
                    (
                        module_of[f as usize].unwrap_or(0xFFFF),
                        object.sections[s as usize].characteristics,
                    )
                }
                ChunkKind::Thunk(f) => {
                    let d = l.import_slots.get(&f).map_or(0, |s| s.0 as usize);
                    (crate::n16(objects + dlls + d), sec.characteristics)
                }
                ChunkKind::ImportLookup(d) | ChunkKind::ImportAddresses(d) => {
                    (crate::n16(objects + d as usize), sec.characteristics)
                }
                _ => (crate::n16(linker_module), sec.characteristics),
            };
            if chunk.size == 0 {
                continue;
            }
            contribs.push(Contrib {
                section,
                offset: chunk.rva - sec.rva,
                size: chunk.size,
                characteristics,
                module,
                data_crc: 0,
                reloc_crc: 0,
            });
        }
    }
    for c in &contribs {
        if let Some((info, _)) = modules.get_mut(usize::from(c.module))
            && info.first.section == 0xFFFF
        {
            info.first = *c;
        }
    }

    // 7. `* Linker *`: the sections, and their groups (the input sections
    // of one name, in a row: `.text$mn`).
    let mut linker = ModuleStream::default();
    {
        objname(&mut linker, b"* Linker *");
        compile3(&mut linker);
        let cwd = std::env::current_dir()
            .map(|p| p.to_string_lossy().into_owned())
            .unwrap_or_default();
        let exe = std::env::current_exe()
            .map(|p| p.to_string_lossy().into_owned())
            .unwrap_or_default();
        let mut e = vec![0u8];
        for (k, v) in [
            ("cwd", cwd.as_str()),
            ("exe", exe.as_str()),
            ("pdb", &pdb_path.to_string_lossy()),
        ] {
            e.extend_from_slice(k.as_bytes());
            e.push(0);
            e.extend_from_slice(v.as_bytes());
            e.push(0);
        }
        e.push(0);
        linker.add_record(0x113D, &e); // S_ENVBLOCK
        for (i, sec) in l.sections.iter().enumerate() {
            let seg = u16::try_from(i + 1).unwrap_or(u16::MAX);
            linker.add_record(0x1136, &section_record(seg, sec));
            let mut groups: Vec<(&[u8], u32, u32, u32)> = Vec::new();
            for &c in &sec.chunks {
                let chunk = &l.chunks[c as usize];
                let ChunkKind::Input(f, s) = chunk.kind else {
                    continue;
                };
                let FileKind::Object { object, .. } = &r.files[f as usize].kind else {
                    continue;
                };
                let input = &object.sections[s as usize];
                let end = chunk.rva + chunk.size;
                match groups.last_mut() {
                    Some(g) if g.0 == input.name => g.2 = end,
                    _ => groups.push((
                        input.name,
                        chunk.rva,
                        end,
                        input.characteristics & scn::OUTPUT_MASK,
                    )),
                }
            }
            for (name, start, end, chars) in groups {
                let mut b = Vec::new();
                b.extend_from_slice(&(end - start).to_le_bytes());
                b.extend_from_slice(&chars.to_le_bytes());
                b.extend_from_slice(&(start - sec.rva).to_le_bytes());
                b.extend_from_slice(&seg.to_le_bytes());
                b.extend_from_slice(name);
                b.push(0);
                linker.add_record(0x1137, &b); // S_COFFGROUP
            }
        }
    }
    let mut ec_names = StringTable::default();
    let pdb_abs = std::path::absolute(pdb_path).unwrap_or_else(|_| pdb_path.to_path_buf());
    let linker_info = ModuleInfo {
        name: b"* Linker *".to_vec(),
        obj: Vec::new(),
        sym_bytes: linker.sym_bytes(),
        c13_bytes: linker.c13_bytes(),
        first: Contrib {
            section: 0xFFFF,
            module: 0xFFFF,
            ..Contrib::default()
        },
        pdb_path_ni: ec_names.insert(pdb_abs.to_string_lossy().as_bytes()),
        ..ModuleInfo::default()
    };
    modules.push((linker_info, Some(linker.finish())));

    lap("names, DLL modules, contributions, linker module");
    // 7. Publics: every external definition in the image, the thunks,
    // and the import address table's entries.
    let mut publics: Vec<Public> = Vec::new();
    let code_section = |seg: u16| {
        l.sections
            .get(usize::from(seg).wrapping_sub(1))
            .is_some_and(|s| s.characteristics & scn::CNT_CODE != 0)
    };
    for (f, file) in r.files.iter().enumerate() {
        match &file.kind {
            FileKind::Object {
                object,
                globals: gl,
                ..
            } => {
                for (i, s) in object.symbols.iter().enumerate() {
                    let SymbolKind::Defined { section, value } = s.kind else {
                        continue;
                    };
                    let g = gl[i];
                    if g == crate::resolve::NONE {
                        continue;
                    }
                    let mine = matches!(r.globals[g as usize].def, Def::Section { file, section: sec, value: v } if file as usize == f && sec == section && v == value);
                    if !mine {
                        continue;
                    }
                    let Some(rva) = image.rva_of(g) else { continue };
                    let (segment, offset) = image.section_offset(rva);
                    let flags = if s.ty & 0xF0 == 0x20 {
                        2
                    } else if code_section(segment) {
                        1
                    } else {
                        0
                    };
                    publics.push(Public {
                        name: s.name.to_vec(),
                        flags,
                        segment,
                        offset,
                    });
                }
            }
            FileKind::Import { address, thunk, .. } => {
                for (g, flags) in [(Some(*address), 0), (*thunk, 2)] {
                    let Some(g) = g else { continue };
                    if let Some(rva) = image.rva_of(g) {
                        let (segment, offset) = image.section_offset(rva);
                        publics.push(Public {
                            name: r.globals[g as usize].name.to_vec(),
                            flags,
                            segment,
                            offset,
                        });
                    }
                }
            }
        }
    }
    // Commons; names that are aliases (`/ALTERNATENAME`, weak externals),
    // at their targets'; absolute symbols, in the section after the last
    // (as link.exe lists them: `__guard_flags`).
    let absolute = u16::try_from(l.sections.len() + 1).unwrap_or(u16::MAX);
    // link.exe's own absolute zero.
    if !r.names.contains_key(&b"__AbsoluteZero"[..])
        && r.names
            .get(&b"_load_config_used"[..])
            .is_some_and(|&g| image.rva_of(g).is_some())
    {
        publics.push(Public {
            name: b"__AbsoluteZero".to_vec(),
            flags: 0,
            segment: absolute,
            offset: 0,
        });
    }
    for (g, global) in r.globals.iter().enumerate() {
        let g32 = crate::n32(g);
        match global.def {
            Def::Common { .. } | Def::Alias(_) | Def::LocalImport(_) => {
                if let Some(rva) = image.rva_of(g32) {
                    let (segment, offset) = image.section_offset(rva);
                    let flags = if code_section(segment) { 2 } else { 0 };
                    publics.push(Public {
                        name: global.name.to_vec(),
                        flags,
                        segment,
                        offset,
                    });
                }
            }
            Def::Absolute(v) => publics.push(Public {
                name: global.name.to_vec(),
                flags: 0,
                segment: absolute,
                offset: u32::try_from(v).unwrap_or(u32::MAX),
            }),
            Def::Synthetic(crate::resolve::Synthetic::Zero)
                if global.referenced_by != crate::resolve::NONE =>
            {
                publics.push(Public {
                    name: global.name.to_vec(),
                    flags: 0,
                    segment: absolute,
                    offset: 0,
                });
            }
            _ => {}
        }
    }
    // The import descriptors' symbols, as an import library's descriptor
    // objects define them: each DLL's directory entry, the null entry, and
    // each DLL's address table's last (null) entry.
    if let Some(dir) = l
        .chunks
        .iter()
        .find(|c| c.kind == ChunkKind::ImportDirectory)
    {
        let mut push = |name: Vec<u8>, rva: u32| {
            let (segment, offset) = image.section_offset(rva);
            publics.push(Public {
                name,
                flags: 0,
                segment,
                offset,
            });
        };
        for (d, dll) in l.dlls.iter().enumerate() {
            let stem = match dll.name.iter().rposition(|&b| b == b'.') {
                Some(p) => &dll.name[..p],
                None => dll.name,
            };
            push(
                [b"__IMPORT_DESCRIPTOR_".as_slice(), stem].concat(),
                dir.rva + 20 * crate::n32(d),
            );
            if let Some(iat) = l
                .chunks
                .iter()
                .find(|c| c.kind == ChunkKind::ImportAddresses(crate::n32(d)))
            {
                push(
                    [b"\x7f".as_slice(), stem, b"_NULL_THUNK_DATA"].concat(),
                    iat.rva + 8 * crate::n32(dll.imports.len()),
                );
            }
        }
        push(
            b"__NULL_IMPORT_DESCRIPTOR".to_vec(),
            dir.rva + 20 * crate::n32(l.dlls.len()),
        );
    }

    // 8. The section headers, as the image's.
    let mut headers = Vec::with_capacity(40 * l.sections.len());
    let mut sections = Vec::with_capacity(l.sections.len());
    for s in &l.sections {
        let mut name = [0u8; 8];
        let k = s.name.len().min(8);
        name[..k].copy_from_slice(&s.name[..k]);
        headers.extend_from_slice(&name);
        for v in [s.virtual_size, s.rva, s.raw_size, s.file_offset, 0, 0] {
            headers.extend_from_slice(&v.to_le_bytes());
        }
        headers.extend_from_slice(&[0; 4]);
        headers.extend_from_slice(&s.characteristics.to_le_bytes());
        sections.push((s.characteristics, s.virtual_size));
    }

    let input = Input {
        machine: r
            .config
            .machine
            .unwrap_or(crate::config::Machine::X64)
            .number(),
        age: 1,
        signature: crate::write::timestamp(&r.config),
        merged,
        modules,
        contribs,
        sections,
        section_headers: headers,
        publics,
        globals,
        names,
        ec_names,
        new_fpo,
    };
    lap("publics, headers");
    let written = turbonop_pdb_write::write(input, 4096).map_err(|e| e.0)?;
    lap("streams and MSF");

    Ok((written, warnings))
}
