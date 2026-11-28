//! Writing the image: its headers, then every chunk at once (each into
//! its own part of the output, split with `split_at_mut`, relocated in
//! place), the base relocations, and `.pdata` sorted.

#[path = "map.rs"]
mod map;

use turbonop_hash::Map;

use crate::config::{Config, Debug, Machine, Subsystem};
use crate::layout::{ChunkKind, Layout, align_up};
use crate::object::{SymbolKind, scn};
use crate::resolve::{Def, FileKind, NONE, Resolved, Synthetic};

/// Where a symbol is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Target {
    /// At this address (relative to the image base).
    Rva(u32),
    /// An absolute value.
    Absolute(u64),
    /// Nowhere (an undefined symbol under `/FORCE:UNRESOLVED`, a section
    /// not in the image).
    None,
}

/// x64 relocation types (`IMAGE_REL_AMD64_*`).
mod amd64 {
    pub const ABSOLUTE: u16 = 0;
    pub const ADDR64: u16 = 1;
    pub const ADDR32: u16 = 2;
    pub const ADDR32NB: u16 = 3;
    pub const REL32: u16 = 4;
    pub const REL32_5: u16 = 9;
    pub const SECTION: u16 = 0xA;
    pub const SECREL: u16 = 0xB;
}

/// What the writer needs to find targets.
pub struct Image<'a> {
    r: &'a Resolved,
    l: &'a Layout,
    base: u64,
    /// Each DLL's import address table chunk.
    iat: Vec<u32>,
    /// The TLS template's range (the .tls groups' chunks), if any.
    tls: Option<(u32, u32)>,
    /// The delay-load chunks' addresses, by kind.
    delay: turbonop_hash::Map<ChunkKind, u32>,
}

impl<'a> Image<'a> {
    /// The image to write.
    pub fn new(r: &'a Resolved, l: &'a Layout) -> Image<'a> {
        let mut iat = vec![u32::MAX; l.dlls.len()];
        for (i, c) in l.chunks.iter().enumerate() {
            if let ChunkKind::ImportAddresses(d) = c.kind {
                iat[d as usize] = crate::n32(i);
            }
        }
        let mut tls: Option<(u32, u32)> = None;
        for c in &l.chunks {
            if let ChunkKind::Input(f, s) = c.kind
                && let FileKind::Object { object, .. } = &r.files[f as usize].kind
                && object.sections[s as usize].name.starts_with(b".tls")
            {
                let (lo, hi) = tls.unwrap_or((u32::MAX, 0));
                tls = Some((lo.min(c.rva), hi.max(c.rva + c.size.max(1))));
            }
        }
        let mut delay = turbonop_hash::Map::default();
        if !l.delay_dlls.is_empty() {
            for c in &l.chunks {
                if !matches!(
                    c.kind,
                    ChunkKind::Input(..)
                        | ChunkKind::Common(_)
                        | ChunkKind::Thunk(_)
                        | ChunkKind::Empty
                ) {
                    delay.insert(c.kind, c.rva);
                }
            }
        }
        Image {
            r,
            l,
            base: image_base(&r.config),
            iat,
            tls,
            delay,
        }
    }

    /// The address of the (delay-load) chunk of kind `k`.
    fn delay_rva(&self, k: ChunkKind) -> Option<u32> {
        self.delay.get(&k).copied()
    }

    fn import_slot(&self, f: u32) -> Option<u32> {
        if let Some(&(d, k)) = self.l.delay_slots.get(&f) {
            return Some(self.delay_rva(ChunkKind::DelayAddresses(d))? + 8 * k);
        }
        let &(d, k) = self.l.import_slots.get(&f)?;
        let chunk = self.l.chunks.get(*self.iat.get(d as usize)? as usize)?;
        Some(chunk.rva + 8 * k)
    }

    fn section_rva(&self, file: u32, section: u32) -> Option<u32> {
        let c = *self
            .l
            .section_chunks
            .get(file as usize)?
            .get(section as usize)?;
        (c != u32::MAX).then(|| self.l.chunks[c as usize].rva)
    }

    /// Where global `g` is.
    fn global(&self, mut g: u32) -> Target {
        for _ in 0..64 {
            let global = &self.r.globals[g as usize];
            return match global.def {
                Def::Alias(t) => {
                    g = t;
                    continue;
                }
                Def::Section {
                    file,
                    section,
                    value,
                } => self
                    .section_rva(file, section)
                    .map_or(Target::None, |s| Target::Rva(s + value)),
                Def::Common { .. } => self.l.common_chunks.get(&g).map_or(Target::None, |&c| {
                    Target::Rva(self.l.chunks[c as usize].rva)
                }),
                Def::Absolute(v) => Target::Absolute(v),
                Def::Synthetic(Synthetic::ImageBase) => Target::Rva(0),
                Def::Synthetic(Synthetic::Zero) => Target::Absolute(0),
                Def::Synthetic(Synthetic::Guard(s)) => match s {
                    crate::guard::Symbol::Table(i) => self
                        .l
                        .chunks
                        .iter()
                        .find(|c| c.kind == ChunkKind::GuardTable(i))
                        .map_or(Target::Absolute(0), |c| Target::Rva(c.rva)),
                    crate::guard::Symbol::Count(i) => {
                        Target::Absolute(self.l.guard.table(i).len() as u64)
                    }
                    crate::guard::Symbol::Flags => {
                        Target::Absolute(u64::from(crate::guard::flags(self.r, &self.l.guard)))
                    }
                },
                Def::Synthetic(Synthetic::VolatileMetadata) => self
                    .l
                    .chunks
                    .iter()
                    .find(|c| c.kind == ChunkKind::VolatileMetadata)
                    .map_or(Target::Absolute(0), |c| Target::Rva(c.rva)),
                Def::ImportAddress(f) => self.import_slot(f).map_or(Target::None, Target::Rva),
                Def::LocalImport(_) => self.l.local_imports.get(&g).map_or(Target::None, |&c| {
                    Target::Rva(self.l.chunks[c as usize].rva)
                }),
                Def::ImportThunk(f) => self.l.thunk_chunks.get(&f).map_or(Target::None, |&c| {
                    Target::Rva(self.l.chunks[c as usize].rva)
                }),
                // (With /FORCE, as link.exe resolves it: the image base.)
                Def::Undefined if self.r.config.force_unresolved => Target::Rva(0),
                Def::Undefined => Target::None,
            };
        }
        Target::None
    }

    /// Where symbol `i` of file `f` is.
    fn symbol(&self, f: u32, i: u32) -> Target {
        let FileKind::Object {
            object, globals, ..
        } = &self.r.files[f as usize].kind
        else {
            return Target::None;
        };
        match globals.get(i as usize) {
            Some(&g) if g != NONE => self.global(g),
            _ => match object.symbols.get(i as usize).map(|s| s.kind) {
                Some(SymbolKind::Local { section, value }) => self
                    .section_rva(f, section)
                    .map_or(Target::None, |s| Target::Rva(s + value)),
                Some(SymbolKind::Absolute(v)) => Target::Absolute(u64::from(v)),
                _ => Target::None,
            },
        }
    }

    /// The addresses (relative to the image base) of the locations that
    /// hold absolute addresses: what the loader adds its displacement to.
    pub fn base_relocation_sites(&self) -> Vec<(u32, u16)> {
        if self.r.config.fixed {
            return Vec::new();
        }
        let mut sites = Vec::new();
        for sec in &self.l.sections {
            if sec.characteristics & scn::MEM_DISCARDABLE != 0 {
                continue;
            }
            for &c in &sec.chunks {
                let chunk = &self.l.chunks[c as usize];
                let ChunkKind::Input(f, s) = chunk.kind else {
                    continue;
                };
                let FileKind::Object { object, .. } = &self.r.files[f as usize].kind else {
                    continue;
                };
                for rel in object.sections[s as usize].relocations {
                    let kind = rel.kind.get();
                    let ty = match kind {
                        amd64::ADDR64 => 10, // DIR64
                        amd64::ADDR32 => 3,  // HIGHLOW
                        _ => continue,
                    };
                    if let Target::Rva(_) = self.symbol(f, rel.symbol_table_index.get()) {
                        sites.push((chunk.rva + rel.virtual_address.get(), ty));
                    }
                }
            }
        }
        // The locally imported symbols' pointers.
        for (&g, &c) in &self.l.local_imports {
            if let Def::LocalImport(t) = self.r.globals[g as usize].def
                && let Target::Rva(_) = self.global(t)
            {
                sites.push((self.l.chunks[c as usize].rva, 10));
            }
        }
        // The delay-load address tables (and unload tables): each slot holds
        // its thunk's address.
        for (d, dll) in self.l.delay_dlls.iter().enumerate() {
            let d = crate::n32(d);
            for kind in [ChunkKind::DelayAddresses(d), ChunkKind::DelayUnload(d)] {
                if let Some(rva) = self.delay_rva(kind) {
                    for k in 0..dll.imports.len() {
                        sites.push((rva + 8 * crate::n32(k), 10));
                    }
                }
            }
        }
        sites.sort_unstable();
        sites
    }

    /// Writes chunk `c` into `out` (its bytes in the output); returns
    /// what went wrong, if anything.
    fn chunk(&self, c: u32, out: &mut [u8]) -> Vec<String> {
        let chunk = &self.l.chunks[c as usize];
        let mut problems = Vec::new();
        match chunk.kind {
            ChunkKind::Input(f, s) => {
                let FileKind::Object { object, .. } = &self.r.files[f as usize].kind else {
                    return problems;
                };
                let sec = &object.sections[s as usize];
                out[..sec.data.len()].copy_from_slice(sec.data);
                // (A discardable section's reference to what isn't in the
                // image, DWARF's to a discarded COMDAT's: left as it is,
                // as link.exe leaves it.)
                let discardable = sec.characteristics & scn::MEM_DISCARDABLE != 0;
                for rel in sec.relocations {
                    let at = rel.virtual_address.get() as usize;
                    let kind = rel.kind.get();
                    let target = self.symbol(f, rel.symbol_table_index.get());
                    if discardable && target == Target::None {
                        continue;
                    }
                    if let Err(m) = self.relocate(out, at, kind, target, chunk.rva) {
                        problems.push(format!("{}: {m}", self.r.files[f as usize].name));
                    }
                }
            }
            ChunkKind::Common(_) | ChunkKind::NullImportDescriptor => {}
            ChunkKind::Thunk(f) => {
                // jmp qword ptr [rip + disp32]
                out[..2].copy_from_slice(&[0xFF, 0x25]);
                let slot = self.import_slot(f).unwrap_or(0);
                let disp = i64::from(slot) - i64::from(chunk.rva + 6);
                // (An image is under 4 GB and its IAT and code under 2 GB apart.)
                let disp = i32::try_from(disp).unwrap_or(0);
                out[2..6].copy_from_slice(&disp.to_le_bytes());
            }
            ChunkKind::ImportDirectory => {
                for (slot, &d) in self.l.directory_order.iter().enumerate() {
                    let d = d as usize;
                    let dll = &self.l.dlls[d];
                    let lookup = self
                        .l
                        .chunks
                        .iter()
                        .find(|c| c.kind == ChunkKind::ImportLookup(crate::n32(d)))
                        .map_or(0, |c| c.rva);
                    let iat = self.l.chunks[self.iat[d] as usize].rva;
                    let names = self.names_rva();
                    let e = &mut out[slot * 20..slot * 20 + 20];
                    e[0..4].copy_from_slice(&lookup.to_le_bytes());
                    e[12..16].copy_from_slice(&(names + self.l.dll_name_offsets[d]).to_le_bytes());
                    e[16..20].copy_from_slice(&iat.to_le_bytes());
                    let _ = dll;
                }
            }
            ChunkKind::ImportLookup(d) | ChunkKind::ImportAddresses(d) => {
                let names = self.names_rva();
                for (k, &f) in self.l.dlls[d as usize].imports.iter().enumerate() {
                    let FileKind::Import { import, .. } = &self.r.files[f as usize].kind else {
                        continue;
                    };
                    let v: u64 = match self.l.name_offsets.get(&f) {
                        Some(&o) => u64::from(names + o),
                        // By ordinal.
                        None => (1 << 63) | u64::from(import.ordinal_or_hint),
                    };
                    out[k * 8..k * 8 + 8].copy_from_slice(&v.to_le_bytes());
                }
            }
            ChunkKind::ImportNames => {
                for dll in &self.l.dlls {
                    for &f in &dll.imports {
                        let FileKind::Import { import, .. } = &self.r.files[f as usize].kind else {
                            continue;
                        };
                        if let (Some(&o), Some(n)) =
                            (self.l.name_offsets.get(&f), import.import_name())
                        {
                            let o = o as usize;
                            out[o..o + 2].copy_from_slice(&import.ordinal_or_hint.to_le_bytes());
                            out[o + 2..o + 2 + n.len()].copy_from_slice(n);
                        }
                    }
                }
                for (d, dll) in self.l.dlls.iter().enumerate() {
                    let o = self.l.dll_name_offsets[d] as usize;
                    out[o..o + dll.name.len()].copy_from_slice(dll.name);
                }
            }
            ChunkKind::Exports => {
                if let Some(t) = &self.l.exports {
                    t.directory.write(
                        out,
                        chunk.rva,
                        &t.dll_name,
                        // link.exe's export directory's time stamp.
                        u32::MAX,
                        |i| match t.targets[i] {
                            u32::MAX => 0,
                            g => self.rva_of(g).unwrap_or(0),
                        },
                    );
                }
            }
            ChunkKind::GuardTable(i) => {
                // The targets' RVAs, ascending (the functions' each with its
                // metadata byte, if any has one).
                let g = &self.l.guard;
                let mut rvas: Vec<(u32, u8)> = g
                    .table(i)
                    .iter()
                    .filter_map(|p| {
                        let rva = match *p {
                            crate::guard::Place::Chunk(c, off) => {
                                Some(self.l.chunks[c as usize].rva + off)
                            }
                            crate::guard::Place::Slot(f) => self.import_slot(f),
                        }?;
                        Some((rva, g.function_flags.get(p).copied().unwrap_or(0)))
                    })
                    .collect();
                rvas.sort_unstable();
                let size = if i == 0 {
                    crate::guard::function_entry_size(g) as usize
                } else {
                    4
                };
                for (k, (v, flags)) in rvas.into_iter().enumerate() {
                    out[size * k..size * k + 4].copy_from_slice(&v.to_le_bytes());
                    if size == 5 {
                        out[size * k + 4] = flags;
                    }
                }
            }
            ChunkKind::Empty
            | ChunkKind::DelayNullDescriptor
            | ChunkKind::DelayBound(_)
            | ChunkKind::DelayBoundRoom
            | ChunkKind::DelayHandle(_) => {}
            ChunkKind::DelayDirectory => {
                let c = &self.r.config;
                for d in 0..crate::n32(self.l.delay_dlls.len()) {
                    let rva = |k| self.delay_rva(k).unwrap_or(0);
                    let fields = [
                        1, // the descriptor's addresses are RVAs
                        rva(ChunkKind::DelayDllName(d)),
                        rva(ChunkKind::DelayHandle(d)),
                        rva(ChunkKind::DelayAddresses(d)),
                        rva(ChunkKind::DelayNames(d)),
                        if c.delay_nobind {
                            0
                        } else {
                            rva(ChunkKind::DelayBound(d))
                        },
                        if c.delay_unload {
                            rva(ChunkKind::DelayUnload(d))
                        } else {
                            0
                        },
                        0, // the time stamp (not bound)
                    ];
                    for (i, v) in fields.into_iter().enumerate() {
                        let at = d as usize * 32 + i * 4;
                        out[at..at + 4].copy_from_slice(&v.to_le_bytes());
                    }
                }
            }
            ChunkKind::DelayNames(d) => {
                let names = self.delay_rva(ChunkKind::DelayHintNames).unwrap_or(0);
                for (k, &f) in self.l.delay_dlls[d as usize].imports.iter().enumerate() {
                    let FileKind::Import { import, .. } = &self.r.files[f as usize].kind else {
                        continue;
                    };
                    let v: u64 = match self.l.delay_name_offsets.get(&f) {
                        Some(&o) => u64::from(names + o),
                        None => (1 << 63) | u64::from(import.ordinal_or_hint),
                    };
                    out[k * 8..k * 8 + 8].copy_from_slice(&v.to_le_bytes());
                }
            }
            ChunkKind::DelayHintNames => {
                for dll in &self.l.delay_dlls {
                    for &f in &dll.imports {
                        let FileKind::Import { import, .. } = &self.r.files[f as usize].kind else {
                            continue;
                        };
                        if let (Some(&o), Some(n)) =
                            (self.l.delay_name_offsets.get(&f), import.import_name())
                        {
                            let o = o as usize;
                            out[o..o + 2].copy_from_slice(&import.ordinal_or_hint.to_le_bytes());
                            out[o + 2..o + 2 + n.len()].copy_from_slice(n);
                        }
                    }
                }
            }
            ChunkKind::DelayAddresses(d) | ChunkKind::DelayUnload(d) => {
                // Each slot starts at its import's __imp_load_ thunk.
                for (k, &f) in self.l.delay_dlls[d as usize].imports.iter().enumerate() {
                    let thunk = self.delay_rva(ChunkKind::DelayThunk(f)).unwrap_or(0);
                    let v = self.base + u64::from(thunk);
                    out[k * 8..k * 8 + 8].copy_from_slice(&v.to_le_bytes());
                }
            }
            ChunkKind::DelayDllName(d) => {
                let name = self.l.delay_dlls[d as usize].name;
                out[..name.len()].copy_from_slice(name);
            }
            ChunkKind::TailMerge(d) => {
                use crate::layout::{TAIL_MERGE, TAIL_MERGE_DESCRIPTOR, TAIL_MERGE_HELPER};
                out[..TAIL_MERGE.len()].copy_from_slice(&TAIL_MERGE);
                let descriptor = self.delay_rva(ChunkKind::DelayDirectory).unwrap_or(0) + 32 * d;
                let helper = self
                    .r
                    .names
                    .get(&b"__delayLoadHelper2"[..])
                    .map(|&g| self.global(g));
                let helper = match helper {
                    Some(Target::Rva(h)) => h,
                    _ => {
                        problems.push("__delayLoadHelper2 is not defined (delayimp.lib)".into());
                        0
                    }
                };
                for (at, to) in [
                    (TAIL_MERGE_DESCRIPTOR, descriptor),
                    (TAIL_MERGE_HELPER, helper),
                ] {
                    let from = i64::from(chunk.rva) + i64::from(crate::n32(at)) + 4;
                    let disp = i32::try_from(i64::from(to) - from).unwrap_or(0);
                    out[at..at + 4].copy_from_slice(&disp.to_le_bytes());
                }
            }
            ChunkKind::TailMergeUnwind(_) => {
                out[..8].copy_from_slice(&crate::layout::TAIL_MERGE_UNWIND);
            }
            ChunkKind::TailMergeFunction(d) => {
                let start = self.delay_rva(ChunkKind::TailMerge(d)).unwrap_or(0);
                let unwind = self.delay_rva(ChunkKind::TailMergeUnwind(d)).unwrap_or(0);
                out[0..4].copy_from_slice(&start.to_le_bytes());
                out[4..8]
                    .copy_from_slice(&(start + crate::layout::TAIL_MERGE_FUNCTION).to_le_bytes());
                out[8..12].copy_from_slice(&unwind.to_le_bytes());
            }
            ChunkKind::DelayThunk(f) => {
                // lea rax, [rip + slot]; jmp tail merge
                let (d, _) = self.l.delay_slots.get(&f).copied().unwrap_or((0, 0));
                let slot = self.import_slot(f).unwrap_or(0);
                let tail = self.delay_rva(ChunkKind::TailMerge(d)).unwrap_or(0);
                let rel = |to: u32, end: u32| {
                    i32::try_from(i64::from(to) - i64::from(chunk.rva + end)).unwrap_or(0)
                };
                out[..3].copy_from_slice(&[0x48, 0x8D, 0x05]);
                out[3..7].copy_from_slice(&rel(slot, 7).to_le_bytes());
                out[7] = 0xE9;
                out[8..12].copy_from_slice(&rel(tail, 12).to_le_bytes());
            }
            ChunkKind::LocalImport(g) => {
                if let Def::LocalImport(t) = self.r.globals[g as usize].def {
                    let va = match self.global(t) {
                        Target::Rva(r) => self.base + u64::from(r),
                        Target::Absolute(v) => v,
                        Target::None => 0,
                    };
                    out[..8].copy_from_slice(&va.to_le_bytes());
                }
            }
            ChunkKind::VolatileMetadata => {
                // Size 0x18, version 0x80008000, no tables.
                out[..8].copy_from_slice(&[0x18, 0, 0, 0, 0, 0x80, 0, 0x80]);
            }
            ChunkKind::DebugDirectory => {
                for (i, ty) in crate::layout::debug_entries(&self.r.config)
                    .into_iter()
                    .enumerate()
                {
                    let data = match ty {
                        2 => ChunkKind::CodeView,
                        12 => ChunkKind::VcFeature,
                        13 => ChunkKind::CoffGroups,
                        _ => ChunkKind::Repro,
                    };
                    let Some(d) = self.l.chunks.iter().find(|c| c.kind == data) else {
                        continue;
                    };
                    let e = &mut out[i * 28..i * 28 + 28];
                    e[4..8].copy_from_slice(&timestamp(&self.r.config).to_le_bytes());
                    e[12..16].copy_from_slice(&ty.to_le_bytes());
                    // (The COFF groups' list is shorter than its room.)
                    let size = if ty == 13 {
                        self.coff_groups()
                            .iter()
                            .map(|(_, _, name)| 8 + ((crate::n32(name.len()) + 1 + 3) & !3))
                            .sum::<u32>()
                            + 4
                    } else {
                        d.size
                    };
                    e[16..20].copy_from_slice(&size.to_le_bytes());
                    e[20..24].copy_from_slice(&d.rva.to_le_bytes());
                    e[24..28].copy_from_slice(&self.file_offset(d.rva).to_le_bytes());
                }
            }
            ChunkKind::CoffGroups => {
                let mut at = 4usize;
                for (start, size, name) in self.coff_groups() {
                    out[at..at + 4].copy_from_slice(&start.to_le_bytes());
                    out[at + 4..at + 8].copy_from_slice(&size.to_le_bytes());
                    out[at + 8..at + 8 + name.len()].copy_from_slice(name);
                    at += 8 + ((name.len() + 1 + 3) & !3);
                }
            }
            ChunkKind::Repro => {
                // The hash's length; the hash is filled in once the image is.
                out[..4].copy_from_slice(&32u32.to_le_bytes());
            }
            ChunkKind::CodeView => {
                // RSDS, the GUID (filled in once the PDB, or /Brepro's
                // hash, gives it), the age, the PDB's name.
                out[..4].copy_from_slice(b"RSDS");
                out[20..24].copy_from_slice(&1u32.to_le_bytes());
                let n = &self.l.codeview_name;
                out[24..24 + n.len()].copy_from_slice(n);
            }
            ChunkKind::VcFeature => {
                for (i, v) in self.vc_features().iter().enumerate() {
                    out[i * 4..i * 4 + 4].copy_from_slice(&v.to_le_bytes());
                }
            }
            ChunkKind::Resources | ChunkKind::ResourceData => {
                // The section's bytes, of which this chunk is the directory
                // or the data (the data right after the directory).
                if let Some(r) = &self.l.resources {
                    let data = chunk.kind == ChunkKind::ResourceData;
                    let start = if data {
                        chunk.rva - r.directory_size
                    } else {
                        chunk.rva
                    };
                    let mut all = vec![0u8; r.size as usize];
                    r.write(&mut all, start);
                    let part = if data {
                        &all[r.directory_size as usize..]
                    } else {
                        &all[..r.directory_size as usize]
                    };
                    out[..part.len()].copy_from_slice(part);
                }
            }
            ChunkKind::BaseRelocations => {
                out[..self.l.base_relocations.len()].copy_from_slice(&self.l.base_relocations);
            }
        }
        problems
    }

    /// Where `rva` is in the file.
    fn file_offset(&self, rva: u32) -> u32 {
        self.l
            .sections
            .iter()
            .find(|s| rva >= s.rva && rva < s.rva + s.virtual_size.max(1))
            .map_or(0, |s| s.file_offset + (rva - s.rva))
    }

    /// Every COFF group: its start, size (to the next group's start, or
    /// to its section's end), and name.
    fn coff_groups(&self) -> Vec<(u32, u32, &[u8])> {
        let mut out = Vec::new();
        for s in &self.l.sections {
            if s.name == b".reloc" {
                continue;
            }
            let starts: Vec<Option<u32>> = s
                .groups
                .iter()
                .map(|(_, chunks)| {
                    let first = chunks.first().and_then(|c| self.l.group_starts.get(c));
                    first
                        .copied()
                        .or_else(|| chunks.iter().map(|&c| self.l.chunks[c as usize].rva).min())
                })
                .collect();
            let end = s.rva + s.virtual_size;
            for (i, (name, _)) in s.groups.iter().enumerate() {
                // An empty group starts where the next one with contents does.
                let start = starts[i..].iter().flatten().next().copied().unwrap_or(end);
                let next = starts[i + 1..]
                    .iter()
                    .flatten()
                    .next()
                    .copied()
                    .unwrap_or(end);
                let start = start.min(next);
                // Only groups with contents (link.exe lists no empty ones).
                if next > start {
                    out.push((start, next - start, name.as_slice()));
                }
            }
        }
        out
    }

    fn names_rva(&self) -> u32 {
        self.l
            .chunks
            .iter()
            .find(|c| c.kind == ChunkKind::ImportNames)
            .map_or(0, |c| c.rva)
    }

    /// Applies one relocation at `at` in `out` (a chunk at `rva`).
    fn relocate(
        &self,
        out: &mut [u8],
        at: usize,
        kind: u16,
        target: Target,
        rva: u32,
    ) -> Result<(), String> {
        let place = rva.wrapping_add(crate::n32(at));
        let size = match kind {
            amd64::ABSOLUTE => return Ok(()),
            amd64::ADDR64 => 8,
            amd64::SECTION => 2,
            _ => 4,
        };
        let field = out
            .get_mut(at..at + size)
            .ok_or_else(|| format!("relocation at {at:#x} past its section's end"))?;
        let s = match target {
            Target::None => {
                return Err(format!(
                    "relocation (type {kind:#x}) at {place:#x} to a symbol not in the image"
                ));
            }
            t => t,
        };
        let va = |t: Target| -> u64 {
            match t {
                Target::Rva(r) => self.base + u64::from(r),
                Target::Absolute(v) => v,
                Target::None => 0,
            }
        };
        let rva_of = |t: Target| -> u64 {
            match t {
                Target::Rva(r) => u64::from(r),
                Target::Absolute(v) => v.wrapping_sub(self.base),
                Target::None => 0,
            }
        };
        let add32 = |field: &mut [u8], v: u64| {
            let old = u32::from_le_bytes([field[0], field[1], field[2], field[3]]);
            field.copy_from_slice(&old.wrapping_add(crate::low32(v)).to_le_bytes());
        };
        match kind {
            amd64::ADDR64 => {
                let old = u64::from_le_bytes(field.try_into().expect("8 bytes"));
                field.copy_from_slice(&old.wrapping_add(va(s)).to_le_bytes());
            }
            amd64::ADDR32 => add32(field, va(s)),
            amd64::ADDR32NB => add32(field, rva_of(s)),
            amd64::REL32..=amd64::REL32_5 => {
                let k = u64::from(kind - amd64::REL32);
                let from = u64::from(place) + 4 + k;
                add32(field, rva_of(s).wrapping_sub(from));
            }
            amd64::SECTION => {
                // An absolute symbol's section is the one after the last
                // (as the PDB's section map has it).
                let n = match s {
                    Target::Rva(r) => self.section_number(r),
                    Target::Absolute(_) => crate::n16(self.l.sections.len() + 1),
                    Target::None => 0,
                };
                field.copy_from_slice(&n.to_le_bytes());
            }
            amd64::SECREL => {
                let off = match s {
                    // A thread-local variable's: from the TLS template's
                    // start (its .tls groups, wherever they were merged).
                    Target::Rva(r) if self.tls.is_some_and(|(lo, hi)| r >= lo && r < hi) => {
                        u64::from(r - self.tls.map_or(0, |t| t.0))
                    }
                    Target::Rva(r) => {
                        let n = self.section_number(r);
                        let start = n
                            .checked_sub(1)
                            .and_then(|i| self.l.sections.get(i as usize))
                            .map_or(0, |sec| sec.rva);
                        u64::from(r - start)
                    }
                    Target::Absolute(v) => v,
                    Target::None => 0,
                };
                add32(field, off);
            }
            other => return Err(format!("relocation type {other:#x} is not supported yet")),
        }
        Ok(())
    }

    fn section_number(&self, rva: u32) -> u16 {
        self.l
            .sections
            .iter()
            .position(|s| rva >= s.rva && rva < s.rva + s.virtual_size.max(1))
            .map_or(0, |i| u16::try_from(i + 1).unwrap_or(0))
    }

    /// Input section `s` of file `f` with its relocations applied (for
    /// debug information: `SECREL` and `SECTION` give a symbol's place in
    /// the image's sections); a relocation to a symbol not in the image
    /// leaves its field as it was.
    pub fn relocated(&self, f: u32, s: u32) -> Vec<u8> {
        let FileKind::Object { object, .. } = &self.r.files[f as usize].kind else {
            return Vec::new();
        };
        let sec = &object.sections[s as usize];
        let mut out = sec.data.to_vec();
        for rel in sec.relocations {
            let at = rel.virtual_address.get() as usize;
            let target = self.symbol(f, rel.symbol_table_index.get());
            let _ = self.relocate(&mut out, at, rel.kind.get(), target, 0);
        }
        out
    }

    /// The section (1-based) and offset of `rva`.
    pub fn section_offset(&self, rva: u32) -> (u16, u32) {
        match self.section_number(rva) {
            0 => (0, rva),
            n => (n, rva - self.l.sections[usize::from(n) - 1].rva),
        }
    }

    /// The VC feature counts (`IMAGE_DEBUG_TYPE_VC_FEATURE`), as link.exe
    /// counts them: of the objects the C or C++ compiler made (by
    /// `@comp.id`'s product: VS2015's and later's, `0x104` to `0x109`),
    /// those from before Visual C++ 11, all of them, and those compiled with
    /// `/GS`, `/sdl`, and `/guard:cf` (`@feat.00`'s bits).
    fn vc_features(&self) -> [u32; 5] {
        let mut v = [0u32; 5];
        for f in &self.r.files {
            let FileKind::Object { object, .. } = &f.kind else {
                continue;
            };
            let product = object.comp_id.map_or(0, |c| c >> 16);
            if !(0x104..=0x109).contains(&product) {
                continue;
            }
            let feat = object.feat_00.unwrap_or(0);
            v[1] += 1;
            v[2] += u32::from(feat & 0x100 != 0);
            v[3] += u32::from(feat & 0x200 != 0);
            v[4] += u32::from(feat & 0x800 != 0);
        }
        v
    }
    /// Where global `g` is, relative to the image base (`None` if
    /// nowhere or absolute).
    pub fn rva_of(&self, g: u32) -> Option<u32> {
        match self.global(g) {
            Target::Rva(r) => Some(r),
            _ => None,
        }
    }

    /// Writes the whole image.
    pub fn write(&self, entry: Option<u32>) -> (Vec<u8>, Vec<String>) {
        let mut out = vec![0u8; self.l.file_size as usize];
        self.headers(&mut out, entry);

        // Each chunk's bytes, split off in file order.
        let mut parts: Vec<(u32, &mut [u8])> = Vec::new();
        let mut rest: &mut [u8] = &mut out[self.l.headers_size as usize..];
        let mut at = self.l.headers_size;
        for sec in &self.l.sections {
            if sec.raw_size == 0 {
                continue;
            }
            debug_assert_eq!(sec.file_offset, at);
            let (mine, tail) = rest.split_at_mut(sec.raw_size as usize);
            rest = tail;
            at += sec.raw_size;
            let mut mine = mine;
            let mut offset = 0u32;
            // Code's padding between chunks is int3 (0xCC), as link.exe's;
            // after the last chunk with contents (before an empty group's
            // place at the end), zeros.
            let code = sec.characteristics & scn::CNT_CODE != 0;
            for &c in &sec.chunks {
                let chunk = &self.l.chunks[c as usize];
                if chunk.uninitialized || chunk.size == 0 {
                    continue;
                }
                let start = chunk.rva - sec.rva;
                let (gap, tail) = mine.split_at_mut((start - offset) as usize);
                if code {
                    gap.fill(0xCC);
                }
                let (part, tail) = tail.split_at_mut(chunk.size as usize);
                parts.push((c, part));
                mine = tail;
                offset = start + chunk.size;
            }
        }
        let problems = std::sync::Mutex::new(Vec::new());
        turbonop_par::for_each_chunk_mut(&mut parts, 64, |part| {
            let mut mine = Vec::new();
            for (c, bytes) in part.iter_mut() {
                mine.extend(self.chunk(*c, bytes));
            }
            if !mine.is_empty() {
                problems
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                    .extend(mine);
            }
        });
        let mut problems = problems
            .into_inner()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        problems.sort();
        drop(parts);

        // `.pdata`, sorted by function (the loader searches it).
        if let Some(p) = self.l.sections.iter().find(|s| s.name == b".pdata") {
            let start = p.file_offset as usize;
            let bytes = &mut out[start..start + p.virtual_size as usize];
            let mut entries: Vec<[u8; 12]> = bytes
                .as_chunks::<12>()
                .0
                .iter()
                .copied()
                .filter(|e| e != &[0u8; 12])
                .collect();
            entries.sort_by_key(|e| u32::from_le_bytes([e[0], e[1], e[2], e[3]]));
            let n = entries.len() * 12;
            for (dst, e) in bytes.as_chunks_mut::<12>().0.iter_mut().zip(&entries) {
                *dst = *e;
            }
            bytes[n..].fill(0);
        }
        // The TLS directory's Characteristics: the TLS data's alignment (its
        // .tls groups' largest), as link.exe fills it in.
        if let Some(g) = self.r.names.get(&b"_tls_used"[..])
            && let Some(rva) = self.rva_of(*g)
        {
            let align = self
                .l
                .chunks
                .iter()
                .filter(|c| self.tls.is_some_and(|(lo, hi)| c.rva >= lo && c.rva < hi))
                .map(|c| c.align)
                .max()
                .unwrap_or(1);
            let is64 = self.r.config.machine.is_none_or(Machine::is_64);
            let field = self.file_offset(rva) as usize + if is64 { 0x24 } else { 0x14 };
            let bits = (align.trailing_zeros() + 1) << 20;
            if let Some(f) = out.get_mut(field..field + 4) {
                let old = u32::from_le_bytes([f[0], f[1], f[2], f[3]]);
                f.copy_from_slice(&((old & !0x00F0_0000) | bits).to_le_bytes());
            }
        }
        // /Brepro, as link.exe's IMAGE::Normalize does it: the SHA-256 of
        // the image with its time stamps (the file header's, each debug
        // directory entry's, the export directory's) and the PDB's GUID and
        // age set to all ones, and the repro entry's length and hash to
        // zeros; then the hash in the repro entry, its last 4 bytes as every
        // time stamp, its first 16 as the PDB's GUID (the age as it was).
        if let Some(c) = self.l.chunks.iter().find(|c| c.kind == ChunkKind::Repro) {
            use sha2::Digest;
            let pe = self.l.rich.2 as usize;
            let repro = self.file_offset(c.rva) as usize;
            let mut stamps = vec![pe + 8];
            if let Some(d) = self
                .l
                .chunks
                .iter()
                .find(|c| c.kind == ChunkKind::DebugDirectory)
            {
                let at = self.file_offset(d.rva) as usize;
                for i in 0..crate::layout::debug_entries(&self.r.config).len() {
                    stamps.push(at + i * 28 + 4);
                }
            }
            if let Some(e) = self.l.chunks.iter().find(|c| c.kind == ChunkKind::Exports) {
                stamps.push(self.file_offset(e.rva) as usize + 4);
            }
            let guid = self
                .l
                .chunks
                .iter()
                .find(|c| c.kind == ChunkKind::CodeView)
                .map(|cv| self.file_offset(cv.rva) as usize + 4);
            let mut normal = out.clone();
            for &at in &stamps {
                normal[at..at + 4].fill(0xFF);
            }
            if let Some(at) = guid {
                // The GUID and the age after it.
                normal[at..at + 20].fill(0xFF);
            }
            normal[repro..repro + 36].fill(0);
            let hash: [u8; 32] = sha2::Sha256::digest(&normal).into();
            drop(normal);
            out[repro + 4..repro + 36].copy_from_slice(&hash);
            let ts = [hash[28], hash[29], hash[30], hash[31]];
            for &at in &stamps {
                out[at..at + 4].copy_from_slice(&ts);
            }
            if let Some(at) = guid {
                out[at..at + 16].copy_from_slice(&hash[..16]);
            }
        }
        (out, problems)
    }

    fn headers(&self, out: &mut [u8], entry: Option<u32>) {
        let c: &Config = &self.r.config;
        let machine = c.machine.unwrap_or(Machine::X64);
        let is64 = machine.is_64();
        // The DOS header and its stub, the Rich header, the PE header.
        let dos = dos_header();
        out[..0x80].copy_from_slice(&dos);
        // The Rich header, then the PE header after it.
        let (entries, key, pe) = &self.l.rich;
        crate::rich::write(&mut out[0x80..], entries, *key);
        out[0x3C..0x40].copy_from_slice(&pe.to_le_bytes());
        let mut w = Writer {
            out,
            at: *pe as usize,
        };
        w.bytes(b"PE\0\0");

        // The file header.
        let sections = &self.l.sections;
        w.u16(machine.number());
        w.u16(u16::try_from(sections.len()).unwrap_or(u16::MAX));
        w.u32(timestamp(c));
        w.u32(0);
        w.u32(0);
        w.u16(if is64 { 240 } else { 224 });
        let mut chars = 0x0002; // executable
        if c.fixed {
            chars |= 0x0001; // relocations stripped
        }
        if c.large_address_aware.unwrap_or(is64) {
            chars |= 0x0020;
        }
        if !is64 {
            chars |= 0x0100; // 32-bit machine
        }
        if c.dll {
            chars |= 0x2000;
        }
        if c.swap_run_cd {
            chars |= 0x0400;
        }
        if c.swap_run_net {
            chars |= 0x0800;
        }
        w.u16(chars);

        // The optional header.
        let sum = |pred: &dyn Fn(u32) -> bool| -> u32 {
            sections
                .iter()
                .filter(|s| pred(s.characteristics))
                .map(|s| s.raw_size.max(align_up(s.virtual_size, self.l.file_align)))
                .sum()
        };
        let code = sum(&|ch| ch & scn::CNT_CODE != 0);
        let data = sum(&|ch| ch & scn::CNT_INITIALIZED_DATA != 0);
        let bss = sum(&|ch| ch & scn::CNT_UNINITIALIZED_DATA != 0);
        let base_of_code = sections
            .iter()
            .find(|s| s.characteristics & scn::CNT_CODE != 0)
            .map_or(0x1000, |s| s.rva); // (link.exe's, with no code)
        w.u16(if is64 { 0x20B } else { 0x10B });
        w.bytes(&[14, 52]); // linker version (link.exe 14.52's, whose output this is)
        w.u32(code);
        w.u32(data);
        w.u32(bss);
        w.u32(entry.and_then(|g| self.rva_of(g)).unwrap_or(0));
        w.u32(base_of_code);
        if is64 {
            w.u64(self.base);
        } else {
            let base_of_data = sections
                .iter()
                .find(|s| s.characteristics & scn::CNT_CODE == 0)
                .map_or(0x1000, |s| s.rva); // (link.exe's, with no code)
            w.u32(base_of_data);
            w.u32(field32(self.base));
        }
        w.u32(self.l.section_align);
        w.u32(self.l.file_align);
        let (os_major, os_minor) = c.os_version.unwrap_or((6, 0));
        w.u16(os_major);
        w.u16(os_minor);
        w.u16(c.version.0);
        w.u16(c.version.1);
        let (sub_major, sub_minor) = c.subsystem_version.unwrap_or((6, 0));
        w.u16(sub_major);
        w.u16(sub_minor);
        w.u32(0); // Win32VersionValue
        w.u32(self.l.image_size);
        w.u32(self.l.headers_size);
        let checksum_at = w.at;
        w.u32(0); // checksum, below
        // (Resolution infers it without /SUBSYSTEM, as link.exe does.)
        let subsystem = c.subsystem.unwrap_or(if c.dll {
            Subsystem::Windows
        } else {
            Subsystem::Console
        });
        w.u16(subsystem.number());
        let mut dll = 0u16;
        if c.dynamic_base && !c.fixed {
            dll |= 0x0040;
            if is64 && c.high_entropy_va.unwrap_or(true) {
                dll |= 0x0020;
            }
        }
        if c.integrity_check {
            dll |= 0x0080;
        }
        if c.nx_compat {
            dll |= 0x0100;
        }
        if !c.allow_isolation {
            dll |= 0x0200;
        }
        if !c.allow_bind {
            dll |= 0x0800;
        }
        if c.app_container {
            dll |= 0x1000;
        }
        if c.guard_cf {
            dll |= 0x4000;
        }
        if !c.dll && c.ts_aware.unwrap_or(true) {
            dll |= 0x8000;
        }
        w.u16(dll);
        let commit = |s: (u64, Option<u64>)| s.1.unwrap_or(0x1000);
        if is64 {
            w.u64(c.stack.0);
            w.u64(commit(c.stack));
            w.u64(c.heap.0);
            w.u64(commit(c.heap));
        } else {
            w.u32(field32(c.stack.0));
            w.u32(field32(commit(c.stack)));
            w.u32(field32(c.heap.0));
            w.u32(field32(commit(c.heap)));
        }
        w.u32(0); // loader flags
        w.u32(16);
        let mut dirs = [(0u32, 0u32); 16];
        if let Some(i) = self
            .l
            .chunks
            .iter()
            .position(|c| c.kind == ChunkKind::Exports)
        {
            // Its size to 4 bytes, as link.exe gives it, unless nothing
            // follows it in its section (then its own size).
            let e = &self.l.chunks[i];
            let last = self
                .l
                .sections
                .iter()
                .any(|s| s.chunks.last() == Some(&crate::n32(i)));
            let size = if last {
                e.size
            } else {
                crate::layout::align_up(e.size, 4)
            };
            dirs[0] = (e.rva, size);
        }
        if let Some(d) = self
            .l
            .chunks
            .iter()
            .find(|c| c.kind == ChunkKind::DebugDirectory)
        {
            dirs[6] = (d.rva, d.size);
        }
        if !self.l.dlls.is_empty() {
            let dir = self
                .l
                .chunks
                .iter()
                .find(|c| c.kind == ChunkKind::ImportDirectory);
            if let Some(d) = dir {
                // The DLLs' entries and the null one after them.
                dirs[1] = (d.rva, d.size + 20);
            }
            let iats: Vec<_> = self
                .iat
                .iter()
                .map(|&c| self.l.chunks[c as usize])
                .collect();
            if let (Some(first), Some(last)) = (
                iats.iter().map(|c| c.rva).min(),
                iats.iter().map(|c| c.rva + c.size).max(),
            ) {
                dirs[12] = (first, last - first);
            }
        }
        // The delay-load directory: the descriptors and the null one.
        if let Some(rva) = self.delay_rva(ChunkKind::DelayDirectory) {
            dirs[13] = (rva, 32 * (crate::n32(self.l.delay_dlls.len()) + 1));
        }
        if let (Some(r), Some(res)) = (
            self.l
                .chunks
                .iter()
                .find(|c| c.kind == ChunkKind::Resources),
            &self.l.resources,
        ) {
            dirs[2] = (r.rva, res.size);
        } else if let Some(r) = sections.iter().find(|s| s.name == b".rsrc") {
            // Resources from objects (cvtres's `.rsrc`$01` and ``).
            dirs[2] = (r.rva, r.virtual_size);
        }
        if let Some(p) = sections.iter().find(|s| s.name == b".pdata") {
            dirs[3] = (p.rva, p.virtual_size);
        }
        if let Some(r) = sections.iter().find(|s| s.name == b".reloc") {
            dirs[5] = (r.rva, r.virtual_size);
        }
        let find = |name: &[u8]| self.r.names.get(name).copied();
        if let Some(g) = find(b"_tls_used")
            && let Some(rva) = self.rva_of(g)
        {
            dirs[9] = (rva, if is64 { 0x28 } else { 0x18 });
        }
        if let Some(g) = find(b"_load_config_used")
            && let Some(rva) = self.rva_of(g)
        {
            let size = self
                .read_u32(rva)
                .unwrap_or(if is64 { 0x140 } else { 0xC0 });
            dirs[10] = (rva, size);
        }
        for (rva, size) in dirs {
            w.u32(rva);
            w.u32(size);
        }

        // The section headers.
        for s in sections {
            let mut name = [0u8; 8];
            let n = s.name.len().min(8);
            name[..n].copy_from_slice(&s.name[..n]);
            w.bytes(&name);
            w.u32(s.virtual_size);
            w.u32(s.rva);
            w.u32(s.raw_size);
            w.u32(s.file_offset);
            w.u32(0);
            w.u32(0);
            w.u16(0);
            w.u16(0);
            w.u32(s.characteristics);
        }
        let _ = (checksum_at, c.release, c.debug == Debug::None);
    }

    /// The `u32` at `rva` in the contents of the input sections (for
    /// `_load_config_used`'s `Size`).
    fn read_u32(&self, rva: u32) -> Option<u32> {
        for (f, chunks) in self.l.section_chunks.iter().enumerate() {
            for (s, &c) in chunks.iter().enumerate() {
                if c == u32::MAX {
                    continue;
                }
                let chunk = &self.l.chunks[c as usize];
                if rva >= chunk.rva && rva + 4 <= chunk.rva + chunk.size {
                    let FileKind::Object { object, .. } = &self.r.files[f].kind else {
                        return None;
                    };
                    let o = (rva - chunk.rva) as usize;
                    let d = object.sections[s].data.get(o..o + 4)?;
                    return Some(u32::from_le_bytes([d[0], d[1], d[2], d[3]]));
                }
            }
        }
        None
    }
}

/// The DOS header and stub link.exe writes ("This program cannot be run
/// in DOS mode."), with e_lfanew zero (it is set once the Rich header's
/// size is known; the Rich header's checksum covers these bytes so).
pub fn dos_header() -> [u8; 0x80] {
    let mut d = [0u8; 0x80];
    let head: [u16; 14] = [0x5A4D, 0x90, 3, 0, 4, 0, 0xFFFF, 0, 0xB8, 0, 0, 0, 0x40, 0];
    for (i, v) in head.iter().enumerate() {
        d[i * 2..i * 2 + 2].copy_from_slice(&v.to_le_bytes());
    }
    const STUB: [u8; 64] = [
        0x0E, 0x1F, 0xBA, 0x0E, 0x00, 0xB4, 0x09, 0xCD, 0x21, 0xB8, 0x01, 0x4C, 0xCD, 0x21, 0x54,
        0x68, 0x69, 0x73, 0x20, 0x70, 0x72, 0x6F, 0x67, 0x72, 0x61, 0x6D, 0x20, 0x63, 0x61, 0x6E,
        0x6E, 0x6F, 0x74, 0x20, 0x62, 0x65, 0x20, 0x72, 0x75, 0x6E, 0x20, 0x69, 0x6E, 0x20, 0x44,
        0x4F, 0x53, 0x20, 0x6D, 0x6F, 0x64, 0x65, 0x2E, 0x0D, 0x0D, 0x0A, 0x24, 0x00, 0x00, 0x00,
        0x00, 0x00, 0x00, 0x00,
    ];
    d[0x40..].copy_from_slice(&STUB);
    d
}

/// The image's base: `/BASE`, or the default (0x140000000 for a 64-bit
/// EXE, 0x180000000 for a 64-bit DLL; 0x400000 and 0x10000000 for 32-bit).
pub fn image_base(c: &Config) -> u64 {
    if let Some(b) = c.base {
        return b;
    }
    let is64 = c.machine.is_none_or(Machine::is_64);
    match (is64, c.dll) {
        (true, false) => 0x1_4000_0000,
        (true, true) => 0x1_8000_0000,
        (false, false) => 0x40_0000,
        (false, true) => 0x1000_0000,
    }
}

/// The image's time stamp: now, or 0 for `/Brepro`.
pub fn timestamp(c: &Config) -> u32 {
    if c.repro {
        return 0;
    }
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| u32::try_from(d.as_secs()).unwrap_or(u32::MAX))
}

/// A PE32 header's 32-bit field for a value given as 64 bits (a base,
/// stack, or heap size, which a 32-bit image can't exceed; the largest if
/// asked for more).
fn field32(v: u64) -> u32 {
    u32::try_from(v).unwrap_or(u32::MAX)
}

struct Writer<'a> {
    out: &'a mut [u8],
    at: usize,
}

impl Writer<'_> {
    fn bytes(&mut self, b: &[u8]) {
        self.out[self.at..self.at + b.len()].copy_from_slice(b);
        self.at += b.len();
    }
    fn u16(&mut self, v: u16) {
        self.bytes(&v.to_le_bytes());
    }
    fn u32(&mut self, v: u32) {
        self.bytes(&v.to_le_bytes());
    }
    fn u64(&mut self, v: u64) {
        self.bytes(&v.to_le_bytes());
    }
}

/// The `.reloc` section's contents for `sites` (sorted): a block per page.
pub fn base_relocation_blocks(sites: &[(u32, u16)]) -> Vec<u8> {
    let mut out = Vec::new();
    let mut i = 0;
    while i < sites.len() {
        let page = sites[i].0 & !0xFFF;
        let start = out.len();
        out.extend_from_slice(&page.to_le_bytes());
        out.extend_from_slice(&0u32.to_le_bytes());
        let mut n = 0;
        while i < sites.len() && sites[i].0 & !0xFFF == page {
            let (rva, ty) = sites[i];
            out.extend_from_slice(&((ty << 12) | (rva & 0xFFF) as u16).to_le_bytes());
            n += 1;
            i += 1;
        }
        if n % 2 == 1 {
            out.extend_from_slice(&0u16.to_le_bytes());
        }
        let size = crate::n32(out.len() - start);
        out[start + 4..start + 8].copy_from_slice(&size.to_le_bytes());
    }
    out
}

/// Each name's first definition, for messages.
pub type Names = Map<&'static [u8], u32>;
