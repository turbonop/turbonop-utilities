//! Writing the model: re-encoded into the layout read while every part
//! keeps its size, else laid out afresh as LLVM's `MachObjectWriter` lays
//! an object out.

use alloc::collections::BTreeMap;
use alloc::vec;
use alloc::vec::Vec;

use crate::{Command, Contents, Endian, Object, Placed, Relocation, WriteError};

/// Appends fields in the object's byte order.
struct Out {
    bytes: Vec<u8>,
    endian: Endian,
}

impl Out {
    fn new(endian: Endian) -> Self {
        Self {
            bytes: Vec::new(),
            endian,
        }
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

    /// A pointer-sized word, checked to fit a 32-bit object's.
    fn word(&mut self, v: u64, wide: bool, what: &'static str) -> Result<(), WriteError> {
        if wide {
            self.u64(v);
        } else {
            self.u32(u32::try_from(v).map_err(|_| WriteError::TooLarge(what))?);
        }
        Ok(())
    }
}

fn fit32(v: u64, what: &'static str) -> Result<u32, WriteError> {
    u32::try_from(v).map_err(|_| WriteError::TooLarge(what))
}

/// LLVM's string table (`StringTableBuilder`, kinds `MachO` and
/// `MachO64`) of `names`: a NUL, then the distinct names sorted by their
/// bytes read from the end, larger first (so a name comes right after
/// every name it is a suffix of), each name that is a suffix of the one
/// written before it sharing that one's bytes, each other followed by a
/// NUL; padded with NULs to 8 bytes (`wide`) or 4. Returns the table and
/// each name's offset.
pub fn llvm_string_table<'n>(
    names: impl Iterator<Item = &'n [u8]>,
    wide: bool,
) -> (Vec<u8>, BTreeMap<&'n [u8], u32>) {
    let mut sorted: Vec<&[u8]> = names.collect();
    sorted.sort_unstable();
    sorted.dedup();
    sorted.sort_unstable_by(|a, b| b.iter().rev().cmp(a.iter().rev()));
    let mut table = vec![0u8];
    let mut offsets = BTreeMap::new();
    let mut previous: &[u8] = &[];
    for s in sorted {
        if previous.ends_with(s) {
            offsets.insert(s, (table.len() - s.len() - 1) as u32);
            continue;
        }
        offsets.insert(s, table.len() as u32);
        table.extend_from_slice(s);
        table.push(0);
        previous = s;
    }
    let align = if wide { 8 } else { 4 };
    table.resize(table.len().div_ceil(align) * align, 0);
    (table, offsets)
}

/// The string table to write, each symbol's name offset, and each
/// alias's value (its target's name's offset).
struct Strings<'t> {
    table: alloc::borrow::Cow<'t, [u8]>,
    offsets: Vec<u32>,
    values: Vec<u64>,
}

/// Whether `name` is at `at` in `table` (followed by a NUL, or the table's
/// end).
fn found(table: &[u8], name: &[u8], at: u64) -> bool {
    let Ok(at) = usize::try_from(at) else {
        return false;
    };
    let n = name.len();
    table.get(at..at + n) == Some(name) && table.get(at + n).is_none_or(|&b| b == 0)
}

/// Whether every symbol's name (and alias's target) is at its offset in
/// `table`.
fn names_found(o: &Object<'_>, table: &[u8]) -> bool {
    o.symbols.iter().all(|s| {
        found(table, &s.name, u64::from(s.name_offset))
            && s.indirect
                .as_ref()
                .is_none_or(|t| found(table, t, s.n_value))
    })
}

fn strings<'t>(o: &'t Object<'_>) -> Strings<'t> {
    if let Some(t) = &o.strings
        && names_found(o, t)
    {
        return Strings {
            table: alloc::borrow::Cow::Borrowed(t),
            offsets: o.symbols.iter().map(|s| s.name_offset).collect(),
            values: o.symbols.iter().map(|s| s.n_value).collect(),
        };
    }
    let names = o
        .symbols
        .iter()
        .flat_map(|s| core::iter::once(&s.name[..]).chain(s.indirect.as_deref()));
    let (table, at) = llvm_string_table(names, o.is_64());
    let offsets = o.symbols.iter().map(|s| at[&s.name[..]]).collect();
    let values = o
        .symbols
        .iter()
        .map(|s| {
            s.indirect
                .as_ref()
                .map_or(s.n_value, |t| u64::from(at[&t[..]]))
        })
        .collect();
    Strings {
        table: alloc::borrow::Cow::Owned(table),
        offsets,
        values,
    }
}

/// Where a command's parts go.
#[derive(Clone, Debug)]
enum CmdLayout {
    Segment {
        vmsize: u64,
        fileoff: u64,
        filesize: u64,
        /// Each section's `offset` and `reloff`.
        sections: Vec<(u32, u32)>,
    },
    Symtab {
        symoff: u32,
        stroff: u32,
    },
    /// The 18 words after `cmdsize`, the ranges and the indirect count
    /// aside (the model's).
    Dysymtab([u32; 18]),
    Linkedit {
        dataoff: u32,
    },
    Other,
}

struct Layout {
    /// Each command's offset and layout.
    commands: Vec<(u64, CmdLayout)>,
    /// `sizeofcmds` (a fresh layout's; a kept one's stays in the copy).
    sizeofcmds: Option<u32>,
    size: u64,
}

fn align_to(v: u64, log2: u32) -> u64 {
    let a = 1u64 << log2.min(32);
    v.div_ceil(a) * a
}

/// Each section's offset in the section data (and address), as LLVM
/// gives them: in file order, the zero-fill sections after the rest, each
/// at the end of the one before it aligned to its alignment.
pub(crate) fn data_offsets(o: &Object<'_>) -> Vec<u64> {
    let secs: Vec<(u32, u64, bool)> = o
        .sections()
        .map(|s| (s.align, s.size(), s.is_zerofill_kind()))
        .collect();
    llvm_offsets(&secs)
}

/// [`data_offsets`] of sections given as their alignments, sizes, and
/// whether they are zero-fill.
pub(crate) fn llvm_offsets(secs: &[(u32, u64, bool)]) -> Vec<u64> {
    let order = secs
        .iter()
        .enumerate()
        .filter(|(_, s)| !s.2)
        .chain(secs.iter().enumerate().filter(|(_, s)| s.2))
        .map(|(i, _)| i);
    let mut out = vec![0u64; secs.len()];
    let mut at = 0u64;
    for i in order {
        at = align_to(at, secs[i].0);
        out[i] = at;
        at = at.saturating_add(secs[i].1);
    }
    out
}

/// A command's size.
fn cmdsize(o: &Object<'_>, c: &Command<'_>) -> u64 {
    let (seg, sect) = if o.is_64() { (72, 80) } else { (56, 68) };
    match c {
        Command::Segment(s) => seg + sect * s.sections.len() as u64,
        Command::Symtab => 24,
        Command::Dysymtab(_) => 80,
        Command::LinkeditData { .. } => 16,
        Command::Other { body, .. } => 8 + body.len() as u64,
    }
}

fn nlist_size(o: &Object<'_>) -> u64 {
    if o.is_64() { 16 } else { 12 }
}

fn has_symtab(o: &Object<'_>) -> bool {
    o.commands.iter().any(|c| matches!(c, Command::Symtab))
}

fn has_dysymtab(o: &Object<'_>) -> bool {
    o.commands.iter().any(|c| matches!(c, Command::Dysymtab(_)))
}

/// Whether the object, written now, keeps the layout it was read with.
pub(crate) fn keeps_layout(o: &Object<'_>) -> bool {
    fits(o, &strings(o))
}

/// Whether every part of the model has the size and kind its place in the
/// file read has.
fn fits(o: &Object<'_>, strings: &Strings<'_>) -> bool {
    let Some(orig) = &o.original else {
        return false;
    };
    if orig.placed.len() != o.commands.len() {
        return false;
    }
    let mut nsyms_ok = !has_symtab(o) && o.symbols.is_empty();
    let mut indirect_ok = !has_dysymtab(o) && o.indirect_symbols.is_empty();
    for ((c, p), &(_, size)) in o.commands.iter().zip(&orig.placed).zip(&orig.commands) {
        if cmdsize(o, c) != u64::from(size) {
            return false;
        }
        let same = match (c, p) {
            (Command::Segment(s), Placed::Segment { sections, .. }) => {
                s.sections.len() == sections.len()
                    && s.sections.iter().zip(sections).all(|(s, p)| {
                        let size = match &s.contents {
                            Contents::Bytes(b) => b.len() as u64,
                            Contents::Zerofill(_) => 0,
                        };
                        size == p.size
                            && s.relocations.len() as u64 == u64::from(p.nreloc)
                            && (matches!(s.contents, Contents::Zerofill(_)) == s.is_zerofill_kind())
                    })
            }
            (Command::Symtab, Placed::Symtab { nsyms, strsize, .. }) => {
                nsyms_ok = o.symbols.len() as u64 == u64::from(*nsyms)
                    && strings.table.len() as u64 == u64::from(*strsize);
                true
            }
            (Command::Dysymtab(_), Placed::Dysymtab(words)) => {
                indirect_ok = o.indirect_symbols.len() as u64 == u64::from(words[13]);
                true
            }
            (Command::LinkeditData { data, .. }, Placed::Linkedit { datasize, .. }) => {
                data.len() as u64 == u64::from(*datasize)
            }
            (Command::Other { .. }, Placed::Other) => true,
            _ => false,
        };
        if !same {
            return false;
        }
    }
    nsyms_ok && indirect_ok
}

fn kept_layout(o: &Object<'_>) -> Layout {
    let orig = o.original.as_ref().expect("a kept layout's original");
    let commands = orig
        .placed
        .iter()
        .zip(&orig.commands)
        .map(|(p, &(at, _))| {
            let l = match p {
                Placed::Segment {
                    vmsize,
                    fileoff,
                    filesize,
                    sections,
                } => CmdLayout::Segment {
                    vmsize: *vmsize,
                    fileoff: *fileoff,
                    filesize: *filesize,
                    sections: sections.iter().map(|s| (s.offset, s.reloff)).collect(),
                },
                Placed::Symtab { symoff, stroff, .. } => CmdLayout::Symtab {
                    symoff: *symoff,
                    stroff: *stroff,
                },
                Placed::Dysymtab(words) => CmdLayout::Dysymtab(*words),
                Placed::Linkedit { dataoff, .. } => CmdLayout::Linkedit { dataoff: *dataoff },
                Placed::Other => CmdLayout::Other,
            };
            (at, l)
        })
        .collect();
    Layout {
        commands,
        sizeofcmds: None,
        size: orig.file.len() as u64,
    }
}

/// LLVM's layout: the header and the load commands; the sections'
/// contents at their offsets ([`data_offsets`]), padded to the pointer
/// size; each section's relocations, in order; the link-edit data, in the
/// commands' order (LLVM's data in code, then its optimization hints); the
/// indirect symbols; the symbols; the strings.
fn fresh_layout(o: &Object<'_>, strings: &Strings<'_>) -> Result<Layout, WriteError> {
    let wide = o.is_64();
    let mut at = o.header.size();
    let mut offsets = Vec::with_capacity(o.commands.len());
    for c in &o.commands {
        offsets.push(at);
        at += cmdsize(o, c);
    }
    let sizeofcmds = fit32(at - o.header.size(), "the load commands")?;
    let data_start = at;
    let data = data_offsets(o);
    let secs: Vec<(u64, u64, bool, u64)> = o
        .sections()
        .zip(&data)
        .map(|(s, &d)| {
            (
                d,
                s.size(),
                s.is_zerofill_kind(),
                s.relocations.len() as u64,
            )
        })
        .collect();
    let data_size = secs
        .iter()
        .filter(|s| !s.2)
        .map(|s| s.0 + s.1)
        .max()
        .unwrap_or(0);
    let data_file_size = align_to(data_size, if wide { 3 } else { 2 });
    // Relocations.
    let mut reloc_at = data_start + data_file_size;
    let mut reloffs = Vec::with_capacity(secs.len());
    for s in &secs {
        reloffs.push(if s.3 != 0 { reloc_at } else { 0 });
        reloc_at += s.3 * 8;
    }
    // Link-edit data.
    let mut cur = reloc_at;
    let mut dataoffs = Vec::new();
    for c in &o.commands {
        if let Command::LinkeditData { data, .. } = c {
            dataoffs.push(cur);
            cur += data.len() as u64;
        }
    }
    let indirect_at = if !o.indirect_symbols.is_empty() && has_dysymtab(o) {
        let a = cur;
        cur += 4 * o.indirect_symbols.len() as u64;
        a
    } else {
        0
    };
    let (symoff, stroff) = if has_symtab(o) {
        let s = cur;
        cur += nlist_size(o) * o.symbols.len() as u64;
        let t = cur;
        cur += strings.table.len() as u64;
        (s, t)
    } else {
        (0, 0)
    };
    let mut commands = Vec::with_capacity(o.commands.len());
    let mut section_index = 0;
    let mut linkedit_index = 0;
    for (c, &at) in o.commands.iter().zip(&offsets) {
        let l = match c {
            Command::Segment(seg) => {
                let range = section_index..section_index + seg.sections.len();
                section_index = range.end;
                let mine = &secs[range.clone()];
                let filed = mine.iter().filter(|s| !s.2);
                let lo = filed.clone().map(|s| s.0).min().unwrap_or(0);
                let hi = filed.map(|s| s.0 + s.1).max().unwrap_or(0);
                let filesize = hi - lo.min(hi);
                let vm_end = seg
                    .sections
                    .iter()
                    .map(|s| s.addr.saturating_add(s.size()))
                    .max()
                    .unwrap_or(0);
                let vmsize = vm_end.saturating_sub(seg.vmaddr).max(hi);
                let mut sections = Vec::with_capacity(mine.len());
                for (k, s) in mine.iter().enumerate() {
                    let offset = if s.2 {
                        0
                    } else {
                        fit32(data_start + s.0, "a section's offset")?
                    };
                    let reloff = fit32(reloffs[range.start + k], "a relocation offset")?;
                    sections.push((offset, reloff));
                }
                CmdLayout::Segment {
                    vmsize,
                    fileoff: data_start + lo,
                    filesize,
                    sections,
                }
            }
            Command::Symtab => CmdLayout::Symtab {
                symoff: fit32(symoff, "the symbol table's offset")?,
                stroff: fit32(stroff, "the string table's offset")?,
            },
            Command::Dysymtab(_) => {
                let mut words = [0u32; 18];
                words[12] = fit32(indirect_at, "the indirect symbol table's offset")?;
                CmdLayout::Dysymtab(words)
            }
            Command::LinkeditData { .. } => {
                let d = dataoffs[linkedit_index];
                linkedit_index += 1;
                CmdLayout::Linkedit {
                    dataoff: fit32(d, "link-edit data's offset")?,
                }
            }
            Command::Other { .. } => CmdLayout::Other,
        };
        commands.push((at, l));
    }
    Ok(Layout {
        commands,
        sizeofcmds: Some(sizeofcmds),
        size: cur,
    })
}

/// A relocation entry's two words.
pub(crate) fn encode_relocation(r: &Relocation, endian: Endian) -> Result<(u32, u32), WriteError> {
    let (pcrel, length, kind) = (
        u32::from(r.pcrel),
        u32::from(r.length & 3),
        u32::from(r.kind & 0xF),
    );
    if let Some(value) = r.value {
        if r.address > 0x00FF_FFFF {
            return Err(WriteError::TooLarge("a scattered relocation's address"));
        }
        return Ok((
            0x8000_0000 | pcrel << 30 | length << 28 | kind << 24 | r.address,
            value,
        ));
    }
    if r.address & 0x8000_0000 != 0 {
        return Err(WriteError::TooLarge("a relocation's address"));
    }
    if r.symbolnum > 0x00FF_FFFF {
        return Err(WriteError::TooLarge("a relocation's symbol number"));
    }
    let external = u32::from(r.external);
    let w1 = match endian {
        Endian::Little => r.symbolnum | pcrel << 24 | length << 25 | external << 27 | kind << 28,
        Endian::Big => r.symbolnum << 8 | pcrel << 7 | length << 5 | external << 4 | kind,
    };
    Ok((r.address, w1))
}

fn put(file: &mut [u8], at: u64, bytes: &[u8], what: &'static str) -> Result<(), WriteError> {
    if bytes.is_empty() {
        return Ok(());
    }
    let at = usize::try_from(at).map_err(|_| WriteError::TooLarge(what))?;
    let dst = file
        .get_mut(at..at + bytes.len())
        .ok_or(WriteError::TooLarge(what))?;
    dst.copy_from_slice(bytes);
    Ok(())
}

pub(crate) fn write(o: &Object<'_>) -> Result<Vec<u8>, WriteError> {
    let wide = o.is_64();
    let endian = o.endian();
    let strings = strings(o);
    let keep = fits(o, &strings);
    let layout = if keep {
        kept_layout(o)
    } else {
        fresh_layout(o, &strings)?
    };
    let mut file = match (&o.original, keep) {
        (Some(orig), true) => orig.file.to_vec(),
        _ => {
            vec![0u8; usize::try_from(layout.size).map_err(|_| WriteError::TooLarge("the object"))?]
        }
    };

    // The header.
    let mut h = Out::new(endian);
    h.u32(o.header.magic);
    h.u32(o.header.cputype);
    h.u32(o.header.cpusubtype);
    h.u32(o.header.filetype);
    put(&mut file, 0, &h.bytes, "the header")?;
    if let Some(sizeofcmds) = layout.sizeofcmds {
        let mut n = Out::new(endian);
        n.u32(fit32(o.commands.len() as u64, "the command count")?);
        n.u32(sizeofcmds);
        put(&mut file, 16, &n.bytes, "the header")?;
    }
    let mut h = Out::new(endian);
    h.u32(o.header.flags);
    if wide {
        h.u32(o.header.reserved);
    }
    put(&mut file, 24, &h.bytes, "the header")?;

    // The commands, and the parts they locate.
    let mut symtab_at = None;
    let mut indirect_at = None;
    for (c, (at, l)) in o.commands.iter().zip(&layout.commands) {
        let mut w = Out::new(endian);
        let size = fit32(cmdsize(o, c), "a load command")?;
        match (c, l) {
            (
                Command::Segment(seg),
                CmdLayout::Segment {
                    vmsize,
                    fileoff,
                    filesize,
                    sections,
                },
            ) => {
                w.u32(if wide {
                    crate::consts::LC_SEGMENT_64
                } else {
                    crate::consts::LC_SEGMENT
                });
                w.u32(size);
                w.bytes.extend_from_slice(&seg.segname);
                w.word(seg.vmaddr, wide, "a segment's address")?;
                w.word(*vmsize, wide, "a segment's size")?;
                w.word(*fileoff, wide, "a segment's offset")?;
                w.word(*filesize, wide, "a segment's file size")?;
                w.u32(seg.maxprot);
                w.u32(seg.initprot);
                w.u32(fit32(seg.sections.len() as u64, "a section count")?);
                w.u32(seg.flags);
                for (s, &(offset, reloff)) in seg.sections.iter().zip(sections) {
                    w.bytes.extend_from_slice(&s.sectname);
                    w.bytes.extend_from_slice(&s.segname);
                    w.word(s.addr, wide, "a section's address")?;
                    w.word(s.size(), wide, "a section's size")?;
                    w.u32(offset);
                    w.u32(s.align);
                    w.u32(reloff);
                    w.u32(fit32(s.relocations.len() as u64, "a relocation count")?);
                    w.u32(s.flags);
                    w.u32(s.reserved1);
                    w.u32(s.reserved2);
                    if wide {
                        w.u32(s.reserved3);
                    }
                    // A zero-fill section has no place in the file, whatever
                    // the model holds.
                    if let Contents::Bytes(b) = &s.contents
                        && !s.is_zerofill_kind()
                    {
                        put(&mut file, u64::from(offset), b, "a section")?;
                    }
                    let mut r = Out::new(endian);
                    for rel in &s.relocations {
                        let (w0, w1) = encode_relocation(rel, endian)?;
                        r.u32(w0);
                        r.u32(w1);
                    }
                    put(&mut file, u64::from(reloff), &r.bytes, "relocations")?;
                }
            }
            (Command::Symtab, CmdLayout::Symtab { symoff, stroff }) => {
                w.u32(crate::consts::LC_SYMTAB);
                w.u32(size);
                w.u32(*symoff);
                w.u32(fit32(o.symbols.len() as u64, "the symbol count")?);
                w.u32(*stroff);
                w.u32(fit32(strings.table.len() as u64, "the string table")?);
                symtab_at = Some((*symoff, *stroff));
            }
            (Command::Dysymtab(d), CmdLayout::Dysymtab(words)) => {
                let mut words = *words;
                words[..6].copy_from_slice(&[
                    d.ilocalsym,
                    d.nlocalsym,
                    d.iextdefsym,
                    d.nextdefsym,
                    d.iundefsym,
                    d.nundefsym,
                ]);
                words[13] = fit32(o.indirect_symbols.len() as u64, "the indirect symbols")?;
                if words[13] == 0 && !keep {
                    words[12] = 0;
                }
                w.u32(crate::consts::LC_DYSYMTAB);
                w.u32(size);
                for v in words {
                    w.u32(v);
                }
                indirect_at = Some(words[12]);
            }
            (Command::LinkeditData { cmd, data }, CmdLayout::Linkedit { dataoff }) => {
                w.u32(*cmd);
                w.u32(size);
                w.u32(*dataoff);
                w.u32(fit32(data.len() as u64, "link-edit data")?);
                put(&mut file, u64::from(*dataoff), data, "link-edit data")?;
            }
            (Command::Other { cmd, body }, CmdLayout::Other) => {
                w.u32(*cmd);
                w.u32(size);
                w.bytes.extend_from_slice(body);
            }
            _ => unreachable!("a layout of another command"),
        }
        put(&mut file, *at, &w.bytes, "a load command")?;
    }

    if let Some(at) = indirect_at {
        let mut w = Out::new(endian);
        for &i in &o.indirect_symbols {
            w.u32(i);
        }
        put(&mut file, u64::from(at), &w.bytes, "the indirect symbols")?;
    }
    if let Some((symoff, stroff)) = symtab_at {
        let mut w = Out::new(endian);
        for ((s, &name), &value) in o.symbols.iter().zip(&strings.offsets).zip(&strings.values) {
            w.u32(name);
            w.bytes.push(s.n_type);
            w.bytes.push(s.n_sect);
            w.u16(s.n_desc);
            w.word(value, wide, "a symbol's value")?;
        }
        put(&mut file, u64::from(symoff), &w.bytes, "the symbols")?;
        put(&mut file, u64::from(stroff), &strings.table, "the strings")?;
    }
    Ok(file)
}
