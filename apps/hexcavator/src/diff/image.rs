//! Comparing two PE images (see `diff`'s module documentation).

use turbonop_pe::{PeFile, consts::machine};

use super::symbols::{self, Names};
use super::{Options, Part, Report};

/// An image read for comparing.
struct Side<'d> {
    data: &'d [u8],
    pe: PeFile<'d>,
    /// The names both images have (to describe addresses alike).
    names: Names,
    /// Every name this image has.
    all: Names,
    base: u64,
    /// (RVA, virtual size, file offset, raw size, name) of each section.
    sections: Vec<(u32, u32, u32, u32, String)>,
    /// The optional header's offset in the file.
    optional: usize,
    /// Each function's length, by its start, from the exception data
    /// (its first part's, for a function split in parts).
    lengths: turbonop_hash::Map<u32, u32>,
    /// Every place something starts (each of its names, each function the
    /// exception data has), sorted: no name's extent runs past the next.
    starts: Vec<u32>,
    is64: bool,
}

impl<'d> Side<'d> {
    fn open(
        data: &'d [u8],
        path: &std::path::Path,
        debug: Option<&std::path::Path>,
        map: Option<&std::path::Path>,
    ) -> Result<Side<'d>, String> {
        let pe = PeFile::parse_image(data).map_err(|e| format!("{}: {e}", path.display()))?;
        let opt = pe
            .optional_header()
            .ok_or_else(|| format!("{}: no optional header", path.display()))?;
        let base = opt.image_base().0;
        let sections: Vec<(u32, u32, u32, u32, String)> = pe
            .sections()
            .iter()
            .map(|s| {
                (
                    s.virtual_address.get(),
                    s.virtual_size.get(),
                    s.pointer_to_raw_data.get(),
                    s.size_of_raw_data.get(),
                    String::from_utf8_lossy(pe.section_name(s)).into_owned(),
                )
            })
            .collect();
        let spans: Vec<(u32, u32)> = sections.iter().map(|s| (s.0, s.1.max(s.3))).collect();
        let names = symbols::load(path, debug, map, base, &spans)?;
        let optional = usize::try_from(pe.file_header_offset()).unwrap_or(0) + 20;
        let mut lengths = turbonop_hash::Map::default();
        for e in pe.runtime_functions() {
            if let turbonop_pe::exception::RuntimeFunction::X64 { begin, end, .. } = e.function {
                lengths
                    .entry(begin.0)
                    .or_insert(end.0.saturating_sub(begin.0));
            }
        }
        Ok(Side {
            data,
            is64: opt.is_64(),
            pe,
            all: names.clone(),
            names,
            base,
            sections,
            optional,
            starts: Vec::new(),
            lengths,
        })
    }

    /// How far a name at `rva` of size `size` (perhaps only a guess) can
    /// extend: no further than the next thing that starts.
    fn extent(&self, rva: u32, size: u32) -> u32 {
        if let Some(&n) = self.lengths.get(&rva) {
            return n;
        }
        let i = self.starts.partition_point(|&s| s <= rva);
        match self.starts.get(i) {
            Some(&next) => size.min(next - rva),
            None => size,
        }
    }

    /// The RVA of file offset `off`, if it is in a section's raw data.
    fn rva_of_offset(&self, off: usize) -> Option<(usize, u32)> {
        let off = u32::try_from(off).ok()?;
        self.sections.iter().enumerate().find_map(|(i, s)| {
            (s.3 > 0 && off >= s.2 && off < s.2.saturating_add(s.3)).then(|| (i, s.0 + (off - s.2)))
        })
    }

    /// The bytes at `rva`, `len` of them (as many as the file has).
    fn bytes(&self, rva: u32, len: u32) -> &'d [u8] {
        for s in &self.sections {
            if rva >= s.0 && rva < s.0.saturating_add(s.3) {
                let start = (s.2 + (rva - s.0)) as usize;
                let avail = (s.3 - (rva - s.0)).min(len) as usize;
                return self.data.get(start..start + avail).unwrap_or(&[]);
            }
        }
        &[]
    }

    fn u16_at(&self, off: usize) -> Option<u16> {
        self.data
            .get(off..off + 2)
            .map(|b| u16::from_le_bytes([b[0], b[1]]))
    }

    fn u32_at(&self, off: usize) -> Option<u32> {
        self.data
            .get(off..off + 4)
            .map(|b| u32::from_le_bytes([b[0], b[1], b[2], b[3]]))
    }

    fn u64_at(&self, off: usize) -> Option<u64> {
        self.data
            .get(off..off + 8)
            .and_then(|b| b.try_into().ok())
            .map(u64::from_le_bytes)
    }

    fn value(&self, off: usize, width: u8) -> Option<u64> {
        match width {
            1 => self.data.get(off).map(|&b| u64::from(b)),
            2 => self.u16_at(off).map(u64::from),
            4 => self.u32_at(off).map(u64::from),
            _ => self.u64_at(off),
        }
    }

    /// Data directory `i`: (RVA, size).
    fn directory(&self, i: usize) -> (u32, u32) {
        self.pe
            .data_directory(i)
            .map_or((0, 0), |d| (d.virtual_address.get(), d.size.get()))
    }

    /// A VA as its RVA, described.
    fn describe_va(&self, va: u64) -> String {
        if va == 0 {
            return "0".into();
        }
        match va
            .checked_sub(self.base)
            .and_then(|r| u32::try_from(r).ok())
        {
            Some(rva) => self.names.describe(rva),
            None => format!("{va:#x}"),
        }
    }
}

/// The file header's fields: (name, offset from the file header, width).
const FILE_HEADER: &[(&str, usize, u8)] = &[
    ("Machine", 0, 2),
    ("NumberOfSections", 2, 2),
    ("TimeDateStamp", 4, 4),
    ("PointerToSymbolTable", 8, 4),
    ("NumberOfSymbols", 12, 4),
    ("SizeOfOptionalHeader", 16, 2),
    ("Characteristics", 18, 2),
];

/// The optional header's fields: (name, PE32 offset, PE32+ offset,
/// PE32 width, PE32+ width); `usize::MAX` for a field one lacks.
const OPTIONAL: &[(&str, usize, usize, u8, u8)] = &[
    ("Magic", 0, 0, 2, 2),
    ("MajorLinkerVersion", 2, 2, 1, 1),
    ("MinorLinkerVersion", 3, 3, 1, 1),
    ("SizeOfCode", 4, 4, 4, 4),
    ("SizeOfInitializedData", 8, 8, 4, 4),
    ("SizeOfUninitializedData", 12, 12, 4, 4),
    ("AddressOfEntryPoint", 16, 16, 4, 4),
    ("BaseOfCode", 20, 20, 4, 4),
    ("BaseOfData", 24, usize::MAX, 4, 0),
    ("ImageBase", 28, 24, 4, 8),
    ("SectionAlignment", 32, 32, 4, 4),
    ("FileAlignment", 36, 36, 4, 4),
    ("MajorOperatingSystemVersion", 40, 40, 2, 2),
    ("MinorOperatingSystemVersion", 42, 42, 2, 2),
    ("MajorImageVersion", 44, 44, 2, 2),
    ("MinorImageVersion", 46, 46, 2, 2),
    ("MajorSubsystemVersion", 48, 48, 2, 2),
    ("MinorSubsystemVersion", 50, 50, 2, 2),
    ("Win32VersionValue", 52, 52, 4, 4),
    ("SizeOfImage", 56, 56, 4, 4),
    ("SizeOfHeaders", 60, 60, 4, 4),
    ("CheckSum", 64, 64, 4, 4),
    ("Subsystem", 68, 68, 2, 2),
    ("DllCharacteristics", 70, 70, 2, 2),
    ("SizeOfStackReserve", 72, 72, 4, 8),
    ("SizeOfStackCommit", 76, 80, 4, 8),
    ("SizeOfHeapReserve", 80, 88, 4, 8),
    ("SizeOfHeapCommit", 84, 96, 4, 8),
    ("LoaderFlags", 88, 104, 4, 4),
    ("NumberOfRvaAndSizes", 92, 108, 4, 4),
];

const DIRECTORIES: [&str; 16] = [
    "Export",
    "Import",
    "Resource",
    "Exception",
    "Certificates",
    "BaseRelocation",
    "Debug",
    "Architecture",
    "GlobalPointer",
    "TLS",
    "LoadConfig",
    "BoundImport",
    "IAT",
    "DelayImport",
    "COMDescriptor",
    "Reserved",
];

const SECTION_FIELDS: &[(&str, usize, u8)] = &[
    ("Name", 0, 8),
    ("VirtualSize", 8, 4),
    ("VirtualAddress", 12, 4),
    ("SizeOfRawData", 16, 4),
    ("PointerToRawData", 20, 4),
    ("PointerToRelocations", 24, 4),
    ("PointerToLinenumbers", 28, 4),
    ("NumberOfRelocations", 32, 2),
    ("NumberOfLinenumbers", 34, 2),
    ("Characteristics", 36, 4),
];

/// What holds file offset `off` of `s`: a header's field, a section
/// header's, or a section's data (and the directory or name there).
fn attribute(s: &Side<'_>, off: usize) -> String {
    let fh = s.optional - 20;
    if off < 0x40 {
        return "DOS header".into();
    }
    if off < fh.saturating_sub(4) {
        return "DOS stub / Rich header".into();
    }
    if off < fh {
        return "PE signature".into();
    }
    if off < s.optional {
        let rel = off - fh;
        for &(name, o, w) in FILE_HEADER {
            if rel >= o && rel < o + usize::from(w) {
                return format!("file header: {name}");
            }
        }
    }
    let dirs_at = s.optional + if s.is64 { 112 } else { 96 };
    if off >= s.optional && off < dirs_at {
        let rel = off - s.optional;
        for &(name, o32, o64, w32, w64) in OPTIONAL {
            let (o, w) = if s.is64 { (o64, w64) } else { (o32, w32) };
            if o != usize::MAX && rel >= o && rel < o + usize::from(w) {
                return format!("optional header: {name}");
            }
        }
        return "optional header".into();
    }
    let sections_at = dirs_at + 8 * 16;
    if off >= dirs_at && off < sections_at {
        let i = (off - dirs_at) / 8;
        let which = if (off - dirs_at) % 8 < 4 {
            "RVA"
        } else {
            "Size"
        };
        return format!(
            "data directory {}: {which}",
            DIRECTORIES.get(i).unwrap_or(&"?")
        );
    }
    let headers_end = sections_at + 40 * s.sections.len();
    if off >= sections_at && off < headers_end {
        let i = (off - sections_at) / 40;
        let rel = (off - sections_at) % 40;
        let field = SECTION_FIELDS
            .iter()
            .find(|&&(_, o, w)| rel >= o && rel < o + usize::from(w))
            .map_or("?", |f| f.0);
        let name = s.sections.get(i).map_or("?", |x| x.4.as_str());
        return format!("section header {} ({name}): {field}", i + 1);
    }
    match s.rva_of_offset(off) {
        Some((i, rva)) => {
            let mut what = format!("{} {}", s.sections[i].4, s.names.describe(rva));
            for (d, name) in DIRECTORIES.iter().enumerate() {
                let (start, size) = s.directory(d);
                if d != 4 && size > 0 && rva >= start && rva < start.saturating_add(size) {
                    what.push_str(&format!(" (in the {name} directory)"));
                }
            }
            what
        }
        None if off < headers_end.max(0x400) => "headers' padding".into(),
        None => "outside every section (padding or overlay)".into(),
    }
}

fn hex(b: &[u8]) -> String {
    b.iter().map(|x| format!("{x:02x}")).collect()
}

/// The bytes: identical, or each differing range and what holds it.
fn bytes(a: &Side<'_>, b: &Side<'_>, o: &Options) -> Part {
    let mut p = Part::new("bytes", "Bytes", o.rows);
    let (x, y) = (a.data, b.data);
    if x == y {
        return p;
    }
    if x.len() != y.len() {
        p.note(format!("sizes: {} | {} bytes", x.len(), y.len()));
    }
    let n = x.len().min(y.len());
    // Ranges of differing bytes, those less than 8 apart joined.
    let mut ranges: Vec<(usize, usize)> = Vec::new();
    let mut i = 0;
    let mut differing = 0usize;
    while i < n {
        if x[i] == y[i] {
            i += 1;
            continue;
        }
        let start = i;
        let mut end = i + 1;
        let mut j = i + 1;
        while j < n && j < end + 8 {
            if x[j] != y[j] {
                end = j + 1;
            }
            j += 1;
        }
        differing += x[start..end]
            .iter()
            .zip(&y[start..end])
            .filter(|(p, q)| p != q)
            .count();
        ranges.push((start, end));
        i = end;
    }
    p.note(format!(
        "{differing} bytes differ in {} ranges (of {n} compared)",
        ranges.len()
    ));
    for (start, end) in ranges {
        p.add_with(|| {
            let show = (end - start).min(16);
            let (wa, wb) = (attribute(a, start), attribute(b, start));
            let at = if wa == wb { wa } else { format!("{wa} | {wb}") };
            format!(
                "{start:#x}..{end:#x} ({} bytes) {at}: {}{} | {}{}",
                end - start,
                hex(&x[start..start + show]),
                if end - start > show { "..." } else { "" },
                hex(&y[start..start + show]),
                if end - start > show { "..." } else { "" },
            )
        });
    }
    if x.len() != y.len() {
        p.add(format!(
            "{} has {} more bytes at the end",
            if x.len() > y.len() { "a" } else { "b" },
            x.len().abs_diff(y.len())
        ));
    }
    p
}

fn headers(a: &Side<'_>, b: &Side<'_>, o: &Options) -> Part {
    let mut p = Part::new("headers", "Headers", o.rows);
    let (fa, fb) = (a.optional - 20, b.optional - 20);
    for &(name, off, w) in FILE_HEADER {
        let (x, y) = (a.value(fa + off, w), b.value(fb + off, w));
        if x != y {
            p.add(format!(
                "file header {name}: {:#x} | {:#x}",
                x.unwrap_or(0),
                y.unwrap_or(0)
            ));
        }
    }
    if a.is64 != b.is64 {
        p.add(format!(
            "optional header: {} | {}",
            if a.is64 { "PE32+" } else { "PE32" },
            if b.is64 { "PE32+" } else { "PE32" }
        ));
        return p;
    }
    for &(name, o32, o64, w32, w64) in OPTIONAL {
        let (off, w) = if a.is64 { (o64, w64) } else { (o32, w32) };
        if off == usize::MAX {
            continue;
        }
        let (x, y) = (a.value(a.optional + off, w), b.value(b.optional + off, w));
        if x != y {
            let (x, y) = (x.unwrap_or(0), y.unwrap_or(0));
            let extra = if name == "AddressOfEntryPoint" {
                let (dx, dy) = (
                    a.names.describe(u32::try_from(x).unwrap_or(0)),
                    b.names.describe(u32::try_from(y).unwrap_or(0)),
                );
                if dx == dy {
                    format!(" (both {dx})")
                } else {
                    format!(" ({dx} | {dy})")
                }
            } else {
                String::new()
            };
            p.add(format!("optional header {name}: {x:#x} | {y:#x}{extra}"));
        }
    }
    // The DOS header and stub, and the Rich header's place, as bytes.
    let (sa, sb) = (
        &a.data[..fa.min(a.data.len())],
        &b.data[..fb.min(b.data.len())],
    );
    if sa != sb {
        p.add(format!(
            "DOS header and stub (with the Rich header): {} | {} bytes, {}",
            sa.len(),
            sb.len(),
            if sa.len() == sb.len() {
                "contents differ"
            } else {
                "sizes differ"
            }
        ));
    }
    p
}

fn sections(a: &Side<'_>, b: &Side<'_>, o: &Options) -> Part {
    let mut p = Part::new("sections", "Section table", o.rows);
    let (na, nb) = (a.sections.len(), b.sections.len());
    if na != nb {
        p.add(format!("count: {na} | {nb}"));
    }
    let list = |s: &Side<'_>| {
        s.sections
            .iter()
            .map(|x| x.4.clone())
            .collect::<Vec<_>>()
            .join(" ")
    };
    if list(a) != list(b) {
        p.add(format!("order: {} | {}", list(a), list(b)));
    }
    let hdr = |s: &Side<'_>| s.optional + if s.is64 { 240 } else { 224 };
    for i in 0..na.min(nb) {
        let (ha, hb) = (hdr(a) + 40 * i, hdr(b) + 40 * i);
        for &(name, off, w) in SECTION_FIELDS {
            if name == "Name" {
                continue;
            }
            let (x, y) = (a.value(ha + off, w), b.value(hb + off, w));
            if x != y {
                p.add(format!(
                    "{} {name}: {:#x} | {:#x}",
                    a.sections[i].4,
                    x.unwrap_or(0),
                    y.unwrap_or(0)
                ));
            }
        }
    }
    // Each section's contents, by name.
    for sa in &a.sections {
        if let Some(sb) = b.sections.iter().find(|x| x.4 == sa.4) {
            let ca = a
                .data
                .get(sa.2 as usize..(sa.2 + sa.3) as usize)
                .unwrap_or(&[]);
            let cb = b
                .data
                .get(sb.2 as usize..(sb.2 + sb.3) as usize)
                .unwrap_or(&[]);
            if ca != cb {
                let same = ca.iter().zip(cb).filter(|(x, y)| x == y).count();
                p.note(format!(
                    "{} contents differ ({} | {} bytes; {} of the first {} the same)",
                    sa.4,
                    ca.len(),
                    cb.len(),
                    same,
                    ca.len().min(cb.len())
                ));
            }
        } else {
            p.add(format!("{} is only in a", sa.4));
        }
    }
    for sb in &b.sections {
        if !a.sections.iter().any(|x| x.4 == sb.4) {
            p.add(format!("{} is only in b", sb.4));
        }
    }
    p
}

fn directories(a: &Side<'_>, b: &Side<'_>, o: &Options) -> Part {
    let mut p = Part::new("directories", "Data directories", o.rows);
    for (i, name) in DIRECTORIES.iter().enumerate() {
        let (x, y) = (a.directory(i), b.directory(i));
        if x != y {
            // (An RVA that names the same thing in both differs only by
            // layout.)
            let (dx, dy) = (a.names.describe(x.0), b.names.describe(y.0));
            let same = if x.0 != 0 && dx == dy && !dx.starts_with("0x") {
                format!(" (both {dx})")
            } else {
                String::new()
            };
            p.add(format!(
                "{name}: {:#x} [{:#x}] | {:#x} [{:#x}]{same}",
                x.0, x.1, y.0, y.1
            ));
        }
    }
    p
}

fn imports(a: &Side<'_>, b: &Side<'_>, o: &Options) -> Part {
    let mut p = Part::new("imports", "Imports", o.rows);
    type Dll = (String, Vec<String>, (u32, u32, u32, u32));
    let read = |s: &Side<'_>, delay: bool| -> Vec<Dll> {
        let name = |t: &turbonop_pe::imports::ImportThunk<'_>| match t.import {
            turbonop_pe::imports::ImportName::Name { hint, name } => {
                format!("{} (hint {hint})", String::from_utf8_lossy(name))
            }
            turbonop_pe::imports::ImportName::Ordinal(n) => format!("#{n}"),
        };
        if delay {
            s.pe.delay_imports()
                .map(|d| {
                    (
                        String::from_utf8_lossy(d.name).into_owned(),
                        d.thunks().map(|t| name(&t)).collect(),
                        (0, 0, 0, 0),
                    )
                })
                .collect()
        } else {
            s.pe.imports()
                .map(|d| {
                    let desc = d.descriptor;
                    (
                        String::from_utf8_lossy(d.name).into_owned(),
                        d.thunks().map(|t| name(&t)).collect(),
                        (
                            desc.import_name_table.get(),
                            desc.import_address_table.get(),
                            desc.time_date_stamp.get(),
                            desc.forwarder_chain.get(),
                        ),
                    )
                })
                .collect()
        }
    };
    for delay in [false, true] {
        let (da, db) = (read(a, delay), read(b, delay));
        let what = if delay { "delay-load " } else { "" };
        let order = |d: &[Dll]| d.iter().map(|x| x.0.clone()).collect::<Vec<_>>().join(", ");
        if order(&da) != order(&db) {
            p.add(format!("{what}DLLs: {} | {}", order(&da), order(&db)));
        }
        for x in &da {
            let Some(y) = db.iter().find(|y| y.0.eq_ignore_ascii_case(&x.0)) else {
                p.add(format!("{what}{} is only in a", x.0));
                continue;
            };
            if x.1 != y.1 {
                let sa: std::collections::BTreeSet<&String> = x.1.iter().collect();
                let sb: std::collections::BTreeSet<&String> = y.1.iter().collect();
                for f in sa.difference(&sb) {
                    p.add(format!("{what}{}: {f} only in a", x.0));
                }
                for f in sb.difference(&sa) {
                    p.add(format!("{what}{}: {f} only in b", x.0));
                }
                if sa == sb {
                    p.add(format!(
                        "{what}{}: the same functions in another order",
                        x.0
                    ));
                }
            }
            let (ta, tb) = (x.2, y.2);
            if ta.2 != tb.2 || ta.3 != tb.3 {
                p.add(format!(
                    "{what}{}: time stamp / forwarder chain {:#x}/{:#x} | {:#x}/{:#x}",
                    x.0, ta.2, ta.3, tb.2, tb.3
                ));
            }
            if (ta.0, ta.1) != (tb.0, tb.1) {
                p.note(format!(
                    "{what}{}: name table at {:#x} | {:#x}, address table at {:#x} | {:#x}",
                    x.0, ta.0, tb.0, ta.1, tb.1
                ));
            }
        }
        for y in &db {
            if !da.iter().any(|x| x.0.eq_ignore_ascii_case(&y.0)) {
                p.add(format!("{what}{} is only in b", y.0));
            }
        }
    }
    p
}

fn exports(a: &Side<'_>, b: &Side<'_>, o: &Options) -> Part {
    let mut p = Part::new("exports", "Exports", o.rows);
    // (the DLL's name, the ordinal base, each export: ordinal, name, target)
    type List = (String, u32, Vec<(u32, String, String)>);
    let read = |s: &Side<'_>| -> Option<List> {
        let e = s.pe.exports()?;
        let list = e
            .iter()
            .map(|x| {
                let target = match x.target {
                    turbonop_pe::exports::ExportTarget::Rva(r) => s.names.describe(r.0),
                    turbonop_pe::exports::ExportTarget::Forwarder(f) => {
                        format!("-> {}", String::from_utf8_lossy(f))
                    }
                };
                (
                    x.ordinal,
                    x.name.map_or_else(
                        || "[NONAME]".into(),
                        |n| String::from_utf8_lossy(n).into_owned(),
                    ),
                    target,
                )
            })
            .collect();
        Some((
            e.dll_name()
                .map(|n| String::from_utf8_lossy(n).into_owned())
                .unwrap_or_default(),
            e.ordinal_base(),
            list,
        ))
    };
    match (read(a), read(b)) {
        (None, None) => {}
        (Some(_), None) => p.add("only a exports"),
        (None, Some(_)) => p.add("only b exports"),
        (Some(x), Some(y)) => {
            p.field("DLL name", &x.0, &y.0);
            p.field("ordinal base", x.1, y.1);
            if x.2.len() != y.2.len() {
                p.add(format!("count: {} | {}", x.2.len(), y.2.len()));
            }
            for e in &x.2 {
                match y.2.iter().find(|f| f.0 == e.0) {
                    None => p.add(format!("#{} {} only in a", e.0, e.1)),
                    Some(f) if f != e => {
                        p.add(format!("#{}: {} = {} | {} = {}", e.0, e.1, e.2, f.1, f.2));
                    }
                    _ => {}
                }
            }
            for f in &y.2 {
                if !x.2.iter().any(|e| e.0 == f.0) {
                    p.add(format!("#{} {} only in b", f.0, f.1));
                }
            }
        }
    }
    p
}

fn relocations(a: &Side<'_>, b: &Side<'_>, o: &Options) -> Part {
    let mut p = Part::new("relocations", "Base relocations", o.rows);
    let read = |s: &Side<'_>| -> Vec<(u32, u8)> {
        s.pe.base_relocs()
            .filter(|r| r.kind != 0)
            .map(|r| (r.rva.0, r.kind))
            .collect()
    };
    let (ra, rb) = (read(a), read(b));
    let blocks = |s: &Side<'_>| s.pe.base_reloc_blocks().count();
    if ra.len() != rb.len() || blocks(a) != blocks(b) {
        p.add(format!(
            "{} relocations in {} pages | {} in {}",
            ra.len(),
            blocks(a),
            rb.len(),
            blocks(b)
        ));
    }
    if ra == rb {
        return p;
    }
    // By what they patch, so that a moved function's are the same.
    let by_name = |s: &Side<'_>, r: &[(u32, u8)]| -> std::collections::BTreeMap<String, usize> {
        let mut m = std::collections::BTreeMap::new();
        for &(rva, kind) in r {
            *m.entry(format!("{} (type {kind})", s.names.describe(rva)))
                .or_insert(0) += 1;
        }
        m
    };
    let (ma, mb) = (by_name(a, &ra), by_name(b, &rb));
    if ma == mb {
        p.note("the same locations (by name), at other addresses");
        return p;
    }
    for (k, &n) in &ma {
        let m = mb.get(k).copied().unwrap_or(0);
        if n != m {
            p.add(format!("{k}: {n} | {m}"));
        }
    }
    for (k, &m) in &mb {
        if !ma.contains_key(k) {
            p.add(format!("{k}: 0 | {m}"));
        }
    }
    p
}

fn exceptions(a: &Side<'_>, b: &Side<'_>, o: &Options) -> Part {
    let mut p = Part::new("exceptions", "Exception data (.pdata)", o.rows);
    type Entry = (String, u32, Vec<u8>);
    let read = |s: &Side<'_>| -> Vec<Entry> {
        s.pe.runtime_functions()
            .map(|e| match e.function {
                turbonop_pe::exception::RuntimeFunction::X64 { begin, end, unwind } => {
                    let info =
                        s.pe.x64_unwind_info(unwind)
                            .map_or(&[][..], |u| s.bytes(unwind.0, u.size()));
                    (
                        s.names.describe(begin.0),
                        end.0.saturating_sub(begin.0),
                        info.to_vec(),
                    )
                }
                turbonop_pe::exception::RuntimeFunction::Arm { begin, unwind } => (
                    s.names.describe(begin.0),
                    0,
                    if unwind & 3 != 0 {
                        unwind.to_le_bytes().to_vec()
                    } else {
                        Vec::new()
                    },
                ),
            })
            .collect()
    };
    let (ea, eb) = (read(a), read(b));
    if ea.len() != eb.len() {
        p.add(format!("entries: {} | {}", ea.len(), eb.len()));
    }
    fn named(e: &[Entry]) -> std::collections::BTreeMap<String, &Entry> {
        e.iter().map(|x| (x.0.clone(), x)).collect()
    }
    let (na, nb) = (named(&ea), named(&eb));
    for (k, x) in &na {
        match nb.get(k) {
            None => p.add(format!("{k}: only in a")),
            Some(y) if x.1 != y.1 => p.add(format!("{k}: length {:#x} | {:#x}", x.1, y.1)),
            // (Unwind codes; a handler's RVA after them may differ by
            // layout alone.)
            Some(y) if x.2.get(..4) != y.2.get(..4) || x.2.len() != y.2.len() => {
                p.add(format!("{k}: unwind info {} | {}", hex(&x.2), hex(&y.2)));
            }
            _ => {}
        }
    }
    for k in nb.keys() {
        if !na.contains_key(k) {
            p.add(format!("{k}: only in b"));
        }
    }
    p
}

fn load_config(a: &Side<'_>, b: &Side<'_>, o: &Options) -> Part {
    use turbonop_pe::load_config::field as f;
    let mut p = Part::new("loadconfig", "Load configuration", o.rows);
    let (ca, cb) = (a.pe.load_config(), b.pe.load_config());
    let (Some(ca), Some(cb)) = (ca, cb) else {
        if ca.is_some() != cb.is_some() {
            p.add(format!("present: {} | {}", ca.is_some(), cb.is_some()));
        }
        return p;
    };
    // (name, field, whether it holds an address)
    let fields: &[(&str, turbonop_pe::load_config::Field, bool)] = &[
        ("Size", f::SIZE, false),
        ("TimeDateStamp", f::TIME_DATE_STAMP, false),
        ("MajorVersion", f::MAJOR_VERSION, false),
        ("MinorVersion", f::MINOR_VERSION, false),
        ("GlobalFlagsClear", f::GLOBAL_FLAGS_CLEAR, false),
        ("GlobalFlagsSet", f::GLOBAL_FLAGS_SET, false),
        (
            "CriticalSectionDefaultTimeout",
            f::CRITICAL_SECTION_DEFAULT_TIMEOUT,
            false,
        ),
        (
            "DeCommitFreeBlockThreshold",
            f::DE_COMMIT_FREE_BLOCK_THRESHOLD,
            false,
        ),
        (
            "DeCommitTotalFreeThreshold",
            f::DE_COMMIT_TOTAL_FREE_THRESHOLD,
            false,
        ),
        ("LockPrefixTable", f::LOCK_PREFIX_TABLE, true),
        ("MaximumAllocationSize", f::MAXIMUM_ALLOCATION_SIZE, false),
        ("VirtualMemoryThreshold", f::VIRTUAL_MEMORY_THRESHOLD, false),
        ("ProcessHeapFlags", f::PROCESS_HEAP_FLAGS, false),
        ("ProcessAffinityMask", f::PROCESS_AFFINITY_MASK, false),
        ("CSDVersion", f::CSD_VERSION, false),
        ("DependentLoadFlags", f::DEPENDENT_LOAD_FLAGS, false),
        ("EditList", f::EDIT_LIST, true),
        ("SecurityCookie", f::SECURITY_COOKIE, true),
        ("SEHandlerTable", f::SE_HANDLER_TABLE, true),
        ("SEHandlerCount", f::SE_HANDLER_COUNT, false),
        (
            "GuardCFCheckFunctionPointer",
            f::GUARD_CF_CHECK_FUNCTION_POINTER,
            true,
        ),
        (
            "GuardCFDispatchFunctionPointer",
            f::GUARD_CF_DISPATCH_FUNCTION_POINTER,
            true,
        ),
        ("GuardCFFunctionTable", f::GUARD_CF_FUNCTION_TABLE, true),
        ("GuardCFFunctionCount", f::GUARD_CF_FUNCTION_COUNT, false),
        ("GuardFlags", f::GUARD_FLAGS, false),
        ("CodeIntegrityFlags", f::CODE_INTEGRITY_FLAGS, false),
        ("CodeIntegrityCatalog", f::CODE_INTEGRITY_CATALOG, false),
        (
            "CodeIntegrityCatalogOffset",
            f::CODE_INTEGRITY_CATALOG_OFFSET,
            false,
        ),
        (
            "GuardAddressTakenIatEntryTable",
            f::GUARD_ADDRESS_TAKEN_IAT_ENTRY_TABLE,
            true,
        ),
        (
            "GuardAddressTakenIatEntryCount",
            f::GUARD_ADDRESS_TAKEN_IAT_ENTRY_COUNT,
            false,
        ),
        (
            "GuardLongJumpTargetTable",
            f::GUARD_LONG_JUMP_TARGET_TABLE,
            true,
        ),
        (
            "GuardLongJumpTargetCount",
            f::GUARD_LONG_JUMP_TARGET_COUNT,
            false,
        ),
        ("DynamicValueRelocTable", f::DYNAMIC_VALUE_RELOC_TABLE, true),
        ("CHPEMetadataPointer", f::CHPE_METADATA_POINTER, true),
        ("GuardRFFailureRoutine", f::GUARD_RF_FAILURE_ROUTINE, true),
        (
            "GuardRFFailureRoutineFunctionPointer",
            f::GUARD_RF_FAILURE_ROUTINE_FUNCTION_POINTER,
            true,
        ),
        (
            "DynamicValueRelocTableOffset",
            f::DYNAMIC_VALUE_RELOC_TABLE_OFFSET,
            false,
        ),
        (
            "DynamicValueRelocTableSection",
            f::DYNAMIC_VALUE_RELOC_TABLE_SECTION,
            false,
        ),
        (
            "GuardRFVerifyStackPointerFunctionPointer",
            f::GUARD_RF_VERIFY_STACK_POINTER_FUNCTION_POINTER,
            true,
        ),
        ("HotPatchTableOffset", f::HOT_PATCH_TABLE_OFFSET, false),
        (
            "EnclaveConfigurationPointer",
            f::ENCLAVE_CONFIGURATION_POINTER,
            true,
        ),
        (
            "VolatileMetadataPointer",
            f::VOLATILE_METADATA_POINTER,
            true,
        ),
        (
            "GuardEHContinuationTable",
            f::GUARD_EH_CONTINUATION_TABLE,
            true,
        ),
        (
            "GuardEHContinuationCount",
            f::GUARD_EH_CONTINUATION_COUNT,
            false,
        ),
        (
            "GuardXFGCheckFunctionPointer",
            f::GUARD_XFG_CHECK_FUNCTION_POINTER,
            true,
        ),
        (
            "GuardXFGDispatchFunctionPointer",
            f::GUARD_XFG_DISPATCH_FUNCTION_POINTER,
            true,
        ),
        (
            "GuardXFGTableDispatchFunctionPointer",
            f::GUARD_XFG_TABLE_DISPATCH_FUNCTION_POINTER,
            true,
        ),
        (
            "CastGuardOsDeterminedFailureMode",
            f::CAST_GUARD_OS_DETERMINED_FAILURE_MODE,
            true,
        ),
        (
            "GuardMemcpyFunctionPointer",
            f::GUARD_MEMCPY_FUNCTION_POINTER,
            true,
        ),
        ("UmaFunctionPointers", f::UMA_FUNCTION_POINTERS, true),
    ];
    for &(name, field, address) in fields {
        let (x, y) = (ca.get(field), cb.get(field));
        if x == y {
            continue;
        }
        let show = |s: &Side<'_>, v: Option<u64>| match v {
            None => "absent".to_owned(),
            Some(v) if address => format!("{v:#x} ({})", s.describe_va(v)),
            Some(v) => format!("{v:#x}"),
        };
        let (sx, sy) = (show(a, x), show(b, y));
        // An address that names the same thing in both: layout only.
        let same = address
            && x.zip(y)
                .is_some_and(|(x, y)| x != 0 && a.describe_va(x) == b.describe_va(y));
        if same {
            p.note(format!("{name}: {sx} | {sy} (the same, moved)"));
        } else {
            p.add(format!("{name}: {sx} | {sy}"));
        }
    }
    let tables = [
        (
            "CFG functions",
            ca.guard_cf_functions(),
            cb.guard_cf_functions(),
        ),
        (
            "CFG address-taken IAT entries",
            ca.guard_address_taken_iat_entries(),
            cb.guard_address_taken_iat_entries(),
        ),
        (
            "CFG long jump targets",
            ca.guard_long_jump_targets(),
            cb.guard_long_jump_targets(),
        ),
        (
            "EH continuation targets",
            ca.guard_eh_continuations(),
            cb.guard_eh_continuations(),
        ),
    ];
    for (name, x, y) in tables {
        let set =
            |s: &Side<'_>, t: &turbonop_pe::GuardTable<'_>| -> std::collections::BTreeSet<String> {
                t.iter()
                    .map(|e| format!("{} [{:#x}]", s.names.describe(e.rva.0), e.flags))
                    .collect()
            };
        let (sx, sy) = (set(a, &x), set(b, &y));
        if sx != sy {
            p.add(format!("{name}: {} | {} entries", sx.len(), sy.len()));
            for e in sx.difference(&sy).take(o.rows) {
                p.add(format!("  {name}: {e} only in a"));
            }
            for e in sy.difference(&sx).take(o.rows) {
                p.add(format!("  {name}: {e} only in b"));
            }
        }
    }
    p
}

fn tls(a: &Side<'_>, b: &Side<'_>, o: &Options) -> Part {
    let mut p = Part::new("tls", "TLS", o.rows);
    match (a.pe.tls(), b.pe.tls()) {
        (None, None) => {}
        (x, y) if x.is_some() != y.is_some() => {
            p.add(format!("present: {} | {}", x.is_some(), y.is_some()));
        }
        (Some(x), Some(y)) => {
            let d = |s: &Side<'_>, v: turbonop_core::Va| s.describe_va(v.0);
            for (name, vx, vy) in [
                ("StartOfRawData", x.start_of_raw_data, y.start_of_raw_data),
                ("EndOfRawData", x.end_of_raw_data, y.end_of_raw_data),
                ("AddressOfIndex", x.address_of_index, y.address_of_index),
                (
                    "AddressOfCallBacks",
                    x.address_of_callbacks,
                    y.address_of_callbacks,
                ),
            ] {
                if d(a, vx) != d(b, vy) {
                    p.add(format!("{name}: {} | {}", d(a, vx), d(b, vy)));
                }
            }
            p.field("SizeOfZeroFill", x.size_of_zero_fill, y.size_of_zero_fill);
            p.field("Characteristics", x.characteristics, y.characteristics);
            let size = |t: &turbonop_pe::tls::TlsDirectory| {
                t.end_of_raw_data.0.saturating_sub(t.start_of_raw_data.0)
            };
            p.field("template size", size(&x), size(&y));
            let cb = |s: &Side<'_>| {
                s.pe.tls_callbacks()
                    .map(|v| s.describe_va(v.0))
                    .collect::<Vec<_>>()
            };
            let (cx, cy) = (cb(a), cb(b));
            if cx != cy {
                p.add(format!("callbacks: {} | {}", cx.join(", "), cy.join(", ")));
            }
        }
        _ => {}
    }
    p
}

fn resources(a: &Side<'_>, b: &Side<'_>, o: &Options) -> Part {
    use turbonop_pe::resources::ResourceTarget;
    let mut p = Part::new("resources", "Resources", o.rows);
    // Each resource, by its path (type/name/language): its size and data.
    let read = |s: &Side<'_>| -> std::collections::BTreeMap<String, Vec<u8>> {
        let mut out = std::collections::BTreeMap::new();
        let Some(root) = s.pe.resource_root() else {
            return out;
        };
        let base = root.base();
        let mut work = vec![(root, String::new(), 0)];
        while let Some((dir, path, depth)) = work.pop() {
            if depth > 3 || out.len() > 100_000 {
                continue;
            }
            for e in dir.iter() {
                let name = e.name.map_or_else(|| "?".into(), |n| n.text());
                let path = if path.is_empty() {
                    name
                } else {
                    format!("{path}/{name}")
                };
                match e.target {
                    ResourceTarget::Directory(rva) => {
                        if let Some(d) = s.pe.resource_directory(base, rva) {
                            work.push((d, path, depth + 1));
                        }
                    }
                    ResourceTarget::Data(rva) => {
                        if let Some(entry) = s.pe.resource_data_entry(rva) {
                            let data = s.bytes(entry.data.get(), entry.size.get());
                            out.insert(path, data.to_vec());
                        }
                    }
                }
            }
        }
        out
    };
    let (ra, rb) = (read(a), read(b));
    for (k, x) in &ra {
        match rb.get(k) {
            None => p.add(format!("{k}: only in a")),
            Some(y) if x != y => p.add(format!(
                "{k}: {} | {} bytes, contents differ",
                x.len(),
                y.len()
            )),
            _ => {}
        }
    }
    for k in rb.keys() {
        if !ra.contains_key(k) {
            p.add(format!("{k}: only in b"));
        }
    }
    p
}

fn debug(a: &Side<'_>, b: &Side<'_>, o: &Options) -> Part {
    let mut p = Part::new("debug", "Debug directory", o.rows);
    let kind_name = |k: u32| match k {
        1 => "COFF".to_owned(),
        2 => "CODEVIEW".into(),
        3 => "FPO".into(),
        4 => "MISC".into(),
        9 => "BORLAND".into(),
        12 => "VC_FEATURE".into(),
        13 => "POGO".into(),
        14 => "ILTCG".into(),
        16 => "REPRO".into(),
        20 => "EX_DLLCHARACTERISTICS".into(),
        k => format!("type {k}"),
    };
    fn read<'d>(s: &Side<'d>) -> Vec<turbonop_pe::DebugEntry<'d>> {
        s.pe.debug_entries().collect()
    }
    let (ea, eb) = (read(a), read(b));
    let kinds = |e: &[turbonop_pe::DebugEntry<'_>]| {
        e.iter()
            .map(|x| kind_name(x.kind()))
            .collect::<Vec<_>>()
            .join(", ")
    };
    if kinds(&ea) != kinds(&eb) {
        p.add(format!("entries: {} | {}", kinds(&ea), kinds(&eb)));
    }
    for (x, y) in ea.iter().zip(&eb) {
        if x.kind() != y.kind() {
            continue;
        }
        let k = kind_name(x.kind());
        let (hx, hy) = (x.header, y.header);
        if hx.time_date_stamp.get() != hy.time_date_stamp.get() {
            p.add(format!(
                "{k}: time stamp {:#x} | {:#x}",
                hx.time_date_stamp.get(),
                hy.time_date_stamp.get()
            ));
        }
        if (hx.major_version.get(), hx.minor_version.get())
            != (hy.major_version.get(), hy.minor_version.get())
        {
            p.add(format!("{k}: version differs"));
        }
        if hx.size_of_data.get() != hy.size_of_data.get() {
            p.add(format!(
                "{k}: size {:#x} | {:#x}",
                hx.size_of_data.get(),
                hy.size_of_data.get()
            ));
        }
        match (x.codeview(), y.codeview()) {
            (Some(cx), Some(cy)) => {
                let fmt = |c: &turbonop_pe::CodeView<'_>| format!("{c:?}");
                if fmt(&cx) != fmt(&cy) {
                    p.add(format!("{k}: {} | {}", fmt(&cx), fmt(&cy)));
                }
                continue;
            }
            (None, None) => {}
            _ => p.add(format!("{k}: only one is a CodeView record")),
        }
        if let (Some(px), Some(py)) = (x.pogo(), y.pogo()) {
            let groups = |s: &Side<'_>, g: &turbonop_pe::Pogo<'_>| {
                g.groups()
                    .map(|c| {
                        format!(
                            "{} {} [{:#x}]",
                            String::from_utf8_lossy(c.name),
                            s.names.describe(c.rva.0),
                            c.size
                        )
                    })
                    .collect::<Vec<_>>()
            };
            let (gx, gy) = (groups(a, &px), groups(b, &py));
            if gx != gy {
                let sx: std::collections::BTreeSet<_> = gx.iter().collect();
                let sy: std::collections::BTreeSet<_> = gy.iter().collect();
                p.add(format!("POGO: {} | {} groups", gx.len(), gy.len()));
                for g in sx.difference(&sy).take(o.rows) {
                    p.add(format!("  POGO group {g} only in a"));
                }
                for g in sy.difference(&sx).take(o.rows) {
                    p.add(format!("  POGO group {g} only in b"));
                }
            }
            continue;
        }
        if x.data != y.data {
            p.add(format!(
                "{k}: data {} | {}",
                hex(x
                    .data
                    .unwrap_or(&[])
                    .get(..32)
                    .unwrap_or(x.data.unwrap_or(&[]))),
                hex(y
                    .data
                    .unwrap_or(&[])
                    .get(..32)
                    .unwrap_or(y.data.unwrap_or(&[])))
            ));
        }
    }
    p
}

fn rich(a: &Side<'_>, b: &Side<'_>, o: &Options) -> Part {
    let mut p = Part::new("rich", "Rich header", o.rows);
    let show = |r: &Option<turbonop_pe::extras::Rich>| match r {
        None => "none".to_owned(),
        Some(r) => r
            .entries
            .iter()
            .map(|e| format!("{}.{}x{}", e.product, e.build, e.count))
            .collect::<Vec<_>>()
            .join(" "),
    };
    let (ra, rb) = (a.pe.rich(), b.pe.rich());
    let (sa, sb) = (show(&ra), show(&rb));
    if sa != sb {
        p.add(format!("{sa} | {sb}"));
    }
    p
}

/// How two copies of one function or datum compare.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Same {
    /// Byte for byte.
    Identical,
    /// Only where they refer to what is elsewhere (a call, a pointer),
    /// and those refer to the same names.
    Moved,
    /// Where they refer to something one image has no name for (so it can't
    /// be told whether the two refer to the same thing), and otherwise
    /// only where they refer to what moved.
    Unnamed,
    /// In the padding after them (link.exe fills code's with int3, 0xCC;
    /// and before it, perhaps where they refer to what moved).
    Padding,
    /// Not the same: from this offset.
    Different(usize),
}

/// Whether the copies `x` (in `a`, at `ra`) and `y` (in `b`, at `rb`)
/// differ only in their references: each differing range explained by a
/// 32-bit relative displacement (x64's RIP-relative operands and calls)
/// or a pointer-sized address, whose targets in each image have one name.
fn compare_copies(a: &Side<'_>, ra: u32, x: &[u8], b: &Side<'_>, rb: u32, y: &[u8]) -> Same {
    if x == y {
        return Same::Identical;
    }
    if x.len() != y.len() {
        return Same::Different(0);
    }
    let pad = |b: u8| b == 0xCC || b == 0;
    if x.iter().zip(y).all(|(p, q)| p == q || (pad(*p) && pad(*q))) {
        // (Padding only if the differences are at the end: what precedes
        // is the same.)
        let first = x.iter().zip(y).position(|(p, q)| p != q).unwrap_or(0);
        if x[first..].iter().all(|&b| pad(b)) && y[first..].iter().all(|&b| pad(b)) {
            return Same::Padding;
        }
    }
    let ptr = if a.is64 { 8 } else { 4 };
    let x64 = a.pe.machine() == machine::AMD64;
    let mut i = 0;
    let mut padded = false;
    let mut unnamed = false;
    while i < x.len() {
        if x[i] == y[i] {
            i += 1;
            continue;
        }
        // The rest is padding in both: the copies differ in their padding
        // (and before it, only where they refer to what moved).
        if x[i..].iter().all(|&b| pad(b)) && y[i..].iter().all(|&b| pad(b)) {
            padded = true;
            break;
        }
        // Try a field covering `i`: a rel32 (x64), then an absolute
        // pointer; each starting up to its width before `i`.
        let mut explained = None;
        // (width, how it's read: 0 relative, 1 an address, 2 an RVA)
        let fields: &[(usize, u8)] = if x64 {
            &[(4, 0), (8, 1), (4, 2), (8, 2)]
        } else {
            &[(4, 1), (4, 2)]
        };
        'try_fields: for &(w, how) in fields {
            for start in i.saturating_sub(w - 1)..=i {
                let end = start + w;
                if end > x.len() {
                    continue;
                }
                let target = |s: &'_ Side<'_>,
                              base: u32,
                              bytes: &[u8]|
                 -> Option<Option<(Vec<String>, u32)>> {
                    let v = &bytes[start..end];
                    let value = if w == 8 {
                        u64::from_le_bytes(v.try_into().ok()?)
                    } else {
                        u64::from(u32::from_le_bytes([v[0], v[1], v[2], v[3]]))
                    };
                    let rva = match how {
                        // A relative displacement: from the field's end
                        // (most instructions; immediates after it would
                        // shift this, so the target's name decides).
                        0 => {
                            let d = i32::from_le_bytes([v[0], v[1], v[2], v[3]]);
                            i64::from(base) + end as i64 + i64::from(d)
                        }
                        1 if w == ptr => i64::try_from(value.checked_sub(s.base)?).ok()?,
                        // An RVA (an import's hint/name entry, a table's).
                        2 => i64::try_from(value).ok()?,
                        _ => return None,
                    };
                    let rva = u32::try_from(rva).ok()?;
                    // (Inside the image, named or not; the headers by their
                    // RVA: __ImageBase is 0 in both.)
                    if !s
                        .sections
                        .iter()
                        .any(|x| rva >= x.0 && rva < x.0.saturating_add(x.1))
                    {
                        let headers = s.sections.first().map_or(0x1000, |x| x.0);
                        return (rva < headers)
                            .then(|| Some((vec![format!("headers+{rva:#x}")], 0)));
                    }
                    // What holds the target, by the names both have: unless
                    // this image names it otherwise (a name only it has).
                    let shared = s.names.all_at(rva);
                    let own = s.all.at(rva).map(|(n, _)| n.rva);
                    if let (Some((_, off)), Some(start)) = (&shared, own)
                        && start != rva - off
                    {
                        return Some(None);
                    }
                    Some(shared.map(|(n, off)| (n.into_iter().map(str::to_owned).collect(), off)))
                };
                match (target(a, ra, x), target(b, rb, y)) {
                    // The same place: an offset into what has a name in both.
                    (Some(Some(tx)), Some(Some(ty)))
                        if tx.1 == ty.1 && tx.0.iter().any(|n| ty.0.contains(n)) =>
                    {
                        explained = Some(end);
                        break 'try_fields;
                    }
                    // Into each image, but without a name in both.
                    (Some(tx), Some(ty)) if tx.is_none() || ty.is_none() => {
                        unnamed = true;
                        explained = Some(end);
                        break 'try_fields;
                    }
                    _ => {}
                }
            }
        }
        match explained {
            Some(end) => i = end,
            None => return Same::Different(i),
        }
    }
    if unnamed {
        Same::Unnamed
    } else if padded {
        Same::Padding
    } else {
        Same::Moved
    }
}

fn functions(a: &Side<'_>, b: &Side<'_>, o: &Options) -> Part {
    let mut p = Part::new("functions", "Functions and data, by name", o.rows);
    if a.all.list.is_empty() || b.all.list.is_empty() {
        p.note(format!(
            "names from: {} | {} (give each a PDB or -map to compare them by name)",
            a.all.source, b.all.source
        ));
        return p;
    }
    p.note(format!("names from: {} | {}", a.all.source, b.all.source));
    let (ma, mb) = (a.names.by_name(), b.names.by_name());
    // Names one has and the other doesn't (a map's statics, or what one
    // linker kept and the other didn't).
    let (fa, fb) = (a.all.by_name(), b.all.by_name());
    let mut common: Vec<(&str, u32, u32, u32, u32)> = Vec::new();
    let mut only_a = 0usize;
    let mut only_b = 0usize;
    for (name, x) in &ma {
        // (Not the debug information's names for the image's own
        // structures, "[PE Directory] Debug" and the like: the parts above
        // compare those.)
        if name.starts_with('[') {
            continue;
        }
        if let Some(y) = mb.get(name) {
            common.push((name, x.rva, x.size, y.rva, y.size));
        }
    }
    common.sort_unstable();
    // Folding: names one image keeps at one address and the other apart,
    // first (it explains much of the rest), and those names not compared
    // copy by copy.
    let mut by_address = [
        turbonop_hash::Map::<u32, Vec<&str>>::default(),
        turbonop_hash::Map::default(),
    ];
    for &(name, ra, _, rb, _) in &common {
        by_address[0].entry(ra).or_default().push(name);
        by_address[1].entry(rb).or_default().push(name);
    }
    let mut folded_names: turbonop_hash::Set<&str> = turbonop_hash::Set::default();
    for (side, which) in [(0usize, "a"), (1, "b")] {
        let other = &by_address[1 - side];
        let mut groups: Vec<&Vec<&str>> =
            by_address[side].values().filter(|g| g.len() > 1).collect();
        groups.sort();
        let place: turbonop_hash::Map<&str, u32> = other
            .iter()
            .flat_map(|(&rva, names)| names.iter().map(move |n| (*n, rva)))
            .collect();
        for g in groups {
            let places: std::collections::BTreeSet<u32> =
                g.iter().filter_map(|n| place.get(n).copied()).collect();
            if places.len() > 1 {
                folded_names.extend(g.iter().copied());
                p.add_with(|| {
                    format!(
                        "folded in {which} only: {} ({} names, at {} places in the other)",
                        g.iter().take(3).copied().collect::<Vec<_>>().join(" = "),
                        g.len(),
                        places.len()
                    )
                });
            }
        }
    }
    common.retain(|c| !folded_names.contains(c.0));
    let mut only: Vec<String> = Vec::new();
    for name in fa.keys().filter(|n| !n.starts_with('[')) {
        if !fb.contains_key(name) {
            only_a += 1;
            only.push(format!("{name}: only in a"));
        }
    }
    for name in fb.keys().filter(|n| !n.starts_with('[')) {
        if !fa.contains_key(name) {
            only_b += 1;
            only.push(format!("{name}: only in b"));
        }
    }
    only.sort();
    let results = turbonop_par::map(&common, 256, |&(name, ra, sa, rb, sb)| {
        // A function's length from the exception data, where it has one;
        // else no further than the next thing either image starts.
        let (sa, sb) = (a.extent(ra, sa), b.extent(rb, sb));
        let size = sa.min(sb);
        let (x, y) = (a.bytes(ra, size), b.bytes(rb, size));
        let same = if sa != sb && sa != 0 && sb != 0 {
            Same::Different(0)
        } else {
            compare_copies(a, ra, x, b, rb, y)
        };
        let bytes_at = |i: usize| {
            (
                hex(x.get(i..(i + 8).min(x.len())).unwrap_or(&[])),
                hex(y.get(i..(i + 8).min(y.len())).unwrap_or(&[])),
            )
        };
        let first = match same {
            Same::Different(i) => Some((i, bytes_at(i))),
            _ => None,
        };
        (name, same, sa, sb, ra, rb, first)
    });
    let (mut identical, mut moved, mut padding, mut different, mut unnamed) =
        (0usize, 0usize, 0usize, 0usize, 0usize);
    for (name, same, sa, sb, ra, rb, first) in &results {
        match same {
            Same::Identical => identical += 1,
            Same::Moved => moved += 1,
            Same::Padding => padding += 1,
            Same::Unnamed => unnamed += 1,
            Same::Different(_) => {
                different += 1;
                p.add_with(|| {
                    if sa == sb {
                        let (at, (x, y)) = first.clone().unwrap_or_default();
                        format!("{name}: contents differ ({sa:#x} bytes at {ra:#x} | {rb:#x}; from +{at:#x}: {x} | {y})")
                    } else {
                        format!("{name}: size {sa:#x} | {sb:#x}")
                    }
                });
            }
        }
    }
    if padding > 0 {
        p.add(format!(
            "{padding} differ only in the padding after them (int3, 0xCC, or zeros)"
        ));
    }
    // Last, what one has and the other doesn't.
    for line in only {
        p.add(line);
    }
    p.summary.insert(
        1,
        format!(
            "{} names in both: {identical} identical, {moved} differ only where they refer to what moved, {padding} only in their padding, {unnamed} where they refer to what only one image names, {different} differ; {only_a} only in a, {only_b} only in b",
            common.len()
        ),
    );
    p
}

/// Compares the images `a` and `b`.
pub(crate) fn compare(a: &[u8], b: &[u8], o: &Options) -> Result<Report, String> {
    let mut sa = Side::open(a, &o.a, o.debug[0].as_deref(), o.map[0].as_deref())?;
    let mut sb = Side::open(b, &o.b, o.debug[1].as_deref(), o.map[1].as_deref())?;
    // Addresses are described by the names both have (a map's statics,
    // say, that the other image's names lack, would otherwise split its
    // functions differently).
    let names = |s: &Side<'_>| -> turbonop_hash::Set<String> {
        s.all.list.iter().map(|n| n.name.clone()).collect()
    };
    let (na, nb) = (names(&sa), names(&sb));
    let both: turbonop_hash::Set<&str> = na
        .iter()
        .filter(|n| nb.contains(*n))
        .map(String::as_str)
        .collect();
    let spans = |s: &Side<'_>| {
        s.sections
            .iter()
            .map(|x| (x.0, x.1.max(x.3)))
            .collect::<Vec<_>>()
    };
    sa.names = sa.all.restricted(&both, &spans(&sa));
    sb.names = sb.all.restricted(&both, &spans(&sb));
    for s in [&mut sa, &mut sb] {
        let mut starts: Vec<u32> = s
            .all
            .list
            .iter()
            .map(|n| n.rva)
            .chain(s.lengths.keys().copied())
            .collect();
        starts.sort_unstable();
        starts.dedup();
        s.starts = starts;
    }
    type Compare = fn(&Side<'_>, &Side<'_>, &Options) -> Part;
    let parts: [(&str, Compare); 14] = [
        ("bytes", bytes),
        ("headers", headers),
        ("sections", sections),
        ("directories", directories),
        ("imports", imports),
        ("exports", exports),
        ("relocations", relocations),
        ("exceptions", exceptions),
        ("loadconfig", load_config),
        ("tls", tls),
        ("resources", resources),
        ("debug", debug),
        ("rich", rich),
        ("functions", functions),
    ];
    let wanted: Vec<_> = parts.iter().filter(|(k, _)| o.wants(k)).collect();
    let done = turbonop_par::map_balanced(&wanted, 1, |(_, f)| f(&sa, &sb, o));
    // A byte-identical pair is identical in every part.
    Ok(Report {
        kind: "image",
        parts: done,
    })
}
