//! Reads every Mach-O object (in `.o` files, in `.a` archives' members,
//! and in universal files' images of either) into the model and writes it
//! back, and checks the result is byte-identical. Then lays each out
//! afresh, as LLVM does, and checks whether that is byte-identical too
//! (whether the writer lays the object out as the LLVM that wrote it did)
//! and whether it reads back to the same model (the same commands,
//! sections, relocations, and symbols, by value). It also checks whether
//! the sections are at the addresses LLVM gives them, whether the
//! addresses local relocations encode are in their target sections, and
//! whether moving every section and moving it back gives the object read
//! (the relocations' bytes, symbols, and link-edit data moved and moved
//! back). It tallies the features the objects have.
//!
//!     cargo run --release -p turbonop-macho-object --example roundtrip -- [-v] <dir, .o, or .a>...
//!
//! Files are checked in parallel, the largest first.

use turbonop_global_alloc as _;

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::time::Instant;

use turbonop_archive::Archive;
use turbonop_macho_object::{Command, Contents, Endian, Object, reloc};

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

#[derive(Default)]
struct Report {
    objects: usize,
    bytes: u64,
    identical: usize,
    relaid_identical: usize,
    /// Relaid out identically but for padding after the object (an
    /// archive member's).
    relaid_identical_padded: usize,
    relaid_same_model: usize,
    llvm_addresses: usize,
    /// Objects moved and moved back to the same bytes.
    moved_back: usize,
    /// Local relocations whose encoded address was decoded, and of those,
    /// the ones in (or at the end of) their target sections.
    local_decoded: usize,
    local_in_target: usize,
    /// Local relocations not decoded (an encoding not handled).
    local_undecoded: BTreeMap<String, usize>,
    /// Objects with each feature.
    features: BTreeMap<String, usize>,
    /// Each failure: its kind, then the object and details.
    failures: Vec<(String, String)>,
    unread: BTreeMap<String, usize>,
}

impl Report {
    fn add(&mut self, r: Report) {
        self.objects += r.objects;
        self.bytes += r.bytes;
        self.identical += r.identical;
        self.relaid_identical += r.relaid_identical;
        self.relaid_identical_padded += r.relaid_identical_padded;
        self.relaid_same_model += r.relaid_same_model;
        self.llvm_addresses += r.llvm_addresses;
        self.moved_back += r.moved_back;
        self.local_decoded += r.local_decoded;
        self.local_in_target += r.local_in_target;
        for (k, v) in r.local_undecoded {
            *self.local_undecoded.entry(k).or_default() += v;
        }
        for (k, v) in r.features {
            *self.features.entry(k).or_default() += v;
        }
        self.failures.extend(r.failures);
        for (k, v) in r.unread {
            *self.unread.entry(k).or_default() += v;
        }
    }
}

fn first_difference(a: &[u8], b: &[u8]) -> Option<usize> {
    a.iter()
        .zip(b)
        .position(|(x, y)| x != y)
        .or_else(|| (a.len() != b.len()).then(|| a.len().min(b.len())))
}

/// What part of the file read an offset is in.
fn part(o: &Object<'_>, data: &[u8], at: usize) -> String {
    let hsize = usize::try_from(o.header.size()).unwrap_or(0);
    if at < hsize {
        return "the header".into();
    }
    let sizeofcmds = data
        .get(20..24)
        .map_or(0, |b| u32::from_le_bytes([b[0], b[1], b[2], b[3]]) as usize);
    if at < hsize + sizeofcmds {
        let mut p = hsize;
        for c in &o.commands {
            let size = data
                .get(p + 4..p + 8)
                .map_or(0, |b| u32::from_le_bytes([b[0], b[1], b[2], b[3]]) as usize);
            if at < p + size {
                return format!("command {:#x} at +{}", c.cmd(), at - p);
            }
            p += size;
        }
        return "the load commands".into();
    }
    "the data after the commands".into()
}

/// Whether two models hold the same values (offsets and the string table
/// aside, which a fresh layout writes its own way).
fn same_model(a: &Object<'_>, b: &Object<'_>) -> Result<(), String> {
    if a.header != b.header {
        return Err("the header".into());
    }
    if a.commands.len() != b.commands.len() {
        return Err("the command count".into());
    }
    for (i, (x, y)) in a.commands.iter().zip(&b.commands).enumerate() {
        if x != y {
            return Err(format!("command {i} ({:#x})", x.cmd()));
        }
    }
    if a.symbols.len() != b.symbols.len() {
        return Err("the symbol count".into());
    }
    for (i, (s, t)) in a.symbols.iter().zip(&b.symbols).enumerate() {
        let value =
            |s: &turbonop_macho_object::Symbol<'_>| s.indirect.is_none().then_some(s.n_value);
        if (&s.name, s.n_type, s.n_sect, s.n_desc, value(s), &s.indirect)
            != (&t.name, t.n_type, t.n_sect, t.n_desc, value(t), &t.indirect)
        {
            return Err(format!("symbol {i}"));
        }
    }
    if a.indirect_symbols != b.indirect_symbols {
        return Err("the indirect symbols".into());
    }
    Ok(())
}

fn check_object(name: &str, data: &[u8], r: &mut Report) {
    if !turbonop_macho::is_macho(data) {
        return;
    }
    r.objects += 1;
    r.bytes += data.len() as u64;
    let o = match Object::parse(data) {
        Ok(o) => o,
        Err(e) => {
            *r.unread.entry(e.to_string()).or_default() += 1;
            return;
        }
    };
    match o.write() {
        Ok(out) if out == data => r.identical += 1,
        Ok(out) => {
            let at = first_difference(&out, data).unwrap_or(0);
            r.failures.push((
                "written back differently".into(),
                format!(
                    "{name}: at {at} in {} (keeps layout: {})",
                    part(&o, data, at),
                    o.keeps_layout()
                ),
            ));
        }
        Err(e) => r
            .failures
            .push(("not written back".into(), format!("{name}: {e}"))),
    }
    let addresses = o.llvm_addresses();
    if o.sections().zip(&addresses).all(|(s, &a)| s.addr == a) {
        r.llvm_addresses += 1;
    } else {
        let i = o
            .sections()
            .zip(&addresses)
            .position(|(s, &a)| s.addr != a)
            .unwrap_or(0);
        r.failures.push((
            "a section not at LLVM's address".into(),
            format!(
                "{name}: section {i} ({}) at {:#x}, LLVM's {:#x}",
                String::from_utf8_lossy(o.section(i).map_or(&b""[..], |s| s.name())),
                o.section(i).map_or(0, |s| s.addr),
                addresses[i]
            ),
        ));
    }
    features(&o, r);
    locals(name, &o, r);
    moves(name, &o, data, r);
    let mut fresh = o.clone();
    fresh.relayout();
    let out = match fresh.write() {
        Ok(out) => out,
        Err(e) => {
            r.failures
                .push(("not laid out afresh".into(), format!("{name}: {e}")));
            return;
        }
    };
    if out == data {
        r.relaid_identical += 1;
    } else if data.len() > out.len()
        && data[..out.len()] == out[..]
        && data[out.len()..].iter().all(|&b| b == 0 || b == b'\n')
    {
        r.relaid_identical_padded += 1;
    } else {
        let at = first_difference(&out, data).unwrap_or(0);
        r.failures.push((
            "laid out afresh differently".into(),
            format!(
                "{name}: at {at} of {} (fresh {}) in {}",
                data.len(),
                out.len(),
                part(&o, data, at)
            ),
        ));
    }
    match Object::parse(&out) {
        Ok(back) => match same_model(&o, &back) {
            Ok(()) => r.relaid_same_model += 1,
            Err(e) => r.failures.push((
                "laid out afresh, read back differently".into(),
                format!("{name}: {e}"),
            )),
        },
        Err(e) => r
            .failures
            .push(("laid out afresh, unreadable".into(), format!("{name}: {e}"))),
    }
}

fn features(o: &Object<'_>, r: &mut Report) {
    let mut f: Vec<String> = Vec::new();
    f.push(format!(
        "{} {}",
        turbonop_macho::arch_name(o.header.cputype, o.header.cpusubtype),
        if o.is_64() { "64-bit" } else { "32-bit" }
    ));
    if o.endian() == Endian::Big {
        f.push("big-endian".into());
    }
    for c in &o.commands {
        match c {
            Command::LinkeditData { cmd, .. } => f.push(format!("link-edit data {cmd:#x}")),
            Command::Other { cmd, .. } => f.push(format!("command {cmd:#x}")),
            _ => {}
        }
    }
    if !o.indirect_symbols.is_empty() {
        f.push("indirect symbols".into());
    }
    let relocs = || o.sections().flat_map(|s| s.relocations.iter());
    if relocs().any(turbonop_macho_object::Relocation::is_scattered) {
        f.push("scattered relocations".into());
    }
    if relocs().any(turbonop_macho_object::Relocation::is_local) {
        f.push("local relocations".into());
    }
    if o.sections().any(|s| {
        reloc::fixups(o.header.cputype, &s.relocations)
            .iter()
            .any(|g| g.len() == 2)
    }) {
        f.push("relocation pairs".into());
    }
    if o.sections()
        .any(|s| matches!(s.contents, Contents::Zerofill(_)))
    {
        f.push("zero-fill sections".into());
    }
    if o.symbols.iter().any(turbonop_macho_object::Symbol::is_stab) {
        f.push("debugger symbols".into());
    }
    if o.symbols
        .iter()
        .any(|s| !s.is_stab() && s.n_type & turbonop_macho::consts::N_TYPE == 0xA)
    {
        f.push("N_INDR symbols".into());
    }
    if o.symbols.iter().any(|s| s.is_undefined() && s.n_value != 0) {
        f.push("common symbols".into());
    }
    if !o.symbols_in_order() {
        f.push("symbols not in the dynamic symbol table's order".into());
    }
    if o.dysymtab().is_some_and(|d| *d != o.computed_dysymtab()) {
        f.push("dysymtab ranges not the computed ones".into());
    }
    f.sort();
    f.dedup();
    for k in f {
        *r.features.entry(k).or_default() += 1;
    }
}

/// Decodes every local relocation's encoded address, and checks it is in
/// its target section.
fn locals(name: &str, o: &Object<'_>, r: &mut Report) {
    let cputype = o.header.cputype;
    for (i, s) in o.sections().enumerate() {
        for (k, rel) in s.relocations.iter().enumerate() {
            if !reloc::names_section(cputype, rel) {
                continue;
            }
            match o.local_address(i, k) {
                Some(a) => {
                    r.local_decoded += 1;
                    let t = o.section(rel.symbolnum as usize - 1);
                    if t.is_some_and(|t| a >= t.addr && a <= t.addr + t.size()) {
                        r.local_in_target += 1;
                    } else {
                        r.failures.push((
                            "a local relocation's address outside its target".into(),
                            format!(
                                "{name}: section {i} relocation {k} (type {}) at {a:#x}",
                                rel.kind
                            ),
                        ));
                    }
                }
                None => {
                    *r.local_undecoded
                        .entry(format!(
                            "type {} pcrel {} length {}",
                            rel.kind, rel.pcrel, rel.length
                        ))
                        .or_default() += 1;
                }
            }
        }
    }
}

/// Moves every section (each by a different amount), checks the decoded
/// local addresses moved with their targets, moves them back, and checks
/// the object is the one read.
fn moves(name: &str, o: &Object<'_>, data: &[u8], r: &mut Report) {
    let mut moved = o.clone();
    let old: Vec<(u64, u64)> = o.sections().map(|s| (s.addr, s.size())).collect();
    let new: Vec<u64> = old
        .iter()
        .enumerate()
        .map(|(i, &(a, _))| a + 0x1_0000 * (i as u64 + 1))
        .collect();
    let before: Vec<Vec<Option<u64>>> = o
        .sections()
        .enumerate()
        .map(|(i, s)| {
            (0..s.relocations.len())
                .map(|k| o.local_address(i, k))
                .collect()
        })
        .collect();
    let unadjusted = moved.move_sections(&new, &old);
    for (i, s) in moved.sections().enumerate() {
        for (k, rel) in s.relocations.iter().enumerate() {
            if let Some(a) = before[i][k] {
                let t = rel.symbolnum as usize - 1;
                let want = a.wrapping_add(new[t].wrapping_sub(old[t].0));
                if moved.local_address(i, k) != Some(want) {
                    r.failures.push((
                        "a local address not moved with its target".into(),
                        format!("{name}: section {i} relocation {k}"),
                    ));
                }
            }
        }
    }
    let back: Vec<u64> = old.iter().map(|&(a, _)| a).collect();
    let now: Vec<(u64, u64)> = moved.sections().map(|s| (s.addr, s.size())).collect();
    moved.move_sections(&back, &now);
    let mut fresh = o.clone();
    fresh.relayout();
    let mut moved_fresh = moved.clone();
    moved_fresh.relayout();
    match (moved_fresh.write(), fresh.write()) {
        (Ok(a), Ok(b)) if a == b && unadjusted.is_empty() => r.moved_back += 1,
        (Ok(a), Ok(b)) if a == b => r.failures.push((
            "moved with relocations unadjusted".into(),
            format!("{name}: {} relocations", unadjusted.len()),
        )),
        (Ok(a), Ok(b)) => r.failures.push((
            "moved and moved back differently".into(),
            format!(
                "{name}: at {:?} of {}",
                first_difference(&a, &b),
                data.len()
            ),
        )),
        _ => r
            .failures
            .push(("moved, not written".into(), name.to_string())),
    }
}

fn check_image(name: &str, data: &[u8], r: &mut Report) {
    if Archive::is_archive(data) {
        let Ok(a) = Archive::parse(data) else {
            r.failures
                .push(("unreadable archive".into(), name.to_string()));
            return;
        };
        if a.is_thin() {
            return;
        }
        for m in a.files() {
            let member = format!("{name}({})", String::from_utf8_lossy(&m.name));
            check_object(&member, m.data, r);
        }
    } else {
        check_object(name, data, r);
    }
}

fn check(path: &Path) -> Report {
    let mut r = Report::default();
    let Ok(map) = turbonop_mmap::Mmap::open(path) else {
        r.failures
            .push(("unreadable file".into(), path.display().to_string()));
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
                let arch = turbonop_macho::arch_name(a.cputype, a.cpusubtype);
                match image {
                    Some(image) => check_image(&format!("{name}[{arch}]"), image, &mut r),
                    None => r
                        .failures
                        .push(("universal image past the file".into(), name.clone())),
                }
            }
        }
        None => check_image(&name, data, &mut r),
    }
    r
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let verbose = args.iter().any(|a| a == "-v");
    let mut paths = Vec::new();
    for a in args.iter().filter(|a| !a.starts_with('-')) {
        find(Path::new(a), true, &mut paths);
    }
    let mut sized: Vec<(u64, PathBuf)> = paths
        .into_iter()
        .map(|p| (std::fs::metadata(&p).map_or(0, |m| m.len()), p))
        .collect();
    sized.sort_by(|a, b| b.0.cmp(&a.0).then_with(|| a.1.cmp(&b.1)));
    let start = Instant::now();
    let reports = turbonop_par::map_balanced(&sized, 1, |(_, p)| check(p));
    let elapsed = start.elapsed();
    let mut total = Report::default();
    for r in reports {
        total.add(r);
    }
    total.failures.sort();
    let mut kinds: BTreeMap<&str, usize> = BTreeMap::new();
    let mut shown: BTreeMap<&str, usize> = BTreeMap::new();
    for (kind, what) in &total.failures {
        *kinds.entry(kind).or_default() += 1;
        let n = shown.entry(kind).or_default();
        if verbose || *n < 10 {
            println!("{kind}: {what}");
        }
        *n += 1;
    }
    println!(
        "{} objects ({:.1} MiB, {} files, {:.1} s)",
        total.objects,
        total.bytes as f64 / f64::from(1u32 << 20),
        sized.len(),
        elapsed.as_secs_f64()
    );
    println!("  {} written back byte-identically", total.identical);
    println!(
        "  {} laid out afresh byte-identically ({} more but for padding after them)",
        total.relaid_identical, total.relaid_identical_padded
    );
    println!(
        "  {} laid out afresh and read back the same",
        total.relaid_same_model
    );
    println!(
        "  {} with every section at LLVM's address",
        total.llvm_addresses
    );
    println!(
        "  {} moved and moved back to the same bytes",
        total.moved_back
    );
    println!(
        "  {} local relocations' addresses decoded, {} of them in their target sections",
        total.local_decoded, total.local_in_target
    );
    for (what, n) in &total.local_undecoded {
        println!(
            "    {n} local relocations not decoded (a SUBTRACTOR pair's halves, or an encoding not handled): {what}"
        );
    }
    println!("  objects with:");
    for (what, n) in &total.features {
        println!("    {n} {what}");
    }
    for (kind, n) in &kinds {
        println!("  {n} {kind}");
    }
    for (why, n) in &total.unread {
        println!("  {n} not read: {why}");
    }
}
