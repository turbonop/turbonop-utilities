//! Compiles `tables/access.rules` into each form's access data: how its
//! operands are accessed, what it reads and writes implicitly, and its
//! flags.
//!
//! # Rule syntax
//!
//! ```text
//! <mnemonic>[/<selector>] ... : [<operand accesses>] [<implicit>...] [f=<read>/<written>/<undefined>]
//! ```
//!
//! A rule applies to every form of its mnemonics, or with a selector, to
//! the forms whose operands are written exactly so (`mul/rAX,Ev`) or that
//! have that many operands (`imul/1`). A selector beats none, and an exact
//! mnemonic beats a prefix (`vfmadd1*`); the longest prefix wins.
//!
//! Operand accesses are comma-separated, in operand order: `r`, `w`, `rw`,
//! `cw` (a conditional write: `cmov`), `rcw`, and `n` (neither: an
//! address-only memory operand, as for `lea`). Operands past the list are
//! read. `-` takes the default (below).
//!
//! Implicit operands are `<register>:<access>`, with `?rep` after the access
//! when only a `rep` prefix makes it (`aCX:rw?rep`). Registers are named as
//! in the opcode tables (`al`, `ax`, `edx`, `xmm0`, `ds`, `st`), or sized
//! like an operand: `rAX`..`rDI` (operand size), `aAX`..`aDI` (address
//! size), `sSP`/`sBP` (stack size); or are `r8`..`r31`, `st0`..`st7`, `cr0`, `cr8`,
//! `mm0`, the system state `mxcsr` `fpsw` `fpcw` `fptw` `xcr0` `msr` `pkru`
//! `ssp` `gdtr` `idtr` `ldtr` `tr` `tilecfg` `uif`, or a group: `gpr*` (all
//! general-purpose registers), `st*`, `mm*`, `zmm*`, `vec16` (the vector
//! registers VEX can reach), `vec8` (`zmm0`..`zmm7`), `k*`, `tmm*`, `bnd*`. `pair0` is the other
//! register of operand 0's even/odd pair (`vp2intersectd` writes both), and `kmask` the EVEX
//! opmask (otherwise implicitly read).
//!
//! Memory: `push`, `push*N` (N stack slots of the operand size, and `sSP`
//! read and written; `push*0`, a size not known in advance), `pop`, `pop*N`,
//! `pop@bp` (`leave`'s pop, at the frame pointer), and
//! `mem@<reg>:<access>/<bytes>` (memory at a register, `mem@aDI:w/16`, or
//! at the address in an operand's register, `mem@op0:w/64`; no size:
//! unknown). Macros: `x87` (`fpsw:w`), `x87push` and `x87pop`
//! (`st*`, `fpsw`, and `fptw` written), `x87pop2`.
//!
//! Flags are letters, `o s z a p c d i t`, or comma-separated words, `ac`
//! `nt` `rf` `vm` `vif` `vip` `id` `iopl` `all`; `cc` reads the condition
//! the mnemonic ends with (`cmovbe`: CF and ZF). Undefined flags count as
//! written.
//!
//! # Defaults
//!
//! Every form of the general-purpose and x87 tables needs a rule; it is an
//! error not to have one. SIMD forms without one write their first operand
//! (and read it too, for a legacy-encoded SSE or MMX form with a register
//! destination and a source, which is destructive) and read the rest; they
//! have no implicit operands and no flags. A rule that lists no operand
//! accesses reads every operand.

use std::collections::HashMap;
use std::fmt::Write as _;

/// What the generator needs to know about a form.
pub struct FormInfo<'a> {
    pub mnemonic: &'a str,
    /// The operands as written (for an APX new-data-destination form, not
    /// counting the destination it adds).
    pub raw_ops: &'a [String],
    pub map: usize,
    pub origin: &'a str,
    /// An APX new-data-destination form, whose added first operand is
    /// written and whose other operands are all read.
    pub ndd_dest: bool,
}

const RW_NONE: u8 = 0;
const RW_R: u8 = 1;
const RW_W: u8 = 2;
const RW_RW: u8 = 3;
const RW_CW: u8 = 4;
const RW_RCW: u8 = 5;
/// Implicit only with a `rep` prefix.
const RW_REP: u8 = 0x80;

fn rw_code(s: &str) -> Option<u8> {
    Some(match s {
        "n" => RW_NONE,
        "r" => RW_R,
        "w" => RW_W,
        "rw" => RW_RW,
        "cw" => RW_CW,
        "rcw" => RW_RCW,
        _ => return None,
    })
}

/// Register classes, matching `RegClass` in src/register.rs.
mod class {
    pub const GPR8: u8 = 0;
    pub const GPR8_HIGH: u8 = 1;
    pub const GPR16: u8 = 2;
    pub const GPR32: u8 = 3;
    pub const GPR64: u8 = 4;
    pub const SEGMENT: u8 = 5;
    pub const CONTROL: u8 = 6;
    pub const ST: u8 = 8;
    pub const MMX: u8 = 9;
    pub const XMM: u8 = 10;
    pub const ZMM: u8 = 12;
    pub const MASK: u8 = 13;
    pub const BOUND: u8 = 14;
    pub const TMM: u8 = 15;
    pub const SYSTEM: u8 = 18;
}

/// The system state registers, matching `Register::MXCSR`... in
/// src/register.rs.
const SYSTEM: [&str; 14] = [
    "mxcsr", "fpsw", "fpcw", "fptw", "xcr0", "msr", "pkru", "ssp", "gdtr", "idtr", "ldtr", "tr",
    "tilecfg", "uif",
];

/// One implicit operand: `Imp { kind, a, b, rw }` (see src/spec.rs).
#[derive(Clone, PartialEq, Eq, Hash)]
struct Imp {
    kind: &'static str,
    a: u8,
    b: u8,
    rw: u8,
}

impl Imp {
    fn new(kind: &'static str, a: u8, b: u8, rw: u8) -> Self {
        Self { kind, a, b, rw }
    }

    fn emit(&self) -> String {
        format!(
            "Imp {{ kind: ImpKind::{}, a: {}, b: {}, rw: {} }}",
            self.kind, self.a, self.b, self.rw
        )
    }
}

/// A register token's implicit operand, with access `rw`.
fn register(name: &str, rw: u8) -> Option<Imp> {
    const GPR8: [&str; 8] = ["al", "cl", "dl", "bl", "spl", "bpl", "sil", "dil"];
    const HIGH8: [&str; 4] = ["ah", "ch", "dh", "bh"];
    const GPR16: [&str; 8] = ["ax", "cx", "dx", "bx", "sp", "bp", "si", "di"];
    const GPR32: [&str; 8] = ["eax", "ecx", "edx", "ebx", "esp", "ebp", "esi", "edi"];
    const GPR64: [&str; 8] = ["rax", "rcx", "rdx", "rbx", "rsp", "rbp", "rsi", "rdi"];
    const SEG: [&str; 6] = ["es", "cs", "ss", "ds", "fs", "gs"];
    const ACC: [&str; 8] = ["AX", "CX", "DX", "BX", "SP", "BP", "SI", "DI"];
    let fixed: [(&[&str], u8); 6] = [
        (&GPR8, class::GPR8),
        (&HIGH8, class::GPR8_HIGH),
        (&GPR16, class::GPR16),
        (&GPR32, class::GPR32),
        (&GPR64, class::GPR64),
        (&SEG, class::SEGMENT),
    ];
    for (list, c) in fixed {
        if let Some(i) = list.iter().position(|&n| n == name) {
            return Some(Imp::new("Reg", c, i as u8, rw));
        }
    }
    let (first, rest) = name.split_at(name.len().min(1));
    if let Some(i) = ACC.iter().position(|&n| n == rest) {
        let kind = match first {
            "r" => "GprOs",
            "a" => "GprAs",
            "s" if rest == "SP" || rest == "BP" => "GprSs",
            _ => return None,
        };
        return Some(Imp::new(kind, i as u8, 0, rw));
    }
    if let Some(i) = SYSTEM.iter().position(|&n| n == name) {
        return Some(Imp::new("Reg", class::SYSTEM, i as u8, rw));
    }
    let group = |c: u8, n: u8| Some(Imp::new("Group", c, n, rw));
    if let Some(i) = name.strip_prefix('r').and_then(|n| n.parse::<u8>().ok())
        && (8..32).contains(&i)
    {
        return Some(Imp::new("Reg", class::GPR64, i, rw));
    }
    match name {
        "st" | "st0" => return Some(Imp::new("Reg", class::ST, 0, rw)),
        "xmm0" => return Some(Imp::new("Reg", class::XMM, 0, rw)),
        "cr0" => return Some(Imp::new("Reg", class::CONTROL, 0, rw)),
        "cr8" => return Some(Imp::new("Reg", class::CONTROL, 8, rw)),
        "mm0" => return Some(Imp::new("Reg", class::MMX, 0, rw)),
        // (0x80: eight outside 64-bit mode.)
        "gpr*" => return group(class::GPR64, 0x80 | 16),
        "st*" => return group(class::ST, 8),
        "mm*" => return group(class::MMX, 8),
        "zmm*" => return group(class::ZMM, 32),
        "vec16" => return group(class::ZMM, 0x80 | 16),
        "vec8" => return group(class::ZMM, 8),
        "k*" => return group(class::MASK, 8),
        "tmm*" => return group(class::TMM, 8),
        "bnd*" => return group(class::BOUND, 4),
        "pair0" => return Some(Imp::new("Pair", 0, 0, rw)),
        "kmask" => return Some(Imp::new("MaskField", 0, 0, rw)),
        _ => {}
    }
    if let Some(i) = name.strip_prefix("st")
        && let Ok(i) = i.parse::<u8>()
        && i < 8
    {
        return Some(Imp::new("Reg", class::ST, i, rw));
    }
    None
}

fn implicit(tok: &str) -> Result<Vec<Imp>, String> {
    let err = || format!("bad implicit operand '{tok}'");
    let stack = |rw| register("sSP", rw).unwrap();
    if tok == "pop@bp" {
        // `leave`: the frame pointer's slot.
        return Ok(vec![Imp::new("Pop", 1, 5, RW_R), stack(RW_RW)]);
    }
    for (word, kind) in [("push", "Push"), ("pop", "Pop")] {
        if let Some(rest) = tok.strip_prefix(word) {
            let slots = match rest {
                "" => 1,
                _ => rest
                    .strip_prefix('*')
                    .and_then(|n| n.parse::<u8>().ok())
                    .ok_or_else(err)?,
            };
            let rw = if kind == "Push" { RW_W } else { RW_R };
            return Ok(vec![Imp::new(kind, slots, 0, rw), stack(RW_RW)]);
        }
    }
    let x87 = |extra: &[Imp]| {
        let mut v = vec![register("fpsw", RW_RW).unwrap()];
        v.extend_from_slice(extra);
        v
    };
    match tok {
        "x87" => return Ok(x87(&[])),
        "x87push" | "x87pop" | "x87pop2" => {
            return Ok(x87(&[
                register("st*", RW_W).unwrap(),
                register("fptw", RW_RW).unwrap(),
            ]));
        }
        _ => {}
    }
    let (name, access) = tok.split_once(':').ok_or_else(err)?;
    let (access, rep) = match access.strip_suffix("?rep") {
        Some(a) => (a, true),
        None => (access, false),
    };
    let (access, bytes) = match access.split_once('/') {
        Some((a, b)) => (a, Some(b.parse::<u8>().map_err(|_| err())?)),
        None => (access, None),
    };
    let rw = rw_code(access).ok_or_else(err)? | if rep { RW_REP } else { 0 };
    if let Some(reg) = name.strip_prefix("mem@") {
        // At the address in an operand (`movdir64b`'s destination).
        if let Some(n) = reg.strip_prefix("op").and_then(|n| n.parse::<u8>().ok()) {
            return Ok(vec![Imp::new("MemAt", 0x80 | n, bytes.unwrap_or(0), rw)]);
        }
        let r = register(reg, rw).ok_or_else(err)?;
        if r.kind != "GprAs" {
            return Err(err());
        }
        return Ok(vec![Imp::new("MemAt", r.a, bytes.unwrap_or(0), rw)]);
    }
    if bytes.is_some() {
        return Err(err());
    }
    Ok(vec![register(name, rw).ok_or_else(err)?])
}

/// EFLAGS bits.
fn flag_set(s: &str) -> Result<u32, String> {
    let mut mask = 0;
    for part in s.split(',').filter(|p| !p.is_empty()) {
        mask |= match part {
            "ac" => 1 << 18,
            "nt" => 1 << 14,
            "rf" => 1 << 16,
            "vm" => 1 << 17,
            "vif" => 1 << 19,
            "vip" => 1 << 20,
            "id" => 1 << 21,
            "iopl" => 3 << 12,
            "all" => 0x003F_7FD5,
            _ => {
                let mut m = 0;
                for c in part.chars() {
                    m |= match c {
                        'c' => 1 << 0,
                        'p' => 1 << 2,
                        'a' => 1 << 4,
                        'z' => 1 << 6,
                        's' => 1 << 7,
                        't' => 1 << 8,
                        'i' => 1 << 9,
                        'd' => 1 << 10,
                        'o' => 1 << 11,
                        _ => return Err(format!("unknown flag '{c}' in '{s}'")),
                    };
                }
                m
            }
        };
    }
    Ok(mask)
}

/// The flags a condition code reads, from the end of a mnemonic.
fn condition(mnemonic: &str) -> Option<u32> {
    const C: u32 = 1;
    const P: u32 = 1 << 2;
    const Z: u32 = 1 << 6;
    const S: u32 = 1 << 7;
    const O: u32 = 1 << 11;
    const CONDS: [(&str, u32); 30] = [
        ("nbe", C | Z),
        ("nae", C),
        ("nge", S | O),
        ("nle", Z | S | O),
        ("no", O),
        ("nb", C),
        ("nc", C),
        ("ae", C),
        ("ne", Z),
        ("nz", Z),
        ("be", C | Z),
        ("na", C | Z),
        ("ns", S),
        ("np", P),
        ("po", P),
        ("pe", P),
        ("ge", S | O),
        ("nl", S | O),
        ("le", Z | S | O),
        ("ng", Z | S | O),
        ("nu", P),
        ("o", O),
        ("b", C),
        ("c", C),
        ("e", Z),
        ("z", Z),
        ("a", C | Z),
        ("s", S),
        ("p", P),
        ("l", S | O),
    ];
    // (Longer families first: `setzu` before `set`.)
    const FAMILIES: [&str; 8] = [
        "cfcmov", "setzu", "fcmov", "cmov", "ccmp", "ctest", "set", "j",
    ];
    for family in FAMILIES {
        if let Some(cc) = mnemonic.strip_prefix(family) {
            if family == "ccmp" || family == "ctest" {
                // Their conditions also include always (t) and never (f).
                if cc == "t" || cc == "f" {
                    return Some(0);
                }
            }
            if cc == "g" {
                return Some(Z | S | O);
            }
            if cc == "u" {
                return Some(P);
            }
            if let Some(&(_, m)) = CONDS.iter().find(|(n, _)| *n == cc) {
                return Some(m);
            }
        }
    }
    None
}

enum Selector {
    Any,
    Count(usize),
    Ops(String),
}

struct Rule {
    ops: Option<Vec<u8>>,
    implicit: Vec<Imp>,
    flags: [u32; 3],
    /// Flags read are the mnemonic's condition (`cc`).
    cc: bool,
    line: usize,
}

struct Rules {
    rules: Vec<Rule>,
    /// By mnemonic (or prefix, ending in `*`): (selector, rule).
    by_name: HashMap<String, Vec<(Selector, usize)>>,
}

fn parse(text: &str) -> Result<Rules, Vec<String>> {
    let mut rules = Rules {
        rules: Vec::new(),
        by_name: HashMap::new(),
    };
    let mut errors = Vec::new();
    for (n, line) in text.lines().enumerate() {
        let line = line.split('#').next().unwrap().trim();
        if line.is_empty() {
            continue;
        }
        let at = |e: String| format!("access.rules:{}: {e}", n + 1);
        let Some((lhs, rhs)) = line.split_once(" : ").or_else(|| {
            line.strip_suffix(" :")
                .or_else(|| line.strip_suffix(':'))
                .map(|l| (l, ""))
        }) else {
            errors.push(at("missing ' : '".to_string()));
            continue;
        };
        let mut rule = Rule {
            ops: None,
            implicit: Vec::new(),
            flags: [0; 3],
            cc: false,
            line: n + 1,
        };
        let mut ok = true;
        for (i, tok) in rhs.split_whitespace().enumerate() {
            if i == 0 && tok == "-" {
                continue;
            }
            if i == 0 && tok.split(',').all(|a| rw_code(a).is_some()) {
                rule.ops = Some(tok.split(',').map(|a| rw_code(a).unwrap()).collect());
                continue;
            }
            if let Some(f) = tok.strip_prefix("f=") {
                let parts: Vec<&str> = f.split('/').collect();
                if parts.len() != 3 {
                    errors.push(at(format!("bad flags '{tok}'")));
                    ok = false;
                    continue;
                }
                let read = if parts[0] == "cc" {
                    rule.cc = true;
                    Ok(0)
                } else {
                    flag_set(parts[0])
                };
                match (read, flag_set(parts[1]), flag_set(parts[2])) {
                    (Ok(r), Ok(w), Ok(u)) => rule.flags = [r, w | u, u],
                    (Err(e), _, _) | (_, Err(e), _) | (_, _, Err(e)) => {
                        errors.push(at(e));
                        ok = false;
                    }
                }
                continue;
            }
            match implicit(tok) {
                Ok(v) => rule.implicit.extend(v),
                Err(e) => {
                    errors.push(at(e));
                    ok = false;
                }
            }
        }
        if !ok {
            continue;
        }
        let index = rules.rules.len();
        rules.rules.push(rule);
        for name in lhs.split_whitespace() {
            let (name, selector) = match name.split_once('/') {
                Some((m, s)) => match s.parse::<usize>() {
                    Ok(count) => (m, Selector::Count(count)),
                    Err(_) => (m, Selector::Ops(s.to_string())),
                },
                None => (name, Selector::Any),
            };
            rules
                .by_name
                .entry(name.to_string())
                .or_default()
                .push((selector, index));
        }
    }
    if errors.is_empty() {
        Ok(rules)
    } else {
        Err(errors)
    }
}

impl Rules {
    fn find(&self, f: &FormInfo) -> Option<&Rule> {
        let ops = f.raw_ops.join(",");
        let pick = |list: &Vec<(Selector, usize)>| {
            let by = |want: fn(&Selector) -> bool| {
                list.iter()
                    .filter(|(s, _)| want(s))
                    .find(|(s, _)| match s {
                        Selector::Any => true,
                        Selector::Count(n) => *n == f.raw_ops.len(),
                        Selector::Ops(o) => *o == ops,
                    })
                    .map(|&(_, i)| &self.rules[i])
            };
            by(|s| matches!(s, Selector::Ops(_)))
                .or_else(|| by(|s| matches!(s, Selector::Count(_))))
                .or_else(|| by(|s| matches!(s, Selector::Any)))
        };
        if let Some(list) = self.by_name.get(f.mnemonic)
            && let Some(r) = pick(list)
        {
            return Some(r);
        }
        // The longest matching prefix.
        (1..=f.mnemonic.len()).rev().find_map(|n| {
            self.by_name
                .get(&format!("{}*", &f.mnemonic[..n]))
                .and_then(pick)
        })
    }
}

/// Maps whose forms may go without a rule: SSE, MMX, 3DNow!, VEX, EVEX
/// (but not APX's map 4 or 7), and XOP.
fn simd(f: &FormInfo) -> bool {
    let file = f.origin.split(':').next().unwrap_or("");
    !matches!(
        file,
        "00-legacy.txt" | "01-0f-system.txt" | "02-x87.txt" | "06-apx.txt"
    )
}

/// Generates `ACCESS`, `IMPLICIT`, and `FLAG_SETS`, parallel to the forms.
pub fn generate(text: &str, forms: &[FormInfo], out: &mut String) -> Result<(), Vec<String>> {
    let rules = parse(text)?;
    let mut errors = Vec::new();
    let mut implicit: Vec<Imp> = Vec::new();
    let mut lists: HashMap<Vec<Imp>, usize> = HashMap::new();
    let mut flag_sets: Vec<[u32; 3]> = vec![[0; 3]];
    let mut used = vec![false; rules.rules.len()];
    let mut entries = Vec::new();
    for f in forms {
        let rule = rules.find(f);
        if rule.is_none() && !simd(f) {
            errors.push(format!("{}: no access rule for '{}'", f.origin, f.mnemonic));
        }
        if let Some(r) = rule {
            used[rules.rules.iter().position(|x| std::ptr::eq(x, r)).unwrap()] = true;
        }
        let count = f.raw_ops.len();
        let mut ops: Vec<u8> = match rule {
            Some(r) => r.ops.clone().unwrap_or_default(),
            None => {
                // SIMD default: the first operand is the destination; a
                // legacy-encoded register one with a source is also read.
                let legacy = (1..=4).contains(&f.map);
                let register = f
                    .raw_ops
                    .first()
                    .is_some_and(|o| o.starts_with(['V', 'P', 'U', 'N']));
                let first = if legacy && register && count >= 2 {
                    RW_RW
                } else {
                    RW_W
                };
                (0..count)
                    .map(|i| if i == 0 { first } else { RW_R })
                    .collect()
            }
        };
        if ops.len() > count {
            errors.push(format!(
                "{}: access rule (line {}) has {} operands, the form {count}",
                f.origin,
                rule.map_or(0, |r| r.line),
                ops.len()
            ));
        }
        ops.resize(count, RW_R);
        if f.ndd_dest {
            // The new destination is written; the old one is only read.
            for a in &mut ops {
                if *a != RW_NONE {
                    *a = RW_R;
                }
            }
            ops.insert(0, RW_W);
        }
        let mut packed = 0u16;
        for (i, a) in ops.iter().enumerate().take(5) {
            packed |= u16::from(*a) << (3 * i);
        }
        let imps = rule.map(|r| r.implicit.clone()).unwrap_or_default();
        let start = if imps.is_empty() {
            0
        } else {
            *lists.entry(imps.clone()).or_insert_with(|| {
                let s = implicit.len();
                implicit.extend(imps.iter().cloned());
                s
            })
        };
        let mut flags = rule.map_or([0; 3], |r| r.flags);
        if rule.is_some_and(|r| r.cc) {
            match condition(f.mnemonic) {
                Some(m) => flags[0] |= m,
                None => errors.push(format!(
                    "{}: '{}' has no condition code",
                    f.origin, f.mnemonic
                )),
            }
        }
        let fi = match flag_sets.iter().position(|s| *s == flags) {
            Some(i) => i,
            None => {
                flag_sets.push(flags);
                flag_sets.len() - 1
            }
        };
        if imps.len() > 255 || fi > 255 {
            errors.push(format!("{}: too much access data", f.origin));
        }
        entries.push(format!(
            "    AccessSpec {{ ops: {packed:#x}, implicit: {start}, implicit_len: {}, flags: {fi} }},",
            imps.len()
        ));
    }
    for (i, u) in used.iter().enumerate() {
        if !u {
            errors.push(format!(
                "access.rules:{}: the rule applies to no form",
                rules.rules[i].line
            ));
        }
    }
    if !errors.is_empty() {
        return Err(errors);
    }
    writeln!(
        out,
        "pub(crate) static ACCESS: [AccessSpec; {}] = [",
        entries.len()
    )
    .unwrap();
    for e in entries {
        out.push_str(&e);
        out.push('\n');
    }
    out.push_str("];\n\n");
    writeln!(
        out,
        "pub(crate) static IMPLICIT: [Imp; {}] = [",
        implicit.len()
    )
    .unwrap();
    for i in &implicit {
        writeln!(out, "    {},", i.emit()).unwrap();
    }
    out.push_str("];\n\n");
    writeln!(
        out,
        "pub(crate) static FLAG_SETS: [[u32; 3]; {}] = {flag_sets:?};\n",
        flag_sets.len()
    )
    .unwrap();
    Ok(())
}
