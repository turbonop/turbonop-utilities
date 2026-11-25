//! Reads every object under the given paths (`.obj` files, and the members
//! of `.lib` files), writes each back, and reports how many are the same
//! file, and where each one that isn't first differs. On every core.
//!
//!     cargo run --release -p turbonop-coff --example roundtrip -- <dir or file>...
//!
//! For each object it also checks:
//!
//! - that laid out afresh (in the style it was read as) it is the file its
//!   tool wrote, which holds for the tools whose conventions the writer
//!   follows, and that whatever the layout it reads back the same;
//! - that converted to the other header and back it is unchanged;
//! - that its section definitions agree with its sections;
//! - whether its COMDAT checksums are [`turbonop_coff::checksum`]'s.

// Offsets in a mapped file fit in a usize.
#![allow(clippy::cast_possible_truncation)]

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::Instant;

use turbonop_coff::{Aux, File, Format, Object, SectionNumber, comdat_select};
use turbonop_global_alloc as _;
use turbonop_pe::consts::section_characteristics as scn;

fn walk(path: &Path, out: &mut Vec<PathBuf>) {
    if path.is_dir() {
        let Ok(entries) = std::fs::read_dir(path) else {
            return;
        };
        for e in entries.flatten() {
            walk(&e.path(), out);
        }
    } else if path
        .extension()
        .and_then(|e| e.to_str())
        .is_some_and(|e| e.eq_ignore_ascii_case("obj") || e.eq_ignore_ascii_case("lib"))
    {
        out.push(path.to_path_buf());
    }
}

/// Counts of outcomes, each with a few examples.
#[derive(Default)]
struct Tally {
    counts: BTreeMap<String, u64>,
    examples: BTreeMap<String, Vec<String>>,
}

impl Tally {
    fn add(&mut self, key: impl Into<String>, example: &str) {
        let key = key.into();
        *self.counts.entry(key.clone()).or_default() += 1;
        let e = self.examples.entry(key).or_default();
        if e.len() < 3 {
            e.push(example.to_string());
        }
    }

    fn merge(&mut self, other: Tally) {
        for (k, c) in other.counts {
            *self.counts.entry(k.clone()).or_default() += c;
            let e = self.examples.entry(k.clone()).or_default();
            for x in other.examples.get(&k).into_iter().flatten() {
                if e.len() < 3 {
                    e.push(x.clone());
                }
            }
        }
    }
}

fn u16_at(b: &[u8], o: usize) -> u16 {
    u16::from_le_bytes([b[o], b[o + 1]])
}

fn u32_at(b: &[u8], o: usize) -> u32 {
    u32::from_le_bytes([b[o], b[o + 1], b[o + 2], b[o + 3]])
}

/// What part of the file `data` (an object read without error) holds the
/// byte at `at`.
fn region(data: &[u8], at: usize) -> String {
    let big = data[..4] == [0, 0, 0xFF, 0xFF];
    let (header, sections, symbols, count, stride) = if big {
        (
            56,
            u32_at(data, 44) as usize,
            u32_at(data, 48),
            u32_at(data, 52),
            20,
        )
    } else {
        let table = 20 + usize::from(u16_at(data, 16));
        (
            table,
            usize::from(u16_at(data, 2)),
            u32_at(data, 8),
            u32_at(data, 12),
            18,
        )
    };
    if at < header {
        return "the header".into();
    }
    if at < header + sections * 40 {
        let field = match (at - header) % 40 {
            0..8 => "name",
            8..16 => "virtual size or address",
            16..20 => "size",
            20..24 => "data pointer",
            24..28 => "relocation pointer",
            28..32 => "line number pointer",
            32..36 => "counts",
            _ => "characteristics",
        };
        return format!("a section header's {field}");
    }
    for i in 0..sections {
        let h = &data[header + i * 40..header + i * 40 + 40];
        let (size, ptr, rptr, lptr) = (u32_at(h, 16), u32_at(h, 20), u32_at(h, 24), u32_at(h, 28));
        let extended = u32_at(h, 36) & scn::LNK_NRELOC_OVFL != 0 && u16_at(h, 32) == 0xFFFF;
        let relocations = if extended {
            u32_at(data, rptr as usize)
        } else {
            u32::from(u16_at(h, 32))
        };
        let inside = |start: u32, len: u64| {
            (start as usize..(u64::from(start) + len) as usize).contains(&at)
        };
        if ptr != 0 && inside(ptr, u64::from(size)) {
            return "section data".into();
        }
        if inside(rptr, u64::from(relocations) * 10) {
            return "relocations".into();
        }
        if inside(lptr, u64::from(u16_at(h, 34)) * 6) {
            return "line numbers".into();
        }
    }
    let strings = symbols as usize + count as usize * stride;
    if (symbols as usize..strings).contains(&at) {
        let field = match (at - symbols as usize) % stride {
            0..8 => "name",
            8..12 => "value",
            _ => "section, type, class, or count",
        };
        return format!("a symbol record's {field}");
    }
    let table = data
        .get(strings..strings + 4)
        .map_or(0, |s| u32_at(s, 0) as usize);
    if at >= strings && at < strings + table {
        return "the string table".into();
    }
    "between parts".into()
}

/// The parts of an object read without error: (start, end) of the
/// headers, each section's data, relocations, and line numbers, and the
/// symbol and string tables.
fn parts(data: &[u8]) -> Vec<(usize, usize)> {
    let big = data[..4] == [0, 0, 0xFF, 0xFF];
    let (header, sections, symbols, count, stride) = if big {
        (
            56,
            u32_at(data, 44) as usize,
            u32_at(data, 48),
            u32_at(data, 52),
            20,
        )
    } else {
        let table = 20 + usize::from(u16_at(data, 16));
        (
            table,
            usize::from(u16_at(data, 2)),
            u32_at(data, 8),
            u32_at(data, 12),
            18,
        )
    };
    let mut parts = vec![(0, header + sections * 40)];
    for i in 0..sections {
        let h = &data[header + i * 40..header + i * 40 + 40];
        let (size, ptr, rptr, lptr) = (u32_at(h, 16), u32_at(h, 20), u32_at(h, 24), u32_at(h, 28));
        let extended = u32_at(h, 36) & scn::LNK_NRELOC_OVFL != 0 && u16_at(h, 32) == 0xFFFF;
        let relocations = if extended {
            u32_at(data, rptr as usize) as usize
        } else {
            usize::from(u16_at(h, 32))
        };
        if ptr != 0 && size != 0 {
            parts.push((ptr as usize, ptr as usize + size as usize));
        }
        if relocations != 0 {
            parts.push((rptr as usize, rptr as usize + relocations * 10));
        }
        let lines = usize::from(u16_at(h, 34));
        if lines != 0 {
            parts.push((lptr as usize, lptr as usize + lines * 6));
        }
    }
    let strings = symbols as usize + count as usize * stride;
    if symbols != 0 {
        parts.push((symbols as usize, strings));
        let table = data
            .get(strings..strings + 4)
            .map_or(0, |s| (u32_at(s, 0) as usize).max(4));
        parts.push((strings, strings + table));
    }
    parts.sort_unstable();
    parts
}

/// Whether the object's tool padded between its parts (as the
/// assemblers do, to align them), which a fresh layout doesn't.
fn pads(data: &[u8]) -> bool {
    let mut at = 0;
    for (start, end) in parts(data) {
        if start > at {
            return true;
        }
        at = at.max(end);
    }
    false
}

/// Where `out` first differs from `data`.
fn difference(data: &[u8], out: &[u8]) -> String {
    match out.iter().zip(data).position(|(a, b)| a != b) {
        Some(at) => region(data, at),
        None => format!("the length ({} for {})", out.len(), data.len()),
    }
}

/// Whether two objects hold the same thing (their layouts aside, and the
/// relocation-overflow flag, which a fresh layout sets as needed).
fn same_content(a: &Object<'_>, b: &Object<'_>) -> bool {
    a.format == b.format
        && a.machine == b.machine
        && a.time_date_stamp == b.time_date_stamp
        && a.characteristics == b.characteristics
        && a.optional_header == b.optional_header
        && a.symbols == b.symbols
        && a.sections.len() == b.sections.len()
        && a.sections.iter().zip(&b.sections).all(|(x, y)| {
            x.name == y.name
                && x.virtual_size == y.virtual_size
                && x.virtual_address == y.virtual_address
                && x.characteristics & !scn::LNK_NRELOC_OVFL
                    == y.characteristics & !scn::LNK_NRELOC_OVFL
                && x.data == y.data
                && x.relocations == y.relocations
                && x.line_numbers == y.line_numbers
        })
}

fn check(data: &[u8], name: &str, t: &mut Tally) {
    let object = match File::parse(data) {
        Ok(File::Object(o)) => o,
        Ok(opaque @ File::Opaque { kind, .. }) => {
            t.add(format!("passed through: {kind:?}"), name);
            if opaque.write().is_ok_and(|out| out == data) {
                t.add("passed through identically", name);
            } else {
                t.add("PASSED THROUGH DIFFERENTLY", name);
            }
            return;
        }
        Err(e) => {
            t.add(format!("READ ERROR: {} ({:?})", e.what, e.kind), name);
            return;
        }
    };
    t.add(
        format!("objects: {:?}, {:?} style", object.format, object.style),
        name,
    );
    match object.write() {
        Ok(out) if out == data => t.add("round trip: identical", name),
        Ok(out) => t.add(
            format!("ROUND TRIP DIFFERS: {}", difference(data, &out)),
            name,
        ),
        Err(e) => t.add(format!("ROUND TRIP WRITE ERROR: {e}"), name),
    }

    for s in &object.symbols {
        let kind = match &s.aux {
            Aux::None => continue,
            Aux::Function(_) => "function definition",
            Aux::BeginEnd(_) => ".bf or .ef",
            Aux::WeakExternal(w) => {
                t.add(format!("aux: weak external, search {}", w.search), name);
                continue;
            }
            Aux::File { .. } => "file",
            Aux::Section(_) => "section definition",
            Aux::ClrToken(_) => "CLR token",
            Aux::Raw(_) => {
                t.add(
                    format!("aux: kept raw, storage class {}", s.storage_class),
                    name,
                );
                continue;
            }
        };
        t.add(format!("aux: {kind}"), name);
    }
    for s in &object.sections {
        if !s.line_numbers.is_empty() {
            t.add("sections with line numbers", name);
        }
        if s.needs_extended_relocations() {
            t.add("sections with the extended relocation count", name);
        }
    }

    // Laid out afresh.
    let mut fresh = object.clone();
    fresh.forget_layout();
    let fresh_bytes = fresh.write();
    match &fresh_bytes {
        Ok(out) if out == data => {
            t.add(format!("fresh {:?} layout: identical", object.style), name);
        }
        Ok(out) => {
            let why = if pads(data) {
                "the original pads between parts".into()
            } else {
                difference(data, out)
            };
            t.add(
                format!("fresh {:?} layout differs: {why}", object.style),
                name,
            );
            match Object::parse(out) {
                Ok(again) if same_content(&again, &object) => {
                    t.add("fresh layout: reads back the same", name);
                }
                Ok(_) => t.add("FRESH LAYOUT READS BACK DIFFERENTLY", name),
                Err(e) => t.add(format!("FRESH LAYOUT UNREADABLE: {e}"), name),
            }
        }
        Err(e) => t.add(format!("FRESH WRITE ERROR: {e}"), name),
    }

    // To the other header and back.
    let other = match object.format {
        Format::Regular => Format::BigObj,
        Format::BigObj => Format::Regular,
    };
    let convertible = match other {
        Format::BigObj => object.characteristics == 0 && object.optional_header.is_empty(),
        Format::Regular => object.sections.len() <= 0xFEFF,
    };
    if convertible {
        let mut converted = object.clone();
        let back = converted
            .set_format(other)
            .and_then(|()| converted.write())
            .and_then(|bytes| {
                let mut back = Object::parse(&bytes)?.into_owned();
                back.set_format(object.format)?;
                back.write()
            });
        match (back, &fresh_bytes) {
            (Ok(a), Ok(b)) if a == *b => t.add(format!("to {other:?} and back: unchanged"), name),
            (Ok(_), Ok(_)) => t.add(format!("TO {other:?} AND BACK: CHANGED"), name),
            (Err(e), _) => t.add(format!("TO {other:?} AND BACK: ERROR {e}"), name),
            (_, Err(_)) => {}
        }
    } else {
        t.add(
            format!("to {other:?}: not attempted (too many sections, or a header field it lacks)"),
            name,
        );
    }

    // Section definitions against their sections.
    let mut synced = object.clone();
    synced.sync_section_definitions();
    for (a, b) in object.symbols.iter().zip(&synced.symbols) {
        let (Aux::Section(x), Aux::Section(y), SectionNumber::Section(id)) =
            (&a.aux, &b.aux, a.section)
        else {
            continue;
        };
        if (x.length, x.relocations, x.line_numbers) != (y.length, y.relocations, y.line_numbers) {
            t.add("SECTION DEFINITION DISAGREES WITH ITS SECTION", name);
        }
        let section = &object.sections[id.index()];
        let what = match x.selection {
            0 => "non-COMDAT",
            comdat_select::ASSOCIATIVE => "associative COMDAT",
            _ => "COMDAT",
        };
        let relocations = if section.relocations.is_empty() {
            "without"
        } else {
            "with"
        };
        let outcome = if x.checksum == 0 {
            "0"
        } else if x.checksum == turbonop_coff::checksum(section.bytes()) {
            "checksum()"
        } else {
            "something else"
        };
        t.add(
            format!(
                "checksums ({:?}): {what} {relocations} relocations: {outcome}",
                object.style
            ),
            name,
        );
    }
}

fn main() {
    let roots: Vec<String> = std::env::args().skip(1).collect();
    if roots.is_empty() {
        eprintln!("usage: roundtrip <dir or file>...");
        std::process::exit(2);
    }
    let started = Instant::now();
    let mut paths = Vec::new();
    for r in &roots {
        walk(Path::new(r), &mut paths);
    }
    let maps: Vec<(PathBuf, turbonop_mmap::Mmap)> = turbonop_par::map_balanced(&paths, 4, |p| {
        turbonop_mmap::Mmap::open(p).map(|m| (p.clone(), m))
    })
    .into_iter()
    .filter_map(Result::ok)
    .collect();
    // Every object, and every member of every library, is one item.
    let members: Vec<Option<Vec<(usize, usize)>>> =
        turbonop_par::map_balanced(&maps, 1, |(_, m)| {
            let data = m.as_slice();
            if !turbonop_archive::Archive::is_archive(data) {
                return None;
            }
            let archive = turbonop_archive::Archive::parse(data).ok()?;
            Some(
                archive
                    .files()
                    .map(|member| (member.data_offset as usize, member.data.len()))
                    .collect(),
            )
        });
    let mut items = Vec::new();
    let (mut objects, mut libraries) = (0, 0);
    for (i, m) in members.into_iter().enumerate() {
        match m {
            Some(m) => {
                libraries += 1;
                items.extend(m.into_iter().map(|(at, len)| (i, at, len)));
            }
            None => {
                objects += 1;
                items.push((i, 0, maps[i].1.as_slice().len()));
            }
        }
    }
    let bytes: u64 = items.iter().map(|i| i.2 as u64).sum();
    eprintln!(
        "{objects} objects and {libraries} libraries: {} files, {:.1} GiB, on {} threads",
        items.len(),
        bytes as f64 / f64::from(1u32 << 30),
        turbonop_par::threads()
    );
    let tally = Mutex::new(Tally::default());
    turbonop_par::map_balanced(&items, 8, |&(file, at, len)| {
        let (path, map) = &maps[file];
        let data = &map.as_slice()[at..at + len];
        let name = if at == 0 && len == map.as_slice().len() {
            path.display().to_string()
        } else {
            format!("{} (member at {at:#x})", path.display())
        };
        let mut t = Tally::default();
        check(data, &name, &mut t);
        tally.lock().unwrap().merge(t);
    });
    let tally = tally.into_inner().unwrap();
    for (k, c) in &tally.counts {
        println!("{c:>10}  {k}");
        // Failures are in capitals; differences and unknowns get examples.
        let notable = k
            .split(' ')
            .next()
            .is_some_and(|w| w.len() > 1 && w.bytes().all(|b| b.is_ascii_uppercase()))
            || k.contains("differs")
            || k.contains("something else");
        if notable {
            for e in &tally.examples[k] {
                println!("              e.g. {e}");
            }
        }
    }
    eprintln!("{:.1} s", started.elapsed().as_secs_f64());
}
