//! Checks the CodeView of every COFF object under the paths given (objects,
//! and the members of libraries), on every core:
//!
//! - every `.debug$T` and `.debug$P` record's index fields
//!   ([`type_refs`]) and every `.debug$S` symbol's and subsection's fields
//!   ([`symbol_refs`], [`c13::fields`]): each index in range (an object's
//!   types and ids share one index space; an `LF_PRECOMP` user's own start
//!   after its precompiled header's), and naming an id record exactly when
//!   the field is an id (a precompiled header's records are found by its
//!   signature among the objects read); each string-table offset at a
//!   string, each checksum offset at an entry, and each file the binary
//!   annotations name too;
//! - the relocations of each `.debug$S`: every one at an address field this
//!   knows of (`SECREL` at the offset, `SECTION` at the section, `ADDR32NB`
//!   at frame data's base), so none falls where no field is known;
//! - writing everything back: each type record ([`write::push_type`] of its
//!   [`write::unpadded`] body), each subsection ([`c13::Contents`]), and
//!   each section, byte for byte;
//! - renumbering every index ([`remap`]) and back, which must give the same
//!   bytes.
//!
//! It prints counts by kind, and every failure by class with examples.
//!
//! ```text
//! cargo run --release -p turbonop-codeview --example corpus -- [options] <dir or file>...
//!   --pdbutil <llvm-pdbutil.exe>  also compare symbols' indices with llvm-pdbutil's on a sample
//!   --sample <n>                  how many objects to compare so (default 200)
//!   --bench                       also time renumbering alone
//! ```

// Offsets in objects and records are 32-bit.
#![allow(clippy::cast_possible_truncation)]

mod check;

use check::*;
use std::collections::BTreeMap;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use turbonop_codeview::c13;
use turbonop_codeview::lines::subsection;
use turbonop_codeview::remap;
use turbonop_codeview::symbol_refs::{self, Field};
use turbonop_codeview::symbols::kind as sk;
use turbonop_codeview::types::leaf;
use turbonop_global_alloc as _;
use turbonop_pe::PeFile;

/// An object compared with llvm-pdbutil: the symbols compared, how many
/// had the same indices, those llvm-pdbutil printed none of (by kind), the
/// differences, and whether llvm-pdbutil stopped early (it crashes on some
/// objects; the last record it printed may be cut short, so it isn't
/// compared).
type Compared = (usize, usize, BTreeMap<String, u64>, Vec<String>, bool);

/// An object to check: a file, or a library's member.
struct Item {
    file: usize,
    start: usize,
    len: usize,
    name: String,
    /// A file of its own (not a member), so llvm-pdbutil can read it.
    standalone: bool,
}

fn walk(path: &Path, out: &mut Vec<PathBuf>) {
    if path.is_dir() {
        let Ok(rd) = std::fs::read_dir(path) else {
            return;
        };
        let mut entries: Vec<PathBuf> = rd.filter_map(|e| e.ok().map(|e| e.path())).collect();
        entries.sort();
        for e in entries {
            walk(&e, out);
        }
    } else if let Some(ext) = path.extension().and_then(|e| e.to_str()) {
        let ext = ext.to_ascii_lowercase();
        if ext == "obj" || ext == "lib" || ext == "o" {
            out.push(path.to_path_buf());
        }
    }
}

/// A record's index values as llvm-pdbutil prints them, per symbol
/// record: (kind name, indices).
fn pdbutil_records(text: &str) -> Vec<(String, Vec<u32>)> {
    const LABELS: [&str; 7] = [
        "type = ",
        "type=",
        "inlinee = ",
        "BuildId = ",
        "function = ",
        "callee: ",
        "caller: ",
    ];
    let mut out: Vec<(String, Vec<u32>)> = Vec::new();
    for line in text.lines() {
        let t = line.trim_start();
        // A record: "<offset> | S_KIND [size = n] ...".
        let header = t
            .split_once(" | ")
            .filter(|(o, rest)| o.chars().all(|c| c.is_ascii_digit()) && rest.starts_with("S_"));
        let rest = if let Some((_, rest)) = header {
            let kind = rest.split(' ').next().unwrap_or("").to_string();
            out.push((kind, Vec::new()));
            rest.split_once(']').map_or("", |(_, r)| r)
        } else {
            t
        };
        let Some(last) = out.last_mut() else { continue };
        if matches!(
            last.0.as_str(),
            "S_ENVBLOCK"
                | "S_COMPILE2"
                | "S_COMPILE3"
                | "S_OBJNAME"
                | "S_ANNOTATION"
                | "S_UNAMESPACE"
        ) {
            continue;
        }
        let mut labels: Vec<(usize, usize)> = Vec::new();
        for l in LABELS.iter().chain(&["inlinee: "]) {
            let mut from = 0;
            while let Some(i) = rest[from..].find(l) {
                labels.push((from + i, l.len()));
                from += i + l.len();
            }
        }
        labels.sort_unstable();
        for (i, n) in labels {
            let after = rest[i + n..].trim_start_matches('`');
            // (Index 0 is printed by name alone.)
            if after.starts_with("<no type>") {
                last.1.push(0);
            } else if let Some(h) = after.find("0x") {
                let digits: String = after[h + 2..]
                    .chars()
                    .take_while(char::is_ascii_hexdigit)
                    .collect();
                if let Ok(v) = u32::from_str_radix(&digits, 16) {
                    last.1.push(v);
                }
            }
        }
    }
    out
}

/// Ours, for the same records: each symbol's type and id indices in
/// order.
fn our_records(data: &[u8]) -> Vec<(String, Vec<u32>)> {
    let mut out = Vec::new();
    let Ok(pe) = PeFile::parse(data) else {
        return out;
    };
    for s in pe.sections() {
        if pe.section_name(s) != b".debug$S" {
            continue;
        }
        let Some(bytes) = section_bytes(&pe, s) else {
            continue;
        };
        let Ok(subs) = c13::section_subsections(bytes) else {
            continue;
        };
        for sub in subs {
            if sub.kind != subsection::DEBUG_S_SYMBOLS {
                continue;
            }
            for r in c13::symbol_records(sub.data).unwrap_or_default() {
                let mut v = Vec::new();
                let _ = symbol_refs::fields(r.kind, r.data, |f| {
                    if let Field::Type(o) | Field::Id(o) = f {
                        v.push(le32(r.data, o as usize));
                    }
                });
                out.push((sk::name(r.kind).to_string(), v));
            }
        }
    }
    out
}

fn main() {
    let mut args = std::env::args().skip(1);
    let mut paths = Vec::new();
    let mut pdbutil: Option<String> = None;
    let mut sample = 200usize;
    let mut bench = false;
    while let Some(a) = args.next() {
        match a.as_str() {
            "--pdbutil" => pdbutil = args.next(),
            "--sample" => sample = args.next().and_then(|s| s.parse().ok()).unwrap_or(sample),
            "--bench" => bench = true,
            _ => paths.push(PathBuf::from(a)),
        }
    }
    let mut files = Vec::new();
    for p in &paths {
        walk(p, &mut files);
    }
    let t0 = std::time::Instant::now();
    let maps: Vec<Option<turbonop_mmap::Mmap>> =
        turbonop_par::map_balanced(&files, 16, |f| turbonop_mmap::Mmap::open(f).ok());
    // Every object: a file, or a member of a library.
    let mut items = Vec::new();
    for (i, (f, m)) in files.iter().zip(&maps).enumerate() {
        let Some(m) = m else {
            eprintln!("can't read {}", f.display());
            continue;
        };
        let data = m.as_slice();
        if turbonop_archive::Archive::is_archive(data) {
            let Ok(a) = turbonop_archive::Archive::parse(data) else {
                eprintln!("can't read the archive {}", f.display());
                continue;
            };
            for member in a.files() {
                items.push(Item {
                    file: i,
                    start: member.data_offset as usize,
                    len: member.data.len(),
                    name: format!("{}({})", f.display(), String::from_utf8_lossy(&member.name)),
                    standalone: false,
                });
            }
        } else {
            items.push(Item {
                file: i,
                start: 0,
                len: data.len(),
                name: f.display().to_string(),
                standalone: true,
            });
        }
    }
    let bytes_of = |it: &Item| -> &[u8] {
        maps[it.file]
            .as_ref()
            .map_or(&[][..], |m| &m.as_slice()[it.start..it.start + it.len])
    };
    // The precompiled headers, by signature.
    let found: Vec<Option<(u32, Vec<u16>)>> =
        turbonop_par::map_balanced(&items, 8, |it| pch_of(bytes_of(it)));
    let mut pchs: HashMap<u32, Pch> = HashMap::new();
    let mut pch_conflicts = 0;
    for (sig, kinds) in found.into_iter().flatten() {
        match pchs.get(&sig) {
            Some(k) if **k != kinds => pch_conflicts += 1,
            Some(_) => {}
            None => {
                pchs.insert(sig, Arc::new(kinds));
            }
        }
    }
    let t1 = std::time::Instant::now();
    let results: Vec<Stats> = turbonop_par::map_balanced(&items, 4, |it| {
        let mut st = Stats {
            inputs: 1,
            ..Stats::default()
        };
        check_object(bytes_of(it), &it.name, &pchs, &mut st);
        st
    });
    let mut st = Stats::default();
    for r in results {
        st.merge(r);
    }
    let checked = t1.elapsed();

    println!(
        "{} files, {} objects to read ({} not COFF objects), {:.1} GiB of objects",
        files.len(),
        items.len(),
        st.not_objects,
        st.bytes as f64 / f64::from(1 << 30)
    );
    println!(
        "{} precompiled headers ({} signatures with different records); {} LF_PRECOMP users, {} type-server objects",
        pchs.len(),
        pch_conflicts,
        st.precomp_users,
        st.type_servers
    );
    println!(
        "sections: {} .debug$T, {} .debug$P, {} .debug$S ({:.2} GiB of types, {:.2} GiB of .debug$S)",
        st.debug_t,
        st.debug_p,
        st.debug_s,
        st.type_bytes as f64 / f64::from(1 << 30),
        st.debug_s_bytes as f64 / f64::from(1 << 30)
    );
    println!(
        "records: {} types, {} symbols; {} relocations; {} forward type references; {} indices into unread precompiled headers",
        st.type_records, st.symbol_records, st.relocations, st.forward_refs, st.pch_unresolved
    );
    println!(
        "read in {:.1?}, checked in {:.1?} on {} threads",
        t1 - t0,
        checked,
        turbonop_par::threads()
    );
    println!("\nfields:");
    for (k, v) in &st.fields {
        println!("  {k:16} {v}");
    }
    println!("\nsubsection kinds:");
    for (k, v) in &st.subsection_kinds {
        println!("  {k:#010x} {v}");
    }
    println!("\nsymbol kinds:");
    for (k, v) in &st.symbol_kinds {
        println!("  {k:#06x} {:40} {v}", sk::name(*k));
    }
    println!("\ntype kinds:");
    for (k, v) in &st.type_kinds {
        println!("  {k:#06x} {:16} {v}", leaf::name(*k));
    }
    println!("\naddress fields without a relocation:");
    for (k, (v, ex)) in &st.unrelocated_addresses {
        println!("  {k:48} {v}");
        for e in ex.iter().take(2) {
            println!("      {e}");
        }
    }
    println!("\nlinks (parent, end, next) that aren't 0:");
    for (k, v) in &st.nonzero_links {
        println!("  {:40} {v}", sk::name(*k));
    }
    println!("\nfailures:");
    if st.failures.is_empty() {
        println!("  none");
    }
    for (k, (n, ex)) in &st.failures {
        println!("  {n:10} {k}");
        for e in ex {
            println!("               {e}");
        }
    }

    if bench {
        // Renumbering alone: copy each object's .debug$T and .debug$S and
        // patch every index.
        let t = std::time::Instant::now();
        let done: Vec<u64> = turbonop_par::map_balanced(&items, 4, |it| {
            let data = bytes_of(it);
            let Ok(pe) = PeFile::parse(data) else {
                return 0;
            };
            let mut n = 0;
            for s in pe.sections() {
                let name = pe.section_name(s);
                let Some(bytes) = section_bytes(&pe, s) else {
                    continue;
                };
                let mut copy = bytes.to_vec();
                let ok = match name {
                    b".debug$T" | b".debug$P" => {
                        remap::debug_t(&mut copy, &mut |i: u32, _| Some(i + 1)).is_ok()
                    }
                    b".debug$S" => remap::debug_s(&mut copy, &mut |i: u32, _| Some(i + 1)).is_ok(),
                    _ => continue,
                };
                if ok {
                    n += copy.len() as u64;
                }
            }
            n
        });
        let e = t.elapsed();
        let total: u64 = done.iter().sum();
        println!(
            "\nrenumbering: {:.2} GiB in {:.2?}: {:.2} GiB/s on {} threads",
            total as f64 / f64::from(1 << 30),
            e,
            total as f64 / f64::from(1 << 30) / e.as_secs_f64(),
            turbonop_par::threads()
        );
    }

    if let Some(exe) = pdbutil {
        let standalone: Vec<&Item> = items.iter().filter(|i| i.standalone).collect();
        let step = (standalone.len() / sample.max(1)).max(1);
        let picked: Vec<&Item> = standalone
            .iter()
            .step_by(step)
            .take(sample)
            .copied()
            .collect();
        let results: Vec<Compared> = turbonop_par::map_balanced(&picked, 1, |it| {
            let out = std::process::Command::new(&exe)
                .args(["dump", "-symbols", &it.name])
                .output();
            let Ok(out) = out else {
                return (
                    0,
                    0,
                    BTreeMap::new(),
                    vec![format!("{}: can't run", it.name)],
                    false,
                );
            };
            let theirs = pdbutil_records(&String::from_utf8_lossy(&out.stdout));
            let ours = our_records(bytes_of(it));
            let mut same = 0;
            let mut skipped = BTreeMap::new();
            let mut diffs = Vec::new();
            let early = theirs.len() < ours.len();
            if theirs.len() > ours.len() {
                diffs.push(format!(
                    "{}: {} records to llvm-pdbutil's {}",
                    it.name,
                    ours.len(),
                    theirs.len()
                ));
            }
            let n = if early {
                theirs.len().saturating_sub(1)
            } else {
                theirs.len()
            };
            for (i, (a, b)) in ours.iter().zip(&theirs).take(n).enumerate() {
                if a.0 != b.0 {
                    diffs.push(format!("{} record {i}: {} vs {}", it.name, a.0, b.0));
                    break;
                }
                if a.1 == b.1 {
                    same += 1;
                } else if b.1.is_empty() {
                    *skipped.entry(a.0.clone()).or_default() += 1;
                } else if diffs.len() < EXAMPLES {
                    diffs.push(format!(
                        "{} record {i} {}: ours {:x?}, llvm-pdbutil's {:x?}",
                        it.name, a.0, a.1, b.1
                    ));
                }
            }
            (n, same, skipped, diffs, early)
        });
        let mut records = 0;
        let mut same = 0;
        let mut skipped: BTreeMap<String, u64> = BTreeMap::new();
        let mut diffs = Vec::new();
        let mut early = 0;
        for (r, s, k, d, e) in results {
            early += usize::from(e);
            records += r;
            same += s;
            for (n, c) in k {
                *skipped.entry(n).or_default() += c;
            }
            diffs.extend(d);
        }
        println!(
            "\nllvm-pdbutil, {} objects: {records} symbols compared, {same} with the same indices; llvm-pdbutil stopped early in {early}",
            picked.len()
        );
        println!("  where llvm-pdbutil printed none (and we have some):");
        for (k, v) in &skipped {
            println!("    {k:40} {v}");
        }
        println!("  differences: {}", diffs.len());
        for d in diffs.iter().take(40) {
            println!("    {d}");
        }
    }
}
