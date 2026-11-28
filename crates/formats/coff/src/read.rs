//! Reading an object into the model, and recording its layout.

use alloc::borrow::Cow;
use alloc::vec::Vec;

use turbonop_core::{Error, ErrorKind, Result};
use turbonop_pe::consts::section_characteristics as scn;

use crate::auxiliary::{self, Primary, Resolve};
use crate::kind::{Kind, kind};
use crate::layout::{Layout, Placement};
use crate::object::{
    BigObjHeader, Format, LineNumber, LineNumberRef, Name, Object, Relocation, Section,
    SectionData, SectionId, SectionNumber, Style, Symbol, SymbolId,
};
use crate::strings;

/// The size of a section header.
pub(crate) const SECTION_HEADER_SIZE: usize = 40;
/// The size of a relocation.
pub(crate) const RELOCATION_SIZE: usize = 10;
/// The size of a line number record.
pub(crate) const LINE_NUMBER_SIZE: usize = 6;
/// The largest section number a regular object's 16-bit field holds:
/// numbers above it are negative (special) numbers.
pub(crate) const MAX_REGULAR_SECTIONS: u32 = 0xFEFF;

fn u16_at(b: &[u8], o: usize) -> u16 {
    u16::from_le_bytes([b[o], b[o + 1]])
}

fn u32_at(b: &[u8], o: usize) -> u32 {
    u32::from_le_bytes([b[o], b[o + 1], b[o + 2], b[o + 3]])
}

fn range(
    data: &[u8],
    offset: u64,
    len: u64,
    what: &'static str,
) -> Result<core::ops::Range<usize>> {
    let end = offset
        .checked_add(len)
        .ok_or(Error::new(ErrorKind::Overflow, offset, what))?;
    if end > data.len() as u64 {
        return Err(Error::new(ErrorKind::Truncated, offset, what));
    }
    // Both fit, being at most the data's length.
    Ok(offset as usize..end as usize)
}

/// The fields of either header.
struct Header {
    format: Format,
    machine: u16,
    time_date_stamp: u32,
    characteristics: u16,
    optional_header: core::ops::Range<usize>,
    bigobj: BigObjHeader,
    sections: u32,
    symbol_table: u32,
    symbols: u32,
    /// Where the section table starts.
    section_table: usize,
}

fn header(data: &[u8]) -> Result<Header> {
    match kind(data) {
        Some(Kind::Regular) => {
            let optional = usize::from(u16_at(data, 16));
            let optional_header = range(data, 20, optional as u64, "optional header")?;
            Ok(Header {
                format: Format::Regular,
                machine: u16_at(data, 0),
                time_date_stamp: u32_at(data, 4),
                characteristics: u16_at(data, 18),
                optional_header,
                bigobj: BigObjHeader::default(),
                sections: u32::from(u16_at(data, 2)),
                symbol_table: u32_at(data, 8),
                symbols: u32_at(data, 12),
                section_table: 20 + optional,
            })
        }
        Some(Kind::BigObj) => Ok(Header {
            format: Format::BigObj,
            machine: u16_at(data, 6),
            time_date_stamp: u32_at(data, 8),
            characteristics: 0,
            optional_header: 0..0,
            bigobj: BigObjHeader {
                version: u16_at(data, 4),
                size_of_data: u32_at(data, 28),
                flags: u32_at(data, 32),
                meta_data_size: u32_at(data, 36),
                meta_data_offset: u32_at(data, 40),
            },
            sections: u32_at(data, 44),
            symbol_table: u32_at(data, 48),
            symbols: u32_at(data, 52),
            section_table: 56,
        }),
        Some(Kind::Import) => Err(Error::new(
            ErrorKind::Unsupported,
            0,
            "an import object, not a COFF object",
        )),
        Some(Kind::Ltcg | Kind::Anonymous) => Err(Error::new(
            ErrorKind::Unsupported,
            0,
            "an anonymous object that is not big-obj",
        )),
        None => Err(Error::new(ErrorKind::Truncated, 0, "COFF header")),
    }
}

/// A section header's fields.
struct RawSection {
    name: [u8; 8],
    virtual_size: u32,
    virtual_address: u32,
    size: u32,
    data: u32,
    relocations: u32,
    line_numbers: u32,
    relocation_count: u16,
    line_count: u16,
    characteristics: u32,
}

impl RawSection {
    fn parse(h: &[u8]) -> Self {
        let mut name = [0u8; 8];
        name.copy_from_slice(&h[..8]);
        Self {
            name,
            virtual_size: u32_at(h, 8),
            virtual_address: u32_at(h, 12),
            size: u32_at(h, 16),
            data: u32_at(h, 20),
            relocations: u32_at(h, 24),
            line_numbers: u32_at(h, 28),
            relocation_count: u16_at(h, 32),
            line_count: u16_at(h, 34),
            characteristics: u32_at(h, 36),
        }
    }
}

/// The section's name: through the string table for `/<offset>`, if the
/// table has that offset, and otherwise the field itself up to its NUL.
pub(crate) fn section_name<'data>(
    field: &[u8; 8],
    strings: Option<&'data [u8]>,
) -> Option<&'data [u8]> {
    strings::section_name_offset(field).and_then(|o| strings::get(strings?, o))
}

fn inline_name(field: &[u8]) -> &[u8] {
    let len = field.iter().position(|&b| b == 0).unwrap_or(field.len());
    &field[..len]
}

/// Detects which tool's conventions a file follows, from what the two
/// styles do differently.
struct StyleDetector {
    /// The string-table names: symbols' (offset, length), and sections'.
    symbols: Vec<(u32, usize)>,
    sections: Vec<(u32, usize)>,
    llvm: bool,
}

impl StyleDetector {
    /// Whether the file follows LLVM's conventions: an empty section that
    /// has a data pointer, or strings that aren't in MSVC's order (one
    /// after another as they are used, symbols' then sections').
    fn llvm(&self) -> bool {
        let mut next = 4u32;
        for &(offset, len) in self.symbols.iter().chain(&self.sections) {
            match offset.cmp(&next) {
                core::cmp::Ordering::Equal => next = next.saturating_add(len as u32 + 1),
                core::cmp::Ordering::Greater => return true,
                core::cmp::Ordering::Less => {}
            }
        }
        self.llvm
    }
}

struct Resolver<'a> {
    /// The symbol at each table index (`u32::MAX` for auxiliary records).
    ids: &'a [u32],
    sections: u32,
    /// Each section's line numbers: (file offset, count, section), by
    /// offset.
    lines: &'a [(u32, u32, u32)],
}

impl Resolve for Resolver<'_> {
    fn symbol(&self, index: u32) -> Option<SymbolId> {
        let id = *self.ids.get(usize::try_from(index).ok()?)?;
        (id != u32::MAX).then_some(SymbolId(id))
    }

    fn section(&self, number: u32) -> Option<SectionId> {
        (1..=self.sections)
            .contains(&number)
            .then(|| SectionId(number - 1))
    }

    fn line_number(&self, offset: u32) -> Option<LineNumberRef> {
        let at = self
            .lines
            .partition_point(|l| l.0 <= offset)
            .checked_sub(1)?;
        let (start, count, section) = self.lines[at];
        let delta = offset - start;
        let index = delta / LINE_NUMBER_SIZE as u32;
        (delta.is_multiple_of(LINE_NUMBER_SIZE as u32) && index < count).then_some(LineNumberRef {
            section: SectionId(section),
            index,
        })
    }
}

impl<'data> Object<'data> {
    /// Reads a COFF object, regular or big-obj.
    ///
    /// Import objects and other anonymous objects (`/GL` objects) are not
    /// COFF objects; see [`File`](crate::File) to pass them through.
    pub fn parse(data: &'data [u8]) -> Result<Self> {
        Self::parse_with(data, |_| true)
    }

    /// Reads a COFF object as [`parse`](Self::parse) does, but reads the
    /// relocations only of the sections whose names `relocations` accepts
    /// (the others are left without any, though their ranges are still
    /// checked). For readers that never look at some sections'
    /// relocations (`.debug$S`'s are most of a `/Z7` object's); an object
    /// read so isn't written back as it was ([`has_recorded_layout`]
    /// is false).
    ///
    /// [`has_recorded_layout`]: Object::has_recorded_layout
    pub fn parse_with(data: &'data [u8], relocations: impl Fn(&[u8]) -> bool) -> Result<Self> {
        let h = header(data)?;
        let format = h.format;
        let stride = format.record_size();
        let count = h.sections as usize;
        let table = range(
            data,
            h.section_table as u64,
            count as u64 * SECTION_HEADER_SIZE as u64,
            "section table",
        )?;
        let raw: Vec<RawSection> = data[table.clone()]
            .as_chunks::<SECTION_HEADER_SIZE>()
            .0
            .iter()
            .map(|h| RawSection::parse(h))
            .collect();

        // The symbol table, and the string table after it.
        if h.symbol_table == 0 && h.symbols != 0 {
            return Err(Error::new(
                ErrorKind::Malformed,
                0,
                "symbol count without a symbol table",
            ));
        }
        let symbol_range = range(
            data,
            u64::from(h.symbol_table),
            u64::from(h.symbols) * stride as u64,
            "symbol table",
        )?;
        let records = &data[symbol_range.clone()];
        let strings_at = symbol_range.end;
        let strings: Option<&'data [u8]> = if h.symbol_table != 0 && strings_at + 4 <= data.len() {
            let declared = u32_at(data, strings_at) as usize;
            let end = strings_at.saturating_add(declared.max(4)).min(data.len());
            Some(&data[strings_at..end])
        } else {
            None
        };

        let mut style = StyleDetector {
            symbols: Vec::new(),
            sections: Vec::new(),
            llvm: false,
        };

        // Sections' names, data, and line numbers (relocations need the
        // symbols).
        let mut sections = Vec::with_capacity(count);
        let mut placements = Vec::with_capacity(count);
        let mut lines = Vec::new();
        for (i, r) in raw.iter().enumerate() {
            let name = match section_name(&r.name, strings) {
                Some(long) => {
                    if let Some(o) = strings::section_name_offset(&r.name) {
                        style.sections.push((o, long.len()));
                    }
                    Name::stored(long, r.name)
                }
                None => {
                    let at = h.section_table + i * SECTION_HEADER_SIZE;
                    Name::stored(inline_name(&data[at..at + 8]), r.name)
                }
            };
            let uninitialized = r.size > 0 && r.data == 0;
            let section_data = if uninitialized {
                SectionData::Uninitialized(r.size)
            } else if r.size == 0 {
                if r.data != 0 && r.characteristics & scn::CNT_UNINITIALIZED_DATA == 0 {
                    style.llvm = true;
                }
                SectionData::Bytes(Cow::Borrowed(&[]))
            } else {
                let at = range(data, u64::from(r.data), u64::from(r.size), "section data")?;
                SectionData::Bytes(Cow::Borrowed(&data[at]))
            };
            if r.line_count > 0 {
                range(
                    data,
                    u64::from(r.line_numbers),
                    u64::from(r.line_count) * LINE_NUMBER_SIZE as u64,
                    "line numbers",
                )?;
                lines.push((r.line_numbers, u32::from(r.line_count), i as u32));
            }
            let extended =
                r.characteristics & scn::LNK_NRELOC_OVFL != 0 && r.relocation_count == 0xFFFF;
            let (relocation_count, extended) = if extended {
                let at = range(
                    data,
                    u64::from(r.relocations),
                    RELOCATION_SIZE as u64,
                    "relocations",
                )?;
                let first = &data[at];
                let total = u32_at(first, 0);
                let count = total.checked_sub(1).ok_or(Error::new(
                    ErrorKind::Malformed,
                    u64::from(r.relocations),
                    "extended relocation count",
                ))?;
                (count, Some((u32_at(first, 4), u16_at(first, 8))))
            } else {
                (u32::from(r.relocation_count), None)
            };
            placements.push(Placement {
                data: r.data,
                relocations: r.relocations,
                line_numbers: r.line_numbers,
                size: r.size,
                uninitialized,
                relocation_count,
                line_count: u32::from(r.line_count),
                extended,
            });
            sections.push(Section {
                name,
                virtual_size: r.virtual_size,
                virtual_address: r.virtual_address,
                characteristics: r.characteristics,
                data: section_data,
                relocations: Vec::new(),
                line_numbers: Vec::with_capacity(usize::from(r.line_count)),
            });
        }
        lines.sort_unstable();

        // The symbols' primary records, and which record is whose.
        let total = h.symbols as usize;
        let mut ids = alloc::vec![u32::MAX; total];
        let mut primaries: Vec<(usize, u8)> = Vec::new();
        let mut i = 0;
        while i < total {
            let r = &records[i * stride..(i + 1) * stride];
            let aux = usize::from(r[stride - 1]);
            if i + aux >= total {
                return Err(Error::new(
                    ErrorKind::Malformed,
                    (symbol_range.start + i * stride) as u64,
                    "auxiliary records past the symbol table",
                ));
            }
            ids[i] = primaries.len() as u32;
            primaries.push((i, r[stride - 1]));
            i += 1 + aux;
        }
        let resolver = Resolver {
            ids: &ids,
            sections: h.sections,
            lines: &lines,
        };
        let mut symbols = Vec::with_capacity(primaries.len());
        for &(index, aux_count) in &primaries {
            let at = symbol_range.start + index * stride;
            let r = &data[at..at + stride];
            let mut field = [0u8; 8];
            field.copy_from_slice(&r[..8]);
            let name = if field[..4] == [0; 4] {
                let offset = u32_at(r, 4);
                let name = strings
                    .and_then(|t| strings::get(t, offset))
                    .or_else(|| (offset == 0).then_some(&[][..]))
                    .ok_or(Error::new(
                        ErrorKind::Malformed,
                        at as u64,
                        "symbol name offset",
                    ))?;
                if offset != 0 {
                    style.symbols.push((offset, name.len()));
                }
                name
            } else {
                inline_name(&r[..8])
            };
            let number = match format {
                Format::Regular => {
                    let n = u16_at(r, 12);
                    if u32::from(n) <= MAX_REGULAR_SECTIONS {
                        i32::from(n)
                    } else {
                        i32::from(n as i16)
                    }
                }
                Format::BigObj => u32_at(r, 12) as i32,
            };
            let section = match number {
                0 => SectionNumber::Undefined,
                -1 => SectionNumber::Absolute,
                -2 => SectionNumber::Debug,
                n if n > 0 && n as u32 <= h.sections => {
                    SectionNumber::Section(SectionId(n as u32 - 1))
                }
                n => SectionNumber::Other(n),
            };
            let primary = Primary {
                value: u32_at(r, 8),
                section,
                kind: u16_at(r, stride - 4),
                storage_class: r[stride - 2],
            };
            let aux_count = usize::from(aux_count);
            let aux_bytes = &data[at + stride..at + stride * (1 + aux_count)];
            let aux = auxiliary::decode(&primary, aux_bytes, aux_count, format, &resolver);
            symbols.push(Symbol {
                name: Name::stored(name, field),
                value: primary.value,
                section: primary.section,
                kind: primary.kind,
                storage_class: primary.storage_class,
                aux,
            });
        }

        // Relocations and line numbers, which name symbols.
        let mut skipped = false;
        for (section, (r, p)) in sections.iter_mut().zip(raw.iter().zip(&placements)) {
            if p.relocation_count == 0 {
                read_line_numbers(section, r, data, &resolver);
                continue;
            }
            let first = u64::from(r.relocations)
                + if p.extended.is_some() {
                    RELOCATION_SIZE as u64
                } else {
                    0
                };
            let at = range(
                data,
                first,
                u64::from(p.relocation_count) * RELOCATION_SIZE as u64,
                "relocations",
            )?;
            if !relocations(section.name.as_bytes()) {
                skipped = true;
                read_line_numbers(section, r, data, &resolver);
                continue;
            }
            section
                .relocations
                .reserve_exact(p.relocation_count as usize);
            for (k, rel) in data[at.clone()]
                .as_chunks::<RELOCATION_SIZE>()
                .0
                .iter()
                .enumerate()
            {
                let symbol = resolver.symbol(u32_at(rel, 4)).ok_or(Error::new(
                    ErrorKind::Malformed,
                    (at.start + k * RELOCATION_SIZE) as u64,
                    "relocation's symbol index",
                ))?;
                section.relocations.push(Relocation {
                    offset: u32_at(rel, 0),
                    symbol,
                    kind: u16_at(rel, 8),
                });
            }
            read_line_numbers(section, r, data, &resolver);
        }

        // (Not for an object read without some relocations: it isn't
        // written back as it was.)
        let layout = (data.len() <= u32::MAX as usize && !skipped).then(|| {
            let fill = fill(
                data,
                &h,
                &placements,
                &symbol_range,
                strings.map_or(0, <[u8]>::len),
            );
            Layout {
                format,
                optional_header: h.optional_header.len(),
                sections: placements,
                symbol_table: h.symbol_table,
                symbol_records: h.symbols,
                strings: strings.map(Cow::Borrowed),
                // A file the reader accepted is addressed by 32-bit offsets,
                // but may run past 4 GiB; such a file is not laid out again.
                len: u32::try_from(data.len()).unwrap_or(u32::MAX),
                fill,
            }
        });
        Ok(Object {
            format,
            machine: h.machine,
            time_date_stamp: h.time_date_stamp,
            characteristics: h.characteristics,
            optional_header: Cow::Borrowed(&data[h.optional_header]),
            bigobj: h.bigobj,
            sections,
            symbols,
            style: if style.llvm() {
                Style::Llvm
            } else {
                Style::Msvc
            },
            layout,
        })
    }
}

/// Reads a section's line numbers (whose range has been checked).
fn read_line_numbers(
    section: &mut Section<'_>,
    r: &RawSection,
    data: &[u8],
    resolver: &Resolver<'_>,
) {
    let at = r.line_numbers as usize;
    let count = usize::from(r.line_count);
    if count == 0 {
        return;
    }
    for rec in data[at..at + count * LINE_NUMBER_SIZE]
        .as_chunks::<LINE_NUMBER_SIZE>()
        .0
    {
        let first = u32_at(rec, 0);
        let number = u16_at(rec, 4);
        section
            .line_numbers
            .push(match (number, resolver.symbol(first)) {
                (0, Some(symbol)) => LineNumber::Function(symbol),
                _ => LineNumber::Line {
                    address: first,
                    number,
                },
            });
    }
}

/// The bytes outside every part that aren't all zero.
fn fill<'data>(
    data: &'data [u8],
    h: &Header,
    placements: &[Placement],
    symbols: &core::ops::Range<usize>,
    strings: usize,
) -> Vec<(u32, Cow<'data, [u8]>)> {
    let mut parts: Vec<(usize, usize)> = Vec::with_capacity(placements.len() * 2 + 3);
    parts.push((0, h.section_table + placements.len() * SECTION_HEADER_SIZE));
    for p in placements {
        if !p.uninitialized && p.size > 0 {
            parts.push((p.data as usize, p.data as usize + p.size as usize));
        }
        let relocations = p.relocation_count as usize + usize::from(p.extended.is_some());
        if relocations > 0 {
            let at = p.relocations as usize;
            parts.push((at, at + relocations * RELOCATION_SIZE));
        }
        if p.line_count > 0 {
            let at = p.line_numbers as usize;
            parts.push((at, at + p.line_count as usize * LINE_NUMBER_SIZE));
        }
    }
    parts.push((symbols.start, symbols.end));
    parts.push((symbols.end, symbols.end + strings));
    parts.sort_unstable();
    let mut fill = Vec::new();
    let mut gap = |from: usize, to: usize| {
        let bytes = &data[from..to];
        if bytes.iter().any(|&b| b != 0) {
            fill.push((from as u32, Cow::Borrowed(bytes)));
        }
    };
    let mut at = 0;
    for (start, end) in parts {
        if start > at {
            gap(at, start);
        }
        at = at.max(end);
    }
    if at < data.len() {
        gap(at, data.len());
    }
    fill
}

#[cfg(test)]
mod tests;
