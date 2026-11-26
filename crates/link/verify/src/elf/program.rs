//! One linked ELF program, read for comparison: its sections, its symbols
//! as labels, its dynamic relocations, and the names of its addresses.
//!
//! Every address is named by a label at or below it and the distance from
//! it. The labels are, in order of preference at one address:
//!
//! 1. global and weak functions and objects of `.symtab` with a size;
//! 2. local ones with a size, by their names where no other symbol has
//!    them or they have a visibility (hidden globals the linker made
//!    local), else qualified by the `STT_FILE` symbol before them
//!    (`name@file`) and, where that repeats, an ordinal (`#2`);
//! 3. other global, then local, symbols (no size);
//! 4. the linker's PLT entries and GOT slots, by what their dynamic
//!    relocation names (`plt:name`, `got:name`, `got:&label`);
//! 5. sections' starts (`section:.name`).
//!
//! An address in no symbol's extent but in a section that isn't code is
//! named by its contents instead (up to 32 bytes, or a NUL): string
//! literals and constants have no symbols, and where the linker puts them
//! (merged strings, other input orders) follows the link, not the program.

use std::collections::BTreeMap;

use turbonop_elf::consts::*;
use turbonop_elf::{ElfFile, Section, Symbol};

/// The offset a content name is given, apart from any label's.
pub(crate) const CONTENT: u32 = 0x8000_0000;

const STT_GNU_IFUNC: u8 = 10;

/// How many bytes a content name holds at most (a string to its NUL:
/// strings that share a long prefix, as `__PRETTY_FUNCTION__`s do, must
/// get names of their own).
const CONTENT_BYTES: usize = 4096;

/// A label of an address.
#[derive(Clone, Debug)]
pub(crate) struct Label {
    pub addr: u64,
    pub size: u64,
    pub name: Box<[u8]>,
    /// Lower is preferred.
    pub rank: u8,
}

/// A dynamic relocation.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct DynReloc {
    pub place: u64,
    pub kind: u32,
    /// The bytes it patches (0 for `COPY` and the like).
    pub size: u8,
    /// The symbol it names (empty for none).
    pub symbol: Box<[u8]>,
    pub addend: i64,
    /// A `RELR` place (its value is in the place).
    pub relr: bool,
}

/// A sized symbol: a function or a datum whose bytes are compared.
#[derive(Clone, Debug)]
pub(crate) struct Region {
    pub key: Box<[u8]>,
    pub addr: u64,
    pub size: u64,
    pub code: bool,
    /// In a `NOBITS` section.
    pub nobits: bool,
}

pub(crate) struct Program<'a> {
    pub file: ElfFile<'a>,
    pub sections: Vec<Section<'a>>,
    /// `.symtab`'s symbols, and each one's key (qualified name; `None`
    /// for section and file symbols and the null symbol).
    pub symbols: Vec<Symbol<'a>>,
    pub keys: Vec<Option<Box<[u8]>>>,
    labels: Vec<Label>,
    /// Indexes of the preferred label at each distinct address, by address.
    primary: Vec<usize>,
    /// Every label's name at each address.
    aliases: BTreeMap<u64, Vec<Box<[u8]>>>,
    /// Functions' extents, by start.
    code: Vec<(u64, u64)>,
    /// Dynamic relocations, by place.
    pub dynrel: Vec<DynReloc>,
    pub regions: Vec<Region>,
    /// Allocated sections' extents (start, end, index), by start.
    extents: Vec<(u64, u64, usize)>,
    /// TLS symbols by their offset in the TLS block, and the block's
    /// size rounded up to its alignment (the thread pointer's distance
    /// from its start, x86-64's variant II).
    tls: Vec<(u64, Box<[u8]>)>,
    tls_size: u64,
    tls_aligned: u64,
}

/// The size of the place a dynamic relocation patches.
fn reloc_size(machine: u16, kind: u32, word: u8) -> u8 {
    match (machine, kind) {
        // R_X86_64_NONE, _COPY.
        (62, 0 | 5) => 0,
        // _PC32, _32, _32S, _DTPOFF32, _TPOFF32, _GOTPC32.
        (62, 2 | 10 | 11 | 21 | 23 | 26) => 4,
        (62, _) => 8,
        // R_AARCH64_NONE, _COPY.
        (183, 0 | 1024) => 0,
        // R_386_NONE, _COPY.
        (3, 0 | 5) => 0,
        _ => word,
    }
}

/// Whether a relocation's value is its addend alone (relative ones).
pub(crate) fn is_relative(machine: u16, kind: u32) -> bool {
    matches!(
        (machine, kind),
        (62, 8 | 37) | (183, 1027 | 1032) | (3, 8 | 42) | (40, 23 | 160)
    )
}

impl<'a> Program<'a> {
    pub fn read(data: &'a [u8], what: &str) -> Result<Self, String> {
        let file = ElfFile::parse(data).map_err(|e| format!("{what}: {e}"))?;
        let sections: Vec<Section<'a>> = file.sections().collect();
        let symtab = sections.iter().find(|s| s.kind == SHT_SYMTAB);
        let symbols = symtab.map(|t| file.symbols(t)).unwrap_or_default();
        if sections
            .iter()
            .any(|s| s.is_alloc() && s.addr + s.size > u64::from(u32::MAX))
        {
            return Err(format!("{what}: addresses past 4 GiB aren't compared"));
        }
        let mut p = Program {
            file,
            sections,
            keys: Vec::new(),
            symbols,
            labels: Vec::new(),
            primary: Vec::new(),
            aliases: BTreeMap::new(),
            code: Vec::new(),
            dynrel: Vec::new(),
            regions: Vec::new(),
            extents: Vec::new(),
            tls: Vec::new(),
            tls_size: 0,
            tls_aligned: 0,
        };
        if let Some(t) = p.file.segments().find(|s| s.kind == PT_TLS) {
            p.tls_size = t.memsz;
            p.tls_aligned = t.memsz.next_multiple_of(t.align.max(1));
        }
        p.extents = p
            .sections
            .iter()
            .enumerate()
            .filter(|(_, s)| s.is_alloc() && s.size > 0 && s.flags & SHF_TLS == 0)
            .map(|(i, s)| (s.addr, s.addr + s.size, i))
            .collect();
        p.extents.sort_unstable();
        p.read_symbols();
        p.index();
        p.read_dynamic_relocations();
        p.label_linker_tables();
        p.index();
        Ok(p)
    }

    pub fn machine(&self) -> u16 {
        self.file.header().machine
    }

    pub fn word(&self) -> u8 {
        match self.file.class() {
            turbonop_elf::Class::Elf32 => 4,
            turbonop_elf::Class::Elf64 => 8,
        }
    }

    fn read_symbols(&mut self) {
        let mut file: &[u8] = b"";
        let mut seen: BTreeMap<(Vec<u8>, Vec<u8>), u32> = BTreeMap::new();
        // How many symbols have each name (a name only one has needs no
        // file: a synthesized member's locals have none of their own, and
        // the linker puts them after another object's file symbol).
        let mut uses: BTreeMap<&[u8], u32> = BTreeMap::new();
        for (i, s) in self.symbols.iter().enumerate() {
            if i != 0 && s.kind != STT_SECTION && s.kind != STT_FILE {
                *uses.entry(s.name).or_default() += 1;
            }
        }
        let mut keys = Vec::with_capacity(self.symbols.len());
        for (i, s) in self.symbols.iter().enumerate() {
            if i == 0 || s.kind == STT_SECTION {
                keys.push(None);
                continue;
            }
            if s.kind == STT_FILE {
                file = s.name;
                keys.push(None);
                continue;
            }
            // A local with a visibility was a global of its object (the
            // linker made it local): its name is the program's alone.
            let alone = s.visibility != STV_DEFAULT || uses.get(s.name) == Some(&1);
            let key: Box<[u8]> = if s.bind == STB_LOCAL && !alone {
                let n = seen.entry((file.to_vec(), s.name.to_vec())).or_insert(0);
                *n += 1;
                let mut k = s.name.to_vec();
                k.push(b'@');
                k.extend_from_slice(file);
                if *n > 1 {
                    k.extend_from_slice(format!("#{n}").as_bytes());
                }
                k.into()
            } else if s.bind == STB_LOCAL {
                let n = seen.entry((Vec::new(), s.name.to_vec())).or_insert(0);
                *n += 1;
                let mut k = s.name.to_vec();
                if *n > 1 {
                    k.extend_from_slice(format!("#{n}").as_bytes());
                }
                k.into()
            } else {
                s.name.into()
            };
            keys.push(Some(key));
        }
        for (s, key) in self.symbols.iter().zip(&keys) {
            let Some(key) = key else { continue };
            let defined = s.shndx != 0 && s.shndx < u32::from(SHN_LORESERVE);
            let Some(sec) = defined
                .then(|| self.sections.get(s.shndx as usize))
                .flatten()
            else {
                continue;
            };
            if s.kind == STT_TLS || sec.flags & SHF_TLS != 0 {
                self.tls.push((s.value, key.clone()));
                continue;
            }
            if !sec.is_alloc() {
                continue;
            }
            let global = s.bind != STB_LOCAL;
            let sized = s.size > 0 && matches!(s.kind, STT_FUNC | STT_OBJECT | STT_GNU_IFUNC);
            let rank = match (sized, global) {
                (true, true) => 0,
                (true, false) => 1,
                (false, true) => 2,
                (false, false) => 3,
            };
            self.labels.push(Label {
                addr: s.value,
                size: s.size,
                name: key.clone(),
                rank,
            });
            if s.size > 0 && !is_linker_table(sec.name) {
                let code = sec.flags & SHF_EXECINSTR != 0;
                if code && matches!(s.kind, STT_FUNC | STT_GNU_IFUNC) {
                    self.code.push((s.value, s.value + s.size));
                }
                self.regions.push(Region {
                    key: key.clone(),
                    addr: s.value,
                    size: s.size,
                    code,
                    nobits: sec.kind == SHT_NOBITS,
                });
            }
        }
        for (i, s) in self.sections.iter().enumerate() {
            if s.is_alloc() && s.flags & SHF_TLS == 0 && i != 0 {
                let mut n = b"section:".to_vec();
                n.extend_from_slice(s.name);
                self.labels.push(Label {
                    addr: s.addr,
                    size: s.size,
                    name: n.into(),
                    rank: 5,
                });
            }
        }
        self.keys = keys;
        self.code.sort_unstable();
        self.tls.sort();
        // One region per extent: the preferred name at it.
        self.regions.sort_by(|a, b| {
            (a.addr, a.size)
                .cmp(&(b.addr, b.size))
                .then_with(|| a.key.cmp(&b.key))
        });
        let ranks: BTreeMap<&[u8], u8> = self.labels.iter().map(|l| (&*l.name, l.rank)).collect();
        let mut out: Vec<Region> = Vec::with_capacity(self.regions.len());
        for r in std::mem::take(&mut self.regions) {
            match out.last_mut() {
                Some(last) if last.addr == r.addr && last.size == r.size => {
                    let better = (ranks.get(&*r.key), &r.key) < (ranks.get(&*last.key), &last.key);
                    if better {
                        *last = r;
                    }
                }
                _ => out.push(r),
            }
        }
        self.regions = out;
    }

    /// Sorts the labels and finds each address's preferred one.
    fn index(&mut self) {
        self.labels
            .sort_by(|a, b| (a.addr, a.rank, &a.name).cmp(&(b.addr, b.rank, &b.name)));
        self.labels
            .dedup_by(|b, a| a.addr == b.addr && a.name == b.name);
        self.primary.clear();
        self.aliases.clear();
        for (i, l) in self.labels.iter().enumerate() {
            if self
                .primary
                .last()
                .is_none_or(|&p| self.labels[p].addr != l.addr)
            {
                self.primary.push(i);
            }
            if l.rank <= 3 {
                self.aliases.entry(l.addr).or_default().push(l.name.clone());
            }
        }
    }

    fn read_dynamic_relocations(&mut self) {
        let machine = self.machine();
        let word = self.word();
        let mut out = Vec::new();
        for s in &self.sections {
            if !s.is_alloc() {
                continue;
            }
            match s.kind {
                SHT_REL | SHT_RELA | SHT_ANDROID_REL | SHT_ANDROID_RELA => {
                    let syms = self
                        .sections
                        .get(s.link as usize)
                        .filter(|t| t.kind == SHT_DYNSYM || t.kind == SHT_SYMTAB)
                        .map(|t| self.file.symbols(t))
                        .unwrap_or_default();
                    for r in self.file.relocations(s) {
                        let symbol: Box<[u8]> = syms
                            .get(r.symbol as usize)
                            .filter(|_| r.symbol != 0)
                            .map_or_else(|| Box::from(&b""[..]), |x| x.name.into());
                        let size = reloc_size(machine, r.kind, word);
                        let addend = match r.addend {
                            Some(a) => a,
                            None => self.read_word(r.offset, size).unwrap_or(0) as i64,
                        };
                        out.push(DynReloc {
                            place: r.offset,
                            kind: r.kind,
                            size,
                            symbol,
                            addend,
                            relr: false,
                        });
                    }
                }
                SHT_RELR | SHT_ANDROID_RELR => {
                    for place in self.file.relr(s) {
                        let addend = self.read_word(place, word).unwrap_or(0) as i64;
                        out.push(DynReloc {
                            place,
                            kind: u32::MAX,
                            size: word,
                            symbol: Box::from(&b""[..]),
                            addend,
                            relr: true,
                        });
                    }
                }
                _ => {}
            }
        }
        out.sort_by_key(|r| r.place);
        self.dynrel = out;
    }

    /// The `size`-byte little- or big-endian word at address `addr`.
    pub fn read_word(&self, addr: u64, size: u8) -> Option<u64> {
        let b = self.bytes(addr, u64::from(size))?;
        let mut v = 0u64;
        match self.file.endian() {
            turbonop_core::Endian::Little => {
                for &x in b.iter().rev() {
                    v = (v << 8) | u64::from(x);
                }
            }
            turbonop_core::Endian::Big => {
                for &x in b {
                    v = (v << 8) | u64::from(x);
                }
            }
        }
        Some(v)
    }

    /// The bytes of `[addr, addr + len)`, if one section with contents
    /// holds them.
    pub fn bytes(&self, addr: u64, len: u64) -> Option<&'a [u8]> {
        let (_, end, i) = *self.extent(addr)?;
        let s = &self.sections[i];
        if addr + len > end || s.kind == SHT_NOBITS {
            return None;
        }
        let data = self.file.section_data(s);
        let at = usize::try_from(addr - s.addr).ok()?;
        data.get(at..at + usize::try_from(len).ok()?)
    }

    fn extent(&self, addr: u64) -> Option<&(u64, u64, usize)> {
        let k = self.extents.partition_point(|e| e.0 <= addr);
        let e = self.extents.get(k.checked_sub(1)?)?;
        (addr < e.1).then_some(e)
    }

    /// The section holding `addr`.
    pub fn section_at(&self, addr: u64) -> Option<&Section<'a>> {
        self.extent(addr).map(|&(_, _, i)| &self.sections[i])
    }

    /// A dynamic relocation's target as text: the symbol and addend, or
    /// (relative ones) the name of the addend.
    pub fn target(&self, r: &DynReloc) -> String {
        if r.relr || (r.symbol.is_empty() && is_relative(self.machine(), r.kind)) {
            return format!("&{}", self.render(r.addend as u64));
        }
        if r.symbol.is_empty() && self.machine() == 62 && matches!(r.kind, 16..=18) {
            return format!("tls:{}", self.render_tls(r.addend as u64));
        }
        if r.symbol.is_empty() {
            return format!("{:#x}", r.addend);
        }
        if r.addend == 0 {
            crate::report::show(&r.symbol)
        } else {
            format!("{}{:+#x}", crate::report::show(&r.symbol), r.addend)
        }
    }

    /// The labels of PLT entries and GOT slots.
    fn label_linker_tables(&mut self) {
        let mut add = Vec::new();
        let mut got: BTreeMap<u64, Box<[u8]>> = BTreeMap::new();
        for r in &self.dynrel {
            let name: Box<[u8]> = if r.symbol.is_empty() {
                let mut n = b"got:".to_vec();
                n.extend_from_slice(self.target(r).as_bytes());
                n.into()
            } else {
                let mut n = b"got:".to_vec();
                n.extend_from_slice(&r.symbol);
                n.into()
            };
            got.entry(r.place).or_insert(name);
        }
        // GOT slots of no dynamic relocation: by the label of their value.
        for s in &self.sections {
            if !(s.name == b".got" || s.name == b".got.plt") || s.kind == SHT_NOBITS {
                continue;
            }
            let w = u64::from(self.word());
            let mut at = s.addr;
            while at + w <= s.addr + s.size {
                if let std::collections::btree_map::Entry::Vacant(e) = got.entry(at) {
                    let v = self.read_word(at, self.word()).unwrap_or(0);
                    let mut n = b"got:=".to_vec();
                    n.extend_from_slice(self.render(v).as_bytes());
                    e.insert(n.into());
                }
                at += w;
            }
        }
        for (&addr, name) in &got {
            if self.section_at(addr).is_some() {
                add.push(Label {
                    addr,
                    size: u64::from(self.word()),
                    name: name.clone(),
                    rank: 4,
                });
            }
        }
        // PLT entries: each 16-byte entry by the GOT slot its first
        // RIP-relative operand reads (x86-64).
        if self.machine() == 62 {
            for s in &self.sections {
                if !s.name.starts_with(b".plt") || s.flags & SHF_EXECINSTR == 0 {
                    continue;
                }
                let data = self.file.section_data(s);
                let mut at = 0usize;
                while at + 16 <= data.len() {
                    if let Some(slot) = plt_slot(&data[at..at + 16], s.addr + at as u64)
                        && let Some(g) = got.get(&slot)
                    {
                        let mut n = b"plt:".to_vec();
                        n.extend_from_slice(&g[4..]);
                        add.push(Label {
                            addr: s.addr + at as u64,
                            size: 16,
                            name: n.into(),
                            rank: 4,
                        });
                    }
                    at += 16;
                }
            }
        }
        self.labels.extend(add);
    }

    /// The name of `addr` (a label and the distance from it).
    pub fn name(&self, addr: u64) -> Option<(&[u8], u32)> {
        let k = self
            .primary
            .partition_point(|&i| self.labels[i].addr <= addr);
        let mut i = self.primary.get(k.checked_sub(1)?).copied()?;
        // A sized label that doesn't reach `addr`, below one that does.
        let l = &self.labels[i];
        if l.size > 0 && addr >= l.addr + l.size {
            // Among the labels at that address, a larger one may cover it.
            let at = l.addr;
            if let Some(j) = (i..self.labels.len())
                .take_while(|&j| self.labels[j].addr == at)
                .find(|&j| addr < at + self.labels[j].size)
            {
                i = j;
            } else if let Some(c) = self.content_name(addr) {
                return Some(c);
            }
        }
        let l = &self.labels[i];
        // Only a section's start below: what lies in the section follows
        // the link's layout (the input order, folded copies taken out), so
        // name it by its contents where they can be read.
        if l.rank == 5
            && addr != l.addr
            && let Some(c) = self.content_name(addr)
        {
            return Some(c);
        }
        let off = u32::try_from(addr - l.addr).ok()?;
        if off >= CONTENT {
            return None;
        }
        Some((&l.name, off))
    }

    /// A name by contents for an address outside every symbol, in a
    /// section with contents that isn't code.
    fn content_name(&self, addr: u64) -> Option<(&'a [u8], u32)> {
        let s = self.section_at(addr)?;
        if s.flags & SHF_EXECINSTR != 0 || s.kind == SHT_NOBITS {
            return None;
        }
        let data = self.file.section_data(s);
        let at = usize::try_from(addr - s.addr).ok()?;
        let rest = data.get(at..)?;
        let n = rest.len().min(CONTENT_BYTES);
        let n = rest[..n].iter().position(|&b| b == 0).map_or(n, |z| z + 1);
        Some((&rest[..n], CONTENT))
    }

    /// The targets of the jump table at `addr`, if one is there: an
    /// address that only its contents name, in a section that isn't code,
    /// holding 32-bit entries relative to it that land in code (x86-64's,
    /// as GCC and clang write them), two or more, up to the first that
    /// doesn't, lands in another function, or is at the next label.
    pub fn jump_table(&self, addr: u64) -> Option<Vec<u64>> {
        if self.machine() != 62 || !matches!(self.name(addr), Some((_, CONTENT))) {
            return None;
        }
        let s = self.section_at(addr)?;
        let data = self.file.section_data(s);
        let mut at = usize::try_from(addr - s.addr).ok()?;
        let next = self.label_after(addr);
        let mut out = Vec::new();
        let mut function: Option<&[u8]> = None;
        while let Some(w) = data.get(at..at + 4) {
            let here = s.addr + at as u64;
            if out.len() >= 1 << 16 || next.is_some_and(|n| here >= n) {
                break;
            }
            let e = i32::from_le_bytes([w[0], w[1], w[2], w[3]]);
            let t = addr.wrapping_add(i64::from(e) as u64);
            if !self
                .section_at(t)
                .is_some_and(|x| x.flags & SHF_EXECINSTR != 0)
            {
                break;
            }
            // Every entry lands in one function: the table ends where
            // another function's (an adjacent table's) begins.
            let Some((name, _)) = self.name(t) else {
                break;
            };
            if function.is_some_and(|f| f != name) {
                break;
            }
            function = Some(name);
            out.push(t);
            at += 4;
        }
        (out.len() >= 2).then_some(out)
    }

    /// The address of the first label after `addr`.
    fn label_after(&self, addr: u64) -> Option<u64> {
        let k = self
            .primary
            .partition_point(|&i| self.labels[i].addr <= addr);
        self.primary.get(k).map(|&i| self.labels[i].addr)
    }

    /// `addr`'s name as text.
    pub fn render(&self, addr: u64) -> String {
        match self.name(addr) {
            Some((n, CONTENT)) => format!("[{}]", crate::report::show(n)),
            Some((n, 0)) => crate::report::show(n),
            Some((n, o)) => format!("{}+{o:#x}", crate::report::show(n)),
            None => format!("{addr:#x}"),
        }
    }

    /// The TLS symbol at `offset` of the TLS block, and the distance.
    pub fn tls_at(&self, offset: u64) -> Option<(&[u8], u32)> {
        if offset >= self.tls_size {
            return None;
        }
        let k = self.tls.partition_point(|t| t.0 <= offset);
        let (at, name) = self.tls.get(k.checked_sub(1)?)?;
        Some((name, u32::try_from(offset - at).ok()?))
    }

    pub fn render_tls(&self, offset: u64) -> String {
        match self.tls_at(offset) {
            Some((n, 0)) => crate::report::show(n),
            Some((n, o)) => format!("{}+{o:#x}", crate::report::show(n)),
            None => format!("{offset:#x}"),
        }
    }

    pub fn in_code(&self, addr: u64) -> bool {
        let k = self.code.partition_point(|c| c.0 <= addr);
        k > 0 && addr < self.code[k - 1].1
    }

    pub fn aliases_at(&self, addr: u64) -> &[Box<[u8]>] {
        self.aliases.get(&addr).map_or(&[], Vec::as_slice)
    }

    /// The start of the label `addr` is named by.
    pub fn label_start(&self, addr: u64) -> Option<u64> {
        let k = self
            .primary
            .partition_point(|&i| self.labels[i].addr <= addr);
        Some(self.labels[self.primary.get(k.checked_sub(1)?).copied()?].addr)
    }

    /// The dynamic relocations with places in `[addr, addr + len)`.
    pub fn dynrel_in(&self, addr: u64, len: u64) -> &[DynReloc] {
        let lo = self.dynrel.partition_point(|r| r.place < addr);
        let hi = self.dynrel.partition_point(|r| r.place < addr + len);
        &self.dynrel[lo..hi]
    }

    /// The bytes of allocated sections with contents, outside `.symtab`'s
    /// sized symbols: what isn't compared as functions or data.
    pub fn uncovered(&self) -> u64 {
        let mut total = 0u64;
        let mut covered: Vec<(u64, u64)> = self
            .regions
            .iter()
            .map(|r| (r.addr, r.addr + r.size))
            .collect();
        covered.sort_unstable();
        for &(start, end, i) in &self.extents {
            if self.sections[i].kind == SHT_NOBITS {
                continue;
            }
            let mut at = start;
            let lo = covered.partition_point(|c| c.1 <= start);
            for &(a, b) in &covered[lo..] {
                if a >= end {
                    break;
                }
                if a > at {
                    total += a - at;
                }
                at = at.max(b.min(end));
            }
            if at < end {
                total += end - at;
            }
        }
        total
    }
}

/// Whether a section is one of the linker's tables, compared as tables
/// (the dynamic section, GOT, PLT, dynamic symbols and relocations,
/// hash tables, versions, unwind tables), not by its bytes.
pub(crate) fn is_linker_table(name: &[u8]) -> bool {
    name == b".dynamic"
        || name.starts_with(b".got")
        || name.starts_with(b".plt")
        || name.starts_with(b".rela")
        || name.starts_with(b".rel.")
        || name.starts_with(b".relr")
        || name.starts_with(b".dyn")
        || name.starts_with(b".gnu.version")
        || name == b".hash"
        || name == b".gnu.hash"
        || name.starts_with(b".eh_frame")
}

/// The GOT slot a 16-byte x86-64 PLT entry jumps through: the first
/// RIP-relative memory operand among its instructions.
fn plt_slot(entry: &[u8], addr: u64) -> Option<u64> {
    use turbonop_x86::{Mode, Operand, decode};
    let mut at = 0usize;
    while at < entry.len() {
        let Ok(i) = decode(&entry[at..], Mode::Bits64, addr + at as u64) else {
            return None;
        };
        if i.is_empty() {
            return None;
        }
        for o in i.operands() {
            if let Operand::Mem(m) = o
                && m.is_rip_relative()
            {
                return Some(i.next_address().wrapping_add(m.disp as u64));
            }
        }
        at += i.len();
    }
    None
}

impl super::super::image::bytes::Names for Program<'_> {
    fn name(&self, rva: u32) -> Option<(&[u8], u32)> {
        Program::name(self, u64::from(rva))
    }

    fn in_code(&self, rva: u32) -> bool {
        Program::in_code(self, u64::from(rva))
    }

    fn image_base(&self) -> u64 {
        0
    }

    fn relocations_in(&self, _rva: u32, _len: u32) -> &[(u32, u8)] {
        // (Dynamic relocations are compared, and masked, before.)
        &[]
    }

    fn jump_table(&self, rva: u32) -> Option<Vec<u32>> {
        Program::jump_table(self, u64::from(rva))?
            .into_iter()
            .map(|t| u32::try_from(t).ok())
            .collect()
    }

    fn tls(&self, tpoff: i64) -> Option<(&[u8], u32)> {
        if self.machine() != 62 || self.tls_aligned == 0 {
            return None;
        }
        let off = u64::try_from(tpoff.checked_add(i64::try_from(self.tls_aligned).ok()?)?).ok()?;
        self.tls_at(off)
    }

    fn aliases(&self, rva: u32) -> &[Box<[u8]>] {
        match self.label_start(u64::from(rva)) {
            Some(a) => self.aliases_at(a),
            None => &[],
        }
    }
}
