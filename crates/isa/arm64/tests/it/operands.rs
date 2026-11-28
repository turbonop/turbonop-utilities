//! Checks the operands of real words (the golden files' and the access
//! oracle's: every encoding) against what the formatter shows and what the
//! access data says:
//!
//! - every register operand (and list register, memory base and index)
//!   is named in the formatted text, as Microsoft names it;
//! - every immediate is in it, as Microsoft writes it;
//! - a register operand written (or read) is in the write (or read) set;
//! - no operand of the template is lost (except optional ones the word
//!   leaves out).

use std::collections::BTreeMap;

use turbonop_arm64::{Element, Instruction, OperandKind, Reg};

/// Microsoft's name for a register (as format.rs's `gpr_name`).
fn name(r: Reg) -> String {
    let gpr = |n: u8, wide: bool| {
        let special = match (n, wide) {
            (16, true) => "xip0",
            (17, true) => "xip1",
            (18, true) => "xpr",
            (29, true) => "fp",
            (30, true) => "lr",
            (16, false) => "wip0",
            (17, false) => "wip1",
            (18, false) => "wpr",
            (29, false) => "wfp",
            (30, false) => "wlr",
            _ => "",
        };
        if special.is_empty() {
            format!("{}{n}", if wide { 'x' } else { 'w' })
        } else {
            special.to_string()
        }
    };
    match r {
        Reg::X(n) => gpr(n, true),
        Reg::W(n) => gpr(n, false),
        Reg::Sp => "sp".into(),
        Reg::Wsp => "wsp".into(),
        Reg::Xzr => "xzr".into(),
        Reg::Wzr => "wzr".into(),
        Reg::B(n) => format!("b{n}"),
        Reg::H(n) => format!("h{n}"),
        Reg::S(n) => format!("s{n}"),
        Reg::D(n) => format!("d{n}"),
        Reg::Q(n) => format!("q{n}"),
        Reg::V(n) => format!("v{n}"),
        Reg::Z(n) => format!("z{n}"),
        Reg::P(n) => format!("p{n}"),
        Reg::Pn(n) => format!("pn{n}"),
        Reg::Za => "za".into(),
        Reg::ZaTile(e, n) => format!(
            "za{n}.{}",
            match e {
                Element::B => 'b',
                Element::H => 'h',
                Element::S => 's',
                Element::D => 'd',
                Element::Q => 'q',
            }
        ),
        Reg::Zt0 => "zt0".into(),
        r => format!("{r:?}").to_ascii_lowercase(),
    }
}

/// Whether `text` has `token` as a whole token (not part of a longer
/// name or number).
fn has_token(text: &str, token: &str) -> bool {
    let b = text.as_bytes();
    let mut from = 0;
    while let Some(i) = text[from..].find(token) {
        let at = from + i;
        let end = at + token.len();
        let ok_before = at == 0 || !b[at - 1].is_ascii_alphanumeric();
        let ok_after = end >= b.len() || !b[end].is_ascii_alphanumeric();
        if ok_before && ok_after {
            return true;
        }
        from = at + 1;
    }
    false
}

/// An immediate as Microsoft writes it.
fn imm(v: i64) -> String {
    let m = v.unsigned_abs();
    let sign = if v < 0 { "-" } else { "" };
    if m < 10 {
        format!("{sign}{m}")
    } else {
        format!("{sign}0x{m:X}")
    }
}

fn words() -> Vec<(u32, u64)> {
    let mut out = Vec::new();
    for line in include_str!("../data/golden.txt").lines() {
        let mut p = line.split(' ');
        let (Some(address), Some(word)) = (p.next(), p.next()) else {
            continue;
        };
        if address == "-" {
            continue;
        }
        out.push((
            u32::from_str_radix(word, 16).unwrap(),
            u64::from_str_radix(address, 16).unwrap(),
        ));
    }
    for line in include_str!("../data/access.txt").lines() {
        if line.starts_with('#') {
            continue;
        }
        let word = line.split(' ').next().unwrap();
        out.push((u32::from_str_radix(word, 16).unwrap(), 0x1000));
    }
    out
}

/// The registers an operand names.
fn registers(k: &OperandKind) -> Vec<Reg> {
    match k {
        OperandKind::Reg(r) => {
            let mut v = vec![r.reg];
            v.extend(r.select.map(|s| s.0));
            v
        }
        OperandKind::List(l) => l.iter().collect(),
        OperandKind::Mem(m) => {
            let mut v = vec![m.base];
            v.extend(m.index);
            v
        }
        OperandKind::Za(z) => vec![z.index],
        _ => Vec::new(),
    }
}

fn check(inst: &Instruction, address: u64, failures: &mut BTreeMap<String, (usize, String)>) {
    let text = turbonop_arm64::format(inst, address);
    let ops = inst.operands(address);
    let mut fail = |what: String| {
        let e = failures
            .entry(format!("{}: {what}", inst.encoding_name()))
            .or_insert((0, String::new()));
        e.0 += 1;
        if e.1.is_empty() {
            e.1 = format!("{:08X} {text}", inst.word());
        }
    };
    // (TCHANGEB, TCHANGEF, TENTER, and TEXIT's `{, NB}` is left out when
    // its bit is clear: no operand.)
    let optional_nb = ["TCHANGE", "TENTER", "TEXIT"]
        .iter()
        .any(|p| inst.encoding_name().starts_with(p));
    if ops.len() != inst.template_operand_count() && !optional_nb {
        fail(format!(
            "{} of {} operands",
            ops.len(),
            inst.template_operand_count()
        ));
    }
    let (writes, reads) = (inst.written_registers(), inst.read_registers());
    for op in &ops {
        for r in registers(&op.kind) {
            // (Lists show as ranges: their middle registers aren't named.)
            // (An optional zero register is left out: `tlbi vale1is`.)
            let named = has_token(&text, &name(r))
                || matches!(r, Reg::Xzr | Reg::Wzr)
                || matches!(op.kind, OperandKind::List(_))
                || matches!(r, Reg::ZaTile(..));
            if !named {
                fail(format!("{} not in the text", name(r)));
            }
        }
        // A register operand's access is the write and read sets'.
        let regs: Vec<Reg> = match &op.kind {
            OperandKind::Reg(r) => vec![r.reg],
            OperandKind::List(l) => l.iter().collect(),
            _ => Vec::new(),
        };
        for r in regs {
            if r.id().is_none() {
                continue;
            }
            if op.access.writes() && !writes.contains(r) {
                fail(format!("{} written, not in the write set", name(r)));
            }
            if op.access.reads() && !reads.contains(r) {
                fail(format!("{} read, not in the read set", name(r)));
            }
        }
        if let OperandKind::Imm(v) = op.kind {
            let t = imm(v);
            // (SYS's CRn and CRm show as C<n>, in decimal.)
            if !has_token(&text, &t)
                && !text.contains(&format!("#{t}"))
                && !has_token(&text, &format!("C{v}"))
            {
                fail(format!("immediate {t} not in the text"));
            }
        }
    }
}

#[test]
fn operands_agree_with_the_text_and_access() {
    let mut failures = BTreeMap::new();
    let mut count = 0;
    for (word, address) in words() {
        let Some(inst) = turbonop_arm64::decode(word).ok() else {
            continue;
        };
        count += 1;
        check(&inst, address, &mut failures);
    }
    let n: usize = failures.values().map(|v| v.0).sum();
    let report: Vec<String> = failures
        .iter()
        .take(80)
        .map(|(k, (n, e))| format!("{k}: {n} (e.g. {e})"))
        .collect();
    assert!(count > 40_000, "too few words: {count}");
    assert!(
        failures.is_empty(),
        "{n} failures in {} kinds, of {count} words:\n{}",
        failures.len(),
        report.join("\n")
    );
}
