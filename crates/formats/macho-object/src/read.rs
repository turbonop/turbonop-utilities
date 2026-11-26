//! Reading an object into the model.

use alloc::borrow::Cow;
use alloc::vec::Vec;

use crate::consts::*;
use crate::{
    Command, Contents, Dysymtab, Endian, Header, Object, Original, Placed, PlacedSection,
    ReadError, Relocation, Section, Segment, Symbol, is_zerofill,
};

/// A cursor over the file's fields in its byte order.
#[derive(Clone, Copy)]
pub(crate) struct Fields<'a> {
    pub data: &'a [u8],
    pub endian: Endian,
}

impl<'a> Fields<'a> {
    fn bytes<const N: usize>(&self, at: u64) -> Option<[u8; N]> {
        let at = usize::try_from(at).ok()?;
        self.data.get(at..at.checked_add(N)?)?.try_into().ok()
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

    /// A pointer-sized word.
    pub fn word(&self, at: u64, wide: bool) -> Option<u64> {
        if wide {
            self.u64(at)
        } else {
            self.u32(at).map(u64::from)
        }
    }

    pub fn name(&self, at: u64) -> Option<[u8; 16]> {
        self.bytes(at)
    }

    pub fn slice(&self, at: u64, len: u64) -> Option<&'a [u8]> {
        let at = usize::try_from(at).ok()?;
        let len = usize::try_from(len).ok()?;
        self.data.get(at..at.checked_add(len)?)
    }
}

/// The NUL-terminated string at `at` in `table` (to its end if none).
fn string(table: &[u8], at: u32) -> Option<&[u8]> {
    let rest = table.get(at as usize..)?;
    let end = rest.iter().position(|&b| b == 0).unwrap_or(rest.len());
    Some(&rest[..end])
}

/// A relocation entry's two words.
pub(crate) fn decode_relocation(w0: u32, w1: u32, endian: Endian) -> Relocation {
    if w0 & 0x8000_0000 != 0 {
        // Scattered: the same bit positions in either byte order.
        return Relocation {
            address: w0 & 0x00FF_FFFF,
            symbolnum: 0,
            pcrel: w0 & 0x4000_0000 != 0,
            length: ((w0 >> 28) & 3) as u8,
            external: false,
            kind: ((w0 >> 24) & 0xF) as u8,
            value: Some(w1),
        };
    }
    match endian {
        Endian::Little => Relocation {
            address: w0,
            symbolnum: w1 & 0x00FF_FFFF,
            pcrel: w1 & (1 << 24) != 0,
            length: ((w1 >> 25) & 3) as u8,
            external: w1 & (1 << 27) != 0,
            kind: (w1 >> 28) as u8,
            value: None,
        },
        Endian::Big => Relocation {
            address: w0,
            symbolnum: w1 >> 8,
            pcrel: w1 & (1 << 7) != 0,
            length: ((w1 >> 5) & 3) as u8,
            external: w1 & (1 << 4) != 0,
            kind: (w1 & 0xF) as u8,
            value: None,
        },
    }
}

pub(crate) fn parse(data: &[u8]) -> Result<Object<'_>, ReadError> {
    let bad = ReadError::Malformed;
    let magic_le = data
        .get(..4)
        .map(|b| u32::from_le_bytes([b[0], b[1], b[2], b[3]]))
        .ok_or(ReadError::NotMachO)?;
    let (endian, magic) = match magic_le {
        MH_MAGIC => (Endian::Little, MH_MAGIC),
        MH_MAGIC_64 => (Endian::Little, MH_MAGIC_64),
        MH_CIGAM => (Endian::Big, MH_MAGIC),
        MH_CIGAM_64 => (Endian::Big, MH_MAGIC_64),
        _ => return Err(ReadError::NotMachO),
    };
    let wide = magic == MH_MAGIC_64;
    let f = Fields { data, endian };
    let h = |at| f.u32(at).ok_or(bad("header"));
    let header = Header {
        magic,
        endian,
        cputype: h(4)?,
        cpusubtype: h(8)?,
        filetype: h(12)?,
        flags: h(24)?,
        reserved: if wide { h(28)? } else { 0 },
    };
    if header.filetype != MH_OBJECT {
        return Err(ReadError::Unsupported("not an object (MH_OBJECT)"));
    }
    let ncmds = h(16)?;
    let sizeofcmds = h(20)?;
    let start = header.size();
    let end = start + u64::from(sizeofcmds);
    if end > data.len() as u64 {
        return Err(bad("load commands past the file"));
    }
    let (seg_size, sect_size) = if wide { (72, 80) } else { (56, 68) };
    let seg_cmd = if wide { LC_SEGMENT_64 } else { LC_SEGMENT };

    let mut commands = Vec::new();
    let mut placed_commands = Vec::new();
    let mut placed = Vec::new();
    let mut symtab = None;
    let mut dysymtab_words = None;
    let mut at = start;
    for _ in 0..ncmds {
        let cmd = f.u32(at).ok_or(bad("a load command"))?;
        let cmdsize = f.u32(at + 4).ok_or(bad("a load command"))?;
        if cmdsize < 8 || at + u64::from(cmdsize) > end {
            return Err(bad("a load command's size"));
        }
        let body = f
            .slice(at + 8, u64::from(cmdsize) - 8)
            .ok_or(bad("a load command"))?;
        let (command, place) = match cmd {
            LC_SEGMENT | LC_SEGMENT_64 if cmd == seg_cmd => {
                let nsects = f.u32(at + seg_size - 8).ok_or(bad("a segment"))?;
                if u64::from(cmdsize) != seg_size + u64::from(nsects) * sect_size {
                    return Err(bad("a segment's size"));
                }
                let w = if wide { 8 } else { 4 };
                let segname = f.name(at + 8).ok_or(bad("a segment"))?;
                let vmaddr = f.word(at + 24, wide).ok_or(bad("a segment"))?;
                let vmsize = f.word(at + 24 + w, wide).ok_or(bad("a segment"))?;
                let fileoff = f.word(at + 24 + 2 * w, wide).ok_or(bad("a segment"))?;
                let filesize = f.word(at + 24 + 3 * w, wide).ok_or(bad("a segment"))?;
                let rest = at + 24 + 4 * w;
                let maxprot = f.u32(rest).ok_or(bad("a segment"))?;
                let initprot = f.u32(rest + 4).ok_or(bad("a segment"))?;
                let flags = f.u32(rest + 12).ok_or(bad("a segment"))?;
                let mut sections = Vec::with_capacity(nsects as usize);
                let mut placed_sections = Vec::with_capacity(nsects as usize);
                for k in 0..u64::from(nsects) {
                    let s = at + seg_size + k * sect_size;
                    let e = || bad("a section");
                    let sectname = f.name(s).ok_or_else(e)?;
                    let segname = f.name(s + 16).ok_or_else(e)?;
                    let addr = f.word(s + 32, wide).ok_or_else(e)?;
                    let size = f.word(s + 32 + w, wide).ok_or_else(e)?;
                    let r = s + 32 + 2 * w;
                    let offset = f.u32(r).ok_or_else(e)?;
                    let align = f.u32(r + 4).ok_or_else(e)?;
                    let reloff = f.u32(r + 8).ok_or_else(e)?;
                    let nreloc = f.u32(r + 12).ok_or_else(e)?;
                    let sflags = f.u32(r + 16).ok_or_else(e)?;
                    let reserved1 = f.u32(r + 20).ok_or_else(e)?;
                    let reserved2 = f.u32(r + 24).ok_or_else(e)?;
                    let reserved3 = if wide {
                        f.u32(r + 28).ok_or_else(e)?
                    } else {
                        0
                    };
                    let zerofill = is_zerofill(sflags);
                    let contents = if zerofill {
                        Contents::Zerofill(size)
                    } else {
                        Contents::Bytes(Cow::Borrowed(
                            f.slice(u64::from(offset), size)
                                .ok_or(bad("a section past the file"))?,
                        ))
                    };
                    if u64::from(reloff) + u64::from(nreloc) * 8 > data.len() as u64 {
                        return Err(bad("relocations past the file"));
                    }
                    let mut relocations = Vec::with_capacity(nreloc as usize);
                    for i in 0..u64::from(nreloc) {
                        let p = u64::from(reloff) + i * 8;
                        let w0 = f.u32(p).ok_or(bad("relocations"))?;
                        let w1 = f.u32(p + 4).ok_or(bad("relocations"))?;
                        relocations.push(decode_relocation(w0, w1, endian));
                    }
                    sections.push(Section {
                        sectname,
                        segname,
                        addr,
                        align,
                        flags: sflags,
                        reserved1,
                        reserved2,
                        reserved3,
                        contents,
                        relocations,
                    });
                    placed_sections.push(PlacedSection {
                        offset,
                        size: if zerofill { 0 } else { size },
                        reloff,
                        nreloc,
                    });
                }
                (
                    Command::Segment(Segment {
                        segname,
                        vmaddr,
                        maxprot,
                        initprot,
                        flags,
                        sections,
                    }),
                    Placed::Segment {
                        vmsize,
                        fileoff,
                        filesize,
                        sections: placed_sections,
                    },
                )
            }
            LC_SEGMENT | LC_SEGMENT_64 => {
                return Err(ReadError::Unsupported("a segment of the other width"));
            }
            LC_SYMTAB => {
                if cmdsize != 24 {
                    return Err(bad("LC_SYMTAB's size"));
                }
                if symtab.is_some() {
                    return Err(ReadError::Unsupported("two LC_SYMTABs"));
                }
                let w = |k: u64| f.u32(at + 8 + 4 * k).ok_or(bad("LC_SYMTAB"));
                let (symoff, nsyms, stroff, strsize) = (w(0)?, w(1)?, w(2)?, w(3)?);
                symtab = Some((symoff, nsyms, stroff, strsize));
                (
                    Command::Symtab,
                    Placed::Symtab {
                        symoff,
                        nsyms,
                        stroff,
                        strsize,
                    },
                )
            }
            LC_DYSYMTAB => {
                if cmdsize != 80 {
                    return Err(bad("LC_DYSYMTAB's size"));
                }
                if dysymtab_words.is_some() {
                    return Err(ReadError::Unsupported("two LC_DYSYMTABs"));
                }
                let mut words = [0u32; 18];
                for (k, w) in words.iter_mut().enumerate() {
                    *w = f.u32(at + 8 + 4 * k as u64).ok_or(bad("LC_DYSYMTAB"))?;
                }
                // ntoc, nmodtab, nextrefsyms, nextrel, nlocrel: images' tables.
                if [7, 9, 11, 15, 17].iter().any(|&k| words[k] != 0) {
                    return Err(ReadError::Unsupported(
                        "LC_DYSYMTAB tables other than the indirect symbols",
                    ));
                }
                dysymtab_words = Some(words);
                (
                    Command::Dysymtab(Dysymtab {
                        ilocalsym: words[0],
                        nlocalsym: words[1],
                        iextdefsym: words[2],
                        nextdefsym: words[3],
                        iundefsym: words[4],
                        nundefsym: words[5],
                    }),
                    Placed::Dysymtab(words),
                )
            }
            LC_CODE_SIGNATURE
            | LC_SEGMENT_SPLIT_INFO
            | LC_FUNCTION_STARTS
            | LC_DATA_IN_CODE
            | LC_DYLIB_CODE_SIGN_DRS
            | LC_LINKER_OPTIMIZATION_HINT
            | LC_DYLD_EXPORTS_TRIE
            | LC_DYLD_CHAINED_FIXUPS
            | LC_ATOM_INFO
            | LC_FUNCTION_VARIANTS
            | LC_FUNCTION_VARIANT_FIXUPS
                if cmdsize == 16 =>
            {
                let dataoff = f.u32(at + 8).ok_or(bad("link-edit data"))?;
                let datasize = f.u32(at + 12).ok_or(bad("link-edit data"))?;
                let blob = f
                    .slice(u64::from(dataoff), u64::from(datasize))
                    .ok_or(bad("link-edit data past the file"))?;
                (
                    Command::LinkeditData {
                        cmd,
                        data: Cow::Borrowed(blob),
                    },
                    Placed::Linkedit { dataoff, datasize },
                )
            }
            _ => (
                Command::Other {
                    cmd,
                    body: Cow::Borrowed(body),
                },
                Placed::Other,
            ),
        };
        commands.push(command);
        placed.push(place);
        placed_commands.push((at, cmdsize));
        at += u64::from(cmdsize);
    }

    let mut symbols = Vec::new();
    let mut strings = None;
    if let Some((symoff, nsyms, stroff, strsize)) = symtab {
        let table = f
            .slice(u64::from(stroff), u64::from(strsize))
            .ok_or(bad("the string table past the file"))?;
        let entry: u64 = if wide { 16 } else { 12 };
        if u64::from(symoff) + u64::from(nsyms) * entry > data.len() as u64 {
            return Err(bad("the symbol table past the file"));
        }
        symbols.reserve(nsyms as usize);
        for k in 0..u64::from(nsyms) {
            let p = u64::from(symoff) + k * entry;
            let e = || bad("symbols");
            let name_offset = f.u32(p).ok_or_else(e)?;
            let n_type = data[p as usize + 4];
            let n_sect = data[p as usize + 5];
            let n_desc = f.u16(p + 6).ok_or_else(e)?;
            let n_value = f.word(p + 8, wide).ok_or_else(e)?;
            let name = string(table, name_offset).ok_or(bad("a symbol's name"))?;
            symbols.push(Symbol {
                name: Cow::Borrowed(name),
                name_offset,
                n_type,
                n_sect,
                n_desc,
                n_value,
            });
        }
        strings = Some(Cow::Borrowed(table));
    }

    let mut indirect_symbols = Vec::new();
    if let Some(words) = dysymtab_words {
        let (off, n) = (u64::from(words[12]), u64::from(words[13]));
        if off + n * 4 > data.len() as u64 {
            return Err(bad("the indirect symbol table past the file"));
        }
        indirect_symbols.reserve(n as usize);
        for k in 0..n {
            indirect_symbols.push(f.u32(off + 4 * k).ok_or(bad("indirect symbols"))?);
        }
    }

    Ok(Object {
        header,
        commands,
        symbols,
        strings,
        indirect_symbols,
        original: Some(Original {
            file: data,
            commands: placed_commands,
            placed,
        }),
    })
}
