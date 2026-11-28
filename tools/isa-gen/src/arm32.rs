//! Compiles tables/aarch32.txt (tools/arm32/extract.py's output) into
//! decode tables: each encoding's fixed bits, "should be" bits, and
//! conditions (an ASL subset of bit slices, patterns, `==`, `!=`, `IN`,
//! `!`, `&&`, `||`), bucketed by some of each instruction set's bits; its
//! operands' fields; and the operand rules its template names.

// (Field widths and table sizes are small; the casts are bounded.)
#![allow(clippy::cast_possible_truncation)]

use std::fmt::Write as _;

#[derive(Default)]
struct Enc {
    name: String,
    iset: String,
    width: u32,
    bits: String,
    sb: String,
    cond: Option<String>,
    feat: String,
    asm: String,
    /// The aliases' templates (for their mnemonics).
    aliases: Vec<String>,
    /// Its fields: name, (hi, lo).
    fields: Vec<(String, (u32, u32))>,
    /// Its operands: display, and the fields they are encoded in.
    operands: Vec<(String, Vec<String>)>,
}

/// An operand rule: its display (`<Rd>`), and its text (a rule) or its
/// alternatives (a choice).
struct Rule {
    id: String,
    display: String,
    text: Option<String>,
    alts: Vec<(String, String)>,
}

fn parse(src: &str) -> (Vec<Enc>, Vec<Rule>) {
    let mut out: Vec<Enc> = Vec::new();
    let mut rules: Vec<Rule> = Vec::new();
    let mut in_alias = false;
    let mut in_rule = false;
    for line in src.lines() {
        if let Some(rest) = line.strip_prefix("rule ") {
            let mut it = rest.splitn(3, ' ');
            rules.push(Rule {
                id: it.next().unwrap().into(),
                display: it.next().unwrap_or("-").into(),
                text: Some(it.next().unwrap_or("").into()),
                alts: Vec::new(),
            });
            in_rule = true;
            continue;
        }
        if let Some(rest) = line.strip_prefix("choice ") {
            let mut it = rest.splitn(2, ' ');
            rules.push(Rule {
                id: it.next().unwrap().into(),
                display: it.next().unwrap_or("-").into(),
                text: None,
                alts: Vec::new(),
            });
            in_rule = true;
            continue;
        }
        if in_rule && let Some(rest) = line.strip_prefix("  alt ") {
            let (name, text) = rest.split_once(' ').unwrap_or((rest, ""));
            rules
                .last_mut()
                .unwrap()
                .alts
                .push((name.into(), text.into()));
            continue;
        }
        if let Some(rest) = line.strip_prefix("enc ") {
            let mut it = rest.split_whitespace();
            out.push(Enc {
                name: it.next().unwrap().into(),
                iset: it.next().unwrap().into(),
                width: it.next().unwrap().parse().unwrap(),
                ..Enc::default()
            });
            in_alias = false;
            in_rule = false;
        } else if line.starts_with("alias ") {
            in_alias = true;
        } else if let Some(rest) = line.strip_prefix("  ") {
            let e = out.last_mut().expect("a line before any encoding");
            let (key, value) = rest.split_once(' ').unwrap_or((rest, ""));
            match (in_alias, key) {
                (false, "bits") => e.bits = value.into(),
                (false, "sb") => e.sb = value.into(),
                (false, "cond") => e.cond = Some(value.into()),
                (false, "feat") => e.feat = value.into(),
                (false, "asm") => e.asm = value.into(),
                (true, "asm") => e.aliases.push(value.into()),
                (false, "fields") => {
                    for f in value.split_whitespace() {
                        let (name, range) = f.split_once('=').unwrap();
                        let (hi, lo) = range.split_once(':').unwrap_or((range, range));
                        e.fields
                            .push((name.into(), (hi.parse().unwrap(), lo.parse().unwrap())));
                    }
                }
                (false, "in") => {
                    let mut it = value.split_whitespace();
                    let display = it.next().unwrap().to_string();
                    e.operands.push((display, it.map(String::from).collect()));
                }
                _ => {}
            }
        }
    }
    (out, rules)
}

/// (mask, value) of a pattern of 0, 1, and x, most significant first.
fn mask_value(p: &str) -> (u32, u32) {
    let (mut m, mut v) = (0u32, 0u32);
    for c in p.chars() {
        m <<= 1;
        v <<= 1;
        match c {
            '0' => m |= 1,
            '1' => {
                m |= 1;
                v |= 1;
            }
            _ => {}
        }
    }
    (m, v)
}

/// A condition, compiled to a Rust expression of `w`.
struct Cond<'a> {
    s: &'a [u8],
    i: usize,
}

impl Cond<'_> {
    fn ws(&mut self) {
        while self.i < self.s.len() && self.s[self.i] == b' ' {
            self.i += 1;
        }
    }
    fn eat(&mut self, t: &str) -> bool {
        self.ws();
        if self.s[self.i..].starts_with(t.as_bytes()) {
            self.i += t.len();
            true
        } else {
            false
        }
    }
    fn or(&mut self) -> String {
        let mut e = self.and();
        while self.eat("||") {
            e = format!("({e} || {})", self.and());
        }
        e
    }
    fn and(&mut self) -> String {
        let mut e = self.unary();
        while self.eat("&&") {
            e = format!("({e} && {})", self.unary());
        }
        e
    }
    fn unary(&mut self) -> String {
        if self.eat("!") {
            return format!("!{}", self.primary());
        }
        self.primary()
    }
    /// `(expr)`, or a comparison of a slice.
    fn primary(&mut self) -> String {
        self.ws();
        if self.s[self.i] == b'(' {
            // A parenthesized expression, or a comparison whose left side
            // is a slice.
            let save = self.i;
            self.i += 1;
            self.ws();
            if self.s[self.i] == b'[' {
                let c = self.comparison();
                if self.eat(")") {
                    return c;
                }
                self.i = save;
            } else {
                self.i = save;
            }
            self.i += 1;
            let e = self.or();
            assert!(self.eat(")"), "expected ) at {}", self.i);
            return format!("({e})");
        }
        if self.eat("TRUE") {
            return "true".into();
        }
        if self.eat("FALSE") {
            return "false".into();
        }
        self.comparison()
    }
    fn slice(&mut self) -> (u32, u32) {
        assert!(self.eat("["));
        let hi = self.number();
        assert!(self.eat(":"));
        let lo = self.number();
        assert!(self.eat("]"));
        (hi, lo)
    }
    fn number(&mut self) -> u32 {
        self.ws();
        let start = self.i;
        while self.s[self.i].is_ascii_digit() {
            self.i += 1;
        }
        std::str::from_utf8(&self.s[start..self.i])
            .unwrap()
            .parse()
            .unwrap()
    }
    fn pattern(&mut self) -> String {
        self.ws();
        assert_eq!(self.s[self.i], b'\'');
        self.i += 1;
        let start = self.i;
        while self.s[self.i] != b'\'' {
            self.i += 1;
        }
        let p = std::str::from_utf8(&self.s[start..self.i])
            .unwrap()
            .to_string();
        self.i += 1;
        p
    }
    fn comparison(&mut self) -> String {
        let (hi, lo) = self.slice();
        let field = format!("((w >> {lo}) & {:#x})", (1u64 << (hi - lo + 1)) - 1);
        let test = |p: &str| -> String {
            assert_eq!(p.len() as u32, hi - lo + 1, "pattern width");
            let (m, v) = mask_value(p);
            format!("({field} & {m:#x} == {v:#x})")
        };
        if self.eat("==") {
            let p = self.pattern();
            test(&p)
        } else if self.eat("!=") {
            let p = self.pattern();
            format!("!{}", test(&p))
        } else if self.eat("IN") {
            assert!(self.eat("{"));
            let mut alts = vec![test(&self.pattern())];
            while self.eat(",") {
                alts.push(test(&self.pattern()));
            }
            assert!(self.eat("}"));
            format!("({})", alts.join(" || "))
        } else {
            panic!("expected a comparison at {}", self.i)
        }
    }
}

fn compile(cond: &str) -> String {
    let mut c = Cond {
        s: cond.as_bytes(),
        i: 0,
    };
    let e = c.or();
    c.ws();
    assert_eq!(c.i, c.s.len(), "trailing input in {cond}");
    e
}

/// The bits each instruction set's buckets are keyed by (hi, lo).
fn bucket_bits(table: &str) -> (u32, u32) {
    match table {
        "A32" => (27, 20),
        "T16" => (15, 8),
        _ => (31, 20),
    }
}

pub fn main(dest: &std::path::Path) {
    let src = std::fs::read_to_string("tables/aarch32.txt").unwrap();
    let (encs, mut rules) = parse(&src);
    let mut out = String::new();
    let mut conds: Vec<String> = Vec::new();
    // What the encodings refer to by index, each once (see src/lib.rs's
    // `Encoding`): their names and templates in one string, and their
    // features, aliases, operands, and fields in tables of their own.
    // The rules, by ID; a template's references to them (`%{Rd__4}`) are
    // a character each, U+0100 + the rule's index (see src/format.rs).
    rules.sort_by(|a, b| a.id.cmp(&b.id));
    rules.dedup_by(|a, b| a.id == b.id);
    let rule_index: std::collections::HashMap<&str, usize> = rules
        .iter()
        .enumerate()
        .map(|(i, r)| (r.id.as_str(), i))
        .collect();
    assert!(rules.len() < 0x700, "rule references are U+0100..=U+07FF");
    let encode = |template: &str| -> String {
        assert!(template.is_ascii(), "{template}");
        let mut out = String::with_capacity(template.len());
        let mut rest = template;
        while let Some(at) = rest.find("%{") {
            out.push_str(&rest[..at]);
            let end = rest[at..].find('}').map_or(rest.len(), |e| at + e);
            let id = &rest[at + 2..end];
            // (An ID no rule has: an index past them, which formats as
            // nothing, as the ID did.)
            let i = rule_index.get(id).copied().unwrap_or(rules.len());
            out.push(char::from_u32(0x100 + i as u32).unwrap());
            rest = &rest[(end + 1).min(rest.len())..];
        }
        out.push_str(rest);
        out
    };
    // Templates once each.
    let mut templates: std::collections::HashMap<String, (usize, usize)> =
        std::collections::HashMap::new();
    let mut text = String::new();
    let mut features: Vec<String> = Vec::new();
    let mut alias_lists: Vec<String> = Vec::new();
    let mut operand_lists: Vec<String> = Vec::new();
    let mut field_lists: Vec<String> = Vec::new();
    fn intern(list: &mut Vec<String>, item: String) -> usize {
        list.iter().position(|x| *x == item).unwrap_or_else(|| {
            list.push(item);
            list.len() - 1
        })
    }
    for (table, keep) in [
        ("A32", (|e: &Enc| e.iset == "A32") as fn(&Enc) -> bool),
        ("T16", |e: &Enc| e.iset == "T32" && e.width == 16),
        ("T32", |e: &Enc| e.iset == "T32" && e.width == 32),
    ] {
        let list: Vec<&Enc> = encs.iter().filter(|e| keep(e)).collect();
        writeln!(
            out,
            "pub(crate) static {table}: [Encoding; {}] = [",
            list.len()
        )
        .unwrap();
        for e in &list {
            let (m, v) = mask_value(&e.bits);
            let (sm, sv) = if e.sb.is_empty() {
                (0, 0)
            } else {
                mask_value(&e.sb)
            };
            let cond = match &e.cond {
                Some(c) => {
                    let code = compile(c);
                    let n = match conds.iter().position(|x| *x == code) {
                        Some(n) => n,
                        None => {
                            conds.push(code);
                            conds.len() - 1
                        }
                    };
                    n.to_string()
                }
                None => "NONE".into(),
            };
            let aliases: Vec<String> = e.aliases.iter().map(|a| format!("{a:?}")).collect();
            // Operands: their fields' bits (hi, lo), most significant
            // first; those of fields the encoding doesn't have, left out.
            let operands: Vec<String> = e
                .operands
                .iter()
                .filter_map(|(display, names)| {
                    let bits: Option<Vec<String>> = names
                        .iter()
                        .map(|n| {
                            e.fields
                                .iter()
                                .find(|f| f.0 == *n)
                                .map(|f| format!("({}, {})", f.1.0, f.1.1))
                        })
                        .collect();
                    Some(format!("({display:?}, &[{}])", bits?.join(", ")))
                })
                .collect();
            let fields: Vec<String> = e
                .fields
                .iter()
                .map(|(n, (h, l))| format!("({n:?}, {h}, {l})"))
                .collect();
            let name_at = text.len();
            text.push_str(&e.name);
            let asm = encode(&e.asm);
            let (asm_at, asm_len) = *templates.entry(asm).or_insert_with_key(|asm| {
                let at = text.len();
                text.push_str(asm);
                (at, asm.len())
            });
            assert!(e.name.len() <= 255 && asm_len <= 0xFFFF && text.len() <= u32::MAX as usize);
            let feature = intern(&mut features, format!("{:?}", e.feat));
            let aliases = intern(&mut alias_lists, format!("&[{}]", aliases.join(", ")));
            let operands = intern(&mut operand_lists, format!("&[{}]", operands.join(", ")));
            let fields = intern(&mut field_lists, format!("&[{}]", fields.join(", ")));
            writeln!(
                out,
                "    Encoding {{ name_at: {name_at}, name_len: {}, mask: {m:#x}, value: {v:#x}, sb_mask: {sm:#x}, sb_value: {sv:#x}, cond: {cond}, feature: {feature}, asm_at: {asm_at}, asm_len: {}, aliases: {aliases}, operands: {operands}, fields: {fields} }},",
                e.name.len(),
                asm_len,
            )
            .unwrap();
        }
        writeln!(out, "];").unwrap();
        // Buckets: for each value of the key bits, the encodings that can
        // match, most fixed bits first (then in table order).
        let (hi, lo) = bucket_bits(table);
        let n = 1u32 << (hi - lo + 1);
        let key_mask = (((1u64 << (hi - lo + 1)) - 1) as u32) << lo;
        let mut buckets: Vec<Vec<u16>> = Vec::new();
        for k in 0..n {
            let kv = k << lo;
            let mut ids: Vec<(u32, usize)> = list
                .iter()
                .enumerate()
                .filter(|(_, e)| {
                    let (m, v) = mask_value(&e.bits);
                    (kv ^ v) & m & key_mask == 0
                })
                .map(|(i, e)| (mask_value(&e.bits).0.count_ones(), i))
                .collect();
            ids.sort_by(|a, b| b.0.cmp(&a.0).then(a.1.cmp(&b.1)));
            buckets.push(ids.into_iter().map(|(_, i)| i as u16).collect());
        }
        // All the buckets in one array, and where each starts (the last,
        // the end).
        let mut starts = vec![0usize];
        for b in &buckets {
            starts.push(starts.last().unwrap() + b.len());
        }
        assert!(*starts.last().unwrap() <= 0xFFFF);
        let flat: Vec<u16> = buckets.concat();
        writeln!(
            out,
            "pub(crate) static {table}_BUCKETS: [u16; {}] = {flat:?};",
            flat.len()
        )
        .unwrap();
        writeln!(
            out,
            "pub(crate) static {table}_STARTS: [u16; {}] = {starts:?};",
            starts.len()
        )
        .unwrap();
        writeln!(
            out,
            "pub(crate) const {table}_KEY: (u32, u32) = ({lo}, {:#x});",
            (1u32 << (hi - lo + 1)) - 1
        )
        .unwrap();
    }
    writeln!(out, "pub(crate) static TEXT: &str = {text:?};").unwrap();
    for (name, ty, list) in [
        ("FEATURES", "&str", &features),
        ("ALIASES", "&[&str]", &alias_lists),
        ("OPERANDS", "&[(&str, &[(u8, u8)])]", &operand_lists),
        ("FIELDS", "&[(&str, u8, u8)]", &field_lists),
    ] {
        assert!(list.len() < 0xFFFF);
        writeln!(
            out,
            "pub(crate) static {name}: [{ty}; {}] = [{}];",
            list.len(),
            list.join(", ")
        )
        .unwrap();
    }
    assert!(conds.len() < 0xFFFF);
    let names: Vec<String> = (0..conds.len()).map(|n| format!("cond_{n}")).collect();
    writeln!(
        out,
        "pub(crate) static CONDS: [fn(u32) -> bool; {}] = [{}];",
        conds.len(),
        names.join(", ")
    )
    .unwrap();
    for (n, code) in conds.iter().enumerate() {
        writeln!(out, "#[allow(unused_parens, clippy::nonminimal_bool, clippy::precedence)]\nfn cond_{n}(w: u32) -> bool {{ {code} }}").unwrap();
    }
    // An index of the rules by a hash of their IDs (open addressing,
    // twice as many slots as rules): `rule` finds one in a step or two
    // rather than a binary search of string comparisons.
    let slots = (rules.len() * 2).next_power_of_two().max(16);
    let mut index = vec![u16::MAX; slots];
    for (i, r) in rules.iter().enumerate() {
        let mut s = rule_hash(&r.id) as usize & (slots - 1);
        while index[s] != u16::MAX {
            s = (s + 1) & (slots - 1);
        }
        index[s] = u16::try_from(i).unwrap();
    }
    writeln!(
        out,
        "pub(crate) static RULE_INDEX: [u16; {slots}] = {index:?};"
    )
    .unwrap();
    writeln!(out, "pub(crate) static RULES: [Rule; {}] = [", rules.len()).unwrap();
    for r in &rules {
        let alts: Vec<String> = r
            .alts
            .iter()
            .map(|(n, t)| format!("({n:?}, {t:?})"))
            .collect();
        writeln!(
            out,
            "    Rule {{ id: {:?}, display: {:?}, text: {:?}, alts: &[{}] }},",
            r.id,
            r.display,
            r.text,
            alts.join(", ")
        )
        .unwrap();
    }
    writeln!(out, "];").unwrap();
    std::fs::write(dest, out).unwrap();
}

/// The hash `rule` indexes rule IDs by (FNV-1a; src/lib.rs's `rule_hash`).
fn rule_hash(id: &str) -> u32 {
    id.bytes().fold(0x811c_9dc5u32, |h, b| {
        (h ^ u32::from(b)).wrapping_mul(0x0100_0193)
    })
}
