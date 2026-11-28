//! A linked program read for comparison: its image, its PDB, and the
//! labels that name its addresses.
//!
//! Every address is named by the nearest **label** at or below it, and
//! the distance from it: `(name, offset)`. Labels come from the PDB: its
//! publics, its modules' procedures and data, and its section
//! contributions (each a COFF section of one object). A contribution's
//! label is its module, characteristics, size, and CRCs, with an
//! ordinal among identical ones in its module. Where several labels
//! start at one address, the first of publics, procedures, data, and
//! contributions names it (and, among those, the least name). Names of
//! turbolib's renamed copies are taken without their prefix, so a call to
//! a renamed copy names the function it copies.

use std::borrow::Cow;

use turbonop_codeview::symbols::{Symbol, kind};
use turbonop_core::{Rva, Va};
use turbonop_hash::Map;
use turbonop_pdb::Pdb;
use turbonop_pe::PeFile;

use crate::library::member::{RENAMED, canonical};

/// Label priorities: lower names an address first.
pub(crate) mod priority {
    pub const PUBLIC: u8 = 0;
    pub const PROCEDURE: u8 = 1;
    pub const DATA: u8 = 2;
    pub const CONTRIBUTION: u8 = 3;
    pub const SECTION: u8 = 4;
}

/// A label: an address and a name.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) struct Label {
    pub rva: u32,
    pub priority: u8,
    /// The name, canonical (without turbolib's prefix).
    pub name: Box<[u8]>,
    /// Whether the name had turbolib's prefix.
    pub renamed: bool,
}

/// An image section.
#[derive(Clone, Debug)]
pub(crate) struct Section<'a> {
    pub name: Vec<u8>,
    pub rva: u32,
    pub virtual_size: u32,
    pub characteristics: u32,
    /// The bytes in the file (as many as it has, up to the virtual size).
    pub data: &'a [u8],
}

/// A section contribution.
#[derive(Clone, Debug)]
pub(crate) struct Contribution {
    pub rva: u32,
    pub size: u32,
    pub characteristics: u32,
    pub data_crc: u32,
    pub reloc_crc: u32,
    /// Its label's name (see the module's documentation).
    pub label: Box<[u8]>,
}

/// A procedure or datum a module names: (RVA, priority, name, whether it
/// is global, a procedure's length).
type Found = (u32, u8, Vec<u8>, bool, Option<u32>);

/// A procedure's code range.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) struct Code {
    pub rva: u32,
    pub len: u32,
}

/// A program, read.
pub(crate) struct Linked<'a> {
    pub pe: PeFile<'a>,
    pub pdb: Pdb<'a>,
    pub machine: u16,
    pub image_base: u64,
    pub sections: Vec<Section<'a>>,
    /// The PDB's section headers (virtual address, size, name), for
    /// `section:offset` addresses.
    pub pdb_sections: Vec<(u32, u32, [u8; 8])>,
    /// Labels, sorted by address, the best first at each.
    pub labels: Vec<Label>,
    /// Every symbol name (public, procedure, datum) at each address that
    /// has more than one, sorted.
    pub aliases: Map<u32, Vec<Box<[u8]>>>,
    pub contributions: Vec<Contribution>,
    /// Each module's name, and its object (or library) name.
    pub modules: Vec<(Vec<u8>, Vec<u8>)>,
    /// Each procedure's code, sorted.
    pub code: Vec<Code>,
    /// The base relocations: (RVA, size), sorted.
    pub relocations: Vec<(u32, u8)>,
    /// The ranges the linker makes of tables that are compared as tables,
    /// not as bytes: the export and debug directories and the debug data
    /// they point to, the delay-import descriptors, and the `/guard`
    /// tables. (Their bytes hold time stamps, PDB signatures, and lists
    /// sorted by address.)
    pub tables: Vec<(u32, u32)>,
    /// The TLS template (each thread's block is a copy): its RVA and size
    /// with the zero fill.
    pub tls: Option<(u32, u32)>,
}

impl<'a> Linked<'a> {
    /// Reads an image and its PDB, and checks that they belong together.
    pub fn read(image: &'a [u8], pdb: &'a [u8], what: &str) -> Result<Self, String> {
        let pe = PeFile::parse_image(image).map_err(|e| format!("{what}'s image: {e}"))?;
        let pdb = Pdb::parse(pdb).map_err(|e| format!("{what}'s PDB: {e}"))?;
        let machine = pe.machine();
        let image_base = pe.optional_header().map_or(0, |h| h.image_base().get());
        let sections = pe
            .sections()
            .iter()
            .map(|s| {
                let size = s.virtual_size.get().max(s.size_of_raw_data.get());
                let raw = s.size_of_raw_data.get().min(size);
                let at = s.pointer_to_raw_data.get() as usize;
                let data = image
                    .get(at..at.saturating_add(raw as usize))
                    .unwrap_or(&[]);
                Section {
                    name: pe.section_name(s).to_vec(),
                    rva: s.virtual_address.get(),
                    virtual_size: s.virtual_size.get(),
                    characteristics: s.characteristics.get(),
                    data,
                }
            })
            .collect();
        let pdb_sections = pdb.sections().map_err(|e| format!("{what}'s PDB: {e}"))?;
        let dbi = pdb.dbi().map_err(|e| format!("{what}'s PDB: {e}"))?;
        let modules: Vec<(Vec<u8>, Vec<u8>)> = dbi
            .modules()
            .iter()
            .map(|m| (m.name.to_vec(), m.object_name.to_vec()))
            .collect();
        let mut relocations: Vec<(u32, u8)> = pe
            .base_relocs()
            .filter_map(|r| match r.kind {
                turbonop_pe::reloc::base_reloc::HIGHLOW => Some((r.rva.get(), 4)),
                turbonop_pe::reloc::base_reloc::DIR64 => Some((r.rva.get(), 8)),
                _ => None,
            })
            .collect();
        relocations.sort_unstable();
        let mut p = Linked {
            pe,
            pdb,
            machine,
            image_base,
            sections,
            pdb_sections,
            labels: Vec::new(),
            aliases: Map::default(),
            contributions: Vec::new(),
            modules,
            code: Vec::new(),
            relocations,
            tables: Vec::new(),
            tls: None,
        };
        p.tables = p.table_ranges();
        p.tls = p.pe.tls().and_then(|t| {
            let start = p.rva(t.start_of_raw_data.get())?.get();
            let end = p.rva(t.end_of_raw_data.get())?.get();
            Some((start, end.checked_sub(start)?.saturating_add(t.size_of_zero_fill)))
        });
        p.read_labels()?;
        Ok(p)
    }

    /// The RVA of `segment:offset`.
    pub fn rva_of(&self, segment: u16, offset: u32) -> Option<u32> {
        if segment == 0 {
            return None;
        }
        let (va, _, _) = self.pdb_sections.get(usize::from(segment) - 1)?;
        va.checked_add(offset)
    }

    /// The section holding `rva`.
    pub fn section_of(&self, rva: u32) -> Option<&Section<'a>> {
        let i = self
            .sections
            .partition_point(|s| s.rva <= rva)
            .checked_sub(1)?;
        let s = &self.sections[i];
        (rva - s.rva < s.virtual_size.max(s.data.len() as u32)).then_some(s)
    }

    /// The bytes at `rva`, `len` of them: the file's, then zeros for the
    /// rest of the section's virtual size.
    pub fn bytes(&self, rva: u32, len: u32) -> Cow<'a, [u8]> {
        let Some(s) = self.section_of(rva) else {
            return Cow::Owned(vec![0; len as usize]);
        };
        let at = (rva - s.rva) as usize;
        let end = at + len as usize;
        if end <= s.data.len() {
            Cow::Borrowed(&s.data[at..end])
        } else {
            let mut v = s.data.get(at..).unwrap_or(&[]).to_vec();
            v.resize(len as usize, 0);
            Cow::Owned(v)
        }
    }

    /// The name of `rva`: its label's name and the distance from it, or
    /// `None` if it is below every label.
    pub fn name(&self, rva: u32) -> Option<(&[u8], u32)> {
        let i = self
            .labels
            .partition_point(|l| l.rva <= rva)
            .checked_sub(1)?;
        let l = &self.labels[i];
        Some((&l.name, rva - l.rva))
    }

    /// The name of a virtual address (see [`name`](Self::name)), if it is
    /// in the image.
    pub fn name_va(&self, va: u64) -> Option<(&[u8], u32)> {
        let rva = Va(va).to_rva(Va(self.image_base))?;
        self.section_of(rva.get())?;
        self.name(rva.get())
    }

    /// The label exactly at `rva`, if one starts there.
    pub fn label_at(&self, rva: u32) -> Option<&Label> {
        let i = self.labels.partition_point(|l| l.rva < rva);
        self.labels.get(i).filter(|l| l.rva == rva)
    }

    /// Gives the contribution label starting at `rva` another name (no
    /// other label names that address).
    pub fn rename(&mut self, rva: u32, name: Box<[u8]>) {
        let i = self.labels.partition_point(|l| l.rva < rva);
        if let Some(l) = self.labels.get_mut(i)
            && l.rva == rva
            && l.priority == priority::CONTRIBUTION
        {
            l.name = name;
        }
    }

    /// Whether `rva` is in a procedure's code.
    pub fn in_code(&self, rva: u32) -> bool {
        let i = self.code.partition_point(|c| c.rva <= rva);
        i.checked_sub(1)
            .is_some_and(|i| rva - self.code[i].rva < self.code[i].len)
    }

    /// The base relocations in `[rva, rva + len)`.
    pub fn relocations_in(&self, rva: u32, len: u32) -> &[(u32, u8)] {
        let from = self.relocations.partition_point(|r| r.0 < rva);
        let to = self
            .relocations
            .partition_point(|r| r.0 < rva.saturating_add(len));
        &self.relocations[from..to]
    }

    /// The name of a thread-local variable by its offset in the TLS
    /// template (as code addresses it: `SECREL`, from the start of the
    /// thread's block), if it is in it.
    pub fn tls_at(&self, offset: i64) -> Option<(&[u8], u32)> {
        let (start, size) = self.tls?;
        let offset = u32::try_from(offset).ok().filter(|&o| o < size)?;
        Linked::name(self, start + offset)
    }

    pub fn rva(&self, va: u64) -> Option<Rva> {
        Va(va).to_rva(Va(self.image_base))
    }

    /// Whether `[rva, rva + len)` overlaps one of [`tables`](Self::tables).
    pub fn in_tables(&self, rva: u32, len: u32) -> bool {
        let end = rva.saturating_add(len.max(1));
        self.tables.iter().any(|&(s, e)| rva < e && s < end)
    }

    fn table_ranges(&self) -> Vec<(u32, u32)> {
        use turbonop_pe::consts::directory;
        use turbonop_pe::load_config::field;
        let mut out = Vec::new();
        for d in [directory::EXPORT, directory::DEBUG, directory::DELAY_IMPORT] {
            if let Some(dd) = self.pe.data_directory(d)
                && dd.size.get() > 0
            {
                let s = dd.virtual_address.get();
                out.push((s, s.saturating_add(dd.size.get())));
            }
        }
        if let Ok(entries) = self.pe.debug_directories() {
            for e in entries {
                let s = e.address_of_raw_data.get();
                if s != 0 {
                    out.push((s, s.saturating_add(e.size_of_data.get())));
                }
            }
        }
        if let Some(c) = self.pe.load_config() {
            let stride = 4 + c.guard_table_stride() as u64;
            for (table, count) in [
                (
                    field::GUARD_CF_FUNCTION_TABLE,
                    field::GUARD_CF_FUNCTION_COUNT,
                ),
                (
                    field::GUARD_ADDRESS_TAKEN_IAT_ENTRY_TABLE,
                    field::GUARD_ADDRESS_TAKEN_IAT_ENTRY_COUNT,
                ),
                (
                    field::GUARD_LONG_JUMP_TARGET_TABLE,
                    field::GUARD_LONG_JUMP_TARGET_COUNT,
                ),
                (
                    field::GUARD_EH_CONTINUATION_TABLE,
                    field::GUARD_EH_CONTINUATION_COUNT,
                ),
            ] {
                let (Some(va), Some(n)) = (c.get(table), c.get(count)) else {
                    continue;
                };
                if let Some(rva) = self.rva(va).filter(|_| va != 0 && n != 0) {
                    let len = u32::try_from(n.saturating_mul(stride)).unwrap_or(u32::MAX);
                    out.push((rva.get(), rva.get().saturating_add(len)));
                }
            }
        }
        out
    }

    fn read_labels(&mut self) -> Result<(), String> {
        let mut labels: Vec<Label> = Vec::new();
        let push = |labels: &mut Vec<Label>, rva: u32, priority: u8, name: &[u8]| {
            // (Some data symbols have no name.)
            if name.is_empty() {
                return;
            }
            labels.push(Label {
                rva,
                priority,
                name: canonical(name).into(),
                renamed: name.starts_with(RENAMED),
            });
        };
        // Publics.
        let records = self.pdb.symbol_records();
        if let Ok(publics) = self.pdb.publics() {
            for off in publics.address_map() {
                let Some(r) = turbonop_pdb::gsi::symbol_at(records, off.get()) else {
                    continue;
                };
                if let Ok(Symbol::Public {
                    segment,
                    offset,
                    name,
                    ..
                }) = r.parse()
                    && let Some(rva) = self.rva_of(segment, offset)
                {
                    push(&mut labels, rva, priority::PUBLIC, name);
                }
            }
        }
        // Procedures and data, from every module, in parallel.
        let dbi = self.pdb.dbi().map_err(|e| e.to_string())?;
        let modules: Vec<turbonop_pdb::Module<'_>> = dbi.modules().to_vec();
        let pdb = &self.pdb;
        let this = &*self;
        let tls = self.tls;
        let found = turbonop_par::map_balanced(&modules, 1, |m| {
            let mut out: Vec<Found> = Vec::new();
            let Ok(Some(stream)) = pdb.module_stream(m) else {
                return out;
            };
            for r in stream.symbols() {
                match r.kind {
                    kind::S_GPROC32 | kind::S_LPROC32 | kind::S_GPROC32_ID | kind::S_LPROC32_ID => {
                        if let Ok(Symbol::Procedure {
                            segment,
                            offset,
                            name,
                            len,
                            global,
                            ..
                        }) = r.parse()
                            && let Some(rva) = this.rva_of(segment, offset)
                        {
                            out.push((rva, priority::PROCEDURE, name.to_vec(), global, Some(len)));
                        }
                    }
                    // (Thread-local data too: its address is in `.tls`, the
                    // template each thread's copy is made from.)
                    kind::S_GDATA32 | kind::S_LDATA32 | kind::S_GTHREAD32 | kind::S_LTHREAD32 => {
                        if let Ok(Symbol::Data {
                            segment,
                            offset,
                            name,
                            global,
                            thread,
                            ..
                        }) = r.parse()
                            && let Some(rva) = if thread {
                                // (A thread-local's offset is from the
                                // template's start, as code's SECREL.)
                                tls.and_then(|(start, _)| start.checked_add(offset))
                            } else {
                                this.rva_of(segment, offset)
                            }
                        {
                            out.push((rva, priority::DATA, name.to_vec(), global, None));
                        }
                    }
                    _ => {}
                }
            }
            out
        });
        // A name that labels more than one address is qualified with its
        // module.
        let mut addresses: Map<&[u8], (u32, bool)> = Map::default();
        for list in &found {
            for (rva, _, name, _, _) in list {
                let e = addresses.entry(name.as_slice()).or_insert((*rva, false));
                if e.0 != *rva {
                    e.1 = true;
                }
            }
        }
        let mut code = Vec::new();
        for (m, list) in found.iter().enumerate() {
            for (rva, p, name, _, len) in list {
                if name.is_empty() {
                    continue;
                }
                if let Some(len) = len {
                    code.push(Code {
                        rva: *rva,
                        len: *len,
                    });
                }
                if addresses.get(name.as_slice()).is_some_and(|a| a.1) {
                    let q = [name.as_slice(), b" (", &self.modules[m].0, b")"].concat();
                    push(&mut labels, *rva, *p, &q);
                } else {
                    push(&mut labels, *rva, *p, name);
                }
            }
        }
        code.sort_unstable();
        code.dedup();
        self.code = code;
        // Contributions.
        let contribs = dbi.section_contribs().map_err(|e| e.to_string())?;
        let mut contributions: Vec<Contribution> = Vec::new();
        let mut ordinals: Map<(usize, u32, u32, u32, u32), u32> = Map::default();
        let mut sorted: Vec<_> = contribs
            .iter()
            .filter_map(|c| {
                let rva = self.rva_of(c.section.get(), u32::try_from(c.offset.get()).ok()?)?;
                Some((rva, c))
            })
            .collect();
        sorted.sort_by_key(|(rva, _)| *rva);
        for (rva, c) in sorted {
            let module = usize::from(c.module_index.get());
            let size = u32::try_from(c.size.get()).unwrap_or(0);
            let key = (
                module,
                c.characteristics.get(),
                size,
                c.data_crc.get(),
                c.reloc_crc.get(),
            );
            let n = ordinals.entry(key).or_insert(0);
            let module_name = self
                .modules
                .get(module)
                .map_or(&b"?"[..], |m| m.0.as_slice());
            let label = format!(
                "[{} {:x} {:x} {:x} {:x} #{}]",
                String::from_utf8_lossy(module_name),
                key.1,
                key.2,
                key.3,
                key.4,
                n
            );
            *n += 1;
            push(&mut labels, rva, priority::CONTRIBUTION, label.as_bytes());
            contributions.push(Contribution {
                rva,
                size,
                characteristics: key.1,
                data_crc: key.3,
                reloc_crc: key.4,
                label: label.into_bytes().into(),
            });
        }
        // Sections.
        for s in &self.sections {
            push(&mut labels, s.rva, priority::SECTION, &s.name);
        }
        labels.sort_unstable();
        let mut aliases: Map<u32, Vec<Box<[u8]>>> = Map::default();
        for w in labels.chunk_by(|x, y| x.rva == y.rva) {
            let mut names: Vec<Box<[u8]>> = w
                .iter()
                .filter(|l| l.priority < priority::CONTRIBUTION)
                .map(|l| l.name.clone())
                .collect();
            names.sort_unstable();
            names.dedup();
            if names.len() > 1 {
                aliases.insert(w[0].rva, names);
            }
        }
        self.aliases = aliases;
        labels.dedup_by(|b, a| a.rva == b.rva);
        self.labels = labels;
        self.contributions = contributions;
        Ok(())
    }
}

impl super::bytes::Names for Linked<'_> {
    fn name(&self, rva: u32) -> Option<(&[u8], u32)> {
        self.section_of(rva)?;
        Linked::name(self, rva)
    }

    fn in_code(&self, rva: u32) -> bool {
        Linked::in_code(self, rva)
    }

    fn image_base(&self) -> u64 {
        self.image_base
    }

    fn relocations_in(&self, rva: u32, len: u32) -> &[(u32, u8)] {
        Linked::relocations_in(self, rva, len)
    }

    fn tls(&self, offset: i64) -> Option<(&[u8], u32)> {
        self.tls_at(offset)
    }

    fn procedure_end(&self, rva: u32) -> Option<u32> {
        let i = self.code.partition_point(|c| c.rva <= rva).checked_sub(1)?;
        let c = self.code[i];
        (rva - c.rva < c.len).then_some(c.rva + c.len)
    }

    fn tls_in_section(&self) -> bool {
        true
    }

    fn aliases(&self, rva: u32) -> &[Box<[u8]>] {
        let i = self.labels.partition_point(|l| l.rva <= rva);
        i.checked_sub(1)
            .and_then(|i| self.aliases.get(&self.labels[i].rva))
            .map_or(&[], Vec::as_slice)
    }
}
