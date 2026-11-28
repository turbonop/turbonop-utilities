//! Checks the access data against LLVM's (tests/data/access.txt, from
//! llvm-mc's disassembler and MCInstrDesc: see tools/arm64/access_golden.py):
//! every register LLVM says a word writes, turbonop-arm64 says it writes
//! (and reads, reads), and loads and stores agree. Where turbonop-arm64
//! says more, it is by its own rules (FPSR, system registers, ...): those
//! extras are counted, not failed.

use std::collections::BTreeMap;

use turbonop_arm64::{Reg, RegSet};

/// A register by the name the golden file uses (`X3`, `SP`, `V7`, `ZA`).
fn reg(name: &str) -> Reg {
    let num = |p: &str| name.strip_prefix(p).and_then(|n| n.parse::<u8>().ok());
    if let Some(n) = num("X") {
        return Reg::X(n);
    }
    if let Some(n) = num("V") {
        return Reg::V(n);
    }
    if let Some(n) = num("P") {
        return Reg::P(n);
    }
    match name {
        "SP" => Reg::Sp,
        "FFR" => Reg::Ffr,
        "ZA" => Reg::Za,
        "ZT0" => Reg::Zt0,
        "NZCV" => Reg::Nzcv,
        "FPCR" => Reg::Fpcr,
        "FPSR" => Reg::Fpsr,
        "FPMR" => Reg::Fpmr,
        "VG" => Reg::Vg,
        _ => panic!("a register name the test doesn't know: {name}"),
    }
}

fn set(list: &str) -> RegSet {
    list.split(',').filter(|r| *r != "-").map(reg).collect()
}

fn names(s: RegSet) -> String {
    s.iter()
        .map(|r| format!("{r:?}"))
        .collect::<Vec<_>>()
        .join(",")
}

#[test]
fn agrees_with_llvm() {
    let text = include_str!("../data/access.txt");
    let mut failures = Vec::new();
    let mut count = 0;
    // Registers turbonop-arm64 adds, by what: (count, an example).
    let mut extra: BTreeMap<String, (usize, String)> = BTreeMap::new();
    for line in text.lines().filter(|l| !l.starts_with('#')) {
        let p: Vec<&str> = line.split(' ').collect();
        let word = u32::from_str_radix(p[0], 16).unwrap();
        let (llvm, writes, reads, mem) = (
            p[1],
            set(&p[2][2..]),
            set(&p[3][2..]),
            p[4][2..].to_string(),
        );
        count += 1;
        let Ok(inst) = turbonop_arm64::decode(word) else {
            failures.push(format!("{word:08X} ({llvm}): doesn't decode"));
            continue;
        };
        let ours_w = inst.written_registers();
        let ours_r = inst.read_registers();
        let what = format!(
            "{word:08X} {} ({}, LLVM's {llvm})",
            turbonop_arm64::format(&inst, 0x1000),
            inst.llvm_record()
        );
        // Where LLVM is wrong (tools/arm64/access.py's DROP): MRS writes
        // only Xt, not NZCV.
        let mut writes = writes;
        if llvm == "MRS" {
            writes.remove(Reg::Nzcv);
        }
        let missing_w = writes.difference(ours_w);
        let missing_r = reads.difference(ours_r);
        if !missing_w.is_empty() {
            failures.push(format!("{what}: doesn't write {}", names(missing_w)));
        }
        if !missing_r.is_empty() {
            failures.push(format!("{what}: doesn't read {}", names(missing_r)));
        }
        // (LLVM decodes allocated hints, YIELD and WFE and the rest, as
        // HINT, which it says may load and store: they don't. Nor do load
        // exclusives store.)
        let hint = llvm == "HINT" && inst.llvm_record() == "HINT_allocated";
        let exclusive = llvm.starts_with("LDXR")
            || llvm.starts_with("LDAXR")
            || llvm.starts_with("LDXP")
            || llvm.starts_with("LDAXP");
        let (load, store) = (
            mem.contains("load") && !hint,
            mem.contains("store") && !hint && !exclusive,
        );
        if (load && !inst.reads_memory()) || (store && !inst.writes_memory()) {
            failures.push(format!(
                "{what}: LLVM's memory access {mem}, ours load {} store {}",
                inst.reads_memory(),
                inst.writes_memory()
            ));
        }
        for (kind, s) in [
            ("writes", ours_w.difference(writes)),
            ("reads", ours_r.difference(reads)),
        ] {
            for r in s.iter() {
                let e = extra
                    .entry(format!("{kind} {r:?}"))
                    .or_insert_with(|| (0, what.clone()));
                e.0 += 1;
            }
        }
        if (inst.reads_memory() && !load) || (inst.writes_memory() && !store) {
            let e = extra
                .entry("memory".into())
                .or_insert_with(|| (0, what.clone()));
            e.0 += 1;
        }
    }
    // How many encodings the oracle's words cover (those no word of which
    // LLVM decodes, it can't).
    let all: std::collections::BTreeSet<&str> = turbonop_arm64::encodings().map(|e| e.0).collect();
    let covered: std::collections::BTreeSet<&str> = text
        .lines()
        .filter(|l| !l.starts_with('#'))
        .filter_map(|l| u32::from_str_radix(l.split(' ').next()?, 16).ok())
        .filter_map(|w| turbonop_arm64::decode(w).ok())
        .map(|i| i.encoding_name())
        .collect();
    let uncovered: Vec<&&str> = all.difference(&covered).collect();
    println!(
        "{} of {} encodings covered; not: {:?}",
        covered.len(),
        all.len(),
        uncovered
    );
    // (Counted by the encodings the words decode as: a base encoding whose
    // words all decode as its preferred alias, LSLV's as LSL's, MOVZ's as
    // MOV's, counts as uncovered. 96.7% when written.)
    assert!(
        covered.len() * 1000 >= all.len() * 965,
        "the oracle covers too few encodings"
    );
    let mut report = String::new();
    for (k, (n, example)) in &extra {
        report.push_str(&format!("  {k}: {n} (e.g. {example})\n"));
    }
    println!("{count} words; beyond LLVM's:\n{report}");
    assert!(count > 25_000, "access golden file too small");
    assert!(
        failures.is_empty(),
        "{} of {count} words disagree:\n{}",
        failures.len(),
        failures[..failures.len().min(40)].join("\n")
    );
}
