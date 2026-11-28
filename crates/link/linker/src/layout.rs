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

/// A delay-loaded DLL's `__tailMerge_dll`, as link.exe writes it (x64): the
/// argument registers saved, `__delayLoadHelper2(descriptor, rax)` called
/// (rax: the import's slot), the registers restored, then a jump to what
/// it returned. The descriptor's displacement is at `TAIL_MERGE_DESCRIPTOR`,
/// the helper's at `TAIL_MERGE_HELPER` (each relative to the 4 bytes' end).
pub const TAIL_MERGE: [u8; 0x79] = [
    0x48, 0x89, 0x4C, 0x24, 0x08, // mov [rsp+8], rcx
    0x48, 0x89, 0x54, 0x24, 0x10, // mov [rsp+10h], rdx
    0x4C, 0x89, 0x44, 0x24, 0x18, // mov [rsp+18h], r8
    0x4C, 0x89, 0x4C, 0x24, 0x20, // mov [rsp+20h], r9
    0x48, 0x83, 0xEC, 0x68, // sub rsp, 68h
    0x66, 0x0F, 0x7F, 0x44, 0x24, 0x20, // movdqa [rsp+20h], xmm0
    0x66, 0x0F, 0x7F, 0x4C, 0x24, 0x30, // movdqa [rsp+30h], xmm1
    0x66, 0x0F, 0x7F, 0x54, 0x24, 0x40, // movdqa [rsp+40h], xmm2
    0x66, 0x0F, 0x7F, 0x5C, 0x24, 0x50, // movdqa [rsp+50h], xmm3
    0x48, 0x8B, 0xD0, // mov rdx, rax
    0x48, 0x8D, 0x0D, 0, 0, 0, 0, // lea rcx, [descriptor]
    0xE8, 0, 0, 0, 0, // call __delayLoadHelper2
    0x66, 0x0F, 0x6F, 0x44, 0x24, 0x20, // movdqa xmm0, [rsp+20h]
    0x66, 0x0F, 0x6F, 0x4C, 0x24, 0x30, // movdqa xmm1, [rsp+30h]
    0x66, 0x0F, 0x6F, 0x54, 0x24, 0x40, // movdqa xmm2, [rsp+40h]
    0x66, 0x0F, 0x6F, 0x5C, 0x24, 0x50, // movdqa xmm3, [rsp+50h]
    0x48, 0x8B, 0x4C, 0x24, 0x70, // mov rcx, [rsp+70h]
    0x48, 0x8B, 0x54, 0x24, 0x78, // mov rdx, [rsp+78h]
    0x4C, 0x8B, 0x84, 0x24, 0x80, 0, 0, 0, // mov r8, [rsp+80h]
    0x4C, 0x8B, 0x8C, 0x24, 0x88, 0, 0, 0, // mov r9, [rsp+88h]
    0x48, 0x83, 0xC4, 0x68, // add rsp, 68h
    0xEB, 0x00, // jmp $+2
    0xFF, 0xE0, // jmp rax
];
/// Where `TAIL_MERGE`'s descriptor displacement is.
pub const TAIL_MERGE_DESCRIPTOR: usize = 0x36;
/// Where `TAIL_MERGE`'s call displacement is.
pub const TAIL_MERGE_HELPER: usize = 0x3B;
/// How much of `TAIL_MERGE` its `.pdata` entry covers (all but `jmp rax`).
pub const TAIL_MERGE_FUNCTION: u32 = 0x77;
/// The tail merge's unwind data: a 0x18-byte prologue that allocates 0x68
/// bytes (`sub rsp, 68h`).
pub const TAIL_MERGE_UNWIND: [u8; 8] = [0x01, 0x18, 0x01, 0x00, 0x18, 0xC2, 0x00, 0x00];

/// One piece of an output section.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum ChunkKind {
    /// An input section: file, 0-based section.
    Input(u32, u32),
    /// Common data: the global.
    Common(u32),
    /// An import's thunk (`jmp [__imp_x]`): the import's file.
    Thunk(u32),
    /// The import directory's entries (one per DLL).
    ImportDirectory,
    /// The import directory's null entry (`.idata$3`).
    NullImportDescriptor,
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
    /// The resources' directory (`.rsrc$01`).
    Resources,
    /// The resources' data (`.rsrc$02`, right after the directory).
    ResourceData,
    /// The debug directory (at the end of the `.rdata` group).
    DebugDirectory,
    /// The `coffgrp` debug data: every COFF group's address, size, name.
    CoffGroups,
    /// `/Brepro`'s debug data: its hash.
    Repro,
    /// `/DEBUG`'s CodeView record (`RSDS`: the PDB's GUID, age, path).
    CodeView,
    /// `/DEBUG`'s VC feature counts (how many objects were compiled how).
    VcFeature,
    /// The volatile metadata the load configuration points to (x64).
    VolatileMetadata,
    /// A group whose every section /OPT:REF discarded: no bytes, but where
    /// it starts is aligned as they were (as link.exe keeps the group:
    /// guard_support.obj's 128-byte `.rdata$CastGuardVftablesA` puts the
    /// next group on 128 bytes).
    Empty,
    /// The delay-load directory (`.didat$2`): a descriptor per delay-loaded
    /// DLL.
    DelayDirectory,
    /// Its null descriptor (`.didat$3`).
    DelayNullDescriptor,
    /// A delay-loaded DLL's name table (`.didat$4`): its index among them.
    DelayNames(u32),
    /// The delay-loaded imports' hint/name entries (`.didat$6`).
    DelayHintNames,
    /// A delay-loaded DLL's bound address table (`.didat$7`, zeros).
    DelayBound(u32),
    /// The room link.exe leaves after the bound tables (zeros: 0x100 bytes
    /// and a tenth of the bound tables' size).
    DelayBoundRoom,
    /// A delay-loaded DLL's unload table (`.didat$8`, `/DELAY:UNLOAD`: a
    /// copy of its address table).
    DelayUnload(u32),
    /// A delay-loaded DLL's address table (`.didat$5`, in `.data`).
    DelayAddresses(u32),
    /// A delay-loaded DLL's name (in `.rdata`, where its first import loads).
    DelayDllName(u32),
    /// A delay-loaded DLL's module handle (`__hmod__dll`, in `.bss`).
    DelayHandle(u32),
    /// A delay-loaded DLL's `__tailMerge_dll`: saves the arguments, calls
    /// `__delayLoadHelper2` with the DLL's descriptor, jumps to the import.
    TailMerge(u32),
    /// The tail merge's unwind data (in `.xdata`).
    TailMergeUnwind(u32),
    /// The tail merge's `.pdata` entry.
    TailMergeFunction(u32),
    /// A delay-loaded import's `__imp_load_x`: `lea rax, [__imp_x]`, then a
    /// jump to its DLL's tail merge (the import's file).
    DelayThunk(u32),
    /// One of `/GUARD:CF`'s tables (as `guard::Symbol::Table` numbers them).
    GuardTable(u8),
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
    /// Its COFF groups (input section names), in order, with their chunks
    /// (none for a group whose sections hold nothing in the image).
    pub groups: Vec<(Vec<u8>, Vec<u32>)>,
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
    /// The delay-loaded DLLs, in the order their first imports loaded.
    pub delay_dlls: Vec<Dll>,
    /// For each delay-loaded import file: its DLL's index (in `delay_dlls`)
    /// and its place in the DLL's name and address tables.
    pub delay_slots: Map<u32, (u32, u32)>,
    /// Each delay-loaded DLL's imports in its name, address, and unload
    /// tables' order (link.exe's sort of them: [`delay_tables`]).
    pub delay_tables: Vec<Vec<u32>>,
    /// Each delay-loaded import's hint/name entry's offset in
    /// `DelayHintNames`.
    pub delay_name_offsets: Map<u32, u32>,
    /// `/OPT:ICF`'s folds: each folded section (file, section) and the one
    /// it folded into (for the map's names in them).
    pub folded: Map<(u32, u32), (u32, u32)>,
    /// `/GUARD:CF`'s tables' entries.
    pub guard: crate::guard::Tables,
    /// The headers' size (file-aligned).
    pub headers_size: u32,
    /// The optional header's size (240 for PE32+, 224 for PE32).
    pub optional_size: u32,
    /// `/FUNCTIONPADMIN`'s bytes before each piece of code (0 without it).
    pub function_pad: u32,
    /// The chunks that padding goes before: code with a function symbol
    /// in it (COMDAT or not; not an assembler's, whose symbols have no
    /// type) and import thunks.
    pub padded: turbonop_hash::Set<u32>,
    /// Each group's start (by its first chunk), where it isn't its first
    /// chunk's: padding before that chunk is the group's.
    pub group_starts: Map<u32, u32>,
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
    /// The Rich header: its entries, its checksum, and the PE header's
    /// offset after it.
    pub rich: (Vec<(u32, u32)>, u32, u32),
    /// `/DEBUG`: the PDB's name as the CodeView record gives it.
    pub codeview_name: Vec<u8>,
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
    // /GUARD's tables: read by the link, empty in the image (link.exe
    // lists them among .rdata's groups).
    (".gfids", ".rdata"),
    (".giats", ".rdata"),
    (".gljmp", ".rdata"),
    (".gehcont", ".rdata"),
    // The TLS template (only the loader reads it: link.exe puts it among
    // .rdata's groups, .rdata staying read-only).
    (".tls", ".rdata"),
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
    let started = std::time::Instant::now();
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
    let setup = started.elapsed();
    frontier.retain(|&i| claim(i as usize));
    for &g in roots {
        global(g, &mut frontier, &mut commons);
    }
    // Every section reachable from the frontier, by whichever thread is
    // free (no steps to wait at: see turbonop_par::traverse).
    let parallel = total >= 4096;
    let found = turbonop_par::traverse(frontier, parallel, Vec::new, |i, more, commons| {
        let i = i as usize;
        let f = file_of(i);
        let s = i - base[f];
        let mut c = first_child[i];
        while c != u32::MAX {
            if claim(c as usize) {
                more.push(redirect[c as usize]);
            }
            c = next_sibling[c as usize];
        }
        let FileKind::Object {
            object, globals, ..
        } = &r.files[f].kind
        else {
            return;
        };
        for rel in object.sections[s].relocations {
            let k = rel.symbol_table_index.get() as usize;
            match globals.get(k) {
                Some(&g) if g != crate::resolve::NONE => global(g, more, commons),
                _ => {
                    if let Some(sym) = object.symbols.get(k)
                        && let SymbolKind::Local { section, .. } = sym.kind
                    {
                        let t = redirect[base[f] + section as usize] as usize;
                        if claim(t) {
                            more.push(crate::n32(t));
                        }
                    }
                }
            }
        }
    });
    for c in found {
        commons.extend(c);
    }
    if r.config.time {
        eprintln!(
            "turbolink:     mark: setup {:.3} ms, traversal {:.3} ms, {total} sections",
            setup.as_secs_f64() * 1e3,
            (started.elapsed() - setup).as_secs_f64() * 1e3
        );
    }
    // Commons go at the end of .bss, by the file whose definition each
    // is (link.exe allocates them module by module): the command line's
    // in order, then each library's, the last loaded first (LIBCMT's
    // ucrt_detection.obj's __scrt_ucrt_dll_is_in_use before
    // dyn_tls_dtor.obj's __dyn_tls_dtor_callback before dyn_tls_init.obj's
    // __dyn_tls_init_callback); one file's by name.
    let owner = |g: u32| -> (u32, u32, &[u8]) {
        let global = &r.globals[g as usize];
        let Def::Common { file, .. } = global.def else {
            return (0, 0, global.name);
        };
        match r.files[file as usize].library {
            None => (0, file, global.name),
            Some(lib) => (lib + 1, u32::MAX - file, global.name),
        }
    };
    commons.sort_unstable_by(|&a, &b| owner(a).cmp(&owner(b)));
    commons.dedup();
    let sections = (0..n)
        .map(|f| {
            (base[f]..base[f + 1])
                .map(|i| live[i].load(Ordering::Relaxed))
                .collect()
        })
        .collect();
    let mut live = Live {
        sections,
        commons,
        imports: imports.iter().map(|b| b.load(Ordering::Relaxed)).collect(),
        thunks: thunks.iter().map(|b| b.load(Ordering::Relaxed)).collect(),
    };
    // Without /OPT:REF, link.exe keeps every loaded import, and a thunk for
    // each loaded function import, referenced or not.
    if !gc {
        for (f, file) in r.files.iter().enumerate() {
            if let FileKind::Import { thunk, .. } = &file.kind {
                live.imports[f] = true;
                live.thunks[f] |= thunk.is_some();
            }
        }
    }
    live
}
/// A partial section's key (output name, input name, characteristics)
/// and its chunks.
type Partial = ((Vec<u8>, Vec<u8>, u32), Vec<u32>);

/// What becomes of an object's section in the layout.
enum Fate {
    /// A chunk (the section, its alignment).
    Chunk(u32, u32),
    /// Discarded or folded: its group stays (name, characteristics,
    /// alignment).
    Discarded(&'static [u8], u32, u32),
    /// A /GUARD table not kept: its group is listed, empty.
    EmptyGroup(&'static [u8], u32),
}

/// Lays the image out.
pub fn layout(
    r: &Resolved,
    roots: &[u32],
    exports: Option<crate::exports::ExportTable>,
    resources: Option<crate::resources::Rsrc>,
    order: &Map<(u32, u32), u32>,
) -> Layout {
    let config: &Config = &r.config;
    // (/TIME's breakdown of the layout.)
    let mut lap = std::time::Instant::now();
    let mut note = |what: &str| {
        if r.config.time {
            eprintln!(
                "turbolink:   layout, {what}: {:.3} ms",
                lap.elapsed().as_secs_f64() * 1e3
            );
        }
        lap = std::time::Instant::now();
    };
    let mut live = mark(r, roots, &Map::default());
    note("mark");
    // /OPT:ICF: each folded section's leader (which comes before it); then
    // the mark again, without the folded copies (what only they referred
    // to isn't kept, as link.exe doesn't keep it).
    let folded = if config.icf() {
        let f = crate::icf::fold(r, &live.sections);
        note("ICF");
        f
    } else {
        Map::default()
    };
    if !folded.is_empty() {
        live = mark(r, roots, &folded);
        note("mark again");
    }
    let leaders: turbonop_hash::Set<(u32, u32)> = folded.values().copied().collect();
    // Each leader's alignment: the largest of its class (as link.exe places
    // main's unwind data on 8 bytes when handlers.obj's, aligned on 8, is
    // folded into it).
    let mut class_align: Map<(u32, u32), u32> = Map::default();
    for (&(f, s), &leader) in &folded {
        if let FileKind::Object { object, .. } = &r.files[f as usize].kind {
            let a = class_align.entry(leader).or_insert(1);
            *a = (*a).max(object.sections[s as usize].alignment());
        }
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

    note("chunks: folds' leaders");
    let mut chunks: Vec<Chunk> = Vec::new();
    // Partial sections: (output name, input name, characteristics) and
    // their chunks in input order.
    let mut partials: Vec<Partial> = Vec::new();
    // Each (input name, characteristics)'s partial section: the names are
    // the inputs' own bytes, so finding one copies nothing; the output
    // name is worked out once per distinct one.
    let mut partial_index: Map<(&'static [u8], u32), usize> = Map::default();
    let guard_cf = config.guard_cf;
    let mut add = |name: &'static [u8], chars: u32, chunk: Chunk, chunks: &mut Vec<Chunk>| -> u32 {
        let chars = chars & scn::OUTPUT_MASK;
        let c = u32::try_from(chunks.len()).expect("fewer than 4 billion chunks");
        chunks.push(chunk);
        let p = *partial_index.entry((name, chars)).or_insert_with(|| {
            // (A delay-load address table, written to as its DLL loads,
            // goes to .data as link.exe puts it, the rest of .didat to
            // .rdata.)
            let out = if name.starts_with(b".didat") && chars & scn::MEM_WRITE != 0 {
                // (With /GUARD:CF, in a section of its own, which the
                // loader can protect once the DLLs load.)
                if guard_cf {
                    b".didat".to_vec()
                } else {
                    b".data".to_vec()
                }
            } else {
                output_name(name, &merges)
            };
            let key = (out, name.to_vec(), chars);
            partials.push((key, Vec::new()));
            partials.len() - 1
        });
        partials[p].1.push(c);
        c
    };

    let mut section_chunks: Vec<Vec<u32>> = Vec::with_capacity(r.files.len());
    let mut empty_groups: Vec<(&'static [u8], u32)> = Vec::new();
    // The sections /OPT:REF discarded: (name, characteristics, alignment).
    let mut discarded: Vec<(&'static [u8], u32, u32)> = Vec::new();
    let mut dll_index: Map<&'static [u8], u32> = Map::default();
    let mut dlls: Vec<Dll> = Vec::new();
    let mut import_slots = Map::default();
    let mut thunk_chunks = Map::default();
    let mut delay_index: Map<&'static [u8], u32> = Map::default();
    let mut delay_dlls: Vec<Dll> = Vec::new();
    let mut delay_slots: Map<u32, (u32, u32)> = Map::default();
    // What becomes of each object's sections, worked out file by file in
    // parallel; then, in order, the chunks are made.
    let fates: Vec<Vec<Fate>> = turbonop_par::map_range(r.files.len(), 64, |f| {
        let file = &r.files[f];
        let FileKind::Object { object, .. } = &file.kind else {
            return Vec::new();
        };
        // An import library's own pieces: the tables are built without
        // them.
        if file.idata.is_some() {
            return Vec::new();
        }
        let mut fates = Vec::new();
        for (s, sec) in object.sections.iter().enumerate() {
            let key = (crate::n32(f), crate::n32(s));
            if folded.contains_key(&key) {
                // Folded: its symbols are its leader's (once every file's
                // chunks are known: the leader may come later). Its group
                // stays, as a discarded section's does (all of .text$yd
                // folded into .text$di: an empty .text$yd still on 16
                // bytes).
                fates.push(Fate::Discarded(
                    sec.name,
                    sec.characteristics,
                    sec.alignment(),
                ));
                continue;
            }
            if !live.sections[f][s] {
                // /GUARD's tables: no contents, but their group is listed
                // (as link.exe lists it).
                if kept_section(r, f, s) && sec.is_image() {
                    if dropped(sec.name) {
                        fates.push(Fate::EmptyGroup(sec.name, sec.characteristics));
                    } else {
                        fates.push(Fate::Discarded(
                            sec.name,
                            sec.characteristics,
                            sec.alignment(),
                        ));
                    }
                }
                continue;
            }
            if let Some((Selection::Associative(parent), _)) = sec.comdat
                && folded.contains_key(&(key.0, parent))
                && !leaders.contains(&key)
            {
                // A folded section's unwind data goes with it (unless other
                // copies' unwind data was folded into it).
                continue;
            }
            if !folded.is_empty()
                && sec.name == b".pdata"
                && pdata_function(r, f, s).is_some_and(|t| folded.contains_key(&t))
            {
                // The .pdata of a function folded into another (a C++
                // funclet, .text$x, whose parent isn't folded): link.exe
                // keeps one entry per function.
                continue;
            }
            let align = sec
                .alignment()
                .max(class_align.get(&key).copied().unwrap_or(1));
            fates.push(Fate::Chunk(crate::n32(s), align));
        }
        fates
    });
    for (f, file) in r.files.iter().enumerate() {
        match &file.kind {
            FileKind::Object { object, .. } => {
                let mut mine = vec![u32::MAX; object.sections.len()];
                for fate in &fates[f] {
                    match *fate {
                        Fate::Discarded(name, chars, align) => discarded.push((name, chars, align)),
                        Fate::EmptyGroup(name, chars) => empty_groups.push((name, chars)),
                        Fate::Chunk(s, align) => {
                            let sec = &object.sections[s as usize];
                            let uninitialized =
                                sec.characteristics & scn::CNT_UNINITIALIZED_DATA != 0;
                            mine[s as usize] = add(
                                sec.name,
                                sec.characteristics,
                                Chunk {
                                    kind: ChunkKind::Input(crate::n32(f), s),
                                    align,
                                    size: sec.size,
                                    uninitialized,
                                    rva: 0,
                                },
                                &mut chunks,
                            );
                        }
                    }
                }
                section_chunks.push(mine);
            }
            FileKind::Import { import, .. } => {
                section_chunks.push(Vec::new());
                if !live.imports[f] {
                    continue;
                }
                let code = scn::CNT_CODE | scn::MEM_EXECUTE | scn::MEM_READ;
                if config.is_delay_loaded(import.dll) {
                    // A delay-loaded DLL's pieces, where its first import
                    // loads (as link.exe places them): the tail merge (code),
                    // its unwind data and .pdata entry, the DLL's name, its
                    // module handle.
                    let d = match delay_index.get(import.dll) {
                        Some(&d) => d,
                        None => {
                            let d = crate::n32(delay_dlls.len());
                            delay_index.insert(import.dll, d);
                            delay_dlls.push(Dll {
                                name: import.dll,
                                imports: Vec::new(),
                            });
                            let data = scn::CNT_INITIALIZED_DATA | scn::MEM_READ;
                            let pieces = [
                                (
                                    &b".text$mn"[..],
                                    code,
                                    ChunkKind::TailMerge(d),
                                    1,
                                    TAIL_MERGE.len(),
                                ),
                                (
                                    b".rdata",
                                    data,
                                    ChunkKind::DelayDllName(d),
                                    16,
                                    import.dll.len() + 1,
                                ),
                                (
                                    b".bss",
                                    scn::CNT_UNINITIALIZED_DATA | scn::MEM_READ | scn::MEM_WRITE,
                                    ChunkKind::DelayHandle(d),
                                    8,
                                    8,
                                ),
                                (
                                    b".xdata",
                                    data,
                                    ChunkKind::TailMergeUnwind(d),
                                    4,
                                    TAIL_MERGE_UNWIND.len(),
                                ),
                                (b".pdata", data, ChunkKind::TailMergeFunction(d), 4, 12),
                            ];
                            for (name, chars, kind, align, size) in pieces {
                                add(
                                    name,
                                    chars,
                                    Chunk {
                                        kind,
                                        align,
                                        size: crate::n32(size),
                                        uninitialized: kind == ChunkKind::DelayHandle(d),
                                        rva: 0,
                                    },
                                    &mut chunks,
                                );
                            }
                            d
                        }
                    };
                    let slot = &mut delay_dlls[d as usize].imports;
                    delay_slots.insert(crate::n32(f), (d, crate::n32(slot.len())));
                    slot.push(crate::n32(f));
                    // A call to the import itself (not through __imp_) goes
                    // through an ordinary thunk to its delay-load slot.
                    if live.thunks[f] {
                        let c = add(
                            b".text$mn",
                            code,
                            Chunk {
                                kind: ChunkKind::Thunk(crate::n32(f)),
                                align: 1,
                                size: 6,
                                uninitialized: false,
                                rva: 0,
                            },
                            &mut chunks,
                        );
                        thunk_chunks.insert(crate::n32(f), c);
                    }
                    add(
                        b".text$mn",
                        code,
                        Chunk {
                            kind: ChunkKind::DelayThunk(crate::n32(f)),
                            align: 1,
                            size: 12,
                            uninitialized: false,
                            rva: 0,
                        },
                        &mut chunks,
                    );
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
                        // Among the code (link.exe's .text$mn group).
                        b".text$mn",
                        code,
                        Chunk {
                            kind: ChunkKind::Thunk(crate::n32(f)),
                            align: 1, // (link.exe packs thunks)
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
    note("chunks: inputs'");
    // The folded sections: each one's chunk is its leader's.
    for (&(f, s), &(lf, ls)) in &folded {
        let c = section_chunks[lf as usize][ls as usize];
        section_chunks[f as usize][s as usize] = c;
    }

    // The import tables' order, as link.exe's (see docs/turbolink.md): the
    // pieces in the order they loaded (each import; each DLL's import
    // descriptor and null thunk, from its import library). The directory
    // and the names keep that order; the lookup tables and the IAT are
    // sorted with qsort (not stable) by the DLL's name without its
    // extension, a null thunk after the imports.
    #[derive(Clone, Copy)]
    enum Piece {
        Import(u32),
        Descriptor(u32),
        NullThunk(u32),
    }
    let stems: Map<&[u8], u32> = dlls
        .iter()
        .enumerate()
        .map(|(d, dll)| (crate::resolve::dll_stem(dll.name), crate::n32(d)))
        .collect();
    let mut pieces = Vec::new();
    for (f, file) in r.files.iter().enumerate() {
        match (&file.idata, &file.kind) {
            // Every import loaded, used or not: link.exe sorts before
            // /OPT:REF drops the unused ones (their places count).
            (_, FileKind::Import { import, .. }) => {
                if stems.contains_key(crate::resolve::dll_stem(import.dll)) {
                    pieces.push(Piece::Import(crate::n32(f)));
                }
            }
            (Some(crate::resolve::IdataPiece::Descriptor(x)), _) => {
                if let Some(&d) = stems.get(x.as_slice()) {
                    pieces.push(Piece::Descriptor(d));
                }
            }
            (Some(crate::resolve::IdataPiece::NullThunk(x)), _) => {
                if let Some(&d) = stems.get(x.as_slice()) {
                    pieces.push(Piece::NullThunk(d));
                }
            }
            _ => {}
        }
    }
    let dll_of = |f: u32| match &r.files[f as usize].kind {
        FileKind::Import { import, .. } => stems[crate::resolve::dll_stem(import.dll)],
        FileKind::Object { .. } => 0,
    };
    let mut iat: Vec<(u32, Option<u32>)> = pieces
        .iter()
        .filter_map(|p| match *p {
            Piece::Import(f) => Some((dll_of(f), Some(f))),
            Piece::NullThunk(d) => Some((d, None)),
            Piece::Descriptor(_) => None,
        })
        .collect();
    crate::msqsort::qsort(&mut iat, |a, b| {
        let (sa, sb) = (
            crate::resolve::dll_stem(dlls[a.0 as usize].name),
            crate::resolve::dll_stem(dlls[b.0 as usize].name),
        );
        match sa.cmp(sb) {
            std::cmp::Ordering::Less => -1,
            std::cmp::Ordering::Greater => 1,
            std::cmp::Ordering::Equal => i32::from(a.1.is_none()) - i32::from(b.1.is_none()),
        }
    });
    // The DLLs in the tables' order; each one's imports in it.
    let mut new_index = vec![u32::MAX; dlls.len()];
    let mut sorted: Vec<Dll> = Vec::with_capacity(dlls.len());
    for &(d, f) in &iat {
        if new_index[d as usize] == u32::MAX {
            new_index[d as usize] = crate::n32(sorted.len());
            sorted.push(Dll {
                name: dlls[d as usize].name,
                imports: Vec::new(),
            });
        }
        if let Some(f) = f
            && live.imports[f as usize]
        {
            let n = new_index[d as usize];
            let slot = &mut sorted[n as usize].imports;
            import_slots.insert(f, (n, crate::n32(slot.len())));
            slot.push(f);
        }
    }
    dlls = sorted;
    // The directory: as the descriptors loaded (then any DLL without one).
    let mut directory_order: Vec<u32> = Vec::with_capacity(dlls.len());
    for p in &pieces {
        if let Piece::Descriptor(d) = *p {
            let n = new_index[d as usize];
            if n != u32::MAX && !directory_order.contains(&n) {
                directory_order.push(n);
            }
        }
    }
    for p in &pieces {
        if let Piece::Import(f) = *p
            && let Some(&(n, _)) = import_slots.get(&f)
            && !directory_order.contains(&n)
        {
            directory_order.push(n);
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
        // The DLLs' entries (`.idata$2`, from each import descriptor), and
        // the null entry after them (`.idata$3`, the null import
        // descriptor's): link.exe's groups.
        // (Each aligned as the import libraries' own pieces are: an import
        // descriptor's .idata$2 may want 8.)
        let piece_align = |name: &[u8]| {
            r.files
                .iter()
                .filter(|f| f.idata.is_some())
                .filter_map(|f| match &f.kind {
                    FileKind::Object { object, .. } => Some(object),
                    FileKind::Import { .. } => None,
                })
                .flat_map(|o| o.sections.iter())
                .filter(|s| s.name == name)
                .map(crate::object::Section::alignment)
                .fold(4, u32::max)
        };
        add(
            b".idata$2",
            data,
            Chunk {
                kind: ChunkKind::ImportDirectory,
                align: piece_align(b".idata$2"),
                size: crate::n32(dlls.len() * 20),
                uninitialized: false,
                rva: 0,
            },
            &mut chunks,
        );
        add(
            b".idata$3",
            data,
            Chunk {
                kind: ChunkKind::NullImportDescriptor,
                align: piece_align(b".idata$3"),
                size: 20,
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
        // The names in load order: each import's hint/name entry, each
        // DLL's name where its import descriptor loaded (a DLL without one:
        // its name last).
        let mut at = 0u32;
        dll_name_offsets = vec![u32::MAX; dlls.len()];
        for p in &pieces {
            match *p {
                Piece::Import(f) => {
                    let FileKind::Import { import, .. } = &r.files[f as usize].kind else {
                        continue;
                    };
                    if !live.imports[f as usize] {
                        continue;
                    }
                    if let Some(n) = import.import_name() {
                        name_offsets.insert(f, at);
                        at += (2 + crate::n32(n.len()) + 1 + 1) & !1;
                    }
                }
                Piece::Descriptor(d) => {
                    let n = new_index[d as usize];
                    if n != u32::MAX && dll_name_offsets[n as usize] == u32::MAX {
                        dll_name_offsets[n as usize] = at;
                        at += (crate::n32(dlls[n as usize].name.len()) + 1 + 1) & !1;
                    }
                }
                Piece::NullThunk(_) => {}
            }
        }
        for (n, dll) in dlls.iter().enumerate() {
            if dll_name_offsets[n] == u32::MAX {
                dll_name_offsets[n] = at;
                at += (crate::n32(dll.name.len()) + 1 + 1) & !1;
            }
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

    // /GUARD:CF's tables, in link.exe's groups (merged into .rdata).
    let guard = if config.guard_cf {
        let chunk_of = |f: u32, s: u32| {
            section_chunks
                .get(f as usize)
                .and_then(|c| c.get(s as usize))
                .copied()
                .unwrap_or(u32::MAX)
        };
        let thunk_of = |f: u32| thunk_chunks.get(&f).copied();
        let delay_thunks: Vec<u32> = chunks
            .iter()
            .enumerate()
            .filter(|(_, c)| matches!(c.kind, ChunkKind::DelayThunk(_)))
            .map(|(i, _)| crate::n32(i))
            .collect();
        crate::guard::tables(r, &chunk_of, &thunk_of, &delay_thunks)
    } else {
        crate::guard::Tables::default()
    };
    // A function that is a valid target starts on 16 bytes, as control-flow
    // guard's targets must: link.exe aligns its section so (an import thunk,
    // packed otherwise; delayimp.lib's __delayLoadHelper2).
    for p in &guard.functions {
        if let crate::guard::Place::Chunk(c, 0) = *p
            && matches!(
                chunks[c as usize].kind,
                ChunkKind::Thunk(_) | ChunkKind::Input(..)
            )
        {
            let a = &mut chunks[c as usize].align;
            *a = (*a).max(16);
        }
    }
    for (i, group) in crate::guard::GROUPS.into_iter().enumerate() {
        let n = guard.table(crate::n32(i).try_into().unwrap_or(0)).len();
        let entry = if i == 0 {
            crate::guard::function_entry_size(&guard)
        } else {
            4
        };
        if n > 0 {
            add(
                group,
                scn::CNT_INITIALIZED_DATA | scn::MEM_READ,
                Chunk {
                    kind: ChunkKind::GuardTable(u8::try_from(i).unwrap_or(0)),
                    align: 4,
                    size: entry * crate::n32(n),
                    uninitialized: false,
                    rva: 0,
                },
                &mut chunks,
            );
        }
    }

    // The delay-load tables (`.didat$`, as link.exe's groups): the
    // descriptors and their null one, each DLL's name table, the hint/name
    // entries, each DLL's bound table (zeros, and room after them all; not
    // with /DELAY:NOBIND), each one's unload table (with
    // /DELAY:UNLOAD), and each one's address table (in .data). The
    // hint/names and bound tables are in the order the imports (and DLLs)
    // loaded; the name, unload and address tables in link.exe's sort of
    // them (`delay_tables`).
    let mut delay_name_offsets = Map::default();
    let (delay_table_dlls, delay_tables, delay_room) =
        delay_tables(r, config, &live.imports, &delay_dlls);
    for (d, table) in delay_tables.iter().enumerate() {
        for (k, &f) in table.iter().enumerate() {
            delay_slots.insert(f, (crate::n32(d), crate::n32(k)));
        }
    }
    if !delay_dlls.is_empty() {
        let data = scn::CNT_INITIALIZED_DATA | scn::MEM_READ;
        let table = |n: usize| crate::n32((n + 1) * 8);
        let mut tables: Vec<(&'static [u8], u32, ChunkKind, u32, u32)> = vec![
            (
                b".didat$2",
                data,
                ChunkKind::DelayDirectory,
                4,
                crate::n32(delay_dlls.len() * 32),
            ),
            (b".didat$3", data, ChunkKind::DelayNullDescriptor, 4, 32),
        ];
        for &d in &delay_table_dlls {
            tables.push((
                b".didat$4",
                data,
                ChunkKind::DelayNames(d),
                8,
                table(delay_dlls[d as usize].imports.len()),
            ));
        }
        let mut at = 0u32;
        for dll in &delay_dlls {
            for &f in &dll.imports {
                if let FileKind::Import { import, .. } = &r.files[f as usize].kind
                    && let Some(n) = import.import_name()
                {
                    delay_name_offsets.insert(f, at);
                    at += (2 + crate::n32(n.len()) + 1 + 1) & !1;
                }
            }
        }
        tables.push((b".didat$6", data, ChunkKind::DelayHintNames, 2, at));
        if !config.delay_nobind {
            for (d, dll) in delay_dlls.iter().enumerate() {
                tables.push((
                    b".didat$7",
                    data,
                    ChunkKind::DelayBound(crate::n32(d)),
                    8,
                    table(dll.imports.len()),
                ));
            }
            if let Some(room) = delay_room {
                tables.push((b".didat$7", data, ChunkKind::DelayBoundRoom, 1, room));
            }
        }
        if config.delay_unload {
            for &d in &delay_table_dlls {
                tables.push((
                    b".didat$8",
                    data,
                    ChunkKind::DelayUnload(d),
                    8,
                    table(delay_dlls[d as usize].imports.len()),
                ));
            }
        }
        for &d in &delay_table_dlls {
            tables.push((
                b".didat$5",
                data | scn::MEM_WRITE,
                ChunkKind::DelayAddresses(d),
                8,
                table(delay_dlls[d as usize].imports.len()),
            ));
        }
        for (name, chars, kind, align, size) in tables {
            add(
                name,
                chars,
                Chunk {
                    kind,
                    align,
                    size,
                    uninitialized: false,
                    rva: 0,
                },
                &mut chunks,
            );
        }
    }

    // The export directory, in `.edata` (merged into `.rdata`).
    if let Some(t) = &exports {
        add(
            b".edata",
            scn::CNT_INITIALIZED_DATA | scn::MEM_READ,
            Chunk {
                kind: ChunkKind::Exports,
                align: 16, // (as link.exe aligns the export directory)
                size: t.directory.size,
                uninitialized: false,
                rva: 0,
            },
            &mut chunks,
        );
    }

    // The resources: their directory (`.rsrc$01`, as link.exe's groups),
    // then their data (`.rsrc$02`).
    if let Some(res) = &resources {
        for (name, kind, size) in [
            (&b".rsrc$01"[..], ChunkKind::Resources, res.directory_size),
            (
                b".rsrc$02",
                ChunkKind::ResourceData,
                res.size - res.directory_size,
            ),
        ] {
            add(
                name,
                scn::CNT_INITIALIZED_DATA | scn::MEM_READ,
                Chunk {
                    kind,
                    align: 8,
                    size,
                    uninitialized: false,
                    rva: 0,
                },
                &mut chunks,
            );
        }
    }

    // What link.exe adds to .rdata: the volatile metadata (x64, for the
    // load configuration), the debug directory (at the end of the .rdata
    // group), and its data (the COFF groups, /Brepro's hash) in
    // .rdata$zzzdbg.
    let data_ro = scn::CNT_INITIALIZED_DATA | scn::MEM_READ;
    let is64 = config.machine.is_none_or(crate::config::Machine::is_64);
    // (In every 64-bit image, load configuration or not: link.exe writes it
    // for an object with no CRT and /NODEFAULTLIB too.)
    if is64 {
        add(
            b".rdata$voltmd",
            data_ro,
            Chunk {
                kind: ChunkKind::VolatileMetadata,
                align: 4,
                size: 24,
                uninitialized: false,
                rva: 0,
            },
            &mut chunks,
        );
    }
    let entries = crate::n32(debug_entries(config).len());
    let mut coff_groups = None;
    if entries > 0 {
        add(
            b".rdata",
            data_ro,
            Chunk {
                kind: ChunkKind::DebugDirectory,
                align: 4,
                size: 28 * entries,
                uninitialized: false,
                rva: 0,
            },
            &mut chunks,
        );
        if config.debug != crate::config::Debug::None {
            // Sized once the PDB's name is known (`set_codeview_name`).
            for (kind, size) in [(ChunkKind::CodeView, 24), (ChunkKind::VcFeature, 20)] {
                add(
                    b".rdata$zzzdbg",
                    data_ro,
                    Chunk {
                        kind,
                        align: 4,
                        size,
                        uninitialized: false,
                        rva: 0,
                    },
                    &mut chunks,
                );
            }
        }
        {
            // Sized once the groups are known (below).
            coff_groups = Some(add(
                b".rdata$zzzdbg",
                data_ro,
                Chunk {
                    kind: ChunkKind::CoffGroups,
                    align: 4,
                    size: 4,
                    uninitialized: false,
                    rva: 0,
                },
                &mut chunks,
            ));
        }
        if config.repro {
            add(
                b".rdata$zzzdbg",
                data_ro,
                Chunk {
                    kind: ChunkKind::Repro,
                    align: 4,
                    size: 36,
                    uninitialized: false,
                    rva: 0,
                },
                &mut chunks,
            );
        }
    }
    note("chunks: the linker's");
    // The groups only discarded sections were in; and the groups with a
    // discarded section aligned more than what stays: such a group still
    // starts at that alignment, as link.exe places it (LIBCMTD's .rdata on
    // 32 bytes though nothing kept in it wants more than 16).
    let mut emptied: Map<(&'static [u8], u32), u32> = Map::default();
    let mut aligned: Map<usize, u32> = Map::default();
    for (name, chars, align) in discarded {
        match partial_index.get(&(name, chars & scn::OUTPUT_MASK)) {
            Some(&p) => {
                let a = aligned.entry(p).or_insert(1);
                *a = (*a).max(align);
            }
            None => {
                let a = emptied.entry((name, chars & scn::OUTPUT_MASK)).or_insert(1);
                *a = (*a).max(align);
            }
        }
    }
    let mut aligned: Vec<_> = aligned.into_iter().collect();
    aligned.sort_unstable();
    for (p, align) in aligned {
        let most = partials[p]
            .1
            .iter()
            .map(|&c| chunks[c as usize].align)
            .max()
            .unwrap_or(1);
        if align > most {
            let c = crate::n32(chunks.len());
            chunks.push(Chunk {
                kind: ChunkKind::Empty,
                align,
                size: 0,
                uninitialized: partials[p].0.2 & scn::CNT_UNINITIALIZED_DATA != 0,
                rva: 0,
            });
            partials[p].1.insert(0, c);
        }
    }
    let mut emptied: Vec<_> = emptied.into_iter().collect();
    emptied.sort_unstable();
    for ((name, chars), align) in emptied {
        let c = crate::n32(chunks.len());
        chunks.push(Chunk {
            kind: ChunkKind::Empty,
            align,
            size: 0,
            uninitialized: chars & scn::CNT_UNINITIALIZED_DATA != 0,
            rva: 0,
        });
        partials.push(((output_name(name, &merges), name.to_vec(), chars), vec![c]));
        partial_index.insert((name, chars), partials.len() - 1);
    }
    // An .edata group is always there (empty without exports), as link.exe
    // lists it.
    if exports.is_none() {
        empty_groups.push((b".edata", scn::CNT_INITIALIZED_DATA | scn::MEM_READ));
    }
    for (name, chars) in empty_groups {
        let chars = chars & scn::OUTPUT_MASK;
        partial_index.entry((name, chars)).or_insert_with(|| {
            partials.push((
                (output_name(name, &merges), name.to_vec(), chars),
                Vec::new(),
            ));
            partials.len() - 1
        });
    }

    // Output sections: link.exe's order first, then the others as their
    // first partial section sorts; within each, partial sections by name
    // (the output section's own name's before those merged into it), and
    // uninitialized data last.
    note("chunks");
    // /ORDER: in each group, the functions it names first, in its order,
    // then the rest as they were (as link.exe orders them).
    if !order.is_empty() {
        for (_, list) in &mut partials {
            // A function's associative sections (its unwind data) follow
            // it.
            list.sort_by_key(|&c| match chunks[c as usize].kind {
                ChunkKind::Input(f, mut s) => {
                    if let FileKind::Object { object, .. } = &r.files[f as usize].kind {
                        for _ in 0..16 {
                            match object.sections[s as usize].comdat {
                                Some((Selection::Associative(p), _)) => s = p,
                                _ => break,
                            }
                        }
                    }
                    order.get(&(f, s)).copied().unwrap_or(u32::MAX)
                }
                _ => u32::MAX,
            });
        }
    }
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
                groups: Vec::new(),
            }),
        }
    }
    // A section's characteristics are its own groups' (sections merged
    // into it take its characteristics, as link.exe's merges do: .rdata
    // stays read-only with .tls in it); a section of merged groups only,
    // theirs.
    for s in &mut sections {
        let own = partials
            .iter()
            .filter(|p| p.0.0 == s.name)
            .filter(|p| {
                let input = &p.0.1;
                let base = input
                    .iter()
                    .position(|&b| b == b'$')
                    .map_or(&input[..], |i| &input[..i]);
                base == s.name.as_slice()
            })
            .fold(0, |a, p| a | p.0.2);
        if own != 0 {
            s.characteristics = own;
        }
    }
    for s in &mut sections {
        let mut mine: Vec<&Partial> = partials.iter().filter(|p| p.0.0 == s.name).collect();
        // link.exe's order (as its COFF groups record it): every group
        // (input section name) by its name, the merged ones among the
        // section's own; except the imports': the address table
        // (`.idata$5`) first, the rest of `.idata$` last.
        // (The export directory goes in below.)
        mine.sort_by(|a, b| {
            let rank = |i: &[u8]| {
                if i == b".idata$5" {
                    0
                } else if i.starts_with(b".idata$") || i.starts_with(b".didat$") {
                    // (The delay-load tables sort among them too.)
                    2
                } else {
                    1
                }
            };
            let (ia, ib) = (&a.0.1, &b.0.1);
            // Uninitialized data (.bss) after the rest, as it is laid out.
            let bss = |p: &Partial| p.0.2 & scn::CNT_UNINITIALIZED_DATA != 0;
            (bss(a), rank(ia), ia).cmp(&(bss(b), rank(ib), ib))
        });
        // The export directory, as link.exe places it: after the inputs'
        // groups, among the last ones by name (after .didat$, before
        // .idata$, before uninitialized data), and its own .rdata$voltmd and
        // .rdata$zzzdbg then each go before the first group that sorts after
        // them: a DLL without imports or unwind data has .rdata, .edata,
        // .rdata$voltmd; with unwind data, .rdata, .rdata$voltmd,
        // .rdata$zzzdbg, .xdata, .edata.
        if let Some(e) = mine.iter().position(|p| p.0.1.as_slice() == b".edata") {
            let edata = mine.remove(e);
            let late =
                |p: &Partial| matches!(p.0.1.as_slice(), b".rdata$voltmd" | b".rdata$zzzdbg");
            let (late_groups, rest): (Vec<&Partial>, Vec<&Partial>) =
                mine.into_iter().partition(|p| late(p));
            let tail = |p: &Partial| {
                let i = p.0.1.as_slice();
                (i.starts_with(b".idata$") && i != b".idata$5")
                    || i.starts_with(b".didat$")
                    || p.0.2 & scn::CNT_UNINITIALIZED_DATA != 0
            };
            // (Among the last groups by name: after .didat$, before .idata$.)
            let cut = rest
                .iter()
                .position(|p| {
                    tail(p)
                        && (p.0.1.as_slice() > b".edata".as_slice()
                            || p.0.2 & scn::CNT_UNINITIALIZED_DATA != 0)
                })
                .unwrap_or(rest.len());
            let mut order: Vec<&Partial> = rest[..cut].to_vec();
            order.push(edata);
            for g in late_groups {
                let at = order
                    .iter()
                    .position(|p| p.0.1 > g.0.1)
                    .unwrap_or(order.len());
                order.insert(at, g);
            }
            order.extend_from_slice(&rest[cut..]);
            mine = order;
        }
        s.groups = mine.iter().map(|p| (p.0.1.clone(), p.1.clone())).collect();
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
    // The COFF groups' list: each section's groups, each 8 bytes and its
    // name (NUL-terminated, to 4 bytes), after a 4-byte signature. Its
    // room is for every group something is in, empty or not (an
    // assembler's empty .gssep$x and .gsspr$y, whose sections aren't
    // written), as link.exe reserves it; only those not empty are listed.
    if let Some(c) = coff_groups {
        chunks[c as usize].size = 4 + sections
            .iter()
            .flat_map(|s| &s.groups)
            .filter(|(_, c)| {
                c.iter()
                    .any(|&c| chunks[c as usize].kind != ChunkKind::Empty)
            })
            .map(|(name, _)| 8 + ((crate::n32(name.len()) + 1 + 3) & !3))
            .sum::<u32>();
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
    // The Rich header (its size depends on its checksum and on the room
    // the headers have), then the PE header.
    let rich_entries = crate::rich::entries(r, resources.as_ref());
    let key = crate::rich::key(&crate::write::dos_header(), &rich_entries);
    let header_sections = sections.len() + usize::from(!config.fixed);
    let mut l = Layout {
        chunks,
        sections,
        section_chunks,
        common_chunks,
        import_slots,
        thunk_chunks,
        dlls,
        directory_order,
        delay_dlls,
        delay_slots,
        delay_tables,
        delay_name_offsets,
        folded,
        guard,
        headers_size: 0,
        optional_size: optional,
        padded: turbonop_hash::Set::default(),
        group_starts: Map::default(),
        // (link.exe's default: 6 bytes on x64, 5 on x86.)
        function_pad: config.function_pad_min.map_or(0, |n| match n {
            0 if optional == 240 => 6,
            0 => 5,
            n => n,
        }),
        image_size: 0,
        file_size: 0,
        section_align,
        file_align,
        name_offsets,
        dll_name_offsets,
        base_relocations: Vec::new(),
        exports,
        resources,
        rich: (rich_entries, key, 0),
        codeview_name: Vec::new(),
    };
    note("sections");
    if l.function_pad > 0 {
        // link.exe pads code with a function in it: a symbol, external or
        // static, whose type is a function's (an assembler's code has none).
        let mut functions = turbonop_hash::Set::<(u32, u32)>::default();
        for (f, file) in r.files.iter().enumerate() {
            if let FileKind::Object { object, .. } = &file.kind {
                for s in &object.symbols {
                    if let SymbolKind::Defined { section, value: 0 }
                    | SymbolKind::Local { section, value: 0 } = s.kind
                        && (s.ty >> 4) & 3 == 2
                    {
                        functions.insert((crate::n32(f), section));
                    }
                }
            }
        }
        for (i, c) in l.chunks.iter().enumerate() {
            let pad = match c.kind {
                ChunkKind::Input(f, s) => match &r.files[f as usize].kind {
                    FileKind::Object { object, .. } => {
                        object.sections[s as usize].characteristics & scn::MEM_EXECUTE != 0
                            && functions.contains(&(f, s))
                    }
                    FileKind::Import { .. } => false,
                },
                ChunkKind::Thunk(_) | ChunkKind::DelayThunk(_) | ChunkKind::TailMerge(_) => true,
                _ => false,
            };
            if pad {
                l.padded.insert(crate::n32(i));
            }
        }
    }
    l.size_headers(header_sections);
    l.assign_addresses();
    l
}

pub(crate) fn align_up(v: u32, a: u32) -> u32 {
    v.div_ceil(a.max(1)) * a.max(1)
}

impl Layout {
    /// Sizes the headers for `header_sections` section headers: the Rich
    /// header (its padding depends on the room the headers have), the PE
    /// header after it.
    fn size_headers(&mut self, header_sections: usize) {
        let (entries, key, _) = &self.rich;
        let padding = crate::rich::padding(
            *key,
            entries.len(),
            header_sections,
            self.optional_size == 240,
            self.file_align,
        );
        let pe = crate::rich::pe_offset(entries.len(), padding);
        self.rich.2 = pe;
        let headers = pe + 4 + 20 + self.optional_size + 40 * crate::n32(header_sections);
        self.headers_size = align_up(headers, self.file_align);
    }

    /// Gives every section and chunk its address and file offset.
    pub fn assign_addresses(&mut self) {
        let mut rva = align_up(self.headers_size, self.section_align);
        let mut file = self.headers_size;
        for s in &mut self.sections {
            s.rva = rva;
            s.file_offset = file;
            let mut at = 0u32;
            let mut init_end = 0u32;
            // Each group starts at the largest alignment of its chunks (as
            // link.exe places them: utility.obj's 8-byte .bss starts .bss on
            // 16 bytes when another .bss in the group wants 16).
            let mut group_start: Map<u32, u32> = Map::default();
            for (_, chunks) in &s.groups {
                if let Some(&first) = chunks.first() {
                    let align = chunks
                        .iter()
                        .map(|&c| self.chunks[c as usize].align)
                        .max()
                        .unwrap_or(1);
                    group_start.insert(first, align);
                }
            }
            // With /FUNCTIONPADMIN, padding before each piece of code (a
            // function, a funclet, an import thunk), then its alignment.
            for &c in &s.chunks {
                let group_align = group_start.get(&c).copied().unwrap_or(1);
                if self.function_pad > 0 && self.padded.contains(&c) {
                    // The group starts before the padding, aligned.
                    if group_start.contains_key(&c) {
                        self.group_starts.insert(c, rva + align_up(at, group_align));
                    }
                    at += self.function_pad;
                }
                let chunk = &mut self.chunks[c as usize];
                at = align_up(at, chunk.align.max(group_align));
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
            // No .reloc: the headers have no room kept for it (as link.exe
            // sizes them: a hello world without relocations has 0x200).
            self.size_headers(self.sections.len());
            self.assign_addresses();
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
            groups: Vec::new(),
        });
        self.base_relocations = data;
        self.assign_addresses();
    }

    /// Sets the CodeView record's PDB name (`/DEBUG`), and lays the image
    /// out again.
    pub fn set_codeview_name(&mut self, name: Vec<u8>) {
        if let Some(c) = self
            .chunks
            .iter_mut()
            .find(|c| c.kind == ChunkKind::CodeView)
        {
            c.size = crate::n32(24 + name.len() + 1);
        }
        self.codeview_name = name;
        self.assign_addresses();
    }

    /// The file offset of `rva` (in a section's initialized data).
    pub fn file_offset_of(&self, rva: u32) -> Option<u32> {
        let s = self
            .sections
            .iter()
            .find(|s| rva >= s.rva && rva < s.rva + s.raw_size)?;
        Some(s.file_offset + (rva - s.rva))
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

/// The order of the delay-loaded DLLs' name, address, and unload tables
/// (`.didat$4`, `$5`, `$8`), as link.exe sorts them: the DLLs' order (their
/// indices in `delay_dlls`), each one's imports in its tables, and the
/// room link.exe leaves after the bound tables (`.didat$7`): 0x100 bytes
/// and a tenth of the bound tables' size as they were before /OPT:REF
/// (8 bytes for each import loaded and each DLL's end; measured over 1 to
/// 7 DLLs of 0 to 20 imports). It goes with the last delay-loaded import
/// to load: none if /OPT:REF drops that one.
///
/// link.exe sorts each of those sections' pieces with the CRT's `qsort`,
/// by the file name (`_wsplitpath`'s, without directory or extension) of
/// the module each came from: an import's archive member's name, which
/// for an import library is its DLL's name. A DLL's table end (its null
/// entry) sorts after its imports. Every delay-loaded import that loaded
/// is in the sort (one `/OPT:REF` drops too, its place then dropped), in
/// the order they loaded, each DLL's end right after its first import:
/// link.exe makes it with the DLL's other pieces. `qsort` isn't stable,
/// so equal names come out as it leaves them: loads A, B, C give C, B, A;
/// A to H give A, G, B, C, D, E, F, H (more than 8 pieces, it partitions).
fn delay_tables(
    r: &Resolved,
    config: &Config,
    live: &[bool],
    delay_dlls: &[Dll],
) -> (Vec<u32>, Vec<Vec<u32>>, Option<u32>) {
    let index: Map<&[u8], u32> = delay_dlls
        .iter()
        .enumerate()
        .map(|(d, dll)| (dll.name, crate::n32(d)))
        .collect();
    // (the module's file name, the import (None: a DLL's end), its DLL's
    // index (u32::MAX: one with nothing live)).
    let mut pieces: Vec<(Vec<u16>, Option<u32>, u32)> = Vec::new();
    let mut seen: Map<&[u8], ()> = Map::default();
    for (f, file) in r.files.iter().enumerate() {
        let FileKind::Import { import, .. } = &file.kind else {
            continue;
        };
        if !config.is_delay_loaded(import.dll) {
            continue;
        }
        let key = module_file_name(file);
        let d = index.get(import.dll).copied().unwrap_or(u32::MAX);
        let first = seen.insert(import.dll, ()).is_none();
        pieces.push((key.clone(), Some(crate::n32(f)), d));
        if first {
            pieces.push((key, None, d));
        }
    }
    let last_live = pieces
        .iter()
        .rev()
        .find_map(|p| p.1)
        .is_some_and(|f| live[f as usize]);
    let room = last_live.then(|| 0x100 + 8 * crate::n32(pieces.len()) / 10);
    crate::msqsort::qsort(&mut pieces, |a, b| match a.0.cmp(&b.0) {
        std::cmp::Ordering::Less => -1,
        std::cmp::Ordering::Greater => 1,
        std::cmp::Ordering::Equal => i32::from(a.1.is_none()) - i32::from(b.1.is_none()),
    });
    let mut order = Vec::with_capacity(delay_dlls.len());
    let mut placed = vec![false; delay_dlls.len()];
    let mut tables = vec![Vec::new(); delay_dlls.len()];
    for (_, f, d) in pieces {
        let Some(slot) = placed.get_mut(d as usize) else {
            continue;
        };
        if !*slot {
            *slot = true;
            order.push(d);
        }
        if let Some(f) = f
            && live[f as usize]
        {
            tables[d as usize].push(f);
        }
    }
    (order, tables, room)
}

/// The name link.exe sorts a module's pieces by: its file name, without
/// directory or extension (as `_wsplitpath` splits it), of the archive
/// member for a library's (`x.lib(KERNEL32.dll)`: `KERNEL32`), of the
/// file otherwise; in UTF-16, as link.exe compares it.
fn module_file_name(file: &crate::resolve::File) -> Vec<u16> {
    let name = file.name.as_str();
    let path = match file.library {
        // `library(member)`: the member is in the last parentheses (a
        // library's path may have its own, `Program Files (x86)`).
        Some(_) => name
            .strip_suffix(')')
            .and_then(|n| {
                let mut depth = 0usize;
                n.char_indices().rev().find_map(|(i, c)| match c {
                    ')' => {
                        depth += 1;
                        None
                    }
                    '(' if depth == 0 => Some(&n[i + 1..]),
                    '(' => {
                        depth -= 1;
                        None
                    }
                    _ => None,
                })
            })
            .unwrap_or(name),
        None => name,
    };
    let base = path.rsplit(['\\', '/', ':']).next().unwrap_or(path);
    let stem = base.rfind('.').map_or(base, |p| &base[..p]);
    stem.encode_utf16().collect()
}

/// The name an import object is imported by, for its hint/name entry.
pub fn import_name(import: &ImportObject<'static>) -> Option<&'static [u8]> {
    import.import_name()
}

/// Whether file `f`'s section `s` stayed (a COMDAT copy that lost didn't).
/// The section (file, section) that .pdata section `s` of file `f` is the
/// unwind entry of: what its first relocation (the function's start) refers to.
fn pdata_function(r: &Resolved, f: usize, s: usize) -> Option<(u32, u32)> {
    let FileKind::Object {
        object, globals, ..
    } = &r.files[f].kind
    else {
        return None;
    };
    let rel = object.sections[s]
        .relocations
        .iter()
        .find(|rel| rel.virtual_address.get() == 0)?;
    let k = rel.symbol_table_index.get() as usize;
    match globals.get(k) {
        Some(&g) if g != crate::resolve::NONE => match r.globals[g as usize].def {
            Def::Section { file, section, .. } => Some((file, section)),
            _ => None,
        },
        _ => match object.symbols.get(k)?.kind {
            SymbolKind::Local { section, .. } => Some((crate::n32(f), section)),
            _ => None,
        },
    }
}

fn kept_section(r: &Resolved, f: usize, s: usize) -> bool {
    match &r.files[f].kind {
        FileKind::Object { kept, .. } => kept[s],
        FileKind::Import { .. } => false,
    }
}

/// The debug directory's entries, in link.exe's order: the POGO (COFF
/// groups) entry without /DEBUG, /Brepro's entry. (/DEBUG's CodeView and
/// VC feature entries come with the PDB.)
pub fn debug_entries(c: &Config) -> Vec<u32> {
    let mut v = Vec::new();
    if c.debug != crate::config::Debug::None {
        v.push(2); // IMAGE_DEBUG_TYPE_CODEVIEW
        v.push(12); // IMAGE_DEBUG_TYPE_VC_FEATURE
    }
    v.push(13); // IMAGE_DEBUG_TYPE_POGO
    if c.repro {
        v.push(16); // IMAGE_DEBUG_TYPE_REPRO
    }
    v
}
