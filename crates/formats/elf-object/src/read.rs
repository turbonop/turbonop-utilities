//! Reading an object into the model.

use alloc::borrow::Cow;
use alloc::vec::Vec;

use crate::{
    Class, Data, Endian, Group, Header, Object, Original, ReadError, RelocationForm, Relocations,
    SHN_XINDEX, Section, Symbol, sht,
};

/// A cursor over the file's fields in its class and byte order.
#[derive(Clone, Copy)]
pub(crate) struct Fields<'a> {
    pub data: &'a [u8],
    pub class: Class,
    pub endian: Endian,
}

impl Fields<'_> {
    fn bytes<const N: usize>(&self, at: u64) -> Option<[u8; N]> {
        let at = usize::try_from(at).ok()?;
        self.data.get(at..at.checked_add(N)?)?.try_into().ok()
    }

    pub fn u8(&self, at: u64) -> Option<u8> {
        self.bytes::<1>(at).map(|[b]| b)
    }

    pub fn u16(&self, at: u64) -> Option<u16> {
        let b = self.bytes(at)?;
        Some(match self.endian {
            Endian::Little => u16::from_le_bytes(b),
            Endian::Big => u16::from_be_bytes(b),
        })
    }

    pub fn u32(&self, at: u64) -> Option<u32> {
        let b = self.bytes(at)?;
        Some(match self.endian {
            Endian::Little => u32::from_le_bytes(b),
            Endian::Big => u32::from_be_bytes(b),
        })
    }

    pub fn u64(&self, at: u64) -> Option<u64> {
        let b = self.bytes(at)?;
        Some(match self.endian {
            Endian::Little => u64::from_le_bytes(b),
            Endian::Big => u64::from_be_bytes(b),
        })
    }

    /// A class-sized word.
    pub fn word(&self, at: u64) -> Option<u64> {
        match self.class {
            Class::Elf32 => self.u32(at).map(u64::from),
            Class::Elf64 => self.u64(at),
        }
    }
}

/// The NUL-terminated string at `at` in `table` (to its end if none).
fn string(table: &[u8], at: u32) -> Option<&[u8]> {
    let rest = table.get(at as usize..)?;
    let end = rest.iter().position(|&b| b == 0).unwrap_or(rest.len());
    Some(&rest[..end])
}

/// A raw section header.
struct Raw {
    name: u32,
    kind: u32,
    flags: u64,
    addr: u64,
    offset: u64,
    size: u64,
    link: u32,
    info: u32,
    addralign: u64,
    entsize: u64,
}

fn section_header(f: &Fields<'_>, at: u64) -> Option<Raw> {
    Some(match f.class {
        Class::Elf32 => Raw {
            name: f.u32(at)?,
            kind: f.u32(at + 4)?,
            flags: u64::from(f.u32(at + 8)?),
            addr: u64::from(f.u32(at + 12)?),
            offset: u64::from(f.u32(at + 16)?),
            size: u64::from(f.u32(at + 20)?),
            link: f.u32(at + 24)?,
            info: f.u32(at + 28)?,
            addralign: u64::from(f.u32(at + 32)?),
            entsize: u64::from(f.u32(at + 36)?),
        },
        Class::Elf64 => Raw {
            name: f.u32(at)?,
            kind: f.u32(at + 4)?,
            flags: f.u64(at + 8)?,
            addr: f.u64(at + 16)?,
            offset: f.u64(at + 24)?,
            size: f.u64(at + 32)?,
            link: f.u32(at + 40)?,
            info: f.u32(at + 44)?,
            addralign: f.u64(at + 48)?,
            entsize: f.u64(at + 56)?,
        },
    })
}

/// The standard sizes: the header, a section header, a symbol, a REL and a
/// RELA relocation.
pub(crate) fn sizes(class: Class) -> (u64, u64, u64, u64, u64) {
    match class {
        Class::Elf32 => (52, 40, 16, 8, 12),
        Class::Elf64 => (64, 64, 24, 16, 24),
    }
}

pub(crate) fn parse(data: &[u8]) -> Result<Object<'_>, ReadError> {
    if !data.starts_with(b"\x7fELF") || data.len() < 16 {
        return Err(ReadError::NotElf);
    }
    let class = match data[4] {
        1 => Class::Elf32,
        2 => Class::Elf64,
        _ => return Err(ReadError::NotElf),
    };
    let endian = match data[5] {
        1 => Endian::Little,
        2 => Endian::Big,
        _ => return Err(ReadError::NotElf),
    };
    let f = Fields {
        data,
        class,
        endian,
    };
    let (ehsize_std, shentsize_std, symsize, relsize, relasize) = sizes(class);
    let bad = |what| ReadError::Malformed(what);
    let w = match class {
        Class::Elf32 => 4,
        Class::Elf64 => 8,
    };
    let mut ident = [0u8; 16];
    ident.copy_from_slice(&data[..16]);
    let kind = f.u16(16).ok_or(bad("header"))?;
    let machine = f.u16(18).ok_or(bad("header"))?;
    let version = f.u32(20).ok_or(bad("header"))?;
    let entry = f.word(24).ok_or(bad("header"))?;
    let phoff = f.word(24 + w).ok_or(bad("header"))?;
    let shoff = f.word(24 + 2 * w).ok_or(bad("header"))?;
    let rest = 24 + 3 * w;
    let flags = f.u32(rest).ok_or(bad("header"))?;
    let ehsize = f.u16(rest + 4).ok_or(bad("header"))?;
    let phentsize = f.u16(rest + 6).ok_or(bad("header"))?;
    let phnum = f.u16(rest + 8).ok_or(bad("header"))?;
    let shentsize = f.u16(rest + 10).ok_or(bad("header"))?;
    let shnum_raw = f.u16(rest + 12).ok_or(bad("header"))?;
    let shstrndx_raw = f.u16(rest + 14).ok_or(bad("header"))?;
    if phnum != 0 {
        return Err(ReadError::Unsupported("program headers"));
    }
    if u64::from(ehsize) != ehsize_std {
        return Err(ReadError::Unsupported("a header size not the standard one"));
    }
    if shoff == 0 {
        return Err(ReadError::Unsupported("no section headers"));
    }
    if u64::from(shentsize) != shentsize_std {
        return Err(ReadError::Unsupported(
            "a section header size not the standard one",
        ));
    }
    let header = Header {
        ident,
        kind,
        machine,
        version,
        entry,
        flags,
    };

    // Section 0 holds the extended count and string table index.
    let s0 = section_header(&f, shoff).ok_or(bad("section header 0"))?;
    let shnum = if shnum_raw == 0 {
        s0.size
    } else {
        u64::from(shnum_raw)
    };
    let shstrndx = if shstrndx_raw == SHN_XINDEX {
        s0.link
    } else {
        u32::from(shstrndx_raw)
    };
    if shnum > (data.len() as u64) / shentsize_std {
        return Err(bad("section count"));
    }
    let mut raws = Vec::with_capacity(usize::try_from(shnum).map_err(|_| bad("section count"))?);
    for i in 0..shnum {
        raws.push(section_header(&f, shoff + i * shentsize_std).ok_or(bad("section headers"))?);
    }
    let contents = |r: &Raw| -> Result<&[u8], ReadError> {
        if r.kind == sht::NOBITS || r.kind == sht::NULL {
            return Ok(&[]);
        }
        let start = usize::try_from(r.offset).map_err(|_| bad("section offset"))?;
        let len = usize::try_from(r.size).map_err(|_| bad("section size"))?;
        data.get(start..start.checked_add(len).ok_or(bad("section size"))?)
            .ok_or(bad("a section past the file"))
    };
    let names: &[u8] = match raws.get(shstrndx as usize) {
        Some(r) if shstrndx != 0 && r.kind != sht::NOBITS => contents(r)?,
        _ => &[],
    };

    // The extended index tables, by the symbol table they serve.
    let mut shndx_of: Vec<Option<&[u8]>> = alloc::vec![None; raws.len()];
    for r in &raws {
        if r.kind == sht::SYMTAB_SHNDX
            && let Some(slot) = shndx_of.get_mut(r.link as usize)
        {
            *slot = Some(contents(r)?);
        }
    }

    let mut sections = Vec::with_capacity(raws.len());
    let mut placed = Vec::with_capacity(raws.len());
    for (i, r) in raws.iter().enumerate() {
        let name: &[u8] = if i == 0 {
            &[]
        } else {
            string(names, r.name).unwrap_or(&[])
        };
        let bytes = contents(r)?;
        let data = match r.kind {
            sht::NULL if i == 0 => Data::NoBits,
            sht::NOBITS => Data::NoBits,
            sht::SYMTAB | 11 if r.entsize == symsize && r.size % symsize == 0 => {
                let strings: &[u8] = match raws.get(r.link as usize) {
                    Some(s) if r.link != 0 => contents(s)?,
                    _ => &[],
                };
                let ext = shndx_of[i];
                let mut symbols = Vec::with_capacity((r.size / symsize) as usize);
                for k in 0..r.size / symsize {
                    let at = r.offset + k * symsize;
                    let (name_offset, value, size, info, other, shndx) = match class {
                        Class::Elf32 => (
                            f.u32(at),
                            f.u32(at + 4).map(u64::from),
                            f.u32(at + 8).map(u64::from),
                            f.u8(at + 12),
                            f.u8(at + 13),
                            f.u16(at + 14),
                        ),
                        Class::Elf64 => (
                            f.u32(at),
                            f.u64(at + 8),
                            f.u64(at + 16),
                            f.u8(at + 4),
                            f.u8(at + 5),
                            f.u16(at + 6),
                        ),
                    };
                    let (
                        Some(name_offset),
                        Some(value),
                        Some(size),
                        Some(info),
                        Some(other),
                        Some(shndx),
                    ) = (name_offset, value, size, info, other, shndx)
                    else {
                        return Err(bad("symbols"));
                    };
                    let xindex = shndx == SHN_XINDEX;
                    let shndx = if xindex {
                        let table = ext.ok_or(bad("SHN_XINDEX without SHT_SYMTAB_SHNDX"))?;
                        let at = usize::try_from(k * 4).map_err(|_| bad("symbols"))?;
                        let b: [u8; 4] = table
                            .get(at..at + 4)
                            .and_then(|b| b.try_into().ok())
                            .ok_or(bad("SHT_SYMTAB_SHNDX"))?;
                        match endian {
                            Endian::Little => u32::from_le_bytes(b),
                            Endian::Big => u32::from_be_bytes(b),
                        }
                    } else {
                        u32::from(shndx)
                    };
                    symbols.push(Symbol {
                        name: Cow::Borrowed(if name_offset == 0 && k == 0 {
                            &[][..]
                        } else {
                            string(strings, name_offset).ok_or(bad("a symbol's name"))?
                        }),
                        name_offset,
                        value,
                        size,
                        info,
                        other,
                        shndx,
                        xindex,
                    });
                }
                Data::Symbols(symbols)
            }
            // CREL: decoded as read (its entries' sizes vary), the bytes
            // kept to write back while nothing changes them; left as bytes
            // where the entries don't fit a `Relocation`'s `r_info`.
            sht::CREL => {
                let probe = Object {
                    header,
                    sections: Vec::new(),
                    shstrndx: 0,
                    original: None,
                };
                let decoded = crate::decode_crel(bytes, class).and_then(|(entries, addends)| {
                    let values = entries
                        .iter()
                        .map(|e| {
                            Some(crate::Relocation {
                                offset: e.offset,
                                info: probe.crel_info(e.symbol, e.kind)?,
                                addend: e.addend,
                            })
                        })
                        .collect::<Option<Vec<_>>>()?;
                    Some((values, addends))
                });
                match decoded {
                    Some((values, addends)) => {
                        Data::Relocations(Relocations::crel(Cow::Borrowed(bytes), values, addends))
                    }
                    None => Data::Bytes(Cow::Borrowed(bytes)),
                }
            }
            sht::REL | sht::RELA => {
                let rela = r.kind == sht::RELA;
                let entry = if rela { relasize } else { relsize };
                let form = RelocationForm {
                    class,
                    endian: f.endian,
                    rela,
                };
                // (Kept as the file's bytes: decoded entry by entry as read.)
                match Relocations::encoded(Cow::Borrowed(bytes), form) {
                    Some(rs) if r.entsize == entry && form.entry() as u64 == entry => {
                        Data::Relocations(rs)
                    }
                    _ => Data::Bytes(Cow::Borrowed(bytes)),
                }
            }
            sht::GROUP if r.size >= 4 && r.size % 4 == 0 => {
                let flags = f.u32(r.offset).ok_or(bad("a group"))?;
                let mut members = Vec::with_capacity((r.size / 4 - 1) as usize);
                for k in 1..r.size / 4 {
                    members.push(f.u32(r.offset + k * 4).ok_or(bad("a group"))?);
                }
                Data::Group(Group { flags, members })
            }
            sht::SYMTAB_SHNDX
                if raws
                    .get(r.link as usize)
                    .is_some_and(|t| t.kind == sht::SYMTAB && r.size == t.size / symsize * 4) =>
            {
                Data::SymtabShndx
            }
            _ => Data::Bytes(Cow::Borrowed(bytes)),
        };
        let nobits_size = if matches!(data, Data::NoBits) {
            r.size
        } else {
            0
        };
        sections.push(Section {
            name: Cow::Borrowed(name),
            name_offset: r.name,
            kind: r.kind,
            flags: r.flags,
            addr: r.addr,
            link: r.link,
            info: r.info,
            addralign: r.addralign,
            entsize: r.entsize,
            nobits_size,
            data,
        });
        placed.push((r.offset, r.size));
    }
    // Section 0's name field and offset are kept as they were.
    Ok(Object {
        header,
        sections,
        shstrndx,
        original: Some(Original {
            file: data,
            sections: placed,
            shoff,
            shnum: shnum_raw,
            shstrndx: shstrndx_raw,
            phoff,
            phentsize,
        }),
    })
}
