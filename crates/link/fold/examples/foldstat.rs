//! What folding does to libraries, summed: `foldstat [--relaxed] <lib>...`
//! rewrites each (in memory) and prints the total report, with each
//! reason's cost in bytes.
use turbonop_fold::rewrite::{Input, Options, Pull, Report, rewrite};
use turbonop_global_alloc as _;

fn main() {
    let mut pull = Pull::Preserve;
    let mut total = Report::default();
    for path in std::env::args().skip(1) {
        if path == "--relaxed" {
            pull = Pull::Relaxed;
            continue;
        }
        let data = std::fs::read(&path).unwrap();
        let archive = turbonop_archive::Archive::parse(&data).unwrap();
        let members: Vec<_> = archive.files().collect();
        let inputs: Vec<Input<'_>> = members
            .iter()
            .map(|m| Input {
                name: &m.name,
                data: m.data,
            })
            .collect();
        let r = rewrite(
            &inputs,
            Options {
                pull,
                weak_externals: false,
                fold: true,
                precomp: turbonop_fold::rewrite::Precomp::None,
            },
        )
        .report;
        total.members += r.members;
        total.units += r.units;
        total.groups += r.groups;
        total.copies_removed += r.copies_removed;
        total.removed_bytes += r.removed_bytes;
        total.bytes_in += r.bytes_in;
        total.bytes_out += r.bytes_out;
        total.synthesized += r.synthesized;
        for (k, v) in r.not_folded {
            *total.not_folded.entry(k).or_default() += v;
        }
        for (k, v) in r.not_folded_bytes {
            *total.not_folded_bytes.entry(k).or_default() += v;
        }
        for (k, v) in r.passed_through {
            *total.passed_through.entry(k).or_default() += v;
        }
    }
    let mib = |b: u64| b as f64 / 1_048_576.0;
    println!(
        "members {}, units {}, groups folded {}, copies removed {} ({:.1} MiB), synthesized {}",
        total.members,
        total.units,
        total.groups,
        total.copies_removed,
        mib(total.removed_bytes),
        total.synthesized
    );
    println!(
        "bytes {:.1} MiB -> {:.1} MiB",
        mib(total.bytes_in),
        mib(total.bytes_out)
    );
    for (k, v) in &total.not_folded {
        println!(
            "not folded: {:>9} copies, {:>9.1} MiB would save: {}",
            v,
            mib(total.not_folded_bytes.get(k).copied().unwrap_or(0)),
            k.describe()
        );
    }
    for (k, v) in &total.passed_through {
        println!("passed through: {v} members: {}", k.describe());
    }
}
