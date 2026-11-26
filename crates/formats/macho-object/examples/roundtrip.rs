//! Reads every Mach-O object (in `.o` files, in `.a` archives' members,
//! and in universal files' images of either) into the model and writes it
//! back, and checks the result is byte-identical. Then lays each out
//! afresh, as LLVM does, and checks whether that is byte-identical too
//! (whether the writer lays the object out as the LLVM that wrote it did)
//! and whether it reads back to the same model (the same commands,
//! sections, relocations, and symbols, by value). It also checks whether
//! the sections are at the addresses LLVM gives them.
//!
//!     cargo run --release -p turbonop-macho-object --example roundtrip -- [-v] <dir, .o, or .a>...
//!
//! Files are checked in parallel, the largest first.

use turbonop_global_alloc as _;

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::time::Instant;

use turbonop_archive::Archive;
use turbonop_macho_object::Object;

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
    let hsize = o.header.size() as usize;
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
        if (&s.name, s.n_type, s.n_sect, s.n_desc, s.n_value)
            != (&t.name, t.n_type, t.n_sect, t.n_desc, t.n_value)
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
    for (kind, n) in &kinds {
        println!("  {n} {kind}");
    }
    for (why, n) in &total.unread {
        println!("  {n} not read: {why}");
    }
}
