//! Writing the model: re-encoded into the layout read while every part
//! keeps its size, else laid out afresh.

use alloc::collections::BTreeMap;
use alloc::vec;
use alloc::vec::Vec;

use crate::read::sizes;
use crate::{
    Class, Data, Endian, Object, SHN_LORESERVE, SHN_XINDEX, Section, Symbol, WriteError, sht,
};

/// Appends fields in the object's class and byte order.
struct Out {
    bytes: Vec<u8>,
    class: Class,
    endian: Endian,
}

impl Out {
    fn u8(&mut self, v: u8) {
        self.bytes.push(v);
    }

    fn u16(&mut self, v: u16) {
        match self.endian {
            Endian::Little => self.bytes.extend_from_slice(&v.to_le_bytes()),
            Endian::Big => self.bytes.extend_from_slice(&v.to_be_bytes()),
        }
    }

    fn u32(&mut self, v: u32) {
        match self.endian {
            Endian::Little => self.bytes.extend_from_slice(&v.to_le_bytes()),
            Endian::Big => self.bytes.extend_from_slice(&v.to_be_bytes()),
        }
    }

    fn u64(&mut self, v: u64) {
        match self.endian {
            Endian::Little => self.bytes.extend_from_slice(&v.to_le_bytes()),
            Endian::Big => self.bytes.extend_from_slice(&v.to_be_bytes()),
        }
    }

    /// A class-sized word, checked to fit a 32-bit object's.
    fn word(&mut self, v: u64, what: &'static str) -> Result<(), WriteError> {
        match self.class {
            Class::Elf32 => self.u32(u32::try_from(v).map_err(|_| WriteError::TooLarge(what))?),
            Class::Elf64 => self.u64(v),
        }
        Ok(())
    }
}

/// The offsets of `names` in `table`, if each is there (followed by its
/// NUL) at the offset given.
fn found<'n>(table: &[u8], mut names: impl Iterator<Item = (&'n [u8], u32)>) -> bool {
    names.all(|(n, at)| {
        let at = at as usize;
        table.get(at..at + n.len()) == Some(n) && table.get(at + n.len()) == Some(&0)
    })
}

/// A string table of `names`, each once, in order, after a NUL; and each
/// name's offset.
fn build<'n>(names: impl Iterator<Item = &'n [u8]>) -> (Vec<u8>, Vec<u32>) {
    let mut table = vec![0u8];
    let mut seen: BTreeMap<&[u8], u32> = BTreeMap::new();
    let mut offsets = Vec::new();
    for n in names {
        if n.is_empty() {
            offsets.push(0);
            continue;
        }
        let at = *seen.entry(n).or_insert_with(|| {
            let at = table.len() as u32;
            table.extend_from_slice(n);
            table.push(0);
            at
        });
        offsets.push(at);
    }
    (table, offsets)
}

/// The string tables to write: each section's name offset, each symbol
/// table's names' offsets, and the tables rebuilt.
struct Strings {
    /// Each section's name offset.
    section_names: Vec<u32>,
    /// Rebuilt tables, by section index.
    rebuilt: BTreeMap<usize, Vec<u8>>,
    /// Each symbol table's names' offsets, by its section index.
    symbol_names: BTreeMap<usize, Vec<u32>>,
}

fn table_bytes<'b>(sections: &'b [Section<'_>], i: usize) -> Option<&'b [u8]> {
    match &sections.get(i)?.data {
        Data::Bytes(b) => Some(b),
        _ => None,
    }
}

/// Plans the string tables. A table is kept while every name its users
/// (the section names, if it is `shstrndx`; each symbol table linked to
/// it) look for is at its offset; else it is rebuilt from them all: the
/// section names first, then each symbol table's, in section order.
fn strings(o: &Object<'_>) -> Strings {
    let shstr = o.shstrndx as usize;
    let mut tables: Vec<usize> = Vec::new();
    if shstr != 0 {
        tables.push(shstr);
    }
    let symtabs: Vec<usize> = o
        .sections
        .iter()
        .enumerate()
        .filter(|(_, s)| matches!(s.data, Data::Symbols(_)))
        .map(|(i, _)| i)
        .collect();
    for &t in &symtabs {
        let link = o.sections[t].link as usize;
        if !tables.contains(&link) {
            tables.push(link);
        }
    }
    let symbols_of = |t: usize| match &o.sections[t].data {
        Data::Symbols(s) => s.as_slice(),
        _ => &[],
    };
    let mut out = Strings {
        section_names: o.sections.iter().map(|s| s.name_offset).collect(),
        rebuilt: BTreeMap::new(),
        symbol_names: symtabs
            .iter()
            .map(|&t| (t, symbols_of(t).iter().map(|s| s.name_offset).collect()))
            .collect(),
    };
    for table in tables {
        let names_here = table == shstr;
        let users: Vec<usize> = symtabs
            .iter()
            .copied()
            .filter(|&t| o.sections[t].link as usize == table)
            .collect();
        let kept = table_bytes(&o.sections, table).is_some_and(|bytes| {
            (!names_here
                || found(
                    bytes,
                    o.sections.iter().skip(1).map(|s| (&*s.name, s.name_offset)),
                ))
                && users.iter().all(|&t| {
                    found(
                        bytes,
                        symbols_of(t)
                            .iter()
                            .skip(1)
                            .map(|s| (&*s.name, s.name_offset)),
                    )
                })
        });
        if kept {
            continue;
        }
        let section_names = names_here.then(|| o.sections.iter().skip(1).map(|s| &*s.name));
        let symbol_names = users
            .iter()
            .flat_map(|&t| symbols_of(t).iter().skip(1).map(|s| &*s.name));
        let all: Vec<&[u8]> = section_names
            .into_iter()
            .flatten()
            .chain(symbol_names)
            .collect();
        let (bytes, offsets) = build(all.iter().copied());
        let mut at = offsets.into_iter();
        if names_here {
            for s in out.section_names.iter_mut().skip(1) {
                *s = at.next().unwrap_or(0);
            }
        }
        for &t in &users {
            let names = out
                .symbol_names
                .get_mut(&t)
                .expect("a symbol table's names");
            for n in names.iter_mut().skip(1) {
                *n = at.next().unwrap_or(0);
            }
            if let Some(first) = names.first_mut() {
                *first = 0;
            }
        }
        out.rebuilt.insert(table, bytes);
    }
    out
}

/// Each section's bytes in the file.
fn encode(o: &Object<'_>, strings: &Strings) -> Result<Vec<Vec<u8>>, WriteError> {
    let class = o.class();
    let endian = o.endian();
    let mut out = Vec::with_capacity(o.sections.len());
    for (i, s) in o.sections.iter().enumerate() {
        let mut w = Out {
            bytes: Vec::new(),
            class,
            endian,
        };
        match &s.data {
            Data::Bytes(b) => match strings.rebuilt.get(&i) {
                Some(t) => w.bytes.extend_from_slice(t),
                None => w.bytes.extend_from_slice(b),
            },
            Data::NoBits => {}
            Data::Symbols(symbols) => {
                let names = &strings.symbol_names[&i];
                for (s, &name) in symbols.iter().zip(names) {
                    let shndx = if s.xindex {
                        SHN_XINDEX
                    } else {
                        u16::try_from(s.shndx)
                            .map_err(|_| WriteError::TooLarge("a symbol's section index"))?
                    };
                    match class {
                        Class::Elf32 => {
                            w.u32(name);
                            w.word(s.value, "a symbol's value")?;
                            w.word(s.size, "a symbol's size")?;
                            w.u8(s.info);
                            w.u8(s.other);
                            w.u16(shndx);
                        }
                        Class::Elf64 => {
                            w.u32(name);
                            w.u8(s.info);
                            w.u8(s.other);
                            w.u16(shndx);
                            w.u64(s.value);
                            w.u64(s.size);
                        }
                    }
                }
            }
            Data::Relocations(rs) => {
                let rela = s.kind == sht::RELA;
                for r in rs {
                    w.word(r.offset, "a relocation's offset")?;
                    w.word(r.info, "a relocation's info")?;
                    if rela {
                        match class {
                            Class::Elf32 => w.u32(
                                i32::try_from(r.addend)
                                    .map_err(|_| WriteError::TooLarge("an addend"))?
                                    as u32,
                            ),
                            Class::Elf64 => w.u64(r.addend as u64),
                        }
                    }
                }
            }
            Data::Group(g) => {
                w.u32(g.flags);
                for &m in &g.members {
                    w.u32(m);
                }
            }
            Data::SymtabShndx => {
                let Some(Data::Symbols(symbols)) = o.sections.get(s.link as usize).map(|t| &t.data)
                else {
                    return Err(WriteError::BadReference("an SHT_SYMTAB_SHNDX's link"));
                };
                for sym in symbols {
                    w.u32(if sym.xindex { sym.shndx } else { 0 });
                }
            }
        }
        out.push(w.bytes);
    }
    Ok(out)
}

/// Whether the object, written now, keeps the layout it was read with.
pub(crate) fn keeps_layout(o: &Object<'_>) -> bool {
    let strings = strings(o);
    match encode(o, &strings) {
        Ok(bytes) => fits(o, &bytes),
        Err(_) => false,
    }
}

fn fits(o: &Object<'_>, bytes: &[Vec<u8>]) -> bool {
    let Some(orig) = &o.original else {
        return false;
    };
    orig.sections.len() == o.sections.len()
        && o.sections
            .iter()
            .zip(bytes)
            .zip(&orig.sections)
            .enumerate()
            .all(|(i, ((s, b), &(_, size)))| {
                i == 0 || s.kind == sht::NOBITS || b.len() as u64 == size
            })
}

/// A symbol whose index needs the extended table, if any.
fn needs_xindex(symbols: &[Symbol<'_>]) -> bool {
    symbols.iter().any(|s| s.xindex)
}

pub(crate) fn write(o: &Object<'_>) -> Result<Vec<u8>, WriteError> {
    let class = o.class();
    let endian = o.endian();
    let (ehsize, shentsize, ..) = sizes(class);
    let strings = strings(o);
    let bytes = encode(o, &strings)?;
    for s in &o.sections {
        if let Data::Symbols(symbols) = &s.data
            && needs_xindex(symbols)
            && !o.sections.iter().any(|t| t.kind == sht::SYMTAB_SHNDX)
        {
            return Err(WriteError::BadReference(
                "SHN_XINDEX without SHT_SYMTAB_SHNDX",
            ));
        }
    }
    let n = o.sections.len();
    let keep = fits(o, &bytes);
    // Where each section and the headers go.
    let (offsets, shoff, size): (Vec<u64>, u64, u64) = match (&o.original, keep) {
        (Some(orig), true) => (
            orig.sections.iter().map(|&(off, _)| off).collect(),
            orig.shoff,
            orig.file.len() as u64,
        ),
        _ => {
            let mut at = ehsize;
            let mut offsets = vec![0u64; n];
            for (i, s) in o.sections.iter().enumerate().skip(1) {
                let align = s.addralign.max(1);
                at = at.div_ceil(align) * align;
                offsets[i] = at;
                if s.kind != sht::NOBITS {
                    at += bytes[i].len() as u64;
                }
            }
            let align = match class {
                Class::Elf32 => 4,
                Class::Elf64 => 8,
            };
            let shoff = at.div_ceil(align) * align;
            (offsets, shoff, shoff + n as u64 * shentsize)
        }
    };
    let mut file = match (&o.original, keep) {
        (Some(orig), true) => orig.file.to_vec(),
        _ => vec![0u8; usize::try_from(size).map_err(|_| WriteError::TooLarge("the object"))?],
    };
    let extended = n >= SHN_LORESERVE as usize;
    let shstrndx_ext = o.shstrndx >= SHN_LORESERVE;
    let (shnum_field, shstrndx_field) = match (&o.original, keep) {
        (Some(orig), true) => (orig.shnum, orig.shstrndx),
        _ => (
            if extended { 0 } else { n as u16 },
            if shstrndx_ext {
                SHN_XINDEX
            } else {
                o.shstrndx as u16
            },
        ),
    };
    // The header.
    let mut h = Out {
        bytes: Vec::with_capacity(ehsize as usize),
        class,
        endian,
    };
    h.bytes.extend_from_slice(&o.header.ident);
    h.u16(o.header.kind);
    h.u16(o.header.machine);
    h.u32(o.header.version);
    h.word(o.header.entry, "the entry point")?;
    let (phoff, phentsize) = match (&o.original, keep) {
        (Some(orig), true) => (orig.phoff, orig.phentsize),
        _ => (0, 0),
    };
    h.word(phoff, "e_phoff")?;
    h.word(shoff, "e_shoff")?;
    h.u32(o.header.flags);
    h.u16(ehsize as u16);
    h.u16(phentsize);
    h.u16(0);
    h.u16(shentsize as u16);
    h.u16(shnum_field);
    h.u16(shstrndx_field);
    file[..h.bytes.len()].copy_from_slice(&h.bytes);
    // The sections.
    for (i, b) in bytes.iter().enumerate() {
        if i == 0 || o.sections[i].kind == sht::NOBITS || b.is_empty() {
            continue;
        }
        let at = usize::try_from(offsets[i]).map_err(|_| WriteError::TooLarge("an offset"))?;
        file[at..at + b.len()].copy_from_slice(b);
    }
    // The section headers.
    let mut sh = Out {
        bytes: Vec::with_capacity(n * shentsize as usize),
        class,
        endian,
    };
    for (i, s) in o.sections.iter().enumerate() {
        let (size, link) = if i == 0 {
            let size = if extended && !keep {
                n as u64
            } else {
                s.nobits_size
            };
            let link = if shstrndx_ext && !keep {
                o.shstrndx
            } else {
                s.link
            };
            (size, link)
        } else if s.kind == sht::NOBITS {
            (s.nobits_size, s.link)
        } else {
            (bytes[i].len() as u64, s.link)
        };
        let offset = match (&o.original, keep) {
            (Some(orig), true) => orig.sections[i].0,
            _ if i == 0 => 0,
            _ => offsets[i],
        };
        sh.u32(strings.section_names[i]);
        sh.u32(s.kind);
        sh.word(s.flags, "a section's flags")?;
        sh.word(s.addr, "a section's address")?;
        sh.word(offset, "a section's offset")?;
        sh.word(size, "a section's size")?;
        sh.u32(link);
        sh.u32(s.info);
        sh.word(s.addralign, "a section's alignment")?;
        sh.word(s.entsize, "a section's entry size")?;
    }
    let at = usize::try_from(shoff).map_err(|_| WriteError::TooLarge("e_shoff"))?;
    let end = at + sh.bytes.len();
    if end > file.len() {
        return Err(WriteError::TooLarge("the section headers"));
    }
    file[at..end].copy_from_slice(&sh.bytes);
    Ok(file)
}
