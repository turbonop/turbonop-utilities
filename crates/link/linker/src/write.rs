//! Writing the image: its headers, then every chunk at once (each into
//! its own part of the output, split with `split_at_mut`, relocated in
//! place), the base relocations, and `.pdata` sorted.

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
}

impl<'a> Image<'a> {
    /// The image to write.
    pub fn new(r: &'a Resolved, l: &'a Layout) -> Image<'a> {
        let mut iat = vec![u32::MAX; l.dlls.len()];
        for (i, c) in l.chunks.iter().enumerate() {
            if let ChunkKind::ImportAddresses(d) = c.kind {
                iat[d as usize] = i as u32;
            }
        }
        Image {
            r,
            l,
            base: image_base(&r.config),
            iat,
        }
    }

    fn import_slot(&self, f: u32) -> Option<u32> {
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
                Def::ImportAddress(f) => self.import_slot(f).map_or(Target::None, Target::Rva),
                Def::ImportThunk(f) => self.l.thunk_chunks.get(&f).map_or(Target::None, |&c| {
                    Target::Rva(self.l.chunks[c as usize].rva)
                }),
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
                for rel in sec.relocations {
                    let at = rel.virtual_address.get() as usize;
                    let kind = rel.kind.get();
                    let target = self.symbol(f, rel.symbol_table_index.get());
                    if let Err(m) = self.relocate(out, at, kind, target, chunk.rva) {
                        problems.push(format!("{}: {m}", self.r.files[f as usize].name));
                    }
                }
            }
            ChunkKind::Common(_) => {}
            ChunkKind::Thunk(f) => {
                // jmp qword ptr [rip + disp32]
                out[..2].copy_from_slice(&[0xFF, 0x25]);
                let slot = self.import_slot(f).unwrap_or(0);
                let disp = i64::from(slot) - i64::from(chunk.rva + 6);
                out[2..6].copy_from_slice(&(disp as i32).to_le_bytes());
            }
            ChunkKind::ImportDirectory => {
                for (d, dll) in self.l.dlls.iter().enumerate() {
                    let lookup = self
                        .l
                        .chunks
                        .iter()
                        .find(|c| c.kind == ChunkKind::ImportLookup(d as u32))
                        .map_or(0, |c| c.rva);
                    let iat = self.l.chunks[self.iat[d] as usize].rva;
                    let names = self.names_rva();
                    let e = &mut out[d * 20..d * 20 + 20];
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
                        timestamp(&self.r.config),
                        |i| match t.targets[i] {
                            u32::MAX => 0,
                            g => self.rva_of(g).unwrap_or(0),
                        },
                        &t.forwarders,
                    );
                }
            }
            ChunkKind::BaseRelocations => {
                out[..self.l.base_relocations.len()].copy_from_slice(&self.l.base_relocations);
            }
        }
        problems
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
        let place = rva.wrapping_add(at as u32);
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
            field.copy_from_slice(&old.wrapping_add(v as u32).to_le_bytes());
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
                let n = match s {
                    Target::Rva(r) => self.section_number(r),
                    _ => 0,
                };
                field.copy_from_slice(&n.to_le_bytes());
            }
            amd64::SECREL => {
                let off = match s {
                    Target::Rva(r) => {
                        let n = self.section_number(r);
                        let start = n
                            .checked_sub(1)
                            .and_then(|i| self.l.sections.get(i as usize))
                            .map_or(0, |sec| sec.rva);
                        u64::from(r - start)
                    }
                    _ => 0,
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
            for &c in &sec.chunks {
                let chunk = &self.l.chunks[c as usize];
                if chunk.uninitialized {
                    continue;
                }
                let start = chunk.rva - sec.rva;
                let (_, tail) = mine.split_at_mut((start - offset) as usize);
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
        (out, problems)
    }

    fn headers(&self, out: &mut [u8], entry: Option<u32>) {
        let c: &Config = &self.r.config;
        let machine = c.machine.unwrap_or(Machine::X64);
        let is64 = machine.is_64();
        // The DOS header and its stub ("This program cannot be run in DOS
        // mode."), the PE header at 0x80.
        const STUB: [u8; 64] = [
            0x0E, 0x1F, 0xBA, 0x0E, 0x00, 0xB4, 0x09, 0xCD, 0x21, 0xB8, 0x01, 0x4C, 0xCD, 0x21,
            0x54, 0x68, 0x69, 0x73, 0x20, 0x70, 0x72, 0x6F, 0x67, 0x72, 0x61, 0x6D, 0x20, 0x63,
            0x61, 0x6E, 0x6E, 0x6F, 0x74, 0x20, 0x62, 0x65, 0x20, 0x72, 0x75, 0x6E, 0x20, 0x69,
            0x6E, 0x20, 0x44, 0x4F, 0x53, 0x20, 0x6D, 0x6F, 0x64, 0x65, 0x2E, 0x0D, 0x0D, 0x0A,
            0x24, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
        ];
        let mut w = Writer { out, at: 0 };
        w.bytes(b"MZ");
        w.u16(0x90); // bytes on the last page
        w.u16(3); // pages
        w.u16(0); // relocations
        w.u16(4); // header paragraphs
        w.u16(0);
        w.u16(0xFFFF);
        w.u16(0); // ss
        w.u16(0xB8); // sp
        w.u16(0); // checksum
        w.u16(0); // ip
        w.u16(0); // cs
        w.u16(0x40); // relocation table
        w.u16(0); // overlay
        w.at = 0x3C;
        w.u32(0x80);
        w.bytes(&STUB);
        debug_assert_eq!(w.at, 0x80);
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
            .map_or(0, |s| s.rva);
        w.u16(if is64 { 0x20B } else { 0x10B });
        w.bytes(&[14, 0]); // linker version
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
                .map_or(0, |s| s.rva);
            w.u32(base_of_data);
            w.u32(self.base as u32);
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
        let subsystem = c.subsystem.unwrap_or(Subsystem::Console);
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
            w.u32(c.stack.0 as u32);
            w.u32(commit(c.stack) as u32);
            w.u32(c.heap.0 as u32);
            w.u32(commit(c.heap) as u32);
        }
        w.u32(0); // loader flags
        w.u32(16);
        let mut dirs = [(0u32, 0u32); 16];
        if let Some(e) = self.l.chunks.iter().find(|c| c.kind == ChunkKind::Exports) {
            dirs[0] = (e.rva, e.size);
        }
        if !self.l.dlls.is_empty() {
            let dir = self
                .l
                .chunks
                .iter()
                .find(|c| c.kind == ChunkKind::ImportDirectory);
            if let Some(d) = dir {
                dirs[1] = (d.rva, d.size);
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
        .map_or(0, |d| d.as_secs() as u32)
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
        let size = (out.len() - start) as u32;
        out[start + 4..start + 8].copy_from_slice(&size.to_le_bytes());
    }
    out
}

/// Each name's first definition, for messages.
pub type Names = Map<&'static [u8], u32>;
