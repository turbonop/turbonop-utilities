//! Writing an object: in the layout it was read with, if it still fits,
//! and otherwise afresh.

use alloc::vec::Vec;

use turbonop_core::{Error, ErrorKind, Result};
use turbonop_pe::consts::section_characteristics as scn;

use crate::auxiliary::{self, Place};
use crate::kind::BIGOBJ_CLASS_ID;
use crate::layout::{Layout, Placement};
use crate::object::{
    Aux, Format, LineNumber, LineNumberRef, Name, Object, SectionData, SectionId, SectionNumber,
    Style, SymbolId,
};
use crate::read::{LINE_NUMBER_SIZE, MAX_REGULAR_SECTIONS, RELOCATION_SIZE, SECTION_HEADER_SIZE};
use crate::strings::{self, Request};

fn invalid(what: &'static str) -> Error {
    Error::new(ErrorKind::Malformed, 0, what)
}

fn too_big(what: &'static str) -> Error {
    Error::new(ErrorKind::Overflow, 0, what)
}

/// How a name is written: its field as read, a fresh inline field, or a
/// string-table request.
enum NameField {
    Field([u8; 8]),
    /// A request in the string table (its index), and the field as read if
    /// it may be kept (it is, if the request gets the same offset).
    Table(usize, Option<[u8; 8]>),
}

/// Where the object's parts go.
struct Plan {
    sections: Vec<Placement>,
    symbol_table: u32,
    len: usize,
}

impl Object<'_> {
    /// The object as a file.
    pub fn write(&self) -> Result<Vec<u8>> {
        let mut out = Vec::new();
        self.write_to(&mut out)?;
        Ok(out)
    }

    /// Appends the object, as a file, to `out`.
    ///
    /// The object is laid out as it was read if every part still has its
    /// size then (and the string table still holds every name it did), and
    /// otherwise afresh in its [`Style`](crate::Style). Fails if the
    /// object can't be written: references to sections or symbols that
    /// don't exist, a name with a NUL, more sections than the format
    /// allows, a header field the format lacks, or a file past 4 GiB.
    pub fn write_to(&self, out: &mut Vec<u8>) -> Result<()> {
        self.check()?;
        let format = self.format;
        let stride = format.record_size();

        // Each symbol's table index.
        let mut indices = Vec::with_capacity(self.symbols.len());
        let mut records = 0usize;
        for s in &self.symbols {
            indices.push(u32::try_from(records).map_err(|_| too_big("symbol table"))?);
            records += 1 + s.aux.count(stride);
        }
        let records = u32::try_from(records).map_err(|_| too_big("symbol table"))?;

        // The names, and the string table.
        let recorded = self.layout.as_ref().and_then(|l| l.strings.as_deref());
        let mut requests = Vec::new();
        // MSVC's order: symbols' names, then sections'.
        let symbol_names: Vec<NameField> = self
            .symbols
            .iter()
            .map(|s| symbol_name(&s.name, recorded, &mut requests))
            .collect();
        let section_names: Vec<NameField> = self
            .sections
            .iter()
            .map(|s| section_name(&s.name, recorded, &mut requests))
            .collect();
        let (table, offsets) = strings::build(&requests, recorded, self.style);
        // With no table as read and no long names, a recorded layout that
        // had no string table keeps none.
        let fits = self
            .layout
            .as_ref()
            .filter(|l| self.fits(l, records, &table, &requests));
        let table: &[u8] = match fits {
            Some(l) if l.strings.is_none() => &[],
            _ => &table,
        };

        let plan = match fits {
            Some(l) => Plan {
                sections: l.sections.clone(),
                symbol_table: l.symbol_table,
                len: l.len as usize,
            },
            None => self.fresh_plan(records, table.len())?,
        };

        let start = out.len();
        out.resize(start + plan.len, 0);
        let file = &mut out[start..];
        self.write_header(file, &plan, records);
        let put16 = |b: &mut [u8], o: usize, v: u16| b[o..o + 2].copy_from_slice(&v.to_le_bytes());
        let put32 = |b: &mut [u8], o: usize, v: u32| b[o..o + 4].copy_from_slice(&v.to_le_bytes());

        // The section table, data, relocations, and line numbers.
        let table_at = self.section_table_offset();
        for (i, (s, p)) in self.sections.iter().zip(&plan.sections).enumerate() {
            let h = &mut file[table_at + i * SECTION_HEADER_SIZE..][..SECTION_HEADER_SIZE];
            let field = match section_names[i] {
                NameField::Field(f) => f,
                NameField::Table(r, kept) => match kept {
                    Some(f) if strings::section_name_offset(&f) == Some(offsets[r]) => f,
                    _ => strings::section_name_field(offsets[r]),
                },
            };
            h[..8].copy_from_slice(&field);
            put32(h, 8, s.virtual_size);
            put32(h, 12, s.virtual_address);
            put32(h, 16, s.size());
            put32(h, 20, p.data);
            put32(h, 24, p.relocations);
            put32(h, 28, p.line_numbers);
            let extended = p.extended.is_some();
            // Counts were checked against the format (and a recorded
            // layout's counts are the counts).
            put16(
                h,
                32,
                if extended {
                    0xFFFF
                } else {
                    s.relocations.len() as u16
                },
            );
            put16(h, 34, s.line_numbers.len() as u16);
            let mut characteristics = s.characteristics;
            if fits.is_none() {
                characteristics &= !scn::LNK_NRELOC_OVFL;
                if extended {
                    characteristics |= scn::LNK_NRELOC_OVFL;
                }
            }
            put32(h, 36, characteristics);

            if let SectionData::Bytes(bytes) = &s.data
                && !bytes.is_empty()
            {
                file[p.data as usize..][..bytes.len()].copy_from_slice(bytes);
            }
            let mut at = p.relocations as usize;
            if let Some((symbol, kind)) = p.extended {
                let r = &mut file[at..at + RELOCATION_SIZE];
                put32(r, 0, s.relocations.len() as u32 + 1);
                put32(r, 4, symbol);
                put16(r, 8, kind);
                at += RELOCATION_SIZE;
            }
            for rel in &s.relocations {
                let r = &mut file[at..at + RELOCATION_SIZE];
                put32(r, 0, rel.offset);
                put32(r, 4, indices[rel.symbol.index()]);
                put16(r, 8, rel.kind);
                at += RELOCATION_SIZE;
            }
            let mut at = p.line_numbers as usize;
            for line in &s.line_numbers {
                let r = &mut file[at..at + LINE_NUMBER_SIZE];
                match *line {
                    LineNumber::Function(symbol) => put32(r, 0, indices[symbol.index()]),
                    LineNumber::Line { address, number } => {
                        put32(r, 0, address);
                        put16(r, 4, number);
                    }
                }
                at += LINE_NUMBER_SIZE;
            }
        }

        // The symbol table.
        let place = Placer {
            indices: &indices,
            sections: &plan.sections,
        };
        let mut at = plan.symbol_table as usize;
        for (s, name) in self.symbols.iter().zip(&symbol_names) {
            let r = &mut file[at..at + stride];
            match *name {
                NameField::Field(f) => r[..8].copy_from_slice(&f),
                NameField::Table(i, _) => put32(r, 4, offsets[i]),
            }
            put32(r, 8, s.value);
            match format {
                // The number was checked to fit.
                Format::Regular => put16(r, 12, s.section.raw() as u16),
                Format::BigObj => put32(r, 12, s.section.raw() as u32),
            }
            put16(r, stride - 4, s.kind);
            r[stride - 2] = s.storage_class;
            let count = s.aux.count(stride);
            r[stride - 1] = count as u8;
            at += stride;
            auxiliary::encode(&s.aux, format, &place, &mut file[at..at + count * stride]);
            at += count * stride;
        }
        file[at..at + table.len()].copy_from_slice(table);

        if let Some(l) = fits {
            for (at, bytes) in &l.fill {
                file[*at as usize..][..bytes.len()].copy_from_slice(bytes);
            }
        }
        Ok(())
    }

    fn section_table_offset(&self) -> usize {
        self.format.header_size() + self.optional_header.len()
    }

    /// Whether the object still fits `layout`: every part is the size it
    /// was, and the string table is the one read.
    fn fits(
        &self,
        layout: &Layout<'_>,
        records: u32,
        table: &[u8],
        requests: &[Request<'_>],
    ) -> bool {
        let strings_fit = match &layout.strings {
            Some(s) => s.len() == table.len(),
            None => requests.is_empty(),
        };
        layout.format == self.format
            && layout.optional_header == self.optional_header.len()
            && layout.sections.len() == self.sections.len()
            && layout.symbol_records == records
            && strings_fit
            && self.sections.iter().zip(&layout.sections).all(|(s, p)| {
                p.size == s.size()
                    && p.uninitialized == matches!(s.data, SectionData::Uninitialized(_))
                    && p.relocation_count as usize == s.relocations.len()
                    && p.line_count as usize == s.line_numbers.len()
            })
    }

    /// A fresh layout: the headers, then each section's data, relocations,
    /// and line numbers, then the symbol and string tables.
    fn fresh_plan(&self, records: u32, strings: usize) -> Result<Plan> {
        let mut at =
            (self.section_table_offset() + self.sections.len() * SECTION_HEADER_SIZE) as u64;
        let offset = |at: u64| u32::try_from(at).map_err(|_| too_big("object past 4 GiB"));
        let mut sections = Vec::with_capacity(self.sections.len());
        for s in &self.sections {
            let data = match &s.data {
                SectionData::Bytes(b) if !b.is_empty() => true,
                SectionData::Bytes(_) => {
                    self.style == Style::Llvm
                        && s.characteristics & scn::CNT_UNINITIALIZED_DATA == 0
                }
                SectionData::Uninitialized(_) => false,
            };
            let data_at = if data { offset(at)? } else { 0 };
            at += s.bytes().len() as u64;
            let extended = s.needs_extended_relocations();
            let relocations = if s.relocations.is_empty() {
                0
            } else {
                offset(at)?
            };
            at += (s.relocations.len() + usize::from(extended)) as u64 * RELOCATION_SIZE as u64;
            let line_numbers = if s.line_numbers.is_empty() {
                0
            } else {
                offset(at)?
            };
            at += s.line_numbers.len() as u64 * LINE_NUMBER_SIZE as u64;
            sections.push(Placement {
                data: data_at,
                relocations,
                line_numbers,
                size: s.size(),
                uninitialized: matches!(s.data, SectionData::Uninitialized(_)),
                relocation_count: s.relocations.len() as u32,
                line_count: s.line_numbers.len() as u32,
                extended: extended.then_some((0, 0)),
            });
        }
        let symbol_table = offset(at)?;
        at += u64::from(records) * self.format.record_size() as u64;
        at += strings as u64;
        offset(at)?;
        Ok(Plan {
            sections,
            symbol_table,
            len: at as usize,
        })
    }

    fn write_header(&self, file: &mut [u8], plan: &Plan, records: u32) {
        let put16 = |b: &mut [u8], o: usize, v: u16| b[o..o + 2].copy_from_slice(&v.to_le_bytes());
        let put32 = |b: &mut [u8], o: usize, v: u32| b[o..o + 4].copy_from_slice(&v.to_le_bytes());
        // Counts were checked to fit.
        let sections = self.sections.len() as u32;
        match self.format {
            Format::Regular => {
                put16(file, 0, self.machine);
                put16(file, 2, sections as u16);
                put32(file, 4, self.time_date_stamp);
                put32(file, 8, plan.symbol_table);
                put32(file, 12, records);
                put16(file, 16, self.optional_header.len() as u16);
                put16(file, 18, self.characteristics);
                file[20..20 + self.optional_header.len()].copy_from_slice(&self.optional_header);
            }
            Format::BigObj => {
                put16(file, 0, 0);
                put16(file, 2, 0xFFFF);
                put16(file, 4, self.bigobj.version);
                put16(file, 6, self.machine);
                put32(file, 8, self.time_date_stamp);
                file[12..28].copy_from_slice(&BIGOBJ_CLASS_ID);
                put32(file, 28, self.bigobj.size_of_data);
                put32(file, 32, self.bigobj.flags);
                put32(file, 36, self.bigobj.meta_data_size);
                put32(file, 40, self.bigobj.meta_data_offset);
                put32(file, 44, sections);
                put32(file, 48, plan.symbol_table);
                put32(file, 52, records);
            }
        }
    }

    /// Checks that the object can be written: that its references name
    /// sections and symbols that exist, and that it fits its format.
    fn check(&self) -> Result<()> {
        let format = self.format;
        let stride = format.record_size();
        let sections = self.sections.len();
        let max_sections = match format {
            Format::Regular => MAX_REGULAR_SECTIONS as usize,
            Format::BigObj => i32::MAX as usize,
        };
        if sections > max_sections {
            return Err(too_big("more sections than the format allows"));
        }
        match format {
            Format::Regular => {
                if self.optional_header.len() > usize::from(u16::MAX) {
                    return Err(too_big("optional header"));
                }
            }
            Format::BigObj => {
                if self.characteristics != 0 || !self.optional_header.is_empty() {
                    return Err(Error::new(
                        ErrorKind::Unsupported,
                        0,
                        "a big-obj header has no characteristics or optional header",
                    ));
                }
            }
        }
        let section = |id: SectionId| {
            (id.index() < sections)
                .then_some(())
                .ok_or(invalid("a reference to a section that doesn't exist"))
        };
        let symbol = |id: SymbolId| {
            (id.index() < self.symbols.len())
                .then_some(())
                .ok_or(invalid("a reference to a symbol that doesn't exist"))
        };
        let lines = |at: LineNumberRef| {
            section(at.section)?;
            ((at.index as usize) < self.sections[at.section.index()].line_numbers.len())
                .then_some(())
                .ok_or(invalid("a reference to a line number that doesn't exist"))
        };
        for s in &self.sections {
            check_name(&s.name)?;
            if s.bytes().len() > u32::MAX as usize {
                return Err(too_big("section data"));
            }
            if s.relocations.len() >= u32::MAX as usize {
                return Err(too_big("relocations"));
            }
            if s.line_numbers.len() > usize::from(u16::MAX) {
                return Err(too_big("line numbers"));
            }
            for r in &s.relocations {
                symbol(r.symbol)?;
            }
            for l in &s.line_numbers {
                if let LineNumber::Function(id) = *l {
                    symbol(id)?;
                }
            }
        }
        for s in &self.symbols {
            check_name(&s.name)?;
            match s.section {
                SectionNumber::Section(id) => section(id)?,
                SectionNumber::Other(n) => {
                    let fits = match format {
                        Format::Regular => (-256..=MAX_REGULAR_SECTIONS as i32).contains(&n),
                        Format::BigObj => true,
                    };
                    if !fits {
                        return Err(invalid("a section number the format can't hold"));
                    }
                }
                _ => {}
            }
            if s.aux.count(stride) > usize::from(u8::MAX) {
                return Err(too_big("auxiliary records"));
            }
            match &s.aux {
                Aux::None => {}
                Aux::Function(f) => {
                    f.tag.map_or(Ok(()), symbol)?;
                    f.next.map_or(Ok(()), symbol)?;
                    f.line_numbers.map_or(Ok(()), lines)?;
                }
                Aux::BeginEnd(b) => b.next.map_or(Ok(()), symbol)?,
                Aux::WeakExternal(w) => symbol(w.tag)?,
                Aux::File(name) => {
                    if name.contains(&0) {
                        return Err(invalid("a file name with a NUL"));
                    }
                }
                Aux::Section(d) => d.number.map_or(Ok(()), section)?,
                Aux::ClrToken(c) => symbol(c.symbol)?,
                Aux::Raw(bytes) => {
                    if bytes.len() % stride != 0 {
                        return Err(invalid(
                            "auxiliary records that aren't a whole number of records",
                        ));
                    }
                }
            }
        }
        Ok(())
    }
}

fn check_name(name: &Name<'_>) -> Result<()> {
    if name.as_bytes().contains(&0) {
        return Err(invalid("a name with a NUL"));
    }
    if name.as_bytes().len() > u32::MAX as usize / 2 {
        return Err(too_big("a name"));
    }
    Ok(())
}

/// How a section's name is written. A field as read is kept if it still
/// gives the name: inline, or through the string table as read.
fn section_name<'a>(
    name: &'a Name<'_>,
    recorded: Option<&[u8]>,
    requests: &mut Vec<Request<'a>>,
) -> NameField {
    let bytes = name.as_bytes();
    if let Some(field) = name.stored {
        match crate::read::section_name(&field, recorded) {
            Some(long) if long == bytes => {
                requests.push(Request {
                    name: bytes,
                    hint: strings::section_name_offset(&field),
                });
                return NameField::Table(requests.len() - 1, Some(field));
            }
            Some(_) => {}
            // An inline name that looks like a long one is kept only with
            // the table it was read with (where it gave no name).
            None if strings::section_name_offset(&field).is_none() || recorded.is_some() => {
                let len = field.iter().position(|&b| b == 0).unwrap_or(8);
                if &field[..len] == bytes {
                    return NameField::Field(field);
                }
            }
            None => {}
        }
    }
    if bytes.len() <= 8 && strings::section_name_offset(&inline_field(bytes)).is_none() {
        NameField::Field(inline_field(bytes))
    } else {
        requests.push(Request {
            name: bytes,
            hint: None,
        });
        NameField::Table(requests.len() - 1, None)
    }
}

/// How a symbol's name is written. A field as read is kept if it still
/// gives the name.
fn symbol_name<'a>(
    name: &'a Name<'_>,
    recorded: Option<&[u8]>,
    requests: &mut Vec<Request<'a>>,
) -> NameField {
    let bytes = name.as_bytes();
    if let Some(field) = name.stored {
        if field[..4] == [0; 4] {
            let offset = u32::from_le_bytes([field[4], field[5], field[6], field[7]]);
            if offset == 0 && bytes.is_empty() {
                return NameField::Field(field);
            }
            if recorded.and_then(|t| strings::get(t, offset)) == Some(bytes) {
                requests.push(Request {
                    name: bytes,
                    hint: Some(offset),
                });
                return NameField::Table(requests.len() - 1, Some(field));
            }
        } else {
            let len = field.iter().position(|&b| b == 0).unwrap_or(8);
            if &field[..len] == bytes {
                return NameField::Field(field);
            }
        }
    }
    if bytes.len() <= 8 {
        NameField::Field(inline_field(bytes))
    } else {
        requests.push(Request {
            name: bytes,
            hint: None,
        });
        NameField::Table(requests.len() - 1, None)
    }
}

fn inline_field(bytes: &[u8]) -> [u8; 8] {
    let mut field = [0u8; 8];
    field[..bytes.len()].copy_from_slice(bytes);
    field
}

struct Placer<'a> {
    indices: &'a [u32],
    sections: &'a [Placement],
}

impl Place for Placer<'_> {
    fn symbol(&self, id: SymbolId) -> u32 {
        self.indices[id.index()]
    }

    fn line_number(&self, at: LineNumberRef) -> u32 {
        self.sections[at.section.index()].line_numbers + at.index * LINE_NUMBER_SIZE as u32
    }
}

#[cfg(test)]
mod tests;
