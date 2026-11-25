//! Re-archives libraries and checks the result is byte-identical: each
//! library read back ([`Contents::read`]: its members in its order, their
//! header fields, its index, and its tables' conventions) and laid out and
//! written again. Also checks that the index `lib.exe` (or `llvm-lib`,
//! whichever the tables look like) would build from the members' own
//! symbols is the index the library has, and that every import object
//! writes back as it was.
//!
//!     cargo run --release -p turbonop-archive --example roundtrip -- [-v | -write] <dir or .lib>...
//!
//! Directories are searched for `.lib` files. Libraries are checked in
//! parallel, the largest first (`TURBONOP_THREADS` sets how many at
//! once). `-v` lists every library, not only those that differ. `-write`
//! instead times writing each one again (its members copied in parallel).

// Offsets and counts fit in 32 bits here: the libraries' offsets are
// 32-bit, and they are read on 64-bit hosts.
#![allow(clippy::cast_possible_truncation)]

use turbonop_global_alloc as _;

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::time::Instant;

use turbonop_archive::symbols::{Tool, is_import_descriptor, member_symbols};
use turbonop_archive::write::{
    Contents, Index, IndexSymbol, MemberSymbol, MemberSymbols, SizeLimit, Style,
};
use turbonop_archive::{Archive, Member, MemberKind};
use turbonop_pe::import_object::ImportObject;

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
        .is_some_and(|e| e.eq_ignore_ascii_case("lib"))
    {
        out.push(path.to_path_buf());
    }
}

#[derive(Default)]
struct Report {
    bytes: u64,
    style: &'static str,
    members: usize,
    /// Why the archive didn't reproduce, if it didn't.
    failure: Option<String>,
    /// Why the computed index differs, if it does.
    index: Option<String>,
    /// Members whose symbols weren't computed, by why.
    not_computed: BTreeMap<String, usize>,
    imports: usize,
    import_failures: Vec<String>,
}

/// Where `a` and `b` first differ.
fn first_difference(a: &[u8], b: &[u8]) -> Option<usize> {
    a.iter()
        .zip(b)
        .position(|(x, y)| x != y)
        .or_else(|| (a.len() != b.len()).then(|| a.len().min(b.len())))
}

fn show(b: &[u8]) -> String {
    String::from_utf8_lossy(b).escape_debug().to_string()
}

fn check(path: &Path) -> Report {
    match turbonop_mmap::Mmap::open(path) {
        Ok(map) => check_data(map.as_slice()),
        Err(e) => Report {
            failure: Some(format!("open: {e}")),
            ..Report::default()
        },
    }
}

/// The symbols `list` gives member `i`.
fn mine<'a>(list: &[IndexSymbol<'a>], i: u32) -> Vec<MemberSymbol<'a>> {
    list.iter()
        .filter(|s| s.member == i)
        .map(|s| MemberSymbol::defined(s.name.clone()))
        .collect()
}

fn sorted<'a>(v: &Option<Vec<IndexSymbol<'a>>>) -> Vec<IndexSymbol<'a>> {
    let mut v = v.clone().unwrap_or_default();
    v.sort_by(|a, b| a.name.cmp(&b.name));
    v
}

/// Compares the archive `contents` lays out with `data`, part by part.
fn compare(contents: &Contents<'_>, data: &[u8], tables: &[Member<'_>]) -> Option<String> {
    let layout = match contents.layout(SizeLimit::FourGiB) {
        Ok(l) => l,
        Err(e) => return Some(format!("layout: {e}")),
    };
    let head = layout.head();
    if let Some(at) = first_difference(head, data.get(..head.len()).unwrap_or(data)) {
        let (name, from) = tables
            .iter()
            .rev()
            .find(|t| t.header_offset as usize <= at)
            .map_or(("signature".into(), 0), |t| {
                (show(&t.name), t.header_offset as usize)
            });
        let context = |b: &[u8]| show(&b[at.saturating_sub(16)..(at + 16).min(b.len())]);
        return Some(format!(
            "head differs at {at:#x} (in table {name}, at +{:#x}): {:?} vs {:?}",
            at - from,
            context(head),
            context(data)
        ));
    }
    let mut span = Vec::new();
    for (i, m) in contents.members.iter().enumerate() {
        let at = layout.offset(i) as usize;
        span.clear();
        span.resize(layout.span(i) as usize, 0);
        layout.write_member(i, m.data, &mut span);
        let original = data
            .get(at..(at + span.len()).min(data.len()))
            .unwrap_or(&[]);
        if let Some(d) = first_difference(&span, original) {
            let what = if d < 60 { "header" } else { "data or padding" };
            return Some(format!(
                "member {i} ({}) at {at:#x}: {what} differs at +{d}",
                show(m.name)
            ));
        }
    }
    if layout.size() != data.len() as u64 {
        return Some(format!("size {:#x}, not {:#x}", layout.size(), data.len()));
    }
    // The whole archive at once, for libraries that fit easily.
    if data.len() < 256 << 20 {
        match contents.write(SizeLimit::FourGiB) {
            Ok(out) if out == data => {}
            Ok(out) => {
                return Some(format!(
                    "write() differs at {:?}",
                    first_difference(&out, data)
                ));
            }
            Err(e) => return Some(format!("write(): {e}")),
        }
    }
    None
}

/// Compares the index the tool would build from the members' symbols with
/// the one the library has.
fn compare_index(contents: &Contents<'_>, r: &mut Report) -> Option<String> {
    let index = &contents.index;
    let tool = match contents.style {
        Some(Style::LlvmLib) => Tool::LlvmLib,
        _ => Tool::LibExe,
    };
    let ec = index.ec_symbols.is_some();
    // lib /DEF and llvm-lib /DEF index an ARM64EC import library's
    // descriptors in both tables.
    let import_library = contents
        .members
        .iter()
        .any(|m| ImportObject::is_import_object(m.data));
    // The members in the order the second linker member lists them.
    let order: Vec<u32> = match &index.member_list {
        Some(list) => list.iter().flatten().copied().collect(),
        None => (0..contents.members.len() as u32).collect(),
    };
    let mut computed = Vec::with_capacity(order.len());
    for i in order {
        let m = &contents.members[i as usize];
        match member_symbols(m.data, tool, ec) {
            Ok(mut s) => {
                if ec && import_library {
                    let both: Vec<_> = s
                        .symbols
                        .iter()
                        .filter(|n| is_import_descriptor(&n.name))
                        .cloned()
                        .collect();
                    s.ec_symbols.extend(both);
                }
                computed.push((i, s));
            }
            Err(e) => {
                *r.not_computed.entry(e.to_string()).or_default() += 1;
                // Its symbols as the library has them, so the rest compare.
                computed.push((
                    i,
                    MemberSymbols {
                        symbols: mine(&index.symbols, i),
                        ec_symbols: mine(index.ec_symbols.as_deref().unwrap_or(&[]), i),
                    },
                ));
            }
        }
    }
    let built = Index::build(computed, tool, ec);
    if built.symbols != index.symbols {
        let at = built
            .symbols
            .iter()
            .zip(&index.symbols)
            .position(|(a, b)| a != b)
            .unwrap_or(built.symbols.len().min(index.symbols.len()));
        let name = |s: Option<&IndexSymbol<'_>>| {
            s.map_or("(none)".into(), |s| {
                format!("{}@{}", show(&s.name), s.member)
            })
        };
        return Some(format!(
            "symbol {at} of {}/{}: computed {}, has {}",
            built.symbols.len(),
            index.symbols.len(),
            name(built.symbols.get(at)),
            name(index.symbols.get(at))
        ));
    }
    if sorted(&built.ec_symbols) != sorted(&index.ec_symbols) {
        return Some("EC symbols differ".into());
    }
    None
}

fn check_data(data: &[u8]) -> Report {
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
    r.style = match contents.style {
        _ if contents.conventions.padding == 0 => "lib (NUL padding)",
        Some(Style::LibExe { .. }) => "lib",
        Some(Style::LibExeBrepro) => "lib /Brepro",
        Some(Style::LlvmLib) => "llvm-lib",
        None => "other",
    };
    let tables: Vec<Member<'_>> = ar
        .members()
        .take_while(|m| m.kind != MemberKind::File)
        .collect();
    r.failure = compare(&contents, data, &tables);
    r.index = compare_index(&contents, &mut r);

    // Import objects.
    for m in &contents.members {
        if ImportObject::is_import_object(m.data) {
            r.imports += 1;
            match ImportObject::parse(m.data).map(|i| i.to_vec()) {
                Ok(Ok(out)) if out == m.data => {}
                Ok(Ok(out)) => r.import_failures.push(format!(
                    "{}: differs at {:?}",
                    show(m.name),
                    first_difference(&out, m.data)
                )),
                Ok(Err(e)) => r.import_failures.push(format!("{}: {e}", show(m.name))),
                Err(e) => r.import_failures.push(format!("{}: {e}", show(m.name))),
            }
        }
    }
    r
}

/// `-write`: times writing each library again, one at a time, its members
/// copied in parallel into their places in one buffer (the reading and
/// the comparing not counted).
fn time_writes(paths: &[PathBuf]) {
    let (mut bytes, mut seconds) = (0u64, 0f64);
    for p in paths {
        let map = turbonop_mmap::Mmap::open(p).unwrap();
        let data = map.as_slice();
        let ar = Archive::parse(data).unwrap();
        let contents = Contents::read(&ar).unwrap();
        let mut out = vec![0u8; data.len()];
        let start = Instant::now();
        let layout = contents.layout(SizeLimit::FourGiB).unwrap();
        let laid = start.elapsed();
        // The data in pieces of at most 4 MiB, so that the threads' shares
        // are even when the members aren't.
        let mut pieces = layout.prepare_mut(&mut out, |i| contents.members[i].data, 4 << 20);
        turbonop_par::for_each_chunk_mut(&mut pieces, 1, |part| {
            for (d, s) in part {
                d.copy_from_slice(s);
            }
        });
        let took = start.elapsed().as_secs_f64();
        assert!(out == data, "{}", p.display());
        bytes += data.len() as u64;
        seconds += took;
        println!(
            "{}: {:.0} MiB in {:.3} s (layout {:.3} s), {:.2} GiB/s",
            p.display(),
            data.len() as f64 / f64::from(1u32 << 20),
            took,
            laid.as_secs_f64(),
            data.len() as f64 / f64::from(1u32 << 30) / took
        );
    }
    println!(
        "{:.2} GiB written in {seconds:.2} s: {:.2} GiB/s",
        bytes as f64 / f64::from(1u32 << 30),
        bytes as f64 / f64::from(1u32 << 30) / seconds
    );
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let verbose = args.iter().any(|a| a == "-v");
    let mut paths = Vec::new();
    for a in args.iter().filter(|a| !a.starts_with('-')) {
        find(Path::new(a), &mut paths);
    }
    if args.iter().any(|a| a == "-write") {
        time_writes(&paths);
        return;
    }
    // The largest first, so no thread is left with one at the end.
    let mut sized: Vec<(u64, PathBuf)> = paths
        .into_iter()
        .map(|p| (std::fs::metadata(&p).map_or(0, |m| m.len()), p))
        .collect();
    sized.sort_by(|a, b| b.0.cmp(&a.0).then_with(|| a.1.cmp(&b.1)));
    let start = Instant::now();
    let reports = turbonop_par::map_balanced(&sized, 1, |(_, p)| check(p));
    let elapsed = start.elapsed();

    let mut by_style: BTreeMap<&str, (usize, usize)> = BTreeMap::new();
    let (mut bytes, mut reproduced, mut index_ok, mut imports, mut import_ok, mut members) =
        (0u64, 0, 0, 0, 0, 0);
    let mut not_computed: BTreeMap<String, usize> = BTreeMap::new();
    for ((_, path), r) in sized.iter().zip(&reports) {
        bytes += r.bytes;
        members += r.members;
        let s = by_style.entry(r.style).or_default();
        s.0 += 1;
        if r.failure.is_none() {
            reproduced += 1;
            s.1 += 1;
        }
        if r.index.is_none() {
            index_ok += 1;
        }
        imports += r.imports;
        import_ok += r.imports - r.import_failures.len();
        for (k, v) in &r.not_computed {
            *not_computed.entry(k.clone()).or_default() += v;
        }
        if verbose || r.failure.is_some() || r.index.is_some() || !r.import_failures.is_empty() {
            println!(
                "{} [{}, {} members]: {}{}",
                path.display(),
                r.style,
                r.members,
                r.failure.as_deref().unwrap_or("reproduced"),
                r.index
                    .as_ref()
                    .map_or(String::new(), |i| format!("; index: {i}"))
            );
            for f in r.import_failures.iter().take(5) {
                println!("    import: {f}");
            }
        }
    }
    println!(
        "{reproduced} of {} libraries reproduced ({:.1} GiB, {members} members, {:.1} s)",
        reports.len(),
        bytes as f64 / f64::from(1u32 << 30),
        elapsed.as_secs_f64()
    );
    for (style, (n, ok)) in &by_style {
        println!("  {style}: {ok} of {n}");
    }
    println!(
        "{index_ok} of {} indexes computed from the members",
        reports.len()
    );
    for (why, n) in &not_computed {
        println!("  {n} members not computed: {why}");
    }
    println!("{import_ok} of {imports} import objects written back");
}
