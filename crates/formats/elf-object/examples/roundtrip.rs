//! Reads every ELF object (in `.o` files and in `.a` archives' members)
//! into the model and writes it back, and checks the result is
//! byte-identical. Then, with `-relayout`, also lays each out afresh, reads
//! that back, and checks its model is the original's (the same sections,
//! symbols, relocations, and groups, by value).
//!
//!     cargo run --release -p turbonop-elf-object --example roundtrip -- [-v] [-relayout] <dir, .o, or .a>...
//!
//! Files are checked in parallel, the largest first.

use turbonop_global_alloc as _;

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::time::Instant;

use turbonop_archive::Archive;
use turbonop_elf_object::{Data, Object};

fn find(path: &Path, out: &mut Vec<PathBuf>) {
    if path.is_dir() {
        let Ok(dir) = std::fs::read_dir(path) else {
            return;
        };
        let mut entries: Vec<PathBuf> = dir.flatten().map(|e| e.path()).collect();
        entries.sort();
        for e in entries {
            find(&e, out);
        }
    } else if path
        .extension()
        .is_some_and(|e| e == "o" || e == "a" || e == "obj" || e == "dwo")
    {
        out.push(path.to_path_buf());
    }
}

#[derive(Default)]
struct Report {
    objects: usize,
    bytes: u64,
    identical: usize,
    relaid: usize,
    failures: Vec<String>,
    unread: BTreeMap<String, usize>,
}

fn first_difference(a: &[u8], b: &[u8]) -> Option<usize> {
    a.iter()
        .zip(b)
        .position(|(x, y)| x != y)
        .or_else(|| (a.len() != b.len()).then(|| a.len().min(b.len())))
}

/// Whether two models hold the same values (string tables aside, which a
/// fresh layout writes its own way, and offsets).
fn same_model(a: &Object<'_>, b: &Object<'_>) -> Result<(), String> {
    if a.header != b.header || a.sections.len() != b.sections.len() {
        return Err("header or section count".into());
    }
    for (i, (x, y)) in a.sections.iter().zip(&b.sections).enumerate() {
        if (
            x.name.as_ref(),
            x.kind,
            x.flags,
            x.addr,
            x.link,
            x.info,
            x.addralign,
            x.entsize,
        ) != (
            y.name.as_ref(),
            y.kind,
            y.flags,
            y.addr,
            y.link,
            y.info,
            y.addralign,
            y.entsize,
        ) {
            return Err(format!("section {i}'s header"));
        }
        match (&x.data, &y.data) {
            (Data::Symbols(p), Data::Symbols(q)) => {
                if p.len() != q.len()
                    || p.iter().zip(q).any(|(s, t)| {
                        (&s.name, s.value, s.size, s.info, s.other, s.shndx, s.xindex)
                            != (&t.name, t.value, t.size, t.info, t.other, t.shndx, t.xindex)
                    })
                {
                    return Err(format!("section {i}'s symbols"));
                }
            }
            (Data::Bytes(_), Data::Bytes(_)) if x.kind == 3 => {}
            (p, q) if p != q => return Err(format!("section {i}'s contents")),
            _ => {}
        }
    }
    Ok(())
}

fn check_object(name: &str, data: &[u8], relayout: bool, r: &mut Report) {
    if !data.starts_with(b"\x7fELF") {
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
        Ok(out) => r.failures.push(format!(
            "{name}: differs at {:?} (keeps layout: {})",
            first_difference(&out, data),
            o.keeps_layout()
        )),
        Err(e) => r.failures.push(format!("{name}: {e}")),
    }
    if relayout {
        let mut fresh = o.clone();
        fresh.relayout();
        match fresh.write() {
            Ok(out) => match Object::parse(&out) {
                Ok(back) => match same_model(&o, &back) {
                    Ok(()) => r.relaid += 1,
                    Err(e) => r.failures.push(format!("{name}: relaid out: {e}")),
                },
                Err(e) => r
                    .failures
                    .push(format!("{name}: relaid out, unreadable: {e}")),
            },
            Err(e) => r.failures.push(format!("{name}: relayout: {e}")),
        }
    }
}

fn check(path: &Path, relayout: bool) -> Report {
    let mut r = Report::default();
    let Ok(map) = turbonop_mmap::Mmap::open(path) else {
        r.failures.push(format!("{}: unreadable", path.display()));
        return r;
    };
    let data = map.as_slice();
    if Archive::is_archive(data) {
        let Ok(a) = Archive::parse(data) else {
            return r;
        };
        if a.is_thin() {
            return r;
        }
        for m in a.files() {
            let name = format!("{}({})", path.display(), String::from_utf8_lossy(&m.name));
            check_object(&name, m.data, relayout, &mut r);
        }
    } else {
        check_object(&path.display().to_string(), data, relayout, &mut r);
    }
    r
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let verbose = args.iter().any(|a| a == "-v");
    let relayout = args.iter().any(|a| a == "-relayout");
    let mut paths = Vec::new();
    for a in args.iter().filter(|a| !a.starts_with('-')) {
        find(Path::new(a), &mut paths);
    }
    let mut sized: Vec<(u64, PathBuf)> = paths
        .into_iter()
        .map(|p| (std::fs::metadata(&p).map_or(0, |m| m.len()), p))
        .collect();
    sized.sort_by(|a, b| b.0.cmp(&a.0).then_with(|| a.1.cmp(&b.1)));
    let start = Instant::now();
    let reports = turbonop_par::map_balanced(&sized, 1, |(_, p)| check(p, relayout));
    let elapsed = start.elapsed();
    let mut total = Report::default();
    for r in reports {
        total.objects += r.objects;
        total.bytes += r.bytes;
        total.identical += r.identical;
        total.relaid += r.relaid;
        total.failures.extend(r.failures);
        for (k, v) in r.unread {
            *total.unread.entry(k).or_default() += v;
        }
    }
    for f in total
        .failures
        .iter()
        .take(if verbose { usize::MAX } else { 50 })
    {
        println!("{f}");
    }
    println!(
        "{} of {} objects written back byte-identically ({:.2} GiB, {} files, {:.1} s)",
        total.identical,
        total.objects,
        total.bytes as f64 / f64::from(1u32 << 30),
        sized.len(),
        elapsed.as_secs_f64()
    );
    if relayout {
        println!(
            "{} of {} laid out afresh and read back the same",
            total.relaid, total.objects
        );
    }
    for (why, n) in &total.unread {
        println!("  {n} not read: {why}");
    }
}
