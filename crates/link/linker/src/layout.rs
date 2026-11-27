//! What the image holds, and where: which sections stay (COMDATs chosen,
//! associative sections with theirs, `/OPT:REF`'s mark), the imports'
//! tables, the order of sections (input sections grouped by name, their
//! `$` suffixes sorted, `/MERGE`s applied, as `link.exe` orders them), and
//! every section's address.

use turbonop_hash::Map;
use turbonop_pe::import_object::ImportObject;

use crate::config::Config;
use crate::object::{Selection, SymbolKind, scn};
use crate::resolve::{Def, FileKind, Resolved};

/// One piece of an output section.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ChunkKind {
    /// An input section: file, 0-based section.
    Input(u32, u32),
    /// Common data: the global.
    Common(u32),
    /// An import's thunk (`jmp [__imp_x]`): the import's file.
    Thunk(u32),
    /// The import directory (one entry per DLL, and the null entry).
    ImportDirectory,
    /// A DLL's import lookup table: its index among the DLLs.
    ImportLookup(u32),
    /// A DLL's import address table.
    ImportAddresses(u32),
    /// The imports' hint/name entries and the DLLs' names.
    ImportNames,
    /// The base relocations.
    BaseRelocations,
    /// The export directory.
    Exports,
    /// The resources (`.rsrc`).
    Resources,
}

/// A chunk placed.
#[derive(Clone, Copy, Debug)]
pub struct Chunk {
    /// What it is.
    pub kind: ChunkKind,
    /// Its alignment.
    pub align: u32,
    /// Its size.
    pub size: u32,
    /// Whether it has no contents in the file (uninitialized data).
    pub uninitialized: bool,
    /// Its address (relative to the image base), once laid out.
    pub rva: u32,
}

/// An output section.
#[derive(Clone, Debug)]
pub struct OutputSection {
    /// Its name (at most 8 bytes are written).
    pub name: Vec<u8>,
    /// Its characteristics.
    pub characteristics: u32,
    /// Its chunks' indices, in order.
    pub chunks: Vec<u32>,
    /// Its address.
    pub rva: u32,
    /// Its size in memory.
    pub virtual_size: u32,
    /// Its size in the file (initialized data, aligned to the file
    /// alignment).
    pub raw_size: u32,
    /// Its contents' offset in the file.
    pub file_offset: u32,
}

/// One DLL's imports.
#[derive(Clone, Debug)]
pub struct Dll {
    /// The DLL's name.
    pub name: &'static [u8],
    /// Its imports: the import files, in order.
    pub imports: Vec<u32>,
}

/// The image's layout.
pub struct Layout {
    /// Every chunk.
    pub chunks: Vec<Chunk>,
    /// The output sections, in order.
    pub sections: Vec<OutputSection>,
    /// For each file's sections, its chunk (`u32::MAX`: not in the image).
    pub section_chunks: Vec<Vec<u32>>,
    /// For each common global, its chunk.
    pub common_chunks: Map<u32, u32>,
    /// For each import file: its DLL's index and its position there.
    pub import_slots: Map<u32, (u32, u32)>,
    /// For each import file with a thunk, the thunk's chunk.
    pub thunk_chunks: Map<u32, u32>,
    /// The DLLs imported from, in the import tables' order (by name).
    pub dlls: Vec<Dll>,
    /// The import directory's order: each DLL (its index in `dlls`) as
    /// its first import was loaded.
    pub directory_order: Vec<u32>,
    /// The headers' size (file-aligned).
    pub headers_size: u32,
    /// The image's size in memory.
    pub image_size: u32,
    /// The file's size.
    pub file_size: u32,
    /// Section alignment.
    pub section_align: u32,
    /// File alignment.
    pub file_align: u32,
    /// Offsets of the import names chunk's entries: per import file, the
    /// hint/name entry's offset; per DLL, its name's.
    pub name_offsets: Map<u32, u32>,
    /// Per DLL, the offset of its name in the names chunk.
    pub dll_name_offsets: Vec<u32>,
    /// The base relocations' page RVAs and entries, once computed.
    pub base_relocations: Vec<u8>,
    /// The exports, for a DLL that has any.
    pub exports: Option<crate::exports::ExportTable>,
    /// The resources, if there are any.
    pub resources: Option<crate::resources::Rsrc>,
}

/// The output section an input section's name goes to: the name without
/// its `$` suffix, through the `/MERGE`s (and the defaults
/// `link.exe` has: `.idata`, `.didat`, `.edata`, `.xdata`, `.00cfg` to
/// `.rdata`; `.bss` to `.data`).
fn output_name(name: &[u8], merges: &[(Vec<u8>, Vec<u8>)]) -> Vec<u8> {
    let base = match name.iter().position(|&b| b == b'$') {
        Some(p) => &name[..p],
        None => name,
    };
    let mut out = base.to_vec();
    // Merges chain (a=b, b=c); a cycle stops after as many steps as
    // there are merges.
    for _ in 0..=merges.len() {
        match merges.iter().find(|(from, _)| *from == out) {
            Some((_, to)) => out.clone_from(to),
            None => break,
        }
    }
    out
}

const DEFAULT_MERGES: &[(&str, &str)] = &[
    (".idata", ".rdata"),
    (".didat", ".rdata"),
    (".edata", ".rdata"),
    (".xdata", ".rdata"),
    (".00cfg", ".rdata"),
    (".bss", ".data"),
];

/// Sections that are never in the image (`/GUARD`'s tables, which the
/// link reads instead, and what only debuggers want).
fn dropped(name: &[u8]) -> bool {
    matches!(
        name,
        b".gfids$y" | b".gfids$x" | b".giats$y" | b".gljmp$y" | b".gehcont$y" | b".chks64"
    ) || name.starts_with(b".debug")
}

/// What is live: input sections, commons, imports, thunks.
struct Live {
    sections: Vec<Vec<bool>>,
    commons: Vec<u32>,
    imports: Vec<bool>,
    thunks: Vec<bool>,
}

/// Marks what the image keeps: every kept non-COMDAT section and what
/// it refers to (with `/OPT:REF`, COMDATs only if something live refers
/// to them; without, every kept one), associative sections with theirs.
///
/// Breadth first, on every core: each step's sections are split across
/// the threads, and a section is claimed by the first thread to flag it,
/// so each is visited once. What is live is a set, the same whatever the
/// threads' order.
fn mark(r: &Resolved, roots: &[u32], folded: &Map<(u32, u32), (u32, u32)>) -> Live {
    use std::sync::atomic::{AtomicBool, Ordering};
    let gc = r.config.gc_sections();
    let n = r.files.len();
    // Every file's sections, numbered one after another.
    let mut base = Vec::with_capacity(n + 1);
    let mut total = 0usize;
    for f in &r.files {
        base.push(total);
        if let FileKind::Object { object, .. } = &f.kind {
            total += object.sections.len();
        }
    }
    base.push(total);
    // Which sections can be in the image, and each one's associative
    // sections (a list through `first_child` and `next_sibling`).
    let mut eligible = vec![false; total];
    let mut first_child = vec![u32::MAX; total];
    let mut next_sibling = vec![u32::MAX; total];
    let mut frontier: Vec<u32> = Vec::new();
    for (f, file) in r.files.iter().enumerate() {
        let FileKind::Object { object, kept, .. } = &file.kind else {
            continue;
        };
        for (s, sec) in object.sections.iter().enumerate() {
            if !kept[s] || !sec.is_image() || dropped(sec.name) {
                continue;
            }
            let i = base[f] + s;
            eligible[i] = true;
            match sec.comdat {
                Some((Selection::Associative(parent), _)) => {
                    let p = base[f] + parent as usize;
                    next_sibling[i] = first_child[p];
                    first_child[p] = crate::n32(i);
                }
                Some(_) if gc => {}
                _ => frontier.push(crate::n32(i)),
            }
        }
    }
    // A folded section's references are its leader's (and it is never
    // live itself).
    let mut redirect: Vec<u32> = (0..crate::n32(total)).collect();
    for (&(f, s), &(lf, ls)) in folded {
        let i = base[f as usize] + s as usize;
        redirect[i] = crate::n32(base[lf as usize] + ls as usize);
        eligible[i] = false;
    }
    let live: Vec<AtomicBool> = (0..total).map(|_| AtomicBool::new(false)).collect();
    let imports: Vec<AtomicBool> = (0..n).map(|_| AtomicBool::new(false)).collect();
    let thunks: Vec<AtomicBool> = (0..n).map(|_| AtomicBool::new(false)).collect();
    // Section `i`'s file, by its number.
    let file_of = |i: usize| base.partition_point(|&b| b <= i) - 1;
    // Claims section `i` if it can be in the image and isn't yet live.
    let claim = |i: usize| {
        let i = redirect[i] as usize;
        eligible[i] && !live[i].swap(true, Ordering::Relaxed)
    };
    // What global `g` keeps: a section (pushed to `next` if newly
    // claimed), a common, an import.
    let global = |g: u32, next: &mut Vec<u32>, commons: &mut Vec<u32>| {
        let mut g = g;
        for _ in 0..64 {
            match r.globals[g as usize].def {
                Def::Alias(t) => g = t,
                Def::Section { file, section, .. } => {
                    let i = redirect[base[file as usize] + section as usize] as usize;
                    if claim(i) {
                        next.push(crate::n32(i));
                    }
                    return;
                }
                Def::Common { .. } => {
                    commons.push(g);
                    return;
                }
                Def::ImportAddress(f) => {
                    imports[f as usize].store(true, Ordering::Relaxed);
                    return;
                }
                Def::ImportThunk(f) => {
                    imports[f as usize].store(true, Ordering::Relaxed);
                    thunks[f as usize].store(true, Ordering::Relaxed);
                    return;
                }
                _ => return,
            }
        }
    };
    let mut commons = Vec::new();
    frontier.retain(|&i| claim(i as usize));
    for &g in roots {
        global(g, &mut frontier, &mut commons);
    }
    while !frontier.is_empty() {
        let steps = turbonop_par::map_chunks(&frontier, 256, |part| {
            let mut next = Vec::new();
            let mut commons = Vec::new();
            for &i in part {
                let i = i as usize;
                let f = file_of(i);
                let s = i - base[f];
                let mut c = first_child[i];
                while c != u32::MAX {
                    if claim(c as usize) {
                        next.push(redirect[c as usize]);
                    }
                    c = next_sibling[c as usize];
                }
                let FileKind::Object {
                    object, globals, ..
                } = &r.files[f].kind
                else {
                    continue;
                };
                for rel in object.sections[s].relocations {
                    let k = rel.symbol_table_index.get() as usize;
                    match globals.get(k) {
                        Some(&g) if g != crate::resolve::NONE => {
                            global(g, &mut next, &mut commons);
                        }
                        _ => {
                            if let Some(sym) = object.symbols.get(k)
                                && let SymbolKind::Local { section, .. } = sym.kind
                            {
                                let t = redirect[base[f] + section as usize] as usize;
                                if claim(t) {
                                    next.push(crate::n32(t));
                                }
                            }
                        }
                    }
                }
            }
            vec![(next, commons)]
        });
        frontier = Vec::new();
        for (next, c) in steps {
            frontier.extend(next);
            commons.extend(c);
        }
    }
    commons.sort_unstable();
    commons.dedup();
    let sections = (0..n)
        .map(|f| {
            (base[f]..base[f + 1])
                .map(|i| live[i].load(Ordering::Relaxed))
                .collect()
        })
        .collect();
    Live {
        sections,
        commons,
        imports: imports.iter().map(|b| b.load(Ordering::Relaxed)).collect(),
        thunks: thunks.iter().map(|b| b.load(Ordering::Relaxed)).collect(),
    }
}
/// A partial section's key (output name, input name, characteristics)
/// and its chunks.
type Partial = ((Vec<u8>, Vec<u8>, u32), Vec<u32>);

/// Lays the image out.
pub fn layout(
    r: &Resolved,
    roots: &[u32],
    exports: Option<crate::exports::ExportTable>,
    resources: Option<crate::resources::Rsrc>,
) -> Layout {
    let config: &Config = &r.config;
    let mut live = mark(r, roots, &Map::default());
    // /OPT:ICF: each folded section's leader (which comes before it); then
    // the mark again, without the folded copies (what only they referred
    // to isn't kept, as link.exe doesn't keep it).
    let folded = if config.icf() {
        crate::icf::fold(r, &live.sections)
    } else {
        Map::default()
    };
    if !folded.is_empty() {
        live = mark(r, roots, &folded);
    }
    let mut merges: Vec<(Vec<u8>, Vec<u8>)> = config
        .merges
        .iter()
        .map(|(a, b)| (a.as_bytes().to_vec(), b.as_bytes().to_vec()))
        .collect();
    for (a, b) in DEFAULT_MERGES {
        if !merges.iter().any(|(f, _)| f == a.as_bytes()) {
            merges.push((a.as_bytes().to_vec(), b.as_bytes().to_vec()));
        }
    }

    let mut chunks: Vec<Chunk> = Vec::new();
    // Partial sections: (output name, input name, characteristics) and
    // their chunks in input order.
    let mut partials: Vec<Partial> = Vec::new();
    // Each (input name, characteristics)'s partial section: the names are
    // the inputs' own bytes, so finding one copies nothing; the output
    // name is worked out once per distinct one.
    let mut partial_index: Map<(&'static [u8], u32), usize> = Map::default();
    let mut add = |name: &'static [u8], chars: u32, chunk: Chunk, chunks: &mut Vec<Chunk>| -> u32 {
        let chars = chars & scn::OUTPUT_MASK;
        let c = u32::try_from(chunks.len()).expect("fewer than 4 billion chunks");
        chunks.push(chunk);
        let p = *partial_index.entry((name, chars)).or_insert_with(|| {
            let key = (output_name(name, &merges), name.to_vec(), chars);
            partials.push((key, Vec::new()));
            partials.len() - 1
        });
        partials[p].1.push(c);
        c
    };

    let mut section_chunks: Vec<Vec<u32>> = Vec::with_capacity(r.files.len());
    let mut dll_index: Map<&'static [u8], u32> = Map::default();
    let mut dlls: Vec<Dll> = Vec::new();
    let mut import_slots = Map::default();
    let mut thunk_chunks = Map::default();
    for (f, file) in r.files.iter().enumerate() {
        match &file.kind {
            FileKind::Object { object, .. } => {
                let mut mine = vec![u32::MAX; object.sections.len()];
                for (s, sec) in object.sections.iter().enumerate() {
                    let key = (crate::n32(f), crate::n32(s));
                    if let Some(&(lf, ls)) = folded.get(&key) {
                        // Folded: its symbols are its leader's.
                        mine[s] = if lf == key.0 {
                            mine[ls as usize]
                        } else {
                            section_chunks[lf as usize][ls as usize]
                        };
                        continue;
                    }
                    if !live.sections[f][s] {
                        continue;
                    }
                    if let Some((Selection::Associative(parent), _)) = sec.comdat
                        && folded.contains_key(&(key.0, parent))
                    {
                        // A folded section's unwind data goes with it.
                        continue;
                    }
                    let uninitialized = sec.characteristics & scn::CNT_UNINITIALIZED_DATA != 0;
                    mine[s] = add(
                        sec.name,
                        sec.characteristics,
                        Chunk {
                            kind: ChunkKind::Input(crate::n32(f), crate::n32(s)),
                            align: sec.alignment(),
                            size: sec.size,
                            uninitialized,
                            rva: 0,
                        },
                        &mut chunks,
                    );
                }
                section_chunks.push(mine);
            }
            FileKind::Import { import, .. } => {
                section_chunks.push(Vec::new());
                if !live.imports[f] {
                    continue;
                }
                let d = *dll_index.entry(import.dll).or_insert_with(|| {
                    dlls.push(Dll {
                        name: import.dll,
                        imports: Vec::new(),
                    });
                    u32::try_from(dlls.len() - 1).unwrap_or(u32::MAX)
                });
                let slot = &mut dlls[d as usize].imports;
                import_slots.insert(crate::n32(f), (d, u32::try_from(slot.len()).unwrap_or(0)));
                slot.push(crate::n32(f));
                if live.thunks[f] {
                    let c = add(
                        b".text$zz",
                        scn::CNT_CODE | scn::MEM_EXECUTE | scn::MEM_READ,
                        Chunk {
                            kind: ChunkKind::Thunk(crate::n32(f)),
                            align: 2,
                            size: 6,
                            uninitialized: false,
                            rva: 0,
                        },
                        &mut chunks,
                    );
                    thunk_chunks.insert(crate::n32(f), c);
                }
            }
        }
    }
    // link.exe builds the import tables from the import library's own
    // pieces, which it sorts by their member's name (the DLL's: byte
    // order, `KERNEL32.dll` before `VCRUNTIME140.dll` before `api-ms-...`),
    // keeping the order they were loaded in otherwise.
    let mut directory_order: Vec<u32> = (0..crate::n32(dlls.len())).collect();
    if dlls.len() > 1 {
        let mut order: Vec<u32> = (0..crate::n32(dlls.len())).collect();
        order.sort_by(|&a, &b| dlls[a as usize].name.cmp(dlls[b as usize].name));
        let mut new_index = vec![0u32; dlls.len()];
        for (new, &old) in order.iter().enumerate() {
            new_index[old as usize] = crate::n32(new);
        }
        let mut old: Vec<Option<Dll>> = dlls.into_iter().map(Some).collect();
        dlls = order
            .iter()
            .map(|&o| old[o as usize].take().expect("each once"))
            .collect();
        for slot in import_slots.values_mut() {
            slot.0 = new_index[slot.0 as usize];
        }
        for d in &mut directory_order {
            *d = new_index[*d as usize];
        }
    }
    let mut common_chunks = Map::default();
    for &g in &live.commons {
        if let Def::Common { size, .. } = r.globals[g as usize].def {
            let align = size.next_power_of_two().clamp(1, 32);
            let c = add(
                b".bss",
                scn::CNT_UNINITIALIZED_DATA | scn::MEM_READ | scn::MEM_WRITE,
                Chunk {
                    kind: ChunkKind::Common(g),
                    align,
                    size,
                    uninitialized: true,
                    rva: 0,
                },
                &mut chunks,
            );
            common_chunks.insert(g, c);
        }
    }

    // The imports' tables, in `.idata$2` (directory), `$4` (lookup), `$5`
    // (addresses), `$6` (names): merged into `.rdata`.
    let mut name_offsets = Map::default();
    let mut dll_name_offsets = Vec::new();
    if !dlls.is_empty() {
        let data = scn::CNT_INITIALIZED_DATA | scn::MEM_READ;
        let table = |n: usize| u32::try_from((n + 1) * 8).unwrap_or(u32::MAX);
        add(
            b".idata$2",
            data,
            Chunk {
                kind: ChunkKind::ImportDirectory,
                align: 4,
                size: u32::try_from((dlls.len() + 1) * 20).unwrap_or(u32::MAX),
                uninitialized: false,
                rva: 0,
            },
            &mut chunks,
        );
        for (d, dll) in dlls.iter().enumerate() {
            let size = table(dll.imports.len());
            add(
                b".idata$4",
                data,
                Chunk {
                    kind: ChunkKind::ImportLookup(crate::n32(d)),
                    align: 8,
                    size,
                    uninitialized: false,
                    rva: 0,
                },
                &mut chunks,
            );
            add(
                b".idata$5",
                data,
                Chunk {
                    kind: ChunkKind::ImportAddresses(crate::n32(d)),
                    align: 8,
                    size,
                    uninitialized: false,
                    rva: 0,
                },
                &mut chunks,
            );
        }
        // Each DLL's hint/name entries, then its name (the import
        // descriptor's, loaded after its imports).
        let mut at = 0u32;
        for dll in &dlls {
            for &f in &dll.imports {
                let FileKind::Import { import, .. } = &r.files[f as usize].kind else {
                    continue;
                };
                if let Some(n) = import.import_name() {
                    name_offsets.insert(f, at);
                    at += (2 + crate::n32(n.len()) + 1 + 1) & !1;
                }
            }
            dll_name_offsets.push(at);
            at += (crate::n32(dll.name.len()) + 1 + 1) & !1;
        }
        add(
            b".idata$6",
            data,
            Chunk {
                kind: ChunkKind::ImportNames,
                align: 2,
                size: at,
                uninitialized: false,
                rva: 0,
            },
            &mut chunks,
        );
    }

    // The export directory, in `.edata` (merged into `.rdata`).
    if let Some(t) = &exports {
        add(
            b".edata",
            scn::CNT_INITIALIZED_DATA | scn::MEM_READ,
            Chunk {
                kind: ChunkKind::Exports,
                align: 4,
                size: t.directory.size,
                uninitialized: false,
                rva: 0,
            },
            &mut chunks,
        );
    }

    if let Some(res) = &resources {
        add(
            b".rsrc",
            scn::CNT_INITIALIZED_DATA | scn::MEM_READ,
            Chunk {
                kind: ChunkKind::Resources,
                align: 8,
                size: res.size,
                uninitialized: false,
                rva: 0,
            },
            &mut chunks,
        );
    }

    // Output sections: link.exe's order first, then the others as their
    // first partial section sorts; within each, partial sections by name
    // (the output section's own name's before those merged into it), and
    // uninitialized data last.
    partials.sort_by(|a, b| a.0.cmp(&b.0));
    const ORDER: &[&[u8]] = &[b".text", b".rdata", b".data", b".pdata", b".tls", b".rsrc"];
    let mut out_names: Vec<(Vec<u8>, u32)> = Vec::new();
    for ((out, _, chars), _) in &partials {
        let chars = *chars;
        if !out_names.iter().any(|(n, c)| n == out && *c == chars) {
            out_names.push((out.clone(), chars));
        }
    }
    let rank = |name: &[u8], chars: u32| -> (u32, usize) {
        let fixed = ORDER.iter().position(|o| *o == name).unwrap_or(ORDER.len());
        let order = if chars & scn::MEM_DISCARDABLE != 0 {
            2
        } else if name == b".rsrc" {
            1
        } else {
            0
        };
        (order, fixed)
    };
    out_names.sort_by_key(|(n, c)| rank(n, *c));
    // Sections of one name with different characteristics are one
    // section, their characteristics combined (link.exe warns, LNK4078).
    let mut sections: Vec<OutputSection> = Vec::new();
    for (name, chars) in out_names {
        match sections.iter_mut().find(|s| s.name == name) {
            Some(s) => s.characteristics |= chars,
            None => sections.push(OutputSection {
                name,
                characteristics: chars,
                chunks: Vec::new(),
                rva: 0,
                virtual_size: 0,
                raw_size: 0,
                file_offset: 0,
            }),
        }
    }
    for s in &mut sections {
        let mut mine: Vec<&Partial> = partials.iter().filter(|p| p.0.0 == s.name).collect();
        // link.exe's order (as its COFF groups record it): every group
        // (input section name) by its name, the merged ones among the
        // section's own; except the imports': the address table
        // (`.idata$5`) first, the rest of `.idata$` last.
        mine.sort_by(|a, b| {
            let rank = |i: &[u8]| {
                if i == b".idata$5" {
                    0
                } else if i.starts_with(b".idata$") {
                    2
                } else {
                    1
                }
            };
            let (ia, ib) = (&a.0.1, &b.0.1);
            (rank(ia), ia).cmp(&(rank(ib), ib))
        });
        let (init, uninit): (Vec<u32>, Vec<u32>) = mine
            .iter()
            .flat_map(|p| p.1.iter().copied())
            .partition(|&c| !chunks[c as usize].uninitialized);
        s.chunks = init;
        s.chunks.extend(uninit);
        // A section with only uninitialized data is uninitialized.
        if s.chunks.iter().all(|&c| chunks[c as usize].uninitialized) {
            s.characteristics =
                (s.characteristics & !scn::CNT_INITIALIZED_DATA) | scn::CNT_UNINITIALIZED_DATA;
        } else {
            s.characteristics &= !scn::CNT_UNINITIALIZED_DATA;
        }
    }
    // Empty sections aren't written (the loader refuses a section of no
    // size).
    sections.retain(|s| s.chunks.iter().any(|&c| chunks[c as usize].size > 0));
    // `/SECTION` attributes.
    for a in &config.sections {
        if let Some(s) = sections.iter_mut().find(|s| s.name == a.name.as_bytes()) {
            s.characteristics = (s.characteristics | a.set) & !a.clear;
        }
    }

    let section_align = config.align.unwrap_or(0x1000);
    let file_align = config.file_align.unwrap_or(0x200);
    // DOS header and stub, PE signature, file header, optional header,
    // section headers (one more for `.reloc`).
    let optional = if config.machine.is_none_or(crate::config::Machine::is_64) {
        240
    } else {
        224
    };
    let headers = 0x80 + 4 + 20 + optional + 40 * (crate::n32(sections.len()) + 1);
    let headers_size = align_up(headers, file_align);
    let mut l = Layout {
        chunks,
        sections,
        section_chunks,
        common_chunks,
        import_slots,
        thunk_chunks,
        dlls,
        directory_order,
        headers_size,
        image_size: 0,
        file_size: 0,
        section_align,
        file_align,
        name_offsets,
        dll_name_offsets,
        base_relocations: Vec::new(),
        exports,
        resources,
    };
    l.assign_addresses();
    l
}

pub(crate) fn align_up(v: u32, a: u32) -> u32 {
    v.div_ceil(a.max(1)) * a.max(1)
}

impl Layout {
    /// Gives every section and chunk its address and file offset.
    pub fn assign_addresses(&mut self) {
        let mut rva = align_up(self.headers_size, self.section_align);
        let mut file = self.headers_size;
        for s in &mut self.sections {
            s.rva = rva;
            s.file_offset = file;
            let mut at = 0u32;
            let mut init_end = 0u32;
            for &c in &s.chunks {
                let chunk = &mut self.chunks[c as usize];
                at = align_up(at, chunk.align);
                chunk.rva = rva + at;
                at += chunk.size;
                if !chunk.uninitialized {
                    init_end = at;
                }
            }
            s.virtual_size = at;
            s.raw_size = align_up(init_end, self.file_align);
            if s.raw_size == 0 {
                s.file_offset = 0;
            }
            file += s.raw_size;
            rva = align_up(rva + at.max(1), self.section_align);
        }
        self.image_size = rva;
        self.file_size = file;
    }

    /// Adds the `.reloc` section (after every other section).
    pub fn add_base_relocations(&mut self, data: Vec<u8>) {
        if data.is_empty() {
            return;
        }
        let c = u32::try_from(self.chunks.len()).unwrap_or(u32::MAX);
        self.chunks.push(Chunk {
            kind: ChunkKind::BaseRelocations,
            align: 4,
            size: u32::try_from(data.len()).unwrap_or(u32::MAX),
            uninitialized: false,
            rva: 0,
        });
        self.sections.push(OutputSection {
            name: b".reloc".to_vec(),
            characteristics: scn::CNT_INITIALIZED_DATA | scn::MEM_DISCARDABLE | scn::MEM_READ,
            chunks: vec![c],
            rva: 0,
            virtual_size: 0,
            raw_size: 0,
            file_offset: 0,
        });
        self.base_relocations = data;
        self.assign_addresses();
    }

    /// The import object's thunk's address, or its IAT entry's.
    pub fn import_address(&self, f: u32) -> Option<u32> {
        let &(d, k) = self.import_slots.get(&f)?;
        let iat = self
            .chunks
            .iter()
            .find(|c| c.kind == ChunkKind::ImportAddresses(d))?;
        Some(iat.rva + 8 * k)
    }
}

/// The name an import object is imported by, for its hint/name entry.
pub fn import_name(import: &ImportObject<'static>) -> Option<&'static [u8]> {
    import.import_name()
}
