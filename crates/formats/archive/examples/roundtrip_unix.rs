//! Re-archives GNU and BSD archives (`.a`) and checks the result is
//! byte-identical: each read back ([`Contents::read`]) and written again
//! with GNU `ar`'s conventions and with `llvm-ar`'s; an archive reproduces
//! if either writes it back. Also checks that the symbol table that tool
//! builds from the members' own symbols ([`member_symbols`]) is the one the
//! archive has.
//!
//!     cargo run --release -p turbonop-archive --example roundtrip_unix -- [-v] [-gnu | -llvm] <dir or .a>...
//!
//! With `-gnu` or `-llvm`, an archive passes only as that tool would write
//! it and index it (without, as either would).
//!
//! Directories are searched for `.a` files. Archives are checked in
//! parallel, the largest first. `-v` lists every archive, not only those
//! that differ.

// Member positions fit in 32 bits (the index's are 32-bit).
#![allow(clippy::cast_possible_truncation)]

use turbonop_global_alloc as _;

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::time::Instant;

use turbonop_archive::Archive;
use turbonop_archive::unix::symbols::member_symbols;
use turbonop_archive::unix::{Archiver, Contents};
use turbonop_archive::write::IndexSymbol;

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
    } else if path.extension().is_some_and(|e| e == "a") {
        out.push(path.to_path_buf());
    }
}

#[derive(Default)]
struct Report {
    bytes: u64,
    members: usize,
    /// Which tools' conventions write it back ("gnu", "llvm", "both").
    writers: &'static str,
    failure: Option<String>,
    /// Why the computed index differs, if it does.
    index: Option<String>,
    not_computed: BTreeMap<String, usize>,
}

fn first_difference(a: &[u8], b: &[u8]) -> Option<usize> {
    a.iter()
        .zip(b)
        .position(|(x, y)| x != y)
        .or_else(|| (a.len() != b.len()).then(|| a.len().min(b.len())))
}

fn show(b: &[u8]) -> String {
    String::from_utf8_lossy(b).escape_debug().to_string()
}

/// The index `archiver` builds from the members (thin members read from
/// their files, relative to the archive's directory).
fn computed_index<'a>(
    contents: &Contents<'a>,
    archiver: Archiver,
    datas: &'a [Vec<u8>],
    r: &mut Report,
) -> Option<Vec<IndexSymbol<'a>>> {
    let mut out = Vec::new();
    let mut objects = false;
    for (i, m) in contents.members.iter().enumerate() {
        let data: &[u8] = if contents.options.thin {
            &datas[i]
        } else {
            m.data
        };
        match member_symbols(data, archiver) {
            Ok(s) => {
                objects |= s.object;
                out.extend(s.symbols.into_iter().map(|name| IndexSymbol {
                    name: name.into(),
                    member: i as u32,
                }));
            }
            Err(e) => {
                *r.not_computed.entry(e.to_string()).or_default() += 1;
                objects = true;
                if let Some(s) = &contents.symbols {
                    out.extend(s.iter().filter(|s| s.member == i as u32).cloned());
                }
            }
        }
    }
    objects.then_some(out)
}

fn check(path: &Path, only: Option<Archiver>) -> Report {
    let map = match turbonop_mmap::Mmap::open(path) {
        Ok(m) => m,
        Err(e) => {
            return Report {
                failure: Some(format!("open: {e}")),
                ..Report::default()
            };
        }
    };
    let data = map.as_slice();
    if turbonop_macho::is_fat(data) {
        return check_universal(data, only);
    }
    check_bytes(data, path.parent().unwrap_or(Path::new(".")), only)
}

/// A universal file of archives: each slice checked as an archive, and the
/// file's header and gaps as LLVM writes them.
fn check_universal(data: &[u8], only: Option<Archiver>) -> Report {
    let mut r = Report {
        bytes: data.len() as u64,
        writers: "both",
        ..Report::default()
    };
    let Some(archs) = turbonop_macho::fat_archs(data) else {
        r.failure = Some("a universal file whose header doesn't read".into());
        return r;
    };
    let shape: Vec<(u32, u32, u64, u32)> = archs
        .iter()
        .map(|a| (a.cputype, a.cpusubtype, a.size, a.align))
        .collect();
    let mut rebuilt = Vec::new();
    if let Some((head, offsets)) = turbonop_macho::fat_layout(&shape) {
        rebuilt.extend_from_slice(&head);
        for (a, &o) in archs.iter().zip(&offsets) {
            let o = usize::try_from(o).unwrap_or(usize::MAX);
            let s = usize::try_from(a.offset).unwrap_or(usize::MAX);
            let e = s.saturating_add(usize::try_from(a.size).unwrap_or(usize::MAX));
            rebuilt.resize(o, 0);
            rebuilt.extend_from_slice(data.get(s..e).unwrap_or(&[]));
        }
    }
    if rebuilt != data {
        r.failure = Some(format!(
            "the universal file's layout differs at {:?}",
            first_difference(data, &rebuilt)
        ));
        return r;
    }
    for a in &archs {
        let s = usize::try_from(a.offset).unwrap_or(usize::MAX);
        let e = s.saturating_add(usize::try_from(a.size).unwrap_or(usize::MAX));
        let slice = check_bytes(data.get(s..e).unwrap_or(&[]), Path::new("."), only);
        r.members += slice.members;
        for (k, v) in slice.not_computed {
            *r.not_computed.entry(k).or_default() += v;
        }
        if slice.writers != "both" {
            r.writers = slice.writers;
        }
        if let Some(f) = slice.failure {
            r.failure = Some(format!("slice {}: {f}", a.cputype));
        }
        if let Some(i) = slice.index {
            r.index = Some(format!("slice {}: {i}", a.cputype));
        }
    }
    r
}

/// An archive's bytes.
fn check_bytes(data: &[u8], dir: &Path, only: Option<Archiver>) -> Report {
    let mut r = Report {
        bytes: data.len() as u64,
        ..Report::default()
    };
    let Ok(ar) = Archive::parse(data) else {
        r.failure = Some("not an archive".into());
        return r;
    };
    let contents = match Contents::read(&ar) {
        Ok(c) => c,
        Err(e) => {
            r.failure = Some(e.to_string());
            return r;
        }
    };
    r.members = contents.members.len();
    let mut ok = Vec::new();
    let mut failures = Vec::new();
    for (archiver, name) in [(Archiver::GnuAr, "gnu"), (Archiver::LlvmAr, "llvm")] {
        match contents.write(archiver) {
            Ok(out) if out == data => ok.push((archiver, name)),
            Ok(out) => failures.push(format!(
                "{name}: differs at {:?}",
                first_difference(&out, data)
            )),
            Err(e) => failures.push(format!("{name}: {e}")),
        }
    }
    r.writers = match ok.as_slice() {
        [] => "neither",
        [(_, n)] => n,
        _ => "both",
    };
    if ok.is_empty() {
        r.failure = Some(failures.join("; "));
        return r;
    }
    // Thin members' data, from their files.
    let datas: Vec<Vec<u8>> = if contents.options.thin {
        contents
            .members
            .iter()
            .map(|m| {
                let p = dir.join(String::from_utf8_lossy(m.entry.name).as_ref());
                let bytes = std::fs::read(&p).unwrap_or_default();
                match m.entry.origin {
                    // A member of another archive: its data there.
                    Some(at) => Archive::parse(&bytes)
                        .ok()
                        .and_then(|a| a.member_at_offset(at))
                        .map(|m| m.data.to_vec())
                        .unwrap_or_default(),
                    None => bytes,
                }
            })
            .collect()
    } else {
        Vec::new()
    };
    let mut index_failures = Vec::new();
    // (Written without one: `S`, nothing to compare.)
    let ok: Vec<(Archiver, &str)> = if contents.symbols.is_none() {
        Vec::new()
    } else {
        ok.into_iter()
            .filter(|&(a, _)| only.is_none_or(|o| o == a))
            .collect()
    };
    if ok.is_empty() && contents.symbols.is_some() {
        r.failure = Some("not written back by the tool asked for".into());
    }
    for &(archiver, name) in &ok {
        let built = computed_index(&contents, archiver, &datas, &mut r);
        if built == contents.symbols {
            index_failures.clear();
            break;
        }
        let (b, h) = (
            built.unwrap_or_default(),
            contents.symbols.clone().unwrap_or_default(),
        );
        let at = b
            .iter()
            .zip(&h)
            .position(|(x, y)| x != y)
            .unwrap_or(b.len().min(h.len()));
        let s = |s: Option<&IndexSymbol<'_>>| {
            s.map_or("(none)".into(), |s| {
                format!("{}@{}", show(&s.name), s.member)
            })
        };
        index_failures.push(format!(
            "{name}: symbol {at} of {}/{}: computed {}, has {}",
            b.len(),
            h.len(),
            s(b.get(at)),
            s(h.get(at))
        ));
    }
    if !index_failures.is_empty() {
        r.index = Some(index_failures.join("; "));
    }
    r
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let verbose = args.iter().any(|a| a == "-v");
    // `-gnu` or `-llvm`: the archives must be that tool's, index and all.
    let only = if args.iter().any(|a| a == "-gnu") {
        Some(Archiver::GnuAr)
    } else if args.iter().any(|a| a == "-llvm") {
        Some(Archiver::LlvmAr)
    } else {
        None
    };
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
    let reports = turbonop_par::map_balanced(&sized, 1, |(_, p)| check(p, only));
    let elapsed = start.elapsed();
    let (mut bytes, mut members, mut reproduced, mut index_ok) = (0u64, 0, 0, 0);
    let mut by_writer: BTreeMap<&str, usize> = BTreeMap::new();
    let mut not_computed: BTreeMap<String, usize> = BTreeMap::new();
    for ((_, path), r) in sized.iter().zip(&reports) {
        bytes += r.bytes;
        members += r.members;
        *by_writer.entry(r.writers).or_default() += 1;
        if r.failure.is_none() {
            reproduced += 1;
        }
        if r.failure.is_none() && r.index.is_none() {
            index_ok += 1;
        }
        for (k, v) in &r.not_computed {
            *not_computed.entry(k.clone()).or_default() += v;
        }
        if verbose || r.failure.is_some() || r.index.is_some() {
            println!(
                "{} [{}, {} members]: {}{}",
                path.display(),
                r.writers,
                r.members,
                r.failure.as_deref().unwrap_or("reproduced"),
                r.index
                    .as_ref()
                    .map_or(String::new(), |i| format!("; index: {i}"))
            );
        }
    }
    println!(
        "{reproduced} of {} archives reproduced ({:.2} GiB, {members} members, {:.1} s)",
        reports.len(),
        bytes as f64 / f64::from(1u32 << 30),
        elapsed.as_secs_f64()
    );
    for (w, n) in &by_writer {
        println!("  written back by {w}: {n}");
    }
    println!(
        "{index_ok} of {} indexes computed from the members",
        reports.len()
    );
    for (why, n) in &not_computed {
        println!("  {n} members not computed: {why}");
    }
}
