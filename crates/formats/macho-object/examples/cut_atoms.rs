//! Cuts atoms out of objects with `Object::cut`, as folding does: an atom
//! runs from its symbol to the next symbol in its section that isn't an
//! alternate entry (`N_ALT_ENTRY`), or to the section's end; with a
//! function go its `__LD,__compact_unwind` entries and its `__eh_frame`
//! FDEs. The symbols the cut orphans are removed.
//!
//! Against LLVM: cuts the atoms of the symbols named from an object and
//! checks the result is a reference object byte for byte, the same source
//! built without them (see `corpus/build.sh`: `keep-*.o`, `cut-*.o`).
//!
//!     cargo run --release -p turbonop-macho-object --example cut_atoms -- <object> <reference> <symbol>...
//!
//! Over a corpus (`-weak`): cuts every weak definition's atom from
//! `__TEXT,__text` of every object (in `.o` files, archives, universal
//! files), and checks the result writes, reads back the same, has every
//! local relocation that doesn't dangle encode an address in its target
//! section, and every symbol in its section.
//!
//!     cargo run --release -p turbonop-macho-object --example cut_atoms -- -weak <dir, .o, or .a>...

use turbonop_global_alloc as _;

use std::collections::BTreeMap;
use std::ops::Range;
use std::path::{Path, PathBuf};

use turbonop_archive::Archive;
use turbonop_macho_object::{Object, ehframe, reloc};

/// `N_ALT_ENTRY` (`n_desc`): a label inside the atom before it.
const N_ALT_ENTRY: u16 = 0x0200;
/// `N_WEAK_DEF` (`n_desc`): a weak definition.
const N_WEAK_DEF: u16 = 0x0080;

/// The ranges to cut for the atoms at `starts` (section index, offset):
/// the atoms, their compact unwind entries, their FDEs.
fn atom_cuts(o: &Object<'_>, starts: &[(usize, u64)]) -> Vec<(usize, Vec<Range<u64>>)> {
    let mut cuts: BTreeMap<usize, Vec<Range<u64>>> = BTreeMap::new();
    for &(i, start) in starts {
        let Some(s) = o.section(i) else { continue };
        let ordinal = i + 1;
        let end = o
            .symbols
            .iter()
            .filter(|t| {
                t.in_section() && usize::from(t.n_sect) == ordinal && t.n_desc & N_ALT_ENTRY == 0
            })
            .map(|t| t.n_value.wrapping_sub(s.addr))
            .filter(|&off| off > start)
            .min()
            .unwrap_or(s.size())
            .min(s.size());
        cuts.entry(i).or_default().push(start..end);
    }
    // The addresses cut, for the unwind information.
    let cut_at = |a: u64| {
        cuts.iter().any(|(&i, rs)| {
            let base = o.section(i).map_or(0, |s| s.addr);
            rs.iter().any(|r| a >= base + r.start && a < base + r.end)
        })
    };
    let mut more: Vec<(usize, Range<u64>)> = Vec::new();
    let entry: u64 = if o.is_64() { 32 } else { 20 };
    for (i, s) in o.sections().enumerate() {
        if s.segment_name() == b"__LD" && s.name() == b"__compact_unwind" {
            for (k, r) in s.relocations.iter().enumerate() {
                let at = u64::from(r.address);
                if at % entry != 0 || r.value.is_some() {
                    continue;
                }
                let function = if r.external {
                    o.symbols
                        .get(r.symbolnum as usize)
                        .filter(|t| t.in_section())
                        .map(|t| t.n_value)
                } else {
                    o.local_address(i, k)
                };
                if function.is_some_and(cut_at) {
                    more.push((i, at..at + entry));
                }
            }
        }
        if s.segment_name() == b"__TEXT"
            && s.name() == b"__eh_frame"
            && let Some(pointers) = s
                .bytes()
                .and_then(|b| ehframe::pointers(b, o.is_64(), o.endian()))
        {
            let b = s.bytes().unwrap_or(&[]);
            for p in pointers {
                if p.kind != ehframe::PointerKind::PcBegin
                    || p.encoding & 0x70 != ehframe::DW_EH_PE_PCREL
                {
                    continue;
                }
                let field = &b[p.offset..p.offset + usize::from(p.size)];
                let v: i64 = match p.size {
                    4 => i64::from(i32::from_le_bytes(field.try_into().unwrap_or([0; 4]))),
                    8 => i64::from_le_bytes(field.try_into().unwrap_or([0; 8])),
                    _ => continue,
                };
                let target = (s.addr + p.offset as u64).wrapping_add(v as u64);
                if cut_at(target) {
                    more.push((i, p.record.0 as u64..p.record.1 as u64));
                }
            }
        }
    }
    for (i, r) in more {
        cuts.entry(i).or_default().push(r);
    }
    cuts.into_iter()
        .map(|(i, mut rs)| {
            rs.sort_by_key(|r| r.start);
            rs.dedup();
            (i, rs)
        })
        .collect()
}

/// Cuts and removes the orphaned symbols. Returns the cut's report.
fn cut(o: &mut Object<'_>, starts: &[(usize, u64)]) -> Result<turbonop_macho_object::Cut, String> {
    let cuts = atom_cuts(o, starts);
    let report = o.cut(&cuts).map_err(|e| e.to_string())?;
    let o_addrs: Vec<u64> = o.sections().map(|s| s.addr).collect();
    // arm64's section-start labels (ltmp0, ...) stay at their sections'
    // starts, as LLVM puts them; the other orphans go.
    let mut keep = vec![true; o.symbols.len()];
    for &(k, at) in &report.orphaned_symbols {
        let s = &mut o.symbols[k as usize];
        let section_start = o_section_addr(&o_addrs, s.n_sect);
        if s.name.starts_with(b"ltmp") && !s.is_external() && Some(at) == section_start {
            s.n_value = at;
        } else {
            keep[k as usize] = false;
        }
    }
    o.retain_symbols(&keep);
    Ok(report)
}

/// Section `n_sect`'s address in `addrs`.
fn o_section_addr(addrs: &[u64], n_sect: u8) -> Option<u64> {
    addrs.get(usize::from(n_sect).checked_sub(1)?).copied()
}

fn oracle(path: &str, reference: &str, names: &[String]) -> Result<(), String> {
    let data = std::fs::read(path).map_err(|e| format!("{path}: {e}"))?;
    let want = std::fs::read(reference).map_err(|e| format!("{reference}: {e}"))?;
    let mut o = Object::parse(&data).map_err(|e| format!("{path}: {e}"))?;
    let mut starts = Vec::new();
    for n in names {
        let s = o
            .symbols
            .iter()
            .find(|s| s.in_section() && s.name.as_ref() == n.as_bytes())
            .ok_or_else(|| format!("{path}: no symbol {n}"))?;
        let i = usize::from(s.n_sect) - 1;
        let base = o.section(i).map_or(0, |s| s.addr);
        starts.push((i, s.n_value - base));
    }
    let report = cut(&mut o, &starts)?;
    let out = o.write().map_err(|e| format!("{path}: {e}"))?;
    let at = out
        .iter()
        .zip(&want)
        .position(|(a, b)| a != b)
        .or_else(|| (out.len() != want.len()).then(|| out.len().min(want.len())));
    let summary = format!(
        "{} orphaned symbols, {} dangling, {} unadjusted, {} sections misaligned",
        report.orphaned_symbols.len(),
        report.dangling.len(),
        report.unadjusted.len(),
        report.misaligned.len()
    );
    match at {
        None => {
            println!(
                "{path} without {}: {reference}, byte for byte ({summary})",
                names.join(" ")
            );
            Ok(())
        }
        Some(at) => {
            let _ = std::fs::write(format!("{path}.cut"), &out);
            Err(format!(
                "{path} without {}: differs from {reference} at {at} ({} and {} bytes; {summary})",
                names.join(" "),
                out.len(),
                want.len()
            ))
        }
    }
}

#[derive(Default)]
struct Report {
    objects: usize,
    with_weak: usize,
    atoms: usize,
    bytes_cut: u64,
    orphaned: usize,
    dangling: usize,
    /// Of `dangling`, those in debug sections.
    dangling_debug: usize,
    misaligned: usize,
    unadjusted: usize,
    checked_relocations: usize,
    checked_symbols: usize,
    failures: Vec<String>,
}

impl Report {
    fn add(&mut self, r: Report) {
        self.objects += r.objects;
        self.with_weak += r.with_weak;
        self.atoms += r.atoms;
        self.bytes_cut += r.bytes_cut;
        self.orphaned += r.orphaned;
        self.dangling += r.dangling;
        self.dangling_debug += r.dangling_debug;
        self.misaligned += r.misaligned;
        self.unadjusted += r.unadjusted;
        self.checked_relocations += r.checked_relocations;
        self.checked_symbols += r.checked_symbols;
        self.failures.extend(r.failures);
    }
}

fn smoke_object(name: &str, data: &[u8], r: &mut Report) {
    if !turbonop_macho::is_macho(data) {
        return;
    }
    let Ok(mut o) = Object::parse(data) else {
        return;
    };
    r.objects += 1;
    let Some(text) = o
        .sections()
        .position(|s| s.segment_name() == b"__TEXT" && s.name() == b"__text")
    else {
        return;
    };
    let base = o.section(text).map_or(0, |s| s.addr);
    let mut starts: Vec<(usize, u64)> = o
        .symbols
        .iter()
        .filter(|s| {
            s.in_section()
                && s.is_external()
                && usize::from(s.n_sect) == text + 1
                && s.n_desc & N_WEAK_DEF != 0
                && s.n_desc & N_ALT_ENTRY == 0
        })
        .map(|s| (text, s.n_value - base))
        .collect();
    starts.sort_unstable();
    starts.dedup();
    if starts.is_empty() {
        return;
    }
    r.with_weak += 1;
    r.atoms += starts.len();
    let before: u64 = o.sections().map(turbonop_macho_object::Section::size).sum();
    let report = match cut(&mut o, &starts) {
        Ok(report) => report,
        Err(e) => {
            r.failures.push(format!("{name}: {e}"));
            return;
        }
    };
    r.bytes_cut += before
        - o.sections()
            .map(turbonop_macho_object::Section::size)
            .sum::<u64>();
    r.orphaned += report.orphaned_symbols.len();
    r.dangling += report.dangling.len();
    r.dangling_debug += report
        .dangling
        .iter()
        .filter(|&&(i, _)| {
            o.section(i)
                .is_some_and(turbonop_macho_object::Section::is_debug)
        })
        .count();
    r.misaligned += report.misaligned.len();
    r.unadjusted += report.unadjusted.len();
    let out = match o.write() {
        Ok(out) => out,
        Err(e) => {
            r.failures.push(format!("{name}: not written: {e}"));
            return;
        }
    };
    let back = match Object::parse(&out) {
        Ok(back) => back,
        Err(e) => {
            r.failures.push(format!("{name}: unreadable: {e}"));
            return;
        }
    };
    if back.commands != o.commands {
        r.failures.push(format!("{name}: read back differently"));
    }
    let cputype = back.header.cputype;
    for (i, s) in back.sections().enumerate() {
        for (k, rel) in s.relocations.iter().enumerate() {
            if !reloc::names_section(cputype, rel) || report.dangling.contains(&(i, k)) {
                continue;
            }
            if let Some(a) = back.local_address(i, k) {
                r.checked_relocations += 1;
                let t = back.section(rel.symbolnum as usize - 1);
                if !t.is_some_and(|t| a >= t.addr && a <= t.addr + t.size()) {
                    r.failures.push(format!(
                        "{name}: section {i} relocation {k}: {a:#x} outside its target"
                    ));
                }
            }
        }
    }
    for (k, s) in back.symbols.iter().enumerate() {
        if !s.in_section() {
            continue;
        }
        r.checked_symbols += 1;
        let t = back.section(usize::from(s.n_sect) - 1);
        if !t.is_some_and(|t| s.n_value >= t.addr && s.n_value <= t.addr + t.size()) {
            r.failures.push(format!(
                "{name}: symbol {k} at {:#x} outside its section",
                s.n_value
            ));
        }
    }
}

fn smoke_image(name: &str, data: &[u8], r: &mut Report) {
    if Archive::is_archive(data) {
        let Ok(a) = Archive::parse(data) else {
            return;
        };
        for m in a.files() {
            smoke_object(
                &format!("{name}({})", String::from_utf8_lossy(&m.name)),
                m.data,
                r,
            );
        }
    } else {
        smoke_object(name, data, r);
    }
}

fn smoke(path: &Path) -> Report {
    let mut r = Report::default();
    let Ok(map) = turbonop_mmap::Mmap::open(path) else {
        r.failures.push(format!("{}: unreadable", path.display()));
        return r;
    };
    let data = map.as_slice();
    let name = path.display().to_string();
    match turbonop_macho::fat_archs(data) {
        Some(archs) => {
            for a in archs {
                let image = usize::try_from(a.offset)
                    .ok()
                    .zip(usize::try_from(a.size).ok())
                    .and_then(|(o, s)| data.get(o..o.checked_add(s)?));
                if let Some(image) = image {
                    let arch = turbonop_macho::arch_name(a.cputype, a.cpusubtype);
                    smoke_image(&format!("{name}[{arch}]"), image, &mut r);
                }
            }
        }
        None => smoke_image(&name, data, &mut r),
    }
    r
}

fn find(path: &Path, explicit: bool, out: &mut Vec<PathBuf>) {
    if path.is_dir() {
        let Ok(dir) = std::fs::read_dir(path) else {
            return;
        };
        let mut entries: Vec<PathBuf> = dir.flatten().map(|e| e.path()).collect();
        entries.sort();
        for e in entries {
            find(&e, false, out);
        }
    } else if explicit || path.extension().is_some_and(|e| e == "o" || e == "a") {
        out.push(path.to_path_buf());
    }
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.first().is_some_and(|a| a == "-weak") {
        let mut paths = Vec::new();
        for a in &args[1..] {
            find(Path::new(a), true, &mut paths);
        }
        let mut sized: Vec<(u64, PathBuf)> = paths
            .into_iter()
            .map(|p| (std::fs::metadata(&p).map_or(0, |m| m.len()), p))
            .collect();
        sized.sort_by(|a, b| b.0.cmp(&a.0).then_with(|| a.1.cmp(&b.1)));
        let mut total = Report::default();
        for r in turbonop_par::map_balanced(&sized, 1, |(_, p)| smoke(p)) {
            total.add(r);
        }
        total.failures.sort();
        for f in total.failures.iter().take(50) {
            println!("{f}");
        }
        println!(
            "{} objects, {} with weak definitions in __text: {} atoms cut ({} bytes, with their unwind entries)",
            total.objects, total.with_weak, total.atoms, total.bytes_cut
        );
        println!(
            "  {} orphaned symbols removed, {} relocations dangling ({} in debug sections), {} unadjusted, {} sections misaligned",
            total.orphaned,
            total.dangling,
            total.dangling_debug,
            total.unadjusted,
            total.misaligned
        );
        println!(
            "  {} local relocations and {} symbols checked in their sections; {} failures",
            total.checked_relocations,
            total.checked_symbols,
            total.failures.len()
        );
        if !total.failures.is_empty() {
            std::process::exit(1);
        }
        return;
    }
    let [path, reference, names @ ..] = &args[..] else {
        eprintln!("usage: cut_atoms <object> <reference> <symbol>... | -weak <path>...");
        std::process::exit(2);
    };
    if let Err(e) = oracle(path, reference, names) {
        println!("{e}");
        std::process::exit(1);
    }
}
