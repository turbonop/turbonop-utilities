//! Compiles `tables/a64.txt` (generated from AARCHMRS and LLVM by `tools/arm64/extract.py`) into decode
//! tables: encodings with their masks, constraints, UNDEFINED conditions,
//! alias preferences, assembler templates, and operand symbols.

#![allow(
    clippy::cast_possible_truncation,
    clippy::type_complexity,
    reason = "a code generator and tools over small table values"
)]

use std::collections::{BTreeMap, HashMap};
use std::fmt::Write as _;
use std::path::Path;

#[derive(Default, Clone)]
struct Enc {
    name: String,
    class: String,
    mask: u32,
    value: u32,
    /// Should-be bits (Arm's `(0)`/`(1)`): not matched; when they differ,
    /// the encoding is CONSTRAINED UNPREDICTABLE.
    should_mask: u32,
    should_value: u32,
    fields: Vec<(String, u32, u32)>,
    not_equal: Vec<String>,
    asm: String,
    undef: Vec<String>,
    unpred: Vec<String>,
    feature: String,
    alias: Option<(String, String)>,
    file: usize,
}

#[derive(Default, Clone)]
struct Sym {
    /// Its LLVM operand (`src=llvm:<record>.<operand>:<type>`): the
    /// operand's name and type.
    llvm: Option<(String, String)>,
    link: String,
    display: String,
    encoded_in: String,
    table: Option<(Vec<String>, Vec<(Vec<String>, String)>)>,
    attrs: Vec<String>,
}

impl Sym {
    fn has(&self, a: &str) -> bool {
        self.attrs.iter().any(|x| x == a)
    }

    fn attr(&self, key: &str) -> Option<&str> {
        self.attrs
            .iter()
            .find_map(|x| x.strip_prefix(key)?.strip_prefix('='))
    }
}

#[derive(Default)]
struct File {
    syms: Vec<Sym>,
}

fn parse(src: &str) -> (Vec<Enc>, Vec<File>) {
    let mut encs = Vec::new();
    let mut files: Vec<File> = Vec::new();
    for line in src.lines() {
        if line.starts_with('#') || line.trim().is_empty() {
            continue;
        }
        if let Some(rest) = line.strip_prefix("file ") {
            let _ = rest;
            files.push(File::default());
        } else if let Some(rest) = line.strip_prefix("enc ") {
            let mut parts = rest.split_whitespace();
            encs.push(Enc {
                name: parts.next().unwrap().to_string(),
                class: parts.next().unwrap_or("").to_string(),
                file: files.len() - 1,
                ..Enc::default()
            });
        } else if let Some(rest) = line.strip_prefix("  bits ") {
            let e = encs.last_mut().unwrap();
            for (i, c) in rest.trim().chars().enumerate() {
                let bit = 31 - i as u32;
                match c {
                    '0' => e.mask |= 1 << bit,
                    '1' => {
                        e.mask |= 1 << bit;
                        e.value |= 1 << bit;
                    }
                    'Z' => e.should_mask |= 1 << bit,
                    'O' => {
                        e.should_mask |= 1 << bit;
                        e.should_value |= 1 << bit;
                    }
                    _ => {}
                }
            }
        } else if let Some(rest) = line.strip_prefix("  fields ") {
            let e = encs.last_mut().unwrap();
            for f in rest.split_whitespace() {
                let (name, range) = f.split_once('=').unwrap();
                let (hi, lo) = match range.split_once(':') {
                    Some((h, l)) => (h.parse().unwrap(), l.parse().unwrap()),
                    None => {
                        let b: u32 = range.parse().unwrap();
                        (b, b)
                    }
                };
                e.fields.push((name.to_string(), hi, lo));
            }
        } else if let Some(rest) = line.strip_prefix("  ne ") {
            encs.last_mut().unwrap().not_equal.push(rest.to_string());
        } else if let Some(rest) = line.strip_prefix("  asm ") {
            encs.last_mut().unwrap().asm = rest.to_string();
        } else if let Some(rest) = line.strip_prefix("  undef ") {
            encs.last_mut().unwrap().undef.push(rest.to_string());
        } else if let Some(rest) = line.strip_prefix("  unpred ") {
            encs.last_mut().unwrap().unpred.push(rest.to_string());
        } else if let Some(rest) = line.strip_prefix("  feat ") {
            encs.last_mut().unwrap().feature = rest.to_string();
        } else if let Some(rest) = line.strip_prefix("  alias ") {
            let (target, cond) = rest.split_once(" when ").unwrap_or((rest, ""));
            encs.last_mut().unwrap().alias = Some((target.to_string(), cond.to_string()));
        } else if let Some(rest) = line.strip_prefix("sym ") {
            let mut s = Sym::default();
            let (head, table) = match rest.split_once(" = ") {
                Some((h, t)) => (h, Some(t)),
                None => (rest, None),
            };
            let mut parts = head.splitn(3, ' ');
            s.link = parts.next().unwrap().to_string();
            s.display = parts.next().unwrap_or("").to_string();
            s.encoded_in = parts.next().unwrap_or("").to_string();
            if let Some(t) = table {
                let mut cells = t.split(" | ");
                let head: Vec<String> = cells
                    .next()
                    .unwrap()
                    .split(',')
                    .map(|c| c.trim().to_string())
                    .collect();
                let rows = cells
                    .filter_map(|r| {
                        let (vals, result) = r.split_once(':')?;
                        Some((
                            vals.split(',').map(|v| v.trim().to_string()).collect(),
                            result.trim().to_string(),
                        ))
                    })
                    .collect();
                s.table = Some((head, rows));
            }
            files.last_mut().unwrap().syms.push(s);
        } else if let Some(rest) = line.strip_prefix("  attr ")
            && let Some(s) = files.last_mut().and_then(|f| f.syms.last_mut())
        {
            // (`src=` says where the attributes came from.)
            if let Some(src) = rest
                .split_whitespace()
                .find_map(|a| a.strip_prefix("src=llvm:"))
                && let Some((rec_op, t)) = src.rsplit_once(':')
                && let Some((_, op)) = rec_op.split_once('.')
                && s.llvm.is_none()
            {
                s.llvm = Some((op.to_string(), t.to_string()));
            }
            s.attrs.extend(
                rest.split_whitespace()
                    .filter(|a| !a.starts_with("src="))
                    .map(str::to_string),
            );
        }
    }
    (encs, files)
}

// ---------------------------------------------------------------------------
// Bit sources: fields, slices, literals, concatenations.

/// A piece of a value: bits `hi..=lo` of the word, or literal bits.
#[derive(Clone, Debug, PartialEq)]
enum Piece {
    Bits(u32, u32),
    Lit(u64, u32),
}

/// Parses `a`, `a<1:0>`, `a[3]`, `'10'`, and `::`/`:` concatenations into
/// pieces, most significant first.
fn pieces(expr: &str, fields: &[(String, u32, u32)]) -> Option<Vec<Piece>> {
    let expr = expr.trim().trim_start_matches('(').trim_end_matches(')');
    let mut out = Vec::new();
    // Split at top-level `::` or `:` (not the `:` of a `[4:1]` slice).
    let parts: Vec<&str> = if expr.contains("::") {
        expr.split("::").collect()
    } else {
        let mut parts = Vec::new();
        let mut depth = 0;
        let mut start = 0;
        for (i, c) in expr.char_indices() {
            match c {
                '[' | '<' => depth += 1,
                ']' | '>' => depth -= 1,
                ':' if depth == 0 => {
                    parts.push(&expr[start..i]);
                    start = i + 1;
                }
                _ => {}
            }
        }
        parts.push(&expr[start..]);
        parts
    };
    for part in parts {
        let part = part.trim();
        if let Some(lit) = part.strip_prefix('\'').and_then(|p| p.strip_suffix('\'')) {
            let v = u64::from_str_radix(lit, 2).ok()?;
            out.push(Piece::Lit(v, lit.len() as u32));
            continue;
        }
        let (name, sel) = match part.find(['<', '[']) {
            Some(i) => (&part[..i], Some(&part[i + 1..part.len() - 1])),
            None => (part, None),
        };
        let &(_, hi, lo) = fields.iter().find(|(n, _, _)| n == name)?;
        match sel {
            None => out.push(Piece::Bits(hi, lo)),
            Some(sel) => {
                let (h, l) = match sel.split_once(':') {
                    Some((h, l)) => (h.parse::<u32>().ok()?, l.parse::<u32>().ok()?),
                    None => {
                        let b = sel.parse::<u32>().ok()?;
                        (b, b)
                    }
                };
                out.push(Piece::Bits(lo + h, lo + l));
            }
        }
    }
    Some(out)
}

fn pieces_code(p: &[Piece]) -> String {
    let mut s = String::from("&[");
    for piece in p {
        match piece {
            Piece::Bits(h, l) => {
                let _ = write!(s, "Piece::Bits({h},{l}),");
            }
            Piece::Lit(v, n) => {
                let _ = write!(s, "Piece::Lit({v},{n}),");
            }
        }
    }
    s.push(']');
    s
}

/// A table row's patterns, one per column ("1x 0", "x", ""), as a mask and
/// value over the columns side by side (the first most significant), or
/// `None` for a row no word matches: a pattern that is not bits (such as
/// "!= 000"), of another width than its column, or without a column.
/// (An empty pattern matches anything.)
fn row_pattern(pats: &[String], cols: &[Vec<Piece>]) -> Option<(u64, u64)> {
    if pats.len() > cols.len() {
        return None;
    }
    let (mut mask, mut value) = (0u64, 0u64);
    for (i, col) in cols.iter().enumerate() {
        let w = width(col);
        let bits: Vec<u8> = pats
            .get(i)
            .map(|p| p.bytes().filter(|&c| c != b' ').collect())
            .unwrap_or_default();
        if bits.iter().any(|c| !matches!(c, b'0' | b'1' | b'x')) {
            return None;
        }
        mask <<= w;
        value <<= w;
        if bits.is_empty() {
            continue;
        }
        if bits.len() != w as usize {
            return None;
        }
        for (j, c) in bits.iter().enumerate() {
            let bit = w as usize - 1 - j;
            match c {
                b'0' => mask |= 1 << bit,
                b'1' => {
                    mask |= 1 << bit;
                    value |= 1 << bit;
                }
                _ => {}
            }
        }
    }
    Some((mask, value))
}

fn width(p: &[Piece]) -> u32 {
    p.iter()
        .map(|x| match x {
            Piece::Bits(h, l) => h - l + 1,
            Piece::Lit(_, n) => *n,
        })
        .sum()
}

// ---------------------------------------------------------------------------
// Conditions: the ASL subset used in UNDEFINED checks and alias
// preferences, compiled to Rust.

#[derive(Debug, Clone, PartialEq)]
pub enum Tok {
    Ident(String),
    Bits(String),
    Num(i64),
    Op(String),
}

pub fn lex(s: &str) -> Option<Vec<Tok>> {
    let b = s.as_bytes();
    let mut i = 0;
    let mut out = Vec::new();
    while i < b.len() {
        let c = b[i] as char;
        if c.is_whitespace() {
            i += 1;
        } else if c == '\'' {
            let end = s[i + 1..].find('\'')? + i + 1;
            out.push(Tok::Bits(s[i + 1..end].replace(' ', "")));
            i = end + 1;
        } else if c.is_ascii_digit() {
            let start = i;
            while i < b.len() && (b[i] as char).is_ascii_digit() {
                i += 1;
            }
            out.push(Tok::Num(s[start..i].parse().ok()?));
        } else if c.is_ascii_alphabetic() || c == '_' {
            let start = i;
            while i < b.len() && ((b[i] as char).is_ascii_alphanumeric() || b[i] == b'_') {
                i += 1;
            }
            out.push(Tok::Ident(s[start..i].to_string()));
        } else {
            let two = s.get(i..i + 2).unwrap_or("");
            if ["==", "!=", "&&", "||", "<=", ">=", "::"].contains(&two) {
                out.push(Tok::Op(two.to_string()));
                i += 2;
            } else {
                out.push(Tok::Op(c.to_string()));
                i += 1;
            }
        }
    }
    Some(out)
}

/// A compiled expression: either a boolean, an integer, or a bit value of
/// known width.
enum Val {
    Bool(String),
    Int(String),
    Bits(String, u32),
}

struct Parser<'a> {
    toks: Vec<Tok>,
    pos: usize,
    fields: &'a [(String, u32, u32)],
}

impl Parser<'_> {
    fn peek(&self) -> Option<&Tok> {
        self.toks.get(self.pos)
    }

    fn eat(&mut self, op: &str) -> bool {
        if self.peek() == Some(&Tok::Op(op.to_string())) {
            self.pos += 1;
            true
        } else {
            false
        }
    }

    fn expect(&mut self, op: &str) -> Option<()> {
        self.eat(op).then_some(())
    }

    fn bool_of(v: Val) -> Option<String> {
        match v {
            Val::Bool(s) => Some(s),
            _ => None,
        }
    }

    fn or(&mut self) -> Option<Val> {
        let first = self.and()?;
        if !matches!(self.peek(), Some(Tok::Op(o)) if o == "||") {
            return Some(first);
        }
        let mut l = Self::bool_of(first)?;
        while self.eat("||") {
            let r = Self::bool_of(self.and()?)?;
            l = or_of(&l, &r);
        }
        Some(Val::Bool(l))
    }

    fn and(&mut self) -> Option<Val> {
        let first = self.cmp()?;
        if !matches!(self.peek(), Some(Tok::Op(o)) if o == "&&") {
            return Some(first);
        }
        let mut l = Self::bool_of(first)?;
        while self.eat("&&") {
            let r = Self::bool_of(self.cmp()?)?;
            l = and_of(&l, &r);
        }
        Some(Val::Bool(l))
    }

    fn cmp(&mut self) -> Option<Val> {
        let l = self.sum()?;
        let op = match self.peek() {
            Some(Tok::Op(o)) if ["==", "!=", "<", ">", "<=", ">="].contains(&o.as_str()) => {
                o.clone()
            }
            Some(Tok::Ident(i)) if i == "IN" => "IN".to_string(),
            _ => return Some(l),
        };
        self.pos += 1;
        if op == "IN" {
            self.expect("{")?;
            let mut alts = Vec::new();
            loop {
                match self.peek()?.clone() {
                    Tok::Bits(b) => {
                        self.pos += 1;
                        alts.push(b);
                    }
                    _ => return None,
                }
                if self.eat("}") {
                    break;
                }
                self.expect(",")?;
            }
            let Val::Bits(code, _) = l else { return None };
            let tests: Vec<String> = alts.iter().map(|b| bits_match(&code, b)).collect();
            return Some(Val::Bool(format!("({})", tests.join(" || "))));
        }
        // A bit pattern on the right of == or != may contain x.
        if let Some(Tok::Bits(b)) = self.peek().cloned() {
            self.pos += 1;
            let Val::Bits(code, _) = l else { return None };
            let m = bits_match(&code, &b);
            return Some(Val::Bool(match op.as_str() {
                "==" => m,
                "!=" => format!("!{m}"),
                _ => return None,
            }));
        }
        let r = self.sum()?;
        let as_int = |v: Val| match v {
            Val::Int(s) | Val::Bits(s, _) => Some(s),
            Val::Bool(_) => None,
        };
        let (l, r) = (as_int(l)?, as_int(r)?);
        Some(Val::Bool(format!("((({l}) as i64) {op} (({r}) as i64))")))
    }

    fn sum(&mut self) -> Option<Val> {
        let mut l = self.unary()?;
        loop {
            let op = match self.peek() {
                Some(Tok::Op(o)) if o == "+" || o == "-" => o.clone(),
                _ => return Some(l),
            };
            self.pos += 1;
            let r = self.unary()?;
            let li = match l {
                Val::Int(s) | Val::Bits(s, _) => s,
                Val::Bool(_) => return None,
            };
            let ri = match r {
                Val::Int(s) | Val::Bits(s, _) => s,
                Val::Bool(_) => return None,
            };
            l = Val::Int(format!("((({li}) as i64) {op} (({ri}) as i64))"));
        }
    }

    fn unary(&mut self) -> Option<Val> {
        if self.eat("!") {
            let v = Self::bool_of(self.unary()?)?;
            return Some(Val::Bool(not_of(&v)));
        }
        let mut v = self.primary()?;
        // Concatenation.
        while self.eat("::") {
            let r = self.primary()?;
            let (Val::Bits(a, wa), Val::Bits(b, wb)) = (v, r) else {
                return None;
            };
            v = Val::Bits(format!("((({a}) << {wb}) | ({b}))"), wa + wb);
        }
        Some(v)
    }

    fn primary(&mut self) -> Option<Val> {
        match self.peek()?.clone() {
            Tok::Op(o) if o == "(" => {
                self.pos += 1;
                let v = self.or()?;
                self.expect(")")?;
                Some(v)
            }
            Tok::Num(n) => {
                self.pos += 1;
                Some(Val::Int(format!("{n}i64")))
            }
            Tok::Bits(b) => {
                self.pos += 1;
                let v = u64::from_str_radix(&b.replace('x', "0"), 2).ok()?;
                Some(Val::Bits(format!("{v}u64"), b.len() as u32))
            }
            Tok::Ident(id) => {
                self.pos += 1;
                match id.as_str() {
                    "TRUE" => return Some(Val::Bool("true".into())),
                    "FALSE" => return Some(Val::Bool("false".into())),
                    _ => {}
                }
                if self.eat("(") {
                    return self.call(&id);
                }
                let &(_, hi, lo) = self.fields.iter().find(|(n, _, _)| *n == id)?;
                let (mut hi, mut lo) = (hi, lo);
                if self.eat("[") {
                    let Tok::Num(a) = self.peek()?.clone() else {
                        return None;
                    };
                    self.pos += 1;
                    let (h, l) = if self.eat(":") {
                        let Tok::Num(b) = self.peek()?.clone() else {
                            return None;
                        };
                        self.pos += 1;
                        (a as u32, b as u32)
                    } else {
                        (a as u32, a as u32)
                    };
                    self.expect("]")?;
                    hi = lo + h;
                    lo += l;
                }
                let n = hi - lo + 1;
                Some(Val::Bits(
                    format!("((w as u64 >> {lo}) & {:#x})", (1u64 << n) - 1),
                    n,
                ))
            }
            _ => None,
        }
    }

    /// A function call; the opening parenthesis has been consumed.
    fn call(&mut self, name: &str) -> Option<Val> {
        // Arguments, as compiled values (feature names are skipped).
        let mut args = Vec::new();
        if !self.eat(")") {
            loop {
                if let Some(Tok::Ident(id)) = self.peek().cloned()
                    && (id.starts_with("FEAT_") || name.starts_with("IsFeature"))
                {
                    self.pos += 1;
                    if self.eat(")") {
                        break;
                    }
                    self.expect(",")?;
                    continue;
                }
                args.push(self.or()?);
                if self.eat(")") {
                    break;
                }
                self.expect(",")?;
            }
        }
        let int = |v: &Val| match v {
            Val::Int(s) | Val::Bits(s, _) => Some(s.clone()),
            Val::Bool(_) => None,
        };
        let width = |v: &Val| match v {
            Val::Bits(_, w) => Some(*w),
            _ => None,
        };
        Some(match name {
            // We decode as if every feature is implemented, with the
            // largest vector lengths, and in Debug state (so that
            // Debug-only instructions such as DCPS3 decode).
            "IsFeatureImplemented"
            | "HaveEL"
            | "HaveSVE2FP8FMA"
            | "HaveSVE2FP8DOT4"
            | "HaveSVE2FP8DOT2" => Val::Bool("true".into()),
            "Halted" => Val::Bool("true".into()),
            "MaxImplementedVL" | "MaxImplementedSVL" | "MaxImplementedAnyVL" => {
                Val::Int("2048i64".into())
            }
            "UInt" => Val::Int(int(args.first()?)?),
            "IsZero" => Val::Bool(format!("({} == 0)", int(args.first()?)?)),
            "IsOnes" => {
                let w = width(args.first()?)?;
                Val::Bool(format!(
                    "({} == {:#x})",
                    int(args.first()?)?,
                    (1u64 << w) - 1
                ))
            }
            "BitCount" => Val::Int(format!(
                "(({}) as u64).count_ones() as i64",
                int(args.first()?)?
            )),
            "BFXPreferred" => Val::Bool(format!(
                "crate::alias::bfx_preferred({}, {}, {}, {})",
                int(&args[0])?,
                int(&args[1])?,
                int(&args[2])?,
                int(&args[3])?
            )),
            "MoveWidePreferred" => Val::Bool(format!(
                "crate::alias::move_wide_preferred({}, {}, {}, {})",
                int(&args[0])?,
                int(&args[1])?,
                int(&args[2])?,
                int(&args[3])?
            )),
            "SVEMoveMaskPreferred" => Val::Bool(format!(
                "crate::alias::sve_move_mask_preferred({})",
                int(&args[0])?
            )),
            "SysOp" => Val::Int(format!(
                "crate::alias::sys_op({}, {}, {}, {})",
                int(&args[0])?,
                int(&args[1])?,
                int(&args[2])?,
                int(&args[3])?
            )),
            _ => return None,
        })
    }
}

/// A condition's constant value, if it is `true` or `false` (in
/// parentheses or not). Conditions fold as they are built: features are
/// all implemented (see `call`), so many are constant, and the decoder
/// shouldn't test what can't vary.
fn literal(s: &str) -> Option<bool> {
    let mut t = s.trim();
    while t.starts_with('(') && t.ends_with(')') && balanced(&t[1..t.len() - 1]) {
        t = t[1..t.len() - 1].trim();
    }
    match t {
        "true" => Some(true),
        "false" => Some(false),
        _ => None,
    }
}

/// Whether `s`'s parentheses balance (so that one around it encloses it).
fn balanced(s: &str) -> bool {
    let mut depth = 0i32;
    for c in s.chars() {
        match c {
            '(' => depth += 1,
            ')' => {
                depth -= 1;
                if depth < 0 {
                    return false;
                }
            }
            _ => {}
        }
    }
    depth == 0
}

fn or_of(l: &str, r: &str) -> String {
    match (literal(l), literal(r)) {
        (Some(true), _) | (_, Some(true)) => "true".into(),
        (Some(false), _) => r.into(),
        (_, Some(false)) => l.into(),
        _ => format!("({l} || {r})"),
    }
}

fn and_of(l: &str, r: &str) -> String {
    match (literal(l), literal(r)) {
        (Some(false), _) | (_, Some(false)) => "false".into(),
        (Some(true), _) => r.into(),
        (_, Some(true)) => l.into(),
        _ => format!("({l} && {r})"),
    }
}

fn not_of(v: &str) -> String {
    match literal(v) {
        Some(b) => (!b).to_string(),
        None => format!("!{v}"),
    }
}

/// Conditions, any of which holds: none if each is `false`; `true` if one
/// is.
fn any_of(conds: Vec<String>) -> Option<String> {
    let live: Vec<String> = conds
        .into_iter()
        .filter(|c| literal(c) != Some(false))
        .collect();
    if live.is_empty() {
        None
    } else if live.iter().any(|c| literal(c) == Some(true)) {
        Some("true".into())
    } else {
        Some(live.join(" || "))
    }
}

/// Rust code testing that `code` matches the bit pattern `b` (with x).
fn bits_match(code: &str, b: &str) -> String {
    let n = b.len();
    let mut mask = 0u64;
    let mut val = 0u64;
    for (i, c) in b.chars().enumerate() {
        let bit = n - 1 - i;
        match c {
            '0' => mask |= 1 << bit,
            '1' => {
                mask |= 1 << bit;
                val |= 1 << bit;
            }
            _ => {}
        }
    }
    format!("((({code}) & {mask:#x}) == {val:#x})")
}

/// Compiles a condition; `None` if it uses something unsupported.
pub fn condition(src: &str, fields: &[(String, u32, u32)]) -> Option<String> {
    match src.trim() {
        "Never" => return Some("false".into()),
        "Unconditionally" | "" => return Some("true".into()),
        _ => {}
    }
    // `SysOp(...) == Sys_AT` and friends compare against named constants.
    let src = src
        .replace("Sys_AT", "1")
        .replace("Sys_DC", "2")
        .replace("Sys_IC", "3")
        .replace("Sys_TLBI", "4")
        .replace("Sys_SYS", "0");
    let toks = lex(&src)?;
    let mut p = Parser {
        toks,
        pos: 0,
        fields,
    };
    let v = p.or()?;
    if p.pos != p.toks.len() {
        return None;
    }
    Parser::bool_of(v)
}

// ---------------------------------------------------------------------------
// Operand values, from the symbols' attributes (which come from LLVM's
// operand types; see tools/arm64/extract.py).

#[derive(Default)]
struct Value {
    pieces: Vec<Piece>,
    mul: i64,
    add: i64,
    modulo: i64,
    /// `N minus field`: the value is `N - field`.
    sub_from: Option<i64>,
    signed: bool,
}

fn attr_value(sym: &Sym, fields: &[(String, u32, u32)]) -> Option<Value> {
    let num = |k: &str| sym.attr(k).and_then(|v| v.parse::<i64>().ok());
    let mut v = Value {
        // (`source=` overrides the encoded-in bits: a strided list's first
        // register, `T:'0':Zt`.)
        pieces: pieces(sym.attr("source").unwrap_or(&sym.encoded_in), fields)?,
        mul: num("mul").or_else(|| num("regmul")).unwrap_or(1),
        add: num("add").unwrap_or(0),
        modulo: num("mod").unwrap_or(0),
        sub_from: num("subfrom"),
        signed: sym.has("signed"),
    };
    // The later registers of a list (`<Zn2>`, `<Vn+1>`) are encoded by the
    // first's field: the next ones, wrapping (or, for an aligned list, the
    // next ones of an aligned group).
    let name = sym.display.trim_matches(['<', '>']);
    let offset = if let Some((_, k)) = name.split_once('+') {
        k.parse::<i64>().ok()
    } else {
        name.chars()
            .last()
            .and_then(|c| c.to_digit(10))
            .filter(|&d| (2..=4).contains(&d))
            .map(|d| i64::from(d) - 1)
    };
    if let Some(k) = offset
        && name.starts_with(['Z', 'V', 'P'])
        && sym.attr("add").is_none()
    {
        if let Some(step) = num("regstep") {
            // Strided: {z0, z8}, {z0, z4, z8, z12}.
            v.add = k * step;
        } else {
            v.add = k;
            if num("regmul").is_none() {
                v.modulo = if name.starts_with('P') { 16 } else { 32 };
            }
        }
    }
    // Aligned lists or registers from a base: `regadd=16` (Z16-Z30).
    v.add += num("regadd").unwrap_or(0);
    class_value(sym, &mut v);
    Some(v)
}

/// What a symbol's LLVM register class says of its value that the
/// attributes of tables/a64.txt may not (extract.py's `attrs` before it
/// knew the decoders `DecodeMulMinMaxRegisterClass`, `DecodeZK`, and
/// `DecodeZPR2Mul2RegisterClass<16, 31>`): SME_MOP4's Z0-Z14 and
/// Z16-Z30 (even, encoded halved), and ZK's Z20-Z23 and Z28-Z31.
fn class_value(sym: &Sym, v: &mut Value) {
    let Some((_, t)) = &sym.llvm else {
        return;
    };
    let has = |k: &str| sym.attrs.iter().any(|a| a.split('=').next() == Some(k));
    let single = t.starts_with("ZPR") && (t.ends_with("Mul2_Lo") || t.ends_with("Mul2_Hi"));
    if single && !has("regmul") && v.mul == 1 {
        v.mul = 2;
    }
    let hi = t.ends_with("Mul2_Hi") || t.ends_with("_mul_r_Hi");
    if hi && !has("regadd") {
        v.add += 16;
    }
    // ZK: K:Zk (3 bits) names Z20-Z23 and Z28-Z31, `'1':K:'1':Zk`.
    if (t == "ZK" || t == "ZK_b") && !has("source") && width(&v.pieces) == 3 {
        let mut bits = Vec::new();
        for p in &v.pieces {
            match *p {
                Piece::Bits(h, l) => bits.extend((l..=h).rev().map(|b| Piece::Bits(b, b))),
                Piece::Lit(x, n) => bits.extend((0..n).rev().map(|i| Piece::Lit((x >> i) & 1, 1))),
            }
        }
        v.pieces = vec![
            Piece::Lit(1, 1),
            bits[0].clone(),
            Piece::Lit(1, 1),
            bits[1].clone(),
            bits[2].clone(),
        ];
    }
}

/// Entries per chunk of a large table (see [`chunked`]).
const CHUNK: usize = 256;

/// Writes `items` (one per line, `n` of them) as the table `name`: in
/// statics of `CHUNK` entries, and `name` the list of them, indexed by
/// `src/tables.rs` (`name[i / CHUNK][i % CHUNK]`). One static of tens of
/// thousands of entries takes rustc minutes to type-check (its time grows
/// faster than the entries); small ones take seconds in all.
fn chunked(code: &mut String, name: &str, ty: &str, items: &str, n: usize) {
    let lines: Vec<&str> = items.lines().collect();
    assert_eq!(lines.len(), n, "{name}: one entry per line");
    let chunks: Vec<&[&str]> = lines.chunks(CHUNK).collect();
    for (k, c) in chunks.iter().enumerate() {
        let _ = writeln!(
            code,
            "static {name}_{k}: [{ty}; {}] = [\n{}\n];",
            c.len(),
            c.join("\n")
        );
    }
    let list: Vec<String> = (0..chunks.len()).map(|k| format!("&{name}_{k}")).collect();
    let _ = writeln!(
        code,
        "pub(crate) static {name}: [&[{ty}]; {}] = [{}];",
        chunks.len(),
        list.join(", ")
    );
    let _ = writeln!(code, "pub(crate) const {name}_CHUNK: usize = {CHUNK};");
    let _ = writeln!(code, "pub(crate) const {name}_LEN: usize = {n};");
}

fn main() {
    let path = Path::new("tables/a64.txt");
    println!("cargo:rerun-if-changed={}", path.display());
    let src = std::fs::read_to_string(path).unwrap();
    let (encs, files) = parse(&src);

    let mut code = String::new();
    let mut warnings = Vec::new();
    let access = access_tables(&encs, &mut code);
    let mut sym_infos: Vec<SymInfo> = Vec::new();

    // Symbols, numbered globally.
    let mut sym_out = String::new();
    let mut n_syms = 0usize;
    // Each symbol's display name, by number.
    let mut displays: Vec<String> = Vec::new();
    // Whether each symbol may fail to render, by number.
    let mut may_fail: Vec<bool> = Vec::new();
    // Symbols by their code, to emit each only once.
    let mut distinct_syms: HashMap<String, usize> = HashMap::new();
    // Symbols are compiled per encoding (their fields differ by class), so
    // index them by (file, link) and compile with the first encoding's
    // fields that uses them.
    // A symbol is compiled once per field layout that uses it: the
    // encodings of a file can place the same field at different bits.
    let fields_key = |f: &[(String, u32, u32)]| format!("{f:?}");
    let mut uses: BTreeMap<(usize, String, String), Vec<(String, u32, u32)>> = BTreeMap::new();
    for e in &encs {
        let mut rest = e.asm.as_str();
        while let Some(i) = rest.find("%{") {
            let end = rest[i..].find('}').unwrap() + i;
            uses.entry((e.file, rest[i + 2..end].to_string(), fields_key(&e.fields)))
                .or_insert_with(|| e.fields.clone());
            rest = &rest[end + 1..];
        }
    }
    let mut sym_by_use: HashMap<(usize, String, String), usize> = HashMap::new();
    for ((fi, link, key), fields) in &uses {
        let Some(s) = files[*fi].syms.iter().find(|s| &s.link == link) else {
            continue;
        };
        let fields = fields.as_slice();
        {
            // The source bits.
            let value = if s.table.is_none() {
                attr_value(s, fields)
            } else {
                None
            };
            let encoded = pieces(&s.encoded_in, fields).unwrap_or_default();
            let table = match &s.table {
                Some((head, rows)) => {
                    let cols: Vec<Vec<Piece>> = head
                        .iter()
                        .map(|c| {
                            pieces(c, fields).unwrap_or_else(|| {
                                warnings.push(format!("table column {c} in {}", s.link));
                                Vec::new()
                            })
                        })
                        .collect();
                    // The columns side by side, as one key: each row is a
                    // mask and value over it.
                    let key: Vec<Piece> = cols.iter().flatten().cloned().collect();
                    assert!(
                        width(&key) <= 64,
                        "table key wider than 64 bits in {}",
                        s.link
                    );
                    let rows: Vec<String> = rows
                        .iter()
                        .filter_map(|(vals, r)| {
                            let (mask, value) = row_pattern(vals, &cols)?;
                            // Results such as "UInt(H :: L :: M)" are computed.
                            let (expr, mul, add) = table_expr(r, fields).unwrap_or_default();
                            let shown = if expr.is_empty() {
                                shown_as_is(s, r)
                                    .map_or("None".into(), |(t, d)| format!("Some(({t:?}, {d}))"))
                            } else {
                                "None".into()
                            };
                            Some(format!(
                                "({mask:#x}, {value:#x}, {r:?}, {}, {mul}, {add}, {shown})",
                                pieces_code(&expr)
                            ))
                        })
                        .collect();
                    format!(
                        "Some(&Table {{ key: {}, rows: &[{}] }})",
                        pieces_code(&key),
                        rows.join(",")
                    )
                }
                None => "None".into(),
            };
            // How the formatter shows the symbol (see src/tables.rs's `Kind`),
            // and whether its `special` may apply.
            let kind = symbol_kind(&s.display, s.has("bitmask"));
            // (What can keep a symbol from rendering: see format.rs's
            // `symbol_renders`.)
            let fails = s.table.is_some() || s.attr("when").is_some() || kind == "Kind::Bitmask";
            let may_special = may_special(s, encoded.is_empty() && value.is_none());
            let value = match value {
                Some(v) => format!(
                    "Some(Value {{ pieces: {}, width: {}, mul: {}, add: {}, modulo: {}, sub_from: {}, signed: {} }})",
                    pieces_code(&v.pieces),
                    width(&v.pieces),
                    v.mul,
                    v.add,
                    v.modulo,
                    v.sub_from.unwrap_or(0),
                    v.signed
                ),
                None => "None".into(),
            };
            // `when=option<0>=0`: the symbol applies only when those bits
            // have that value (another alternative applies otherwise).
            let when = s
                .attr("when")
                .and_then(|t| t.rsplit_once('='))
                .and_then(|(f, v)| {
                    let p = pieces(f, fields)?;
                    let value = u64::from_str_radix(v, 2).ok()?;
                    Some(format!("Some(({}, {value}))", pieces_code(&p)))
                })
                .unwrap_or_else(|| "None".to_string());
            // Its LLVM operand's register class (src/tables.rs's
            // `REG_CLASSES`), if it has one; the operand's name is for the
            // operand compiler (and keeps symbols of different operands
            // apart).
            let class = s
                .llvm
                .as_ref()
                .and_then(|(_, t)| access.classes.get(t).copied());
            let line = format!(
                "    Symbol {{ display: {:?}, encoded: {}, table: {table}, value: {value}, when: {when}, attrs: {:?}, flags: {:#x}, kind: {kind}, class: {} }}, // {}",
                s.display,
                pieces_code(&encoded),
                s.attrs.join(" "),
                attr_flags(&s.attrs)
                    | if may_special {
                        1 << ATTR_FLAGS.len()
                    } else {
                        0
                    },
                class.map_or(255, |c| c.0),
                s.llvm.as_ref().map_or("", |l| l.0.as_str())
            );
            // One symbol for each that is the same (most are: the same
            // operand in many encodings).
            let n = *distinct_syms.entry(line.clone()).or_insert_with(|| {
                sym_out.push_str(&line);
                sym_out.push('\n');
                displays.push(s.display.clone());
                may_fail.push(fails);
                sym_infos.push(SymInfo {
                    display: s.display.clone(),
                    kind: kind.clone(),
                    results: s
                        .table
                        .as_ref()
                        .map(|(_, rows)| rows.iter().map(|(_, r)| r.clone()).collect()),

                    llvm: s.llvm.as_ref().map(|l| l.0.clone()),
                    file: class.map(|c| c.1),
                });
                n_syms += 1;
                n_syms - 1
            });
            sym_by_use.insert((*fi, link.clone(), key.clone()), n);
        }
    }

    // Encodings.
    let by_name: HashMap<&str, usize> = encs
        .iter()
        .enumerate()
        .map(|(i, e)| (e.name.as_str(), i))
        .collect();
    // Instruction classes and features, by number (few of each).
    let mut classes: Vec<String> = Vec::new();
    let mut features: Vec<String> = Vec::new();
    let mut by_first_name: HashMap<&str, usize> = HashMap::new();
    for (i, e) in encs.iter().enumerate() {
        by_first_name.entry(e.name.as_str()).or_insert(i);
    }
    let mut fns = String::new();
    let mut enc_out = String::new();
    let mut semantics_out = String::new();
    let mut n_fns = 0;
    // What the encodings refer to by index, each once (see src/tables.rs's
    // `Encoding`): their names in one string, their `ne` pairs in one
    // array, and their templates' token lists.
    let mut names = String::new();
    let mut ne_pool: Vec<String> = Vec::new();
    let mut tok_lists: Vec<String> = Vec::new();
    let mut tok_index: HashMap<String, usize> = HashMap::new();
    // `Some(cond_N)` as N, `None` as `NONE`.
    let cond_index = |f: &str| -> String {
        f.strip_prefix("Some(cond_")
            .and_then(|s| s.strip_suffix(')'))
            .map_or_else(|| "NONE".to_string(), str::to_string)
    };
    for (ei, e) in encs.iter().enumerate() {
        // Constraints.
        let mut ne = Vec::new();
        for c in &e.not_equal {
            let (lhs, bits) = c.split_once("!=").unwrap();
            let p = match lhs.split_once(':') {
                Some((h, l)) => vec![Piece::Bits(h.parse().unwrap(), l.parse().unwrap())],
                None => pieces(lhs, &e.fields).unwrap_or_default(),
            };
            if let [Piece::Bits(h, l)] = p[..] {
                let mut mask = 0u32;
                let mut val = 0u32;
                for (i, ch) in bits.chars().enumerate() {
                    let bit = h - i as u32;
                    if bit < l {
                        break;
                    }
                    match ch {
                        '0' => mask |= 1 << bit,
                        '1' => {
                            mask |= 1 << bit;
                            val |= 1 << bit;
                        }
                        _ => {}
                    }
                }
                ne.push(format!("({mask:#x},{val:#x})"));
            }
        }
        // UNDEFINED conditions.
        let mut undef_fn = "None".to_string();
        let mut conds = Vec::new();
        for u in &e.undef {
            match condition(u, &e.fields) {
                Some(c) => conds.push(c),
                None => warnings.push(format!("undef in {}: {u}", e.name)),
            }
        }
        if let Some(c) = any_of(conds) {
            let _ = writeln!(
                fns,
                "#[allow(unused_parens, unused_variables, clippy::all, clippy::pedantic)]\nfn cond_{n_fns}(w: u32) -> bool {{ {c} }}"
            );
            undef_fn = format!("Some(cond_{n_fns})");
            n_fns += 1;
        }
        // CONSTRAINED UNPREDICTABLE conditions.
        let mut unpred_fn = "None".to_string();
        let conds: Vec<String> = e
            .unpred
            .iter()
            .filter_map(|u| condition(u, &e.fields))
            .collect();
        if let Some(c) = any_of(conds) {
            let _ = writeln!(
                fns,
                "#[allow(unused_parens, unused_variables, clippy::all, clippy::pedantic)]
fn cond_{n_fns}(w: u32) -> bool {{ {c} }}"
            );
            unpred_fn = format!("Some(cond_{n_fns})");
            n_fns += 1;
        }
        // Alias.
        let (alias_of, prefer) = match &e.alias {
            Some((target, cond)) => {
                let target = by_name.get(target.as_str()).copied();
                let pref = match condition(cond, &e.fields) {
                    Some(c) => {
                        let _ = writeln!(
                            fns,
                            "#[allow(unused_parens, unused_variables, clippy::all, clippy::pedantic)]\nfn cond_{n_fns}(w: u32) -> bool {{ {c} }}"
                        );
                        n_fns += 1;
                        format!("Some(cond_{})", n_fns - 1)
                    }
                    None => {
                        warnings.push(format!("alias in {}: {cond}", e.name));
                        "None".into()
                    }
                };
                (target.map_or("NONE".to_string(), |t| t.to_string()), pref)
            }
            None => ("NONE".to_string(), "None".into()),
        };
        // The template, with symbols as indexes into SYMBOLS.
        let listed = comma_lists(&e.asm, &files[e.file].syms, &e.name);
        let mut asm = String::new();
        let mut rest = listed.as_str();
        while let Some(i) = rest.find("%{") {
            asm.push_str(&rest[..i]);
            let end = rest[i..].find('}').unwrap() + i;
            let link = &rest[i + 2..end];
            match sym_by_use.get(&(e.file, link.to_string(), fields_key(&e.fields))) {
                Some(n) => {
                    let _ = write!(asm, "%{n}%");
                }
                None => {
                    warnings.push(format!("symbol {link} in {}", e.name));
                    asm.push('?');
                }
            }
            rest = &rest[end + 1..];
        }
        asm.push_str(rest);
        // Register-list braces have spaces inside ("{ <Zt1>.B-<Zt2>.B }");
        // optional groups do not. Mark the list braces so the formatter
        // prints them literally.
        let asm = mark_list_braces(&asm);
        let (mnem, ops, cond) = template_code(&asm, &displays);
        // The operands (with the condition Microsoft folds into the
        // mnemonic, last).
        let record = &access.enc_records[ei].1;
        let record_ops = &access.record_ops[record];
        let flags = access.record_flags[record];
        let file_access = |f: u8| {
            record_ops
                .values()
                .filter(|(_, c)| {
                    access
                        .classes
                        .values()
                        .any(|&(i, file)| i == *c && file == f)
                })
                .fold(0, |a, (x, _)| a | x)
        };
        let mut op_text = asm.split_once(' ').map_or("", |(_, o)| o).to_string();
        let cond_sym = cond.parse::<usize>().ok();
        if cond_sym.is_some()
            && let Some(i) = op_text.rfind(", %")
        {
            op_text.truncate(i);
        }
        let operands = operand_specs(
            &e.name,
            &op_text,
            cond_sym,
            &sym_infos,
            record_ops,
            file_access(14),
            file_access(15),
            u8::from(flags & 1 != 0) | u8::from(flags & 2 != 0) << 1,
            record == "CHKFEAT",
        );
        // Whether a word can fail to render: whether any of the template's
        // symbols can.
        let checked = asm
            .split('%')
            .skip(1)
            .step_by(2)
            .filter_map(|n| n.parse::<usize>().ok())
            .any(|n| may_fail.get(n).copied().unwrap_or(true));
        // The first encoding of the same name (an encoding can be in parts,
        // one per operand layout, all with its name).
        let group = by_first_name[e.name.as_str()];
        let ms = ms_preference(&e.name);
        let rule = ms_rule(&e.name);
        // Microsoft keeps a space before the register of a post-indexed
        // structure load or store: "[x2], x0".
        let post_index = e.name.contains("_asisdlsop_") || e.name.contains("_asisdlsep_");
        let name_at = names.len();
        names.push_str(&e.name);
        assert!(e.name.len() <= 255, "{}: a name too long", e.name);
        let ne_at = ne_pool.len();
        let ne_len = ne.len();
        ne_pool.extend(ne);
        assert!(
            ne_at + ne_len <= 0xFFFF && ne_len <= 255,
            "too many `ne` pairs"
        );
        let mut list = |toks: String| -> usize {
            *tok_index.entry(toks.clone()).or_insert_with(|| {
                tok_lists.push(toks);
                tok_lists.len() - 1
            })
        };
        let mnem = list(mnem);
        let ops = list(ops);
        let _ = writeln!(
            enc_out,
            "    Encoding {{ name_at: {name_at}, name_len: {}, class: {}, feature: {}, mask: {:#010x}, value: {:#010x}, should: ({:#010x}, {:#010x}), ne_at: {ne_at}, ne_len: {ne_len}, undef: {}, unpred: {}, alias_of: {alias_of}, prefer: {}, mnem: {mnem}, ops: {ops}, cond: {cond}, checked: {checked}, group: {group}, ms: {ms}, post_index: {post_index}, rule: {rule} }},",
            e.name.len(),
            index_of(&mut classes, &e.class),
            index_of(&mut features, &e.feature),
            e.mask,
            e.value,
            e.should_mask,
            e.should_value,
            cond_index(&undef_fn),
            cond_index(&unpred_fn),
            cond_index(&prefer),
        );
        // (Apart from the encodings, which decoding scans: what it reads
        // and writes, and its operands.)
        let _ = writeln!(
            semantics_out,
            "    Semantics {{ record: {}, operands: {operands} }},",
            access.enc_records[ei].0
        );
    }

    // A first-level index on bits 31:21 (and 13:10 within groups too large
    // to scan): each bucket lists the encodings that can match.
    let mut buckets: Vec<Vec<u16>> = vec![Vec::new(); 2048];
    for (i, e) in encs.iter().enumerate() {
        let m = e.mask >> 21;
        let v = e.value >> 21;
        for b in 0..2048u32 {
            if b & m == v {
                buckets[b as usize].push(i as u16);
            }
        }
    }
    let mut bucket_out = String::new();
    for b in &buckets {
        let ids: Vec<String> = b.iter().map(u16::to_string).collect();
        let _ = writeln!(bucket_out, "    &[{}],", ids.join(","));
    }

    let _ = writeln!(code, "pub(crate) const NONE: u16 = u16::MAX;");
    chunked(&mut code, "ENCODINGS", "Encoding", &enc_out, encs.len());
    assert!(names.len() <= u32::MAX as usize);
    let _ = writeln!(code, "pub(crate) static NAMES: &str = {names:?};");
    let _ = writeln!(
        code,
        "pub(crate) static NE: [(u32, u32); {}] = [{}];",
        ne_pool.len(),
        ne_pool.join(",")
    );
    assert!(tok_lists.len() < 0xFFFF && n_fns < 0xFFFF);
    let _ = writeln!(
        code,
        "pub(crate) static TOK_LISTS: [&[Tok]; {}] = [\n{}\n];",
        tok_lists.len(),
        tok_lists.join(",\n")
    );
    let conds: Vec<String> = (0..n_fns).map(|n| format!("cond_{n}")).collect();
    let _ = writeln!(
        code,
        "static CONDS: [fn(u32) -> bool; {n_fns}] = [{}];",
        conds.join(", ")
    );
    chunked(
        &mut code,
        "SEMANTICS",
        "Semantics",
        &semantics_out,
        encs.len(),
    );
    assert!(classes.len() <= 256 && features.len() <= 256);
    for (name, list) in [("CLASSES", &classes), ("FEATURES", &features)] {
        let _ = writeln!(
            code,
            "pub(crate) static {name}: [&str; {}] = {list:?};",
            list.len()
        );
    }
    chunked(&mut code, "SYMBOLS", "Symbol", &sym_out, n_syms);
    let _ = writeln!(
        code,
        "pub(crate) static BUCKETS: [&[u16]; 2048] = [\n{bucket_out}];"
    );
    code.push_str(&fns);

    // System register names, by op0:op1:CRn:CRm:op2.
    let sysregs_path = Path::new("tables/sysregs.txt");
    println!("cargo:rerun-if-changed={}", sysregs_path.display());
    let mut sysregs: Vec<(u32, String)> = std::fs::read_to_string(sysregs_path)
        .unwrap()
        .lines()
        .filter(|l| !l.starts_with('#'))
        .filter_map(|l| {
            let p: Vec<&str> = l.split_whitespace().collect();
            let n = |i: usize| p.get(i)?.parse::<u32>().ok();
            let key = n(0)? << 14 | n(1)? << 11 | n(2)? << 7 | n(3)? << 3 | n(4)?;
            Some((key, p.get(5)?.to_string()))
        })
        .collect();
    sysregs.sort();
    sysregs.dedup_by_key(|e| e.0);
    let _ = writeln!(
        code,
        "pub(crate) static SYSREGS: [(u32, &str); {}] = [",
        sysregs.len()
    );
    for (k, n) in &sysregs {
        let _ = writeln!(code, "    ({k:#x}, {n:?}),");
    }
    let _ = writeln!(code, "];");

    let out = Path::new(&std::env::var("OUT_DIR").unwrap()).join("tables.rs");
    std::fs::write(out, code).unwrap();
    let report = Path::new(&std::env::var("OUT_DIR").unwrap()).join("warnings.txt");
    std::fs::write(report, warnings.join("\n")).unwrap();
}

/// A table result that is a computed value: `UInt(<pieces>)`, optionally
/// plus or minus a constant.
fn table_expr(r: &str, fields: &[(String, u32, u32)]) -> Option<(Vec<Piece>, i64, i64)> {
    let r = r.trim();
    // "64 - UInt(immh:immb)".
    if let Some((n, rest)) = r.split_once(" - UInt(") {
        let n: i64 = n.trim().parse().ok()?;
        let inner = rest.strip_suffix(')')?;
        return Some((pieces(inner, fields)?, -1, n));
    }
    let start = r.find("UInt(")?;
    let end = start + r[start..].find(')')?;
    let inner = &r[start + 5..end];
    let p = pieces(inner, fields)?;
    let rest = r[end + 1..].trim();
    let add = if let Some(n) = rest.strip_prefix('+') {
        n.trim().parse().ok()?
    } else if let Some(n) = rest.strip_prefix('-') {
        -n.trim().parse::<i64>().ok()?
    } else if rest.is_empty() {
        0
    } else {
        return None;
    };
    if start != 0 {
        return None;
    }
    Some((p, 1, add))
}

/// Marks register-list braces ("{ <Zt1>.B-<Zt2>.B }") as \u{1} and \u{2},
/// leaving optional groups ("{, <shift>}", "{ <amount>}") alone: a list's
/// closing brace has a space before it.
fn mark_list_braces(asm: &str) -> String {
    let chars: Vec<char> = asm.chars().collect();
    let mut out = chars.clone();
    for (i, &c) in chars.iter().enumerate() {
        if c != '{' || chars.get(i + 1) != Some(&' ') {
            continue;
        }
        let mut depth = 0;
        for j in i..chars.len() {
            match chars[j] {
                '{' => depth += 1,
                '}' => {
                    depth -= 1;
                    if depth == 0 {
                        if chars[j - 1] == ' ' {
                            out[i] = '\u{1}';
                            out[j] = '\u{2}';
                            // Drop the spaces inside the braces.
                            out[i + 1] = '\u{0}';
                            out[j - 1] = '\u{0}';
                        }
                        break;
                    }
                }
                _ => {}
            }
        }
    }
    out.into_iter().filter(|&c| c != '\u{0}').collect()
}

/// Microsoft writes a register list as a range ("{z0.d-z1.d}") when its
/// first register is aligned to the list's length (encoded "times 2"), and
/// as a list ("{z2.s,z3.s}") when any register can start it.
fn comma_lists(asm: &str, syms: &[Sym], name: &str) -> String {
    // (Except for the multiply-long-long forms, which Microsoft writes as
    // ranges.)
    if name.contains("mlall") || name.contains("mlsll") {
        return asm.to_string();
    }
    let mut out = String::new();
    let mut rest = asm;
    while let Some(i) = rest.find("{ %{") {
        out.push_str(&rest[..i]);
        let Some(close) = rest[i..].find(" }") else {
            // Not a register list (`{ %{amount}}`): the rest as it is.
            rest = &rest[i..];
            break;
        };
        let list = &rest[i..i + close];
        let first = list[4..].split('}').next().unwrap_or("");
        let aligned = syms
            .iter()
            .find(|s| s.link == first)
            .is_some_and(|s| s.has("aligned") || s.table.is_some());
        // Only two-register lists ("<Zn1>-<Zn2>", not "<Zn1>-<Zn4>") are
        // written with a comma.
        let last = list
            .split("-%{")
            .nth(1)
            .and_then(|s| s.split('}').next())
            .unwrap_or("");
        let two = syms
            .iter()
            .find(|s| s.link == last)
            .is_some_and(|s| s.display.trim_end_matches('>').ends_with('2'));
        if aligned || !two || !list.contains("-%{") {
            out.push_str(list);
        } else {
            out.push_str(&list.replace("-%{", ", %{"));
        }
        rest = &rest[i + close..];
    }
    out.push_str(rest);
    out
}

// ---------------------------------------------------------------------------
// Templates, compiled to tokens (what src/format.rs's `render` walks): the
// same reading of the template text the formatter made at run time.

/// Code for `Tok`s of the template text `t` (`ops`: the operands, not
/// the mnemonic).
fn tokens(t: &str, displays: &[String], ops: bool) -> String {
    let mut out = Vec::new();
    let mut rest = t;
    while !rest.is_empty() {
        // (Alternations, "(<prfop>|#<imm5>)", would need the formatter to
        // try each in turn: no template has one.)
        assert!(
            !rest
                .strip_prefix('(')
                .is_some_and(|a| alternation_end(a).is_some()),
            "an alternation in template {t:?}"
        );
        if let Some(after) = rest.strip_prefix('{') {
            let mut depth = 1;
            let mut end = None;
            for (i, c) in after.char_indices() {
                match c {
                    '{' => depth += 1,
                    '}' => {
                        depth -= 1;
                        if depth == 0 {
                            end = Some(i);
                            break;
                        }
                    }
                    _ => {}
                }
            }
            let end = end.unwrap_or_else(|| panic!("an unclosed group in template {t:?}"));
            let inner = &after[..end];
            // Groups of plain text ("{, VGx2}") are always shown, except a
            // zero offset ("{, #0}").
            let optional = inner.contains('%') || inner.trim() == ", #0";
            out.push(format!(
                "Tok::Group({}, {optional})",
                tokens(inner, displays, ops)
            ));
            rest = &after[end + 1..];
        } else if let Some(after) = rest.strip_prefix('%') {
            let index = after
                .find('%')
                .and_then(|end| Some((after[..end].parse::<usize>().ok()?, end)));
            let (index, end) = index
                .filter(|(i, _)| *i < displays.len())
                .unwrap_or_else(|| panic!("a bad symbol in template {t:?}"));
            out.push(format!("Tok::Sym({index})"));
            rest = &after[end + 1..];
        } else {
            // Literal text, up to the next group, symbol, or alternation (a
            // lone parenthesis is literal).
            let end = rest[1..]
                .find(['{', '%', '('])
                .map_or(rest.len(), |i| i + 1);
            let next = &rest[end..];
            out.push(format!(
                "Tok::Lit({:?})",
                literal_text(&rest[..end], next, ops)
            ));
            rest = next;
        }
    }
    format!("&[{}]", out.join(","))
}

/// A template's literal text as the formatter shows it (`next` is the
/// template after it; `ops`: in the operands).
fn literal_text(raw: &str, next: &str, ops: bool) -> String {
    // mark_list_braces marks register-list braces as \u{1} and \u{2}.
    let literal = raw.replace('\u{1}', "{").replace('\u{2}', "}");
    // Lower case, except for MUL VL and MUL #.
    let mut lower = literal
        .to_ascii_lowercase()
        .replace("mul vl", "MUL VL")
        .replace("mul #", "MUL #")
        .replace("rctx", "RCTX");
    // A system register by number: S<op0>_<op1>_C<n>_C<m>_<op2>.
    let sysreg = next.starts_with('%')
        && next[1..]
            .find('%')
            .is_some_and(|i| next[i + 2..].starts_with('_'));
    if literal.ends_with('S') && sysreg {
        lower.pop();
        lower.push('S');
    }
    let text = fix_numbers(&lower);
    if !ops {
        return text;
    }
    // Microsoft's operands have no space after a comma. (No symbol's text
    // has a comma or starts with a space, so the literals hold them all.)
    assert!(!text.contains(",  "), "a comma and two spaces in {raw:?}");
    // Comparisons with zero are "#0", not "#0.0".
    text.replace(", ", ",").replace("#0.0", "#0")
}

/// src/format.rs's `fix_numbers`, for literal text (which has no
/// verbatim marks).
fn fix_numbers(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut rest = s;
    while let Some(i) = rest.find('#') {
        out.push_str(&rest[..=i]);
        rest = &rest[i + 1..];
        let neg = rest.starts_with('-');
        let digits: String = rest[usize::from(neg)..]
            .chars()
            .take_while(char::is_ascii_digit)
            .collect();
        let after = &rest[usize::from(neg) + digits.len()..];
        if digits.is_empty()
            || after.starts_with('.')
            || after.starts_with('x')
            || after.starts_with('\u{3}')
        {
            continue;
        }
        let v: i64 = digits.parse().unwrap_or(0);
        let v = if neg { -v } else { v };
        let (sign, m) = if v < 0 {
            ("-", v.unsigned_abs())
        } else {
            ("", v as u64)
        };
        if m < 10 {
            let _ = write!(out, "{sign}{m}");
        } else {
            let _ = write!(out, "{sign}0x{m:X}");
        }
        rest = after;
    }
    out.push_str(rest);
    out
}

/// If `s` (just after a `(`) holds a top-level `|` before its closing
/// parenthesis, the index of that parenthesis.
fn alternation_end(s: &str) -> Option<usize> {
    let mut depth = 0;
    let mut bar = false;
    for (i, c) in s.char_indices() {
        match c {
            '(' | '{' | '[' => depth += 1,
            ']' | '}' => depth -= 1,
            ')' if depth == 0 => return bar.then_some(i),
            ')' => depth -= 1,
            '|' if depth == 0 => bar = true,
            _ => {}
        }
    }
    None
}

/// A template's mnemonic and operand tokens, and the condition operand
/// Microsoft folds into the mnemonic (`csel x0,x1,x2,eq` is `cseleq
/// x0,x1,x2`, as `b.eq` is `beq`), if any.
fn template_code(asm: &str, displays: &[String]) -> (String, String, String) {
    let (mnemonic, operands) = asm.split_once(' ').unwrap_or((asm, ""));
    let mut operands = operands;
    let mut cond = "NONE".to_string();
    if let Some(i) = operands.rfind(", %") {
        let tail = &operands[i + 3..];
        if let Some(index) = tail.strip_suffix('%').and_then(|s| s.parse::<usize>().ok())
            && displays
                .get(index)
                .is_some_and(|d| matches!(d.as_str(), "<cond>" | "<invcond>"))
        {
            cond = index.to_string();
            operands = &operands[..i];
        }
    }
    (
        tokens(mnemonic, displays, false),
        tokens(operands, displays, true),
        cond,
    )
}

/// The attributes the formatter tests as flags, in the order of
/// src/tables.rs's `attr` module.
const ATTR_FLAGS: [&str; 15] = [
    "orimm",
    "lslsp",
    "fp8",
    "x30",
    "zr31",
    "pn8",
    "page",
    "bitmask",
    "memext",
    "present01",
    "tsz",
    "bytemask",
    "mult16",
    "dupindex",
    "add",
];

/// A symbol's attributes (`a` or `a=value`) as flags.
fn attr_flags(attrs: &[String]) -> u32 {
    attrs
        .iter()
        .filter_map(|a| {
            let key = a.split_once('=').map_or(a.as_str(), |(k, _)| k);
            ATTR_FLAGS.iter().position(|f| *f == key)
        })
        .fold(0, |flags, i| flags | 1 << i)
}

/// How src/format.rs's `render_symbol` shows a symbol that has no table
/// (and that `special` doesn't), from its display name: code for a
/// `Kind`.
fn symbol_kind(display: &str, bitmask: bool) -> String {
    let name = display.trim_start_matches('<').trim_end_matches('>');
    let first = name.chars().next().unwrap_or(' ');
    let rest = name.get(1..).unwrap_or("");
    // "d", "n2", "n+1" (the next register of a list).
    let is_reg_suffix = |s: &str| {
        !s.is_empty()
            && s.chars()
                .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '+')
    };
    if (name.starts_with("W(") || name.starts_with("X(")) && name.ends_with("+1)") {
        return format!("Kind::Pair({})", first == 'X');
    }
    if let Some(base) = name.strip_suffix("|WSP").or(name.strip_suffix("|SP")) {
        return format!("Kind::GprSp({})", base.starts_with('X'));
    }
    if (first == 'W' || first == 'X') && is_reg_suffix(rest) {
        return format!("Kind::Gpr({})", first == 'X');
    }
    if matches!(first, 'B' | 'H' | 'S' | 'D' | 'Q' | 'V') && is_reg_suffix(rest) {
        return format!("Kind::Reg({:?})", first.to_ascii_lowercase().to_string());
    }
    if let Some(r) = name.strip_prefix("ZA")
        && (is_reg_suffix(r) || r.is_empty())
    {
        return "Kind::Reg(\"za\")".into();
    }
    if first == 'Z' && is_reg_suffix(rest) {
        return "Kind::Reg(\"z\")".into();
    }
    if let Some(r) = name.strip_prefix("PN")
        && is_reg_suffix(r)
    {
        return "Kind::Pn".into();
    }
    if first == 'P' && is_reg_suffix(rest) {
        return "Kind::Reg(\"p\")".into();
    }
    if matches!(name, "d" | "n" | "m" | "t" | "dn" | "a" | "s" | "t2" | "t1") {
        return "Kind::Num".into();
    }
    match name {
        "index" => "Kind::Reg(\"\")".into(),
        "label" => "Kind::Label".into(),
        "const" | "imm" if bitmask => "Kind::Bitmask".into(),
        "systemreg" => "Kind::SysReg".into(),
        "Cn" | "Cm" => "Kind::Reg(\"C\")".into(),
        _ => "Kind::Imm".into(),
    }
}

/// Whether src/format.rs's `special` may show the symbol (what it tests
/// that does not depend on the encoding or the word; `unencoded`: the
/// symbol has no bits and no value).
fn may_special(s: &Sym, unencoded: bool) -> bool {
    // (As format.rs tests attributes: `a` or `a=value`.)
    let has = |a: &str| attr_flags(&[a.to_string()]) & attr_flags(&s.attrs) != 0;
    (has("memext") && s.display == "<amount>")
        || has("present01")
        || has("tsz")
        || s.display == "<mask>"
        || unencoded
        || has("bytemask")
        || has("mult16")
        || matches!(
            s.display.as_str(),
            "<imm>" | "<lsb>" | "<width>" | "<shift>"
        )
}

/// Where Microsoft's disassembler prefers an alias differently from ARM
/// (code for src/tables.rs's `ms` constants).
fn ms_preference(alias: &str) -> &'static str {
    // Aliases that ARM's description prefers but Microsoft's disassembler
    // does not use.
    const NOT_PREFERRED: &[&str] = &[
        "MOV_INS_asimdins_IV_v",
        "MOV_INS_asimdins_IR_r",
        "MOV_DUP_asisdone_only",
        "UXTL_USHLL_asimdshf_L",
    ];
    if NOT_PREFERRED.contains(&alias) {
        return "ms::NOT_PREFERRED";
    }
    // CINC, CINV, CNEG, CSET, and CSETM: Microsoft uses them even with the
    // conditions AL and NV, which ARM excludes.
    match alias.split('_').next().unwrap_or("") {
        "CINC" | "CINV" => "ms::CINC",
        "CNEG" => "ms::CNEG",
        "CSET" | "CSETM" => "ms::CSET",
        _ => "ms::ARM",
    }
}

/// A table result format.rs's `render_symbol` shows as it is (lower case,
/// its numbers Microsoft's way), with no rule of its own for the symbol or
/// the result: the text, and whether it is the default an optional group
/// leaves out. `None` for the others, which the formatter works out.
fn shown_as_is(s: &Sym, result: &str) -> Option<(String, bool)> {
    let has = |a: &str| attr_flags(&[a.to_string()]) & attr_flags(&s.attrs) != 0;
    let d = s.display.as_str();
    let special_symbol = matches!(d, "<cond>" | "<invcond>" | "<prfop>" | "<pattern>")
        || d.ends_with("_op>")
        || has("lslsp")
        || has("orimm");
    let special_result = matches!(result, "RESERVED" | "[absent]" | "[present]")
        || result.starts_with("#u")
        || result.starts_with("#imm")
        || result.contains('|');
    if special_symbol || special_result {
        return None;
    }
    let default = result.ends_with(" #0")
        || result == "#0"
        || result.is_empty()
        || (result == "LSL" && d == "<extend>");
    Some((fix_numbers(&result.to_ascii_lowercase()), default))
}

/// The Microsoft formatting rule for an encoding (a src/tables.rs `rule`
/// constant).
fn ms_rule(name: &str) -> &'static str {
    if matches!(name, "EXT_asimdext_only") {
        "rule::EXT8"
    } else if matches!(name, "movprfx_z_p_z_" | "movprfx_z_z_") {
        "rule::MOVEPRFX"
    } else if name.starts_with("MOV_ORR_") && name.ends_with("_log_imm") {
        "rule::MOV_ORR"
    } else if name == "ORR_32_log_imm" || name == "ORR_64_log_imm" {
        "rule::ORR_IMM"
    } else if name.starts_with("LSR_UBFM") {
        "rule::LSR_UBFM"
    } else if name == "UBFX_UBFM_64M_bitfield" {
        "rule::UBFX_64"
    } else if ["ADD_", "ADDS_", "SUB_", "SUBS_"]
        .iter()
        .any(|p| name.starts_with(p))
        && name.ends_with("_addsub_shift")
    {
        "rule::ADD_SUB_SHIFT"
    } else {
        "rule::NONE"
    }
}

/// The index of `s` in `list`, added if it is not there.
fn index_of(list: &mut Vec<String>, s: &str) -> usize {
    list.iter().position(|x| x == s).unwrap_or_else(|| {
        list.push(s.to_string());
        list.len() - 1
    })
}

// ---------------------------------------------------------------------------
// Access data: `tables/a64-access.txt` (generated from LLVM by
// `tools/arm64/access.py`), compiled to src/tables.rs's `RegClass`,
// `LlvmOperand`, and `Record`.

/// src/access.rs's register numbers (`RegSet` bits) of an implicit
/// register the access table names.
fn implicit_bit(name: &str) -> u32 {
    let numbered = |prefix: &str| {
        name.strip_prefix(prefix)
            .and_then(|n| n.parse::<u32>().ok())
    };
    if let Some(n) = numbered("X").filter(|&n| n <= 30) {
        return n;
    }
    if let Some(n) = numbered("W").filter(|&n| n <= 30) {
        return n;
    }
    if let Some(n) = numbered("Z").filter(|&n| n < 32) {
        return 32 + n;
    }
    if let Some(n) = numbered("P").filter(|&n| n < 16) {
        return 64 + n;
    }
    match name {
        "LR" => 30,
        "FP" => 29,
        "SP" | "WSP" => 31,
        "FFR" => 80,
        "ZA" => 81,
        "ZT0" => 82,
        "NZCV" => 83,
        "FPCR" => 84,
        "FPSR" => 85,
        "FPMR" => 86,
        "VG" => 87,
        "SVCR" => 88,
        "PSTATE" => 89,
        "SYSREGS" => 90,
        _ => panic!("tables/a64-access.txt: an implicit register build.rs doesn't know: {name}"),
    }
}

/// The access flags (src/tables.rs's `flag` constants), in their order.
const ACCESS_FLAGS: [&str; 14] = [
    "load",
    "store",
    "call",
    "branch",
    "indirect",
    "return",
    "barrier",
    "side",
    "fpexc",
    "trap",
    "exception",
    "msr",
    "mrs",
    "msrimm",
];

/// Register files, as src/tables.rs's `file` constants.
const FILES: [&str; 16] = [
    "X", "XSP", "W", "WSP", "B", "H", "S", "D", "Q", "V64", "V128", "Z", "P", "PN", "ZA", "ZT0",
];

/// Each record's operands by name: their access (1 read, 2 write) and
/// register class.
type RecordOperands = HashMap<String, HashMap<String, (u8, usize)>>;

/// Compiles the access table into `code`. Returns each encoding's record
/// (by index, in tables/a64.txt's order) and the records' operands by
/// name (for the symbols' access), by record name.
/// What the access table says, for the operand compiler.
struct AccessData {
    /// Each encoding's record: index and name.
    enc_records: Vec<(usize, String)>,
    record_ops: RecordOperands,
    /// Each record's flags (`ACCESS_FLAGS` bits), by name.
    record_flags: HashMap<String, u32>,
    /// Each register class's index and file, by name.
    classes: HashMap<String, (usize, u8)>,
}

fn access_tables(encs: &[Enc], code: &mut String) -> AccessData {
    let path = Path::new("tables/a64-access.txt");
    println!("cargo:rerun-if-changed={}", path.display());
    let src = std::fs::read_to_string(path).unwrap();
    let mut classes: Vec<String> = Vec::new();
    let mut class_index: HashMap<String, usize> = HashMap::new();
    let mut records: Vec<String> = Vec::new();
    let mut record_index: HashMap<String, usize> = HashMap::new();
    let mut record_ops: RecordOperands = HashMap::new();
    let mut record_flags: HashMap<String, u32> = HashMap::new();
    let mut class_files: HashMap<String, (usize, u8)> = HashMap::new();
    let mut enc_records = Vec::new();
    // The record being read: name, flags, operands, defs, uses.
    type Current = Option<(String, u32, Vec<String>, u128, u128)>;
    let mut cur: Current = None;
    let flush = |cur: &mut Current,
                 records: &mut Vec<String>,
                 record_index: &mut HashMap<String, usize>| {
        if let Some((name, flags, ops, defs, uses)) = cur.take() {
            record_index.insert(name.clone(), records.len());
            records.push(format!(
                "    Record {{ name: {name:?}, flags: {flags:#x}, ops: &[{}], defs: {defs:#x}, uses: {uses:#x} }},",
                ops.join(", ")
            ));
        }
    };
    for line in src.lines() {
        if line.starts_with('#') || line.trim().is_empty() {
            continue;
        }
        if let Some(rest) = line.strip_prefix("class ") {
            let p: Vec<&str> = rest.split_whitespace().collect();
            let file = FILES
                .iter()
                .position(|f| *f == p[1])
                .unwrap_or_else(|| panic!("a register file build.rs doesn't know: {}", p[1]));
            let element = match p[2] {
                "b" => 1,
                "h" => 2,
                "s" => 3,
                "d" => 4,
                "q" => 5,
                _ => 0,
            };
            let firsts: Vec<String> = p[5]
                .split(',')
                .map(|f| {
                    if f == "-" {
                        "255".into()
                    } else {
                        f.to_string()
                    }
                })
                .collect();
            class_index.insert(p[0].to_string(), classes.len());
            class_files.insert(p[0].to_string(), (classes.len(), file as u8));
            classes.push(format!(
                "    RegClass {{ name: {:?}, file: {file}, element: {element}, count: {}, stride: {}, firsts: &[{}] }},",
                p[0],
                p[3],
                p[4],
                firsts.join(",")
            ));
        } else if let Some(rest) = line.strip_prefix("inst ") {
            flush(&mut cur, &mut records, &mut record_index);
            let p: Vec<&str> = rest.split_whitespace().collect();
            let mut flags = 0u32;
            for f in p[1].split(',').filter(|f| *f != "-") {
                let i = ACCESS_FLAGS
                    .iter()
                    .position(|a| *a == f)
                    .unwrap_or_else(|| panic!("an access flag build.rs doesn't know: {f}"));
                flags |= 1 << i;
            }
            record_ops.entry(p[0].to_string()).or_default();
            record_flags.insert(p[0].to_string(), flags);
            cur = Some((p[0].to_string(), flags, Vec::new(), 0, 0));
        } else if let Some(rest) = line.strip_prefix("  op ") {
            let (name, _, ops, _, _) = cur.as_mut().expect("an op line before any inst line");
            let p: Vec<&str> = rest.split_whitespace().collect();
            let access = match p[1] {
                "r" => 1u8,
                "w" => 2,
                "rw" => 3,
                a => panic!("an access build.rs doesn't know: {a}"),
            };
            let class = class_index[p[2]];
            let bits: Vec<Piece> = if p[3] == "-" {
                Vec::new()
            } else {
                p[3].split(',')
                    .map(|b| match b.split_once(':') {
                        Some((h, l)) => Piece::Bits(h.parse().unwrap(), l.parse().unwrap()),
                        None => {
                            let b: u32 = b.parse().unwrap();
                            Piece::Bits(b, b)
                        }
                    })
                    .collect()
            };
            let by_name = record_ops.get_mut(name.as_str()).unwrap();
            for n in p[0].split('/') {
                by_name.insert(n.to_string(), (access, class));
            }
            ops.push(format!(
                "LlvmOperand {{ access: {access}, class: {class}, bits: {} }}",
                pieces_code(&bits)
            ));
        } else if let Some(rest) = line.strip_prefix("  def ") {
            let (_, _, _, defs, _) = cur.as_mut().expect("a def line before any inst line");
            for r in rest.split_whitespace() {
                *defs |= 1u128 << implicit_bit(r);
            }
        } else if let Some(rest) = line.strip_prefix("  use ") {
            let (_, _, _, _, uses) = cur.as_mut().expect("a use line before any inst line");
            for r in rest.split_whitespace() {
                *uses |= 1u128 << implicit_bit(r);
            }
        } else if let Some(rest) = line.strip_prefix("enc ") {
            flush(&mut cur, &mut records, &mut record_index);
            let (e, r) = rest.split_once(' ').unwrap();
            enc_records.push((e.to_string(), r.to_string()));
        } else {
            panic!("tables/a64-access.txt: a line build.rs doesn't know: {line}");
        }
    }
    flush(&mut cur, &mut records, &mut record_index);
    // One record for each encoding, in the same order.
    let stale = "tables/a64-access.txt is not for this tables/a64.txt: run tools/arm64/access.py";
    assert_eq!(enc_records.len(), encs.len(), "{stale}");
    let mut out = Vec::new();
    for (e, (name, record)) in encs.iter().zip(&enc_records) {
        assert_eq!(&e.name, name, "{stale}");
        out.push((record_index[record], record.clone()));
    }
    assert!(records.len() < usize::from(u16::MAX) && classes.len() < 255);
    let _ = writeln!(
        code,
        "pub(crate) static REG_CLASSES: [RegClass; {}] = [\n{}\n];",
        classes.len(),
        classes.join("\n")
    );
    chunked(
        code,
        "RECORDS",
        "Record",
        &(records.join("\n") + "\n"),
        records.len(),
    );
    AccessData {
        enc_records: out,
        record_ops,
        record_flags,
        classes: class_files,
    }
}

// ---------------------------------------------------------------------------
// Operands: each encoding's template, split into its operands (at the
// top-level commas) and each classified by its shape, compiled to
// src/tables.rs's `OpSpec`s (what src/operand.rs evaluates).

/// What the operand compiler knows of a symbol.
#[derive(Clone)]
struct SymInfo {
    display: String,
    /// build.rs's `symbol_kind` code (`Kind::Gpr(true)`, ...).
    kind: String,
    /// Its table's results, if it has a table.
    results: Option<Vec<String>>,
    /// Its LLVM operand's name, and register file (src/tables.rs's
    /// `file`), if any.
    llvm: Option<String>,
    file: Option<u8>,
}

/// Operand kinds (src/tables.rs's `opk`), parts' roles (`role`).
mod opk {
    pub const REG: u8 = 0;
    pub const LIST: u8 = 1;
    pub const IMM: u8 = 2;
    pub const FPIMM: u8 = 3;

    pub const COND: u8 = 5;
    pub const SHIFT: u8 = 6;
    pub const MUL: u8 = 7;
    pub const MEM: u8 = 8;
    pub const ZA: u8 = 9;
    pub const SYSREG: u8 = 10;
    pub const NAME: u8 = 11;
}

mod role {
    /// A register (its number); in a list, each register in order.
    pub const REG: u8 = 0;
    /// A register's prefix: `<V>` or `<R>`, or a literal `V`, `D`, ...
    pub const PREFIX: u8 = 1;
    /// An arrangement or element size: `<T>`, or a literal `16B`, `D`.
    pub const ARR: u8 = 2;
    /// A lane index.
    pub const LANE: u8 = 3;
    /// Predication: `M` or `Z`, or `<ZM>`.
    pub const PRED: u8 = 4;
    /// Writeback (`!`): pre-indexed, or a register updated in place.
    pub const WB: u8 = 5;
    /// The last register of a range (`{ z0.d-z3.d }`).
    pub const RANGE: u8 = 6;
    /// A value: an immediate, label, condition, name, or a shift amount.
    pub const VALUE: u8 = 7;
    /// A shift's or extend's name (`LSL`, `<shift>`, `<extend>`).
    pub const SHIFT: u8 = 8;
    /// A memory operand's index register.
    pub const INDEX: u8 = 9;
    /// A memory operand's offset.
    pub const OFFSET: u8 = 10;
    /// `MUL VL`.
    pub const MULVL: u8 = 11;
    /// Post-indexing: by an immediate (`VALUE` follows) or a register.
    pub const POST: u8 = 12;
    /// ZA: a tile (number), `H` or `V`, the slice index register, offset,
    /// last offset, vector group (`VGx2`).
    pub const TILE: u8 = 13;
    pub const HV: u8 = 14;
    pub const ZAINDEX: u8 = 15;
    pub const ZAOFF: u8 = 16;
    pub const ZALAST: u8 = 17;
    pub const VGX: u8 = 18;
    /// The amount of a shift or extend.
    pub const AMOUNT: u8 = 19;
}

/// A template token: a symbol or a literal character (spaces dropped).
#[derive(Clone, Copy, PartialEq, Debug)]
enum Tk {
    S(usize),
    C(char),
}

/// A part of a compiled operand: (role, symbol or NONE, literal).
type Part = (u8, Option<usize>, String);

fn tokens_of(text: &str) -> Vec<Tk> {
    let mut out = Vec::new();
    let mut rest = text;
    while let Some(c) = rest.chars().next() {
        if c == '%'
            && let Some(end) = rest[1..].find('%')
            && let Ok(n) = rest[1..=end].parse::<usize>()
        {
            out.push(Tk::S(n));
            rest = &rest[end + 2..];
            continue;
        }
        if c != ' ' {
            out.push(Tk::C(c));
        }
        rest = &rest[c.len_utf8()..];
    }
    out
}

/// The template's operands, split at top-level commas: optional groups
/// (`{, <shift> #<amount>}`) are flattened (their commas split too); list
/// braces (marked `\u{1}` and `\u{2}`) and brackets nest.
fn split_operands(ops: &str) -> Vec<Vec<Tk>> {
    let flat: String = ops.chars().filter(|&c| c != '{' && c != '}').collect();
    let mut items = Vec::new();
    let mut cur = String::new();
    let mut depth = 0i32;
    for c in flat.chars() {
        match c {
            '[' | '\u{1}' => depth += 1,
            ']' | '\u{2}' => depth -= 1,
            _ => {}
        }
        if c == ',' && depth == 0 {
            items.push(std::mem::take(&mut cur));
        } else {
            cur.push(c);
        }
    }
    if !cur.trim().is_empty() {
        items.push(cur);
    }
    items
        .iter()
        .map(|i| tokens_of(i))
        .filter(|t| !t.is_empty())
        .collect()
}

/// A cursor over an operand's tokens.
struct Cur<'a> {
    t: &'a [Tk],
    i: usize,
}

impl Cur<'_> {
    fn done(&self) -> bool {
        self.i >= self.t.len()
    }
    fn peek(&self) -> Option<Tk> {
        self.t.get(self.i).copied()
    }
    fn char(&mut self, c: char) -> bool {
        if self.peek() == Some(Tk::C(c)) {
            self.i += 1;
            true
        } else {
            false
        }
    }
    fn word(&mut self, w: &str) -> bool {
        let n = w.chars().count();
        let ok = self.t.len() >= self.i + n
            && w.chars()
                .zip(&self.t[self.i..])
                .all(|(c, t)| *t == Tk::C(c));
        if ok {
            self.i += n;
        }
        ok
    }
    fn sym(&mut self) -> Option<usize> {
        if let Some(Tk::S(n)) = self.peek() {
            self.i += 1;
            Some(n)
        } else {
            None
        }
    }
    /// Literal characters up to (not including) any of `stop` or a symbol.
    fn text(&mut self, stop: &[char]) -> String {
        let mut s = String::new();
        while let Some(Tk::C(c)) = self.peek() {
            if stop.contains(&c) {
                break;
            }
            s.push(c);
            self.i += 1;
        }
        s
    }
}

/// Whether symbol `n` is a register (by its kind or its LLVM class).
fn is_register(syms: &[SymInfo], n: usize) -> bool {
    let s = &syms[n];
    s.file.is_some()
        || s.kind.starts_with("Kind::Gpr")
        || s.kind.starts_with("Kind::GprSp")
        || s.kind.starts_with("Kind::Pair")
        || s.kind == "Kind::Pn"
        || s.kind == "Kind::Num"
        || (s.kind.starts_with("Kind::Reg(")
            && !matches!(s.kind.as_str(), "Kind::Reg(\"\")" | "Kind::Reg(\"C\")"))
}

/// Whether symbol `n` is a shift or extend name (a table of them).
fn is_shift_name(syms: &[SymInfo], n: usize) -> bool {
    const NAMES: [&str; 14] = [
        "LSL", "LSR", "ASR", "ROR", "MSL", "UXTB", "UXTH", "UXTW", "UXTX", "SXTB", "SXTH", "SXTW",
        "SXTX", "LSL|UXTW",
    ];
    let s = &syms[n];
    matches!(s.display.as_str(), "<shift>" | "<extend>" | "<mod>")
        && s.results.as_ref().is_some_and(|r| {
            r.iter().any(|x| {
                NAMES.contains(&x.as_str())
                    || NAMES.iter().any(|n| x.starts_with(&format!("{n} ")))
                    || x.contains('|')
            })
        })
}

/// A register and what follows it: `.<T>`, `[<index>]`, `/M`, `!`.
fn register_parts(c: &mut Cur, syms: &[SymInfo], parts: &mut Vec<Part>, reg_role: u8) -> bool {
    let start = c.i;
    // A prefix: `<V><d>`, `<R><n>`, or a literal letter (`V<m>`, `D<d>`).
    let mut prefix: Option<Part> = None;
    if let Some(Tk::S(p)) = c.peek()
        && matches!(c.t.get(c.i + 1), Some(Tk::S(r)) if is_register(syms, *r))
        && matches!(syms[p].display.as_str(), "<V>" | "<R>" | "<Va>" | "<Vb>")
    {
        c.i += 1;
        prefix = Some((role::PREFIX, Some(p), String::new()));
    } else if let Some(Tk::C(l)) = c.peek()
        && "VDHSBQZWXP".contains(l)
        && matches!(c.t.get(c.i + 1), Some(Tk::S(r)) if is_register(syms, *r))
    {
        c.i += 1;
        prefix = Some((role::PREFIX, None, l.to_string()));
    }
    let Some(r) = c.sym().filter(|&r| is_register(syms, r)) else {
        c.i = start;
        return false;
    };
    parts.extend(prefix);
    parts.push((reg_role, Some(r), String::new()));
    if c.char('.') {
        match c.sym() {
            Some(a) => parts.push((role::ARR, Some(a), String::new())),
            None => {
                let t = c.text(&['[', ']', '/', '!', '-', ',', '\u{2}']);
                parts.push((role::ARR, None, t));
            }
        }
    }
    lane_parts(c, parts);
    if c.char('/') {
        match c.sym() {
            Some(p) => parts.push((role::PRED, Some(p), String::new())),
            None => {
                let t = c.text(&[]);
                parts.push((role::PRED, None, t));
            }
        }
    }
    if c.char('!') {
        parts.push((role::WB, None, String::new()));
    }
    true
}

/// `[<index>]` or `[1]`.
fn lane_parts(c: &mut Cur, parts: &mut Vec<Part>) {
    let start = c.i;
    if !c.char('[') {
        return;
    }
    let part = match c.sym() {
        Some(i) => (role::LANE, Some(i), String::new()),
        None => (role::LANE, None, c.text(&[']'])),
    };
    if c.char(']') {
        parts.push(part);
    } else {
        c.i = start;
    }
}

/// A value after an optional `#`: a symbol, or literal digits (`#16`,
/// `#0.0`, `#-8`).
fn value_part(c: &mut Cur, role: u8) -> Option<Part> {
    let start = c.i;
    c.char('#');
    if let Some(s) = c.sym() {
        return Some((role, Some(s), String::new()));
    }
    let t = c.text(&[',', ']', '!']);
    if !t.is_empty()
        && t.chars()
            .all(|ch| ch.is_ascii_digit() || ch == '.' || ch == '-')
    {
        return Some((role, None, t));
    }
    c.i = start;
    None
}

/// The classified operand: (kind, parts).
type Spec = (u8, Vec<Part>);

fn classify(t: &[Tk], syms: &[SymInfo]) -> Option<Spec> {
    let mut c = Cur { t, i: 0 };
    let mut parts = Vec::new();
    // Memory: `[base{, offset|index{, extend amount}}{, MUL VL}]{!}`.
    if c.char('[') {
        if !register_parts(&mut c, syms, &mut parts, role::REG) {
            return None;
        }
        while c.char(',') {
            if c.word("MULVL") {
                parts.push((role::MULVL, None, String::new()));
            } else if c.peek() == Some(Tk::C('#')) {
                parts.push(value_part(&mut c, role::OFFSET)?);
            } else if register_parts(&mut c, syms, &mut parts, role::INDEX)
                || shift_parts(&mut c, syms, &mut parts).is_some()
            {
            } else {
                parts.push(value_part(&mut c, role::OFFSET)?);
            }
        }
        if !c.char(']') {
            return None;
        }
        if c.char('!') {
            parts.push((role::WB, None, String::new()));
        }
        return c.done().then_some((opk::MEM, parts));
    }
    // Register lists: `{ <Zt1>.<T>-<Zt4>.<T> }{[<index>]}`.
    if c.char('\u{1}') {
        // (ZERO { <mask> }, { ZT0 }, LD1B { ZA0<HV>.B[<Ws>, <offs>] }: not
        // lists of registers.)
        if let Some(Tk::S(m)) = c.peek()
            && syms[m].display == "<mask>"
        {
            c.i += 1;
            return c
                .char('\u{2}')
                .then(|| (opk::NAME, vec![(role::VALUE, Some(m), String::new())]));
        }
        if c.word("ZT0") && c.char('\u{2}') {
            return Some((opk::REG, vec![(role::REG, None, "ZT0".into())]));
        }
        let inner_start = c.i;
        if za_parts(&mut c, syms, &mut parts) && c.char('\u{2}') {
            return c.done().then_some((opk::ZA, parts));
        }
        c.i = inner_start;
        parts.clear();
        loop {
            if !register_parts(&mut c, syms, &mut parts, role::REG) {
                return None;
            }
            if c.char('-') && !register_parts(&mut c, syms, &mut parts, role::RANGE) {
                return None;
            }
            if c.char('\u{2}') {
                break;
            }
            if !c.char(',') {
                return None;
            }
        }
        lane_parts(&mut c, &mut parts);
        return c.done().then_some((opk::LIST, parts));
    }
    // SME: ZA, its tiles and slices; ZT0.
    let start = c.i;
    if za_parts(&mut c, syms, &mut parts) && c.done() {
        return Some((opk::ZA, parts));
    }
    c.i = start;
    parts.clear();
    if c.word("ZT0") {
        parts.push((role::REG, None, "ZT0".into()));
        if c.char('[') {
            let off = value_part(&mut c, role::LANE)?;
            parts.push(off);
            if c.char(',') && !c.word("MULVL") {
                return None;
            }
            if !c.char(']') {
                return None;
            }
        }
        return c.done().then_some((opk::REG, parts));
    }
    // A system register by number: S<op0>_<op1>_<Cn>_<Cm>_<op2>.
    if c.char('S')
        && let Some(op0) = c.sym()
        && c.peek() == Some(Tk::C('_'))
    {
        parts.push((role::VALUE, Some(op0), String::new()));
        while c.char('_') {
            c.char('C');
            parts.push((role::VALUE, Some(c.sym()?), String::new()));
        }
        return c.done().then_some((opk::SYSREG, parts));
    }
    c.i = start;
    parts.clear();
    // Registers.
    if register_parts(&mut c, syms, &mut parts, role::REG) {
        // (`<Pm>.<T>[<Wv>, <imm>]`: PSEL's predicate, indexed by a
        // register and an immediate.)
        if c.char('[') {
            parts.push((role::ZAINDEX, Some(c.sym()?), String::new()));
            if !c.char(',') {
                return None;
            }
            parts.push(value_part(&mut c, role::ZAOFF)?);
            if !c.char(']') {
                return None;
            }
        }
        return c.done().then_some((opk::REG, parts));
    }
    // A fixed register (`CHKFEAT X16`).
    if c.word("X16") && c.done() {
        return Some((opk::REG, vec![(role::REG, None, "X16".into())]));
    }
    // MUL #<imm>.
    if c.word("MUL#") {
        parts.push((role::VALUE, Some(c.sym()?), String::new()));
        return c.done().then_some((opk::MUL, parts));
    }
    // Shifts and extends.
    let s = c.i;
    if shift_parts(&mut c, syms, &mut parts).is_some() && c.done() {
        return Some((opk::SHIFT, parts));
    }
    c.i = s;
    parts.clear();
    // Immediates, labels, conditions, names.
    if let Some(p) = value_part(&mut c, role::VALUE) {
        if !c.done() {
            // (`<option>nXS`: a barrier option, then the literal.)
            let rest = c.text(&[]);
            if rest == "nXS" && c.done() {
                return Some((opk::NAME, vec![p]));
            }
            return None;
        }
        // (Which of an immediate, a label, a condition, or a name: src/
        // operand.rs works it out from the symbol and its value.)
        let kind = if p.1.is_none() && p.2.contains('.') {
            opk::FPIMM
        } else {
            opk::IMM
        };
        return Some((kind, vec![p]));
    }
    // Literal names: CSYNC, DSYNC, RCTX, PH, STSHSTRM, ...
    let text = c.text(&[]);
    if c.done() && !text.is_empty() && text.chars().all(|ch| ch.is_ascii_alphanumeric()) {
        return Some((opk::NAME, vec![(role::VALUE, None, text)]));
    }
    None
}

/// A shift or extend: `LSL #<amount>`, `<shift> #<amount>`, `<extend>{
/// <amount>}`, `<mod> #3`, `LSL #3`, `SXTW`, `<shift>` (a table of
/// `LSL #8`), or ZA's `VGx2` (which `za_parts` takes).
fn shift_parts(c: &mut Cur, syms: &[SymInfo], parts: &mut Vec<Part>) -> Option<()> {
    let start = c.i;
    if let Some(n) = c.sym() {
        if !is_shift_name(syms, n) {
            c.i = start;
            return None;
        }
        parts.push((role::SHIFT, Some(n), String::new()));
    } else {
        let name = c.text(&['#', ',', ']']);
        const NAMES: [&str; 13] = [
            "LSL", "LSR", "ASR", "ROR", "MSL", "UXTB", "UXTH", "UXTW", "UXTX", "SXTB", "SXTH",
            "SXTW", "SXTX",
        ];
        // (`LSL<amount>`: a literal LSL and the amount symbol.)
        if !NAMES.contains(&name.as_str()) {
            c.i = start;
            return None;
        }
        parts.push((role::SHIFT, None, name));
    }
    if matches!(c.peek(), Some(Tk::S(_) | Tk::C('#'))) {
        let p = value_part(c, role::AMOUNT)?;
        parts.push(p);
    }
    Some(())
}

/// SME's ZA operands: `ZA`, `ZA.<T>[<Wv>, <offs>{, VGx2}]`,
/// `ZA[<Wv>, <offs>]`, a tile `<ZAda>.S`, a slice
/// `ZA0<HV>.B[<Ws>, <offs>]` or `<ZAd><HV>.H[<Ws>, <offs1>:<offs2>]`.
fn za_parts(c: &mut Cur, syms: &[SymInfo], parts: &mut Vec<Part>) -> bool {
    let start = c.i;
    let tile = match c.peek() {
        Some(Tk::S(n)) if syms[n].display.starts_with("<ZA") => {
            c.i += 1;
            Some((role::TILE, Some(n), String::new()))
        }
        Some(Tk::C('Z')) if c.word("ZA") => {
            let digits = c.text(&['.', '[', '<', ',', '\u{2}']);
            if digits.is_empty() {
                None
            } else if digits.chars().all(|d| d.is_ascii_digit()) {
                Some((role::TILE, None, digits))
            } else {
                c.i = start;
                return false;
            }
        }
        _ => return false,
    };
    parts.extend(tile);
    if let Some(Tk::S(n)) = c.peek()
        && syms[n].display == "<HV>"
    {
        c.i += 1;
        parts.push((role::HV, Some(n), String::new()));
    }
    if c.char('.') {
        match c.sym() {
            Some(a) => parts.push((role::ARR, Some(a), String::new())),
            None => {
                let t = c.text(&['[', ',', '\u{2}']);
                parts.push((role::ARR, None, t));
            }
        }
    }
    if c.char('[') {
        let Some(index) = c.sym() else {
            c.i = start;
            return false;
        };
        parts.push((role::ZAINDEX, Some(index), String::new()));
        if c.char(',') {
            match value_part(c, role::ZAOFF) {
                Some(p) => parts.push(p),
                None => {
                    c.i = start;
                    return false;
                }
            }
            if c.char(':') {
                match value_part(c, role::ZALAST) {
                    Some(p) => parts.push(p),
                    None => {
                        c.i = start;
                        return false;
                    }
                }
            }
            if c.char(',') {
                let vg = c.text(&[']']);
                parts.push((role::VGX, None, vg));
            }
        }
        if !c.char(']') {
            c.i = start;
            return false;
        }
    }
    true
}

/// Each operand of an encoding, as code for src/tables.rs's `OpSpec`s.
/// `access`: the LLVM record's operands' access by name; `za_access` and
/// `zt_access` the access of its ZA and ZT0 operands (which have no
/// symbol); `mem`: its memory access (1 load, 2 store).
#[allow(clippy::too_many_arguments)]
fn operand_specs(
    name: &str,
    ops: &str,
    cond: Option<usize>,
    syms: &[SymInfo],
    access: &HashMap<String, (u8, usize)>,
    za_access: u8,
    zt_access: u8,
    mem: u8,
    implicit_x16: bool,
) -> String {
    let items = split_operands(ops);
    let mut specs: Vec<Spec> = Vec::new();
    for t in &items {
        let Some(spec) = classify(t, syms) else {
            let shape: String = t
                .iter()
                .map(|x| match x {
                    Tk::S(n) => syms[*n].display.clone(),
                    Tk::C(c) => c.to_string(),
                })
                .collect();
            panic!("{name}: an operand build.rs can't classify: {shape:?}");
        };
        specs.push(spec);
    }
    if let Some(c) = cond {
        specs.push((opk::COND, vec![(role::VALUE, Some(c), String::new())]));
    }
    // Post-indexing: a memory operand without `!` whose base is written
    // back takes the operand after it as its offset.
    let base_access = |parts: &[Part]| {
        parts
            .iter()
            .find(|p| p.0 == role::REG)
            .and_then(|p| p.1)
            .and_then(|n| syms[n].llvm.as_ref())
            .and_then(|op| access.get(op))
            .map_or(1, |a| a.0)
    };
    let mut out: Vec<Spec> = Vec::new();
    let mut i = 0;
    while i < specs.len() {
        let (kind, mut parts) = specs[i].clone();
        if kind == opk::MEM
            && !parts.iter().any(|p| p.0 == role::WB)
            && base_access(&parts) & 2 != 0
            && let Some((next_kind, next)) = specs.get(i + 1)
            && matches!(*next_kind, opk::IMM | opk::REG)
        {
            parts.push((role::POST, None, String::new()));
            for p in next {
                // (A post-index register is an index; an immediate, the
                // offset.)
                let r = if *next_kind == opk::REG {
                    role::INDEX
                } else {
                    role::OFFSET
                };
                parts.push((
                    if p.0 == role::REG || p.0 == role::VALUE {
                        r
                    } else {
                        p.0
                    },
                    p.1,
                    p.2.clone(),
                ));
            }
            out.push((kind, parts));
            i += 2;
            continue;
        }
        out.push((kind, parts));
        i += 1;
    }
    // Each operand's access: a register's, its LLVM operand's (the first
    // of several mentions of a register written and read is the write,
    // the others the reads); memory's, the record's.
    let mut seen: std::collections::HashSet<usize> = std::collections::HashSet::new();
    let mut code = Vec::new();
    for (kind, parts) in &out {
        let first_reg = parts
            .iter()
            .find(|p| p.0 == role::REG || p.0 == role::TILE)
            .and_then(|p| p.1);
        let a = match *kind {
            opk::MEM => mem,
            opk::ZA => {
                match first_reg
                    .and_then(|n| syms[n].llvm.as_ref())
                    .and_then(|op| access.get(op))
                {
                    Some(a) => a.0,
                    None => za_access,
                }
            }
            opk::REG | opk::LIST => match first_reg {
                Some(n) => match syms[n]
                    .llvm
                    .as_ref()
                    .and_then(|op| access.get(op))
                    .map(|a| a.0)
                {
                    Some(3) => {
                        if seen.insert(n) {
                            // (Written, and read again later: ADD Zdn,
                            // Pg/M, Zdn, Zm. Written and read here if not.)
                            let mentions = out
                                .iter()
                                .flat_map(|(_, p)| p.iter())
                                .filter(|p| p.0 == role::REG && p.1 == Some(n))
                                .count();
                            if mentions > 1 { 2 } else { 3 }
                        } else {
                            1
                        }
                    }
                    Some(a) => a,
                    // (No LLVM operand: read.)
                    None => 1,
                },
                None if parts.iter().any(|p| p.2 == "ZT0") => zt_access,
                None if implicit_x16 => 3,
                None => 1,
            },
            _ => 0,
        };
        let parts_code: Vec<String> = parts
            .iter()
            .map(|(r, s, l)| {
                format!(
                    "({r}, {}, {l:?})",
                    s.map_or("NONE".to_string(), |n| n.to_string())
                )
            })
            .collect();
        code.push(format!(
            "OpSpec {{ kind: {kind}, access: {a}, parts: &[{}] }}",
            parts_code.join(", ")
        ));
    }
    format!("&[{}]", code.join(", "))
}
