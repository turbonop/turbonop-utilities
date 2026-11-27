//! Every module of the bitcode fixtures in testdata/bitcode (see
//! tools/bitcode/build.cmd) against llvm-dis's IR of it (`<file>.ll`,
//! written by tools/bitcode/oracle.py): the triple, data layout, and
//! source file; every global, function, alias, ifunc, and comdat, with
//! its linkage, visibility, and other properties; the named structures'
//! bodies; the function attribute sets; and for every function body, each
//! instruction's opcode in order, the basic blocks, the debug records, the
//! functions it calls, and the globals it refers to; the debug
//! information's compile units, files, and subprograms, and which
//! subprogram each function has. Then the function blocks' sizes against
//! llvm-bcanalyzer's dump, and the symbol tables against llvm-lto2's
//! `dump-symtab`.

use turbonop_global_alloc as _;

use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;

use turbonop_bitcode::symtab::flags;
use turbonop_bitcode::types::llvm_name;
use turbonop_bitcode::{
    AliasKind, Attribute, Bitcode, Callee, Linkage, Module, Opcode, Visibility,
};

fn testdata(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../../testdata/bitcode")
        .join(name)
}

fn read(name: &str) -> Vec<u8> {
    std::fs::read(testdata(name)).unwrap_or_else(|e| panic!("{name}: {e}"))
}

const FIXTURES: &[&str] = &[
    "kinds.bc",
    "shapes-linux.bc",
    "shapes-windows.bc",
    "kinds-thin.bc",
    "every.bc",
    "embed-darwin.bc",
];

/// A string literal's contents, `\XX` escapes undone.
fn unescape(s: &str) -> String {
    let b = s.as_bytes();
    let mut out = Vec::new();
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'\\' && b.get(i + 1) == Some(&b'\\') {
            out.push(b'\\');
            i += 2;
            continue;
        }
        if b[i] == b'\\'
            && let Some(v) = s
                .get(i + 1..i + 3)
                .and_then(|h| u8::from_str_radix(h, 16).ok())
        {
            out.push(v);
            i += 3;
            continue;
        }
        out.push(b[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// The value of `key: "..."` in a metadata line.
fn field_str(line: &str, key: &str) -> Option<String> {
    let at = line.find(&format!("{key}: \""))? + key.len() + 3;
    let end = at + line[at..].find('"')?;
    Some(unescape(&line[at..end]))
}

/// The value of `key: N` in a metadata line.
fn field_num(line: &str, key: &str) -> Option<u64> {
    let pat = format!(" {key}: ");
    let at = line.find(&pat)? + pat.len();
    let end = line[at..]
        .find(|c: char| !c.is_ascii_digit())
        .map_or(line.len(), |e| at + e);
    line[at..end].parse().ok()
}

/// Every `@name` in a line (as written), with where it ends.
fn globals_in(line: &str) -> Vec<(String, usize)> {
    let b = line.as_bytes();
    let mut out = Vec::new();
    let mut i = 0;
    let mut in_string = false;
    while i < b.len() {
        let c = b[i];
        if c == b'"' {
            in_string = !in_string;
        }
        if c == b'@' && !in_string {
            let start = i;
            i += 1;
            if b.get(i) == Some(&b'"') {
                i += 1;
                while i < b.len() && b[i] != b'"' {
                    i += 1;
                }
                i += 1;
            } else {
                while i < b.len() && (b[i].is_ascii_alphanumeric() || b"-$._".contains(&b[i])) {
                    i += 1;
                }
            }
            out.push((line[start..i.min(b.len())].to_string(), i));
            continue;
        }
        i += 1;
    }
    out
}

#[derive(Debug, Default)]
struct LlFunction {
    /// Each call's function attribute set (`#N`), in order.
    call_attributes: Vec<Option<String>>,
    opcodes: Vec<String>,
    blocks: usize,
    debug_records: usize,
    callees: BTreeSet<String>,
    globals: BTreeSet<String>,
    dbg: Option<String>,
}

#[derive(Debug, Default)]
struct Ll {
    triple: String,
    datalayout: String,
    source_filename: String,
    defined: BTreeMap<String, (String, LlFunction)>,
    declared: BTreeMap<String, String>,
    globals: BTreeMap<String, String>,
    aliases: BTreeMap<String, String>,
    comdats: BTreeSet<(String, String)>,
    structs: BTreeSet<(String, String)>,
    attribute_sets: BTreeMap<String, BTreeSet<String>>,
    metadata: BTreeMap<String, String>,
}

/// The attribute set (`#N`) a line refers to, if any.
fn attribute_ref(code: &str) -> Option<String> {
    let mut found = None;
    for (i, _) in code.match_indices(" #") {
        let digits: String = code[i + 2..]
            .chars()
            .take_while(char::is_ascii_digit)
            .collect();
        if !digits.is_empty() {
            found = Some(format!("#{digits}"));
        }
    }
    found
}

/// The names in an attribute set's text (`noinline "key"="value"
/// memory(none)`): keywords, and string attributes' keys.
fn attribute_names(text: &str) -> BTreeSet<String> {
    let b = text.as_bytes();
    let mut out = BTreeSet::new();
    let mut i = 0;
    while i < b.len() {
        match b[i] {
            b' ' => i += 1,
            b'"' => {
                let end = i + 1 + text[i + 1..].find('"').unwrap();
                out.insert(unescape(&text[i + 1..end]));
                i = end + 1;
                if b.get(i) == Some(&b'=') {
                    let vend = i + 2 + text[i + 2..].find('"').unwrap();
                    i = vend + 1;
                }
            }
            _ => {
                let start = i;
                while i < b.len() && b[i] != b' ' && b[i] != b'(' {
                    i += 1;
                }
                out.insert(text[start..i].to_string());
                if b.get(i) == Some(&b'(') {
                    let mut depth = 0;
                    while i < b.len() {
                        if b[i] == b'(' {
                            depth += 1;
                        } else if b[i] == b')' {
                            depth -= 1;
                            if depth == 0 {
                                i += 1;
                                break;
                            }
                        }
                        i += 1;
                    }
                }
            }
        }
    }
    out
}

fn parse_ll(text: &str) -> Ll {
    let mut ll = Ll::default();
    let mut current: Option<(String, String, LlFunction, bool)> = None;
    for line in text.lines() {
        if let Some((name, header, f, seen_line)) = current.as_mut() {
            if line == "}" {
                let (name, header, mut f, seen_line) = current.take().unwrap();
                if !seen_line {
                    f.blocks += 1;
                }
                ll.defined.insert(name, (header, f));
                continue;
            }
            let _ = (name, header);
            if line.trim().is_empty() {
                continue;
            }
            if line.starts_with("    #dbg_") {
                f.debug_records += 1;
                continue;
            }
            if !line.starts_with(' ') {
                // A label.
                if !*seen_line {
                    // The entry block is labeled: it isn't an extra one.
                    *seen_line = true;
                }
                f.blocks += 1;
                continue;
            }
            if !line.starts_with("  ")
                || line.as_bytes()[2] == b' '
                || line.trim_start().starts_with(']')
            {
                // A continuation of the instruction before.
                for (g, _) in globals_in(line) {
                    f.globals.insert(g);
                }
                continue;
            }
            if !*seen_line {
                // An unlabeled entry block.
                *seen_line = true;
                f.blocks += 1;
            }
            let mut rest = line.trim_start();
            if rest.starts_with('%') {
                rest = &rest[rest.find(" = ").unwrap() + 3..];
            }
            let mut words = rest.split_whitespace();
            let mut op = words.next().unwrap();
            if matches!(op, "tail" | "musttail" | "notail") {
                op = words.next().unwrap();
            }
            f.opcodes.push(op.trim_end_matches(',').to_string());
            // The code, without metadata attachments.
            let code = line.split(", !").next().unwrap();
            let gs = globals_in(code);
            if matches!(op, "call" | "invoke" | "callbr") {
                if let Some((g, _)) = gs
                    .iter()
                    .find(|(_, end)| code.as_bytes().get(*end) == Some(&b'('))
                {
                    f.callees.insert(g.clone());
                }
                f.call_attributes.push(attribute_ref(code));
            }
            for (g, _) in gs {
                f.globals.insert(g);
            }
            continue;
        }
        if let Some(rest) = line.strip_prefix("define ") {
            let (name, _) = globals_in(rest)
                .into_iter()
                .find(|(_, end)| rest.as_bytes().get(*end) == Some(&b'('))
                .unwrap();
            let f = LlFunction {
                dbg: line
                    .rsplit_once("!dbg ")
                    .map(|(_, d)| d.trim_end_matches(" {").to_string()),
                ..LlFunction::default()
            };
            current = Some((name, rest.to_string(), f, false));
        } else if let Some(rest) = line.strip_prefix("declare ") {
            let (name, _) = globals_in(rest)
                .into_iter()
                .find(|(_, end)| rest.as_bytes().get(*end) == Some(&b'('))
                .unwrap();
            ll.declared.insert(name, rest.to_string());
        } else if let Some(v) = line.strip_prefix("target triple = \"") {
            ll.triple = unescape(v.trim_end_matches('"'));
        } else if let Some(v) = line.strip_prefix("target datalayout = \"") {
            ll.datalayout = unescape(v.trim_end_matches('"'));
        } else if let Some(v) = line.strip_prefix("source_filename = \"") {
            ll.source_filename = unescape(v.trim_end_matches('"'));
        } else if line.starts_with('$') {
            let (name, kind) = line.split_once(" = comdat ").unwrap();
            ll.comdats.insert((name.to_string(), kind.to_string()));
        } else if line.starts_with('@') {
            let (name, rest) = line.split_once(" = ").unwrap();
            let words: Vec<&str> = rest.split_whitespace().collect();
            if words.iter().any(|w| *w == "alias" || *w == "ifunc")
                && !words.iter().any(|w| *w == "global" || *w == "constant")
            {
                ll.aliases.insert(name.to_string(), rest.to_string());
            } else {
                ll.globals.insert(name.to_string(), rest.to_string());
            }
        } else if line.starts_with('%') && line.contains(" = type ") {
            let (name, body) = line.split_once(" = type ").unwrap();
            ll.structs.insert((name.to_string(), body.to_string()));
        } else if let Some(rest) = line.strip_prefix("attributes #") {
            let (id, body) = rest.split_once(" = { ").unwrap();
            ll.attribute_sets.insert(
                format!("#{id}"),
                attribute_names(body.trim_end_matches(" }")),
            );
        } else if line.starts_with('!') && line.as_bytes().get(1).is_some_and(u8::is_ascii_digit) {
            let (id, rest) = line.split_once(" = ").unwrap();
            ll.metadata.insert(id.to_string(), rest.to_string());
        }
    }
    ll
}

const LINKAGES: &[&str] = &[
    "private",
    "internal",
    "available_externally",
    "linkonce",
    "weak",
    "common",
    "appending",
    "extern_weak",
    "linkonce_odr",
    "weak_odr",
    "external",
];

/// The linkage keyword a line's words start with ("" for none).
fn ll_linkage(words: &str) -> String {
    let first = words.split_whitespace().next().unwrap_or("");
    if LINKAGES.contains(&first) {
        first.to_string()
    } else {
        String::new()
    }
}

fn linkage_of(l: Linkage, declaration: bool) -> String {
    if l == Linkage::External && declaration {
        // llvm-dis writes declarations' linkage for globals.
        return String::new();
    }
    l.name().to_string()
}

fn name(prefix: char, n: &[u8]) -> String {
    llvm_name(prefix, n)
}

fn check_module(file: &str, m: &Module<'_>, ll: &Ll) {
    let s = |b: &[u8]| String::from_utf8_lossy(b).into_owned();
    assert_eq!(s(&m.triple), ll.triple, "{file}: triple");
    assert_eq!(s(&m.datalayout), ll.datalayout, "{file}: datalayout");
    assert_eq!(
        s(&m.source_filename),
        ll.source_filename,
        "{file}: source_filename"
    );
    assert!(m.errors.is_empty(), "{file}: {:?}", m.errors);

    // Comdats.
    let comdats: BTreeSet<(String, String)> = m
        .comdats
        .iter()
        .map(|c| (name('$', &c.name), c.selection_name().to_string()))
        .collect();
    assert_eq!(comdats, ll.comdats, "{file}: comdats");

    // Global variables.
    let globals: BTreeSet<String> = m
        .globals
        .iter()
        .map(|g| m.ir_name(g.value).unwrap())
        .collect();
    assert_eq!(
        globals,
        ll.globals.keys().cloned().collect(),
        "{file}: globals"
    );
    for g in &m.globals {
        let n = m.ir_name(g.value).unwrap();
        let text = &ll.globals[&n];
        let mut want = ll_linkage(text);
        if want == "external" {
            want = String::new();
        }
        assert_eq!(
            linkage_of(g.linkage, g.is_declaration()),
            want,
            "{file}: {n} linkage"
        );
        let words: Vec<&str> = text.split_whitespace().collect();
        let kind = words
            .iter()
            .find(|w| **w == "global" || **w == "constant")
            .unwrap();
        assert_eq!(g.constant, *kind == "constant", "{file}: {n} constant");
        assert_eq!(
            g.visibility == Visibility::Hidden,
            words.contains(&"hidden"),
            "{file}: {n} hidden"
        );
        assert_eq!(
            g.visibility == Visibility::Protected,
            words.contains(&"protected"),
            "{file}: {n} protected"
        );
        assert_eq!(
            g.thread_local != turbonop_bitcode::ThreadLocal::None,
            words.iter().any(|w| w.starts_with("thread_local")),
            "{file}: {n} thread_local"
        );
        let section = text
            .split_once(", section \"")
            .map(|(_, r)| unescape(r.split('"').next().unwrap()));
        assert_eq!(g.section.as_deref().map(s), section, "{file}: {n} section");
        let align = text.split_once(", align ").map(|(_, r)| {
            r.split(|c: char| !c.is_ascii_digit())
                .next()
                .unwrap()
                .parse::<u64>()
                .unwrap()
        });
        assert_eq!(g.alignment, align, "{file}: {n} align");
        assert_eq!(
            g.comdat.is_some(),
            text.contains("comdat"),
            "{file}: {n} comdat"
        );
        assert_eq!(
            g.initializer.is_none(),
            words.contains(&"external")
                || words.contains(&"extern_weak")
                || ll_linkage(text) == "external",
            "{file}: {n} declaration"
        );
    }

    // Aliases and ifuncs.
    let aliases: BTreeSet<String> = m
        .aliases
        .iter()
        .map(|a| m.ir_name(a.value).unwrap())
        .collect();
    assert_eq!(
        aliases,
        ll.aliases.keys().cloned().collect(),
        "{file}: aliases"
    );
    for a in &m.aliases {
        let n = m.ir_name(a.value).unwrap();
        let text = &ll.aliases[&n];
        assert_eq!(
            linkage_of(a.linkage, false),
            ll_linkage(text),
            "{file}: {n} linkage"
        );
        let kind = if a.kind == AliasKind::Alias {
            "alias"
        } else {
            "ifunc"
        };
        assert!(
            text.split_whitespace().any(|w| w == kind),
            "{file}: {n} {kind}"
        );
        // The target is named in the text.
        let target = m.ir_name(a.target);
        if let Some(t) = target {
            assert!(text.contains(&t), "{file}: {n} -> {t}: {text}");
        }
    }

    // Functions.
    let declared: BTreeSet<String> = m
        .functions
        .iter()
        .filter(|f| f.is_declaration)
        .map(|f| m.ir_name(f.value).unwrap())
        .collect();
    assert_eq!(
        declared,
        ll.declared.keys().cloned().collect(),
        "{file}: declared functions"
    );
    let defined: BTreeSet<String> = m
        .functions
        .iter()
        .filter(|f| !f.is_declaration)
        .map(|f| m.ir_name(f.value).unwrap())
        .collect();
    assert_eq!(
        defined,
        ll.defined.keys().cloned().collect(),
        "{file}: defined functions"
    );
    for f in &m.functions {
        let n = m.ir_name(f.value).unwrap();
        let header = if f.is_declaration {
            &ll.declared[&n]
        } else {
            &ll.defined[&n].0
        };
        assert_eq!(
            linkage_of(f.linkage, f.is_declaration),
            ll_linkage(header),
            "{file}: {n} linkage"
        );
        let words: Vec<&str> = header.split_whitespace().collect();
        assert_eq!(
            f.visibility == Visibility::Hidden,
            words.contains(&"hidden"),
            "{file}: {n} hidden"
        );
        assert_eq!(
            f.visibility == Visibility::Protected,
            words.contains(&"protected"),
            "{file}: {n} protected"
        );
        let section = header
            .split_once(" section \"")
            .map(|(_, r)| unescape(r.split('"').next().unwrap()));
        assert_eq!(f.section.as_deref().map(s), section, "{file}: {n} section");
        assert_eq!(f.body.is_some(), !f.is_declaration, "{file}: {n} body");
    }

    // Named structures.
    let structs: BTreeSet<(String, String)> = (0..m.types.declared)
        .filter_map(|i| {
            let i = u32::try_from(i).unwrap();
            let body = m.types.body(i)?;
            Some((m.types.name(i), body))
        })
        .collect();
    assert_eq!(structs, ll.structs, "{file}: structures");

    // Function attribute sets: each function's, and each call's. (Not
    // intrinsics': LLVM's reader replaces them with the intrinsic's own.)
    let fn_set = |list: u64| -> BTreeSet<String> {
        m.attribute_list(list)
            .iter()
            .filter(|g| g.index == 0xFFFF_FFFF)
            .flat_map(|g| g.attributes.iter().map(|a| a.name().into_owned()))
            .collect()
    };
    let ll_set = |r: Option<String>| -> BTreeSet<String> {
        r.map(|r| ll.attribute_sets[&r].clone()).unwrap_or_default()
    };
    for f in &m.functions {
        let n = m.ir_name(f.value).unwrap();
        if n.starts_with("@llvm.") {
            continue;
        }
        let header = if f.is_declaration {
            &ll.declared[&n]
        } else {
            &ll.defined[&n].0
        };
        let header = header.split(" !dbg").next().unwrap();
        assert_eq!(
            fn_set(f.attributes),
            ll_set(attribute_ref(header)),
            "{file}: {n} attributes"
        );
    }
    for g in &m.attribute_groups {
        for a in &g.attributes {
            assert!(
                !a.name().starts_with('<'),
                "{file}: unknown attribute {a:?}"
            );
            if let Attribute::String(k, _) = a {
                assert!(!k.is_empty());
            }
        }
    }

    // Bodies.
    let mut instructions = 0;
    for (f, b) in m.defined() {
        let n = m.ir_name(f.value).unwrap();
        let (_, want) = &ll.defined[&n];
        assert!(b.error.is_none(), "{file}: {n}: {:?}", b.error);
        assert_eq!(b.unknown_records, 0, "{file}: {n}: unknown records");
        let ops: Vec<&str> = b.instructions.iter().map(|i| i.opcode.name()).collect();
        assert_eq!(ops, want.opcodes, "{file}: {n} instructions");
        assert_eq!(
            usize::try_from(b.blocks).unwrap(),
            want.blocks,
            "{file}: {n} blocks"
        );
        // Each block ends with its terminator.
        let terminators = b
            .instructions
            .iter()
            .filter(|i| i.opcode.is_terminator())
            .count();
        assert_eq!(terminators, want.blocks, "{file}: {n} terminators");
        assert_eq!(
            b.debug_records as usize, want.debug_records,
            "{file}: {n} debug records"
        );
        let callees: BTreeSet<String> = b
            .calls
            .iter()
            .filter_map(|c| match c.callee {
                Callee::Function(i) => m.ir_name(m.functions[i].value),
                Callee::Alias(i) => m.ir_name(m.aliases[i].value),
                Callee::Global(i) => m.ir_name(m.globals[i].value),
                Callee::InlineAsm | Callee::Indirect => None,
            })
            .collect();
        assert_eq!(callees, want.callees, "{file}: {n} callees");
        assert_eq!(
            b.calls.len(),
            want.call_attributes.len(),
            "{file}: {n} calls"
        );
        for (c, r) in b.calls.iter().zip(&want.call_attributes) {
            assert_eq!(
                fn_set(c.attributes),
                ll_set(r.clone()),
                "{file}: {n} call {} attributes",
                c.instruction
            );
        }
        let globals: BTreeSet<String> = b
            .referenced_globals
            .iter()
            .map(|&v| m.ir_name(v).unwrap())
            .collect();
        assert_eq!(globals, want.globals, "{file}: {n} referenced globals");
        // Every call's result: a value unless the callee returns void.
        for i in &b.instructions {
            if i.opcode.is_call() {
                assert!(i.ty.is_some(), "{file}: {n}: a call without a type");
            }
            if i.opcode == Opcode::Store || i.opcode == Opcode::Br {
                assert!(i.value.is_none());
            }
        }
        // The subprogram.
        let ours = b.subprogram(m).map(|sp| s(sp.name));
        let theirs = want
            .dbg
            .as_ref()
            .map(|d| field_str(&ll.metadata[d], "name").unwrap_or_default());
        assert_eq!(ours, theirs, "{file}: {n} subprogram");
        instructions += b.instructions.len();
    }
    let want: usize = ll.defined.values().map(|(_, f)| f.opcodes.len()).sum();
    assert_eq!(instructions, want, "{file}: instructions");

    // Debug information.
    let units: BTreeSet<(String, String)> = m
        .metadata
        .compile_units()
        .iter()
        .map(|u| {
            (
                s(u.producer),
                u.file.as_ref().map(|f| s(f.filename)).unwrap_or_default(),
            )
        })
        .collect();
    let want: BTreeSet<(String, String)> = ll
        .metadata
        .values()
        .filter(|l| l.contains("!DICompileUnit("))
        .map(|l| {
            let file = l.split_once("file: ").unwrap().1.split(',').next().unwrap();
            let file = field_str(&ll.metadata[file], "filename").unwrap();
            (field_str(l, "producer").unwrap(), file)
        })
        .collect();
    assert_eq!(units, want, "{file}: compile units");
    let mut subprograms: Vec<(String, String, u64, bool)> = std::iter::once(m.metadata.view())
        .chain(m.bodies.iter().map(|b| b.metadata(m)))
        .enumerate()
        .flat_map(|(i, v)| {
            // A function's view repeats the module's: take its own.
            let first = if i == 0 { 0 } else { m.metadata.nodes.len() };
            v.subprograms()
                .into_iter()
                .filter(move |sp| sp.id as usize >= first)
        })
        .collect::<Vec<_>>()
        .iter()
        .map(|sp| (s(sp.name), s(sp.linkage_name), sp.line, sp.definition))
        .collect();
    subprograms.sort();
    let mut want: Vec<(String, String, u64, bool)> = ll
        .metadata
        .values()
        .filter(|l| l.contains("!DISubprogram("))
        .map(|l| {
            (
                field_str(l, "name").unwrap_or_default(),
                field_str(l, "linkageName").unwrap_or_default(),
                field_num(l, "line").unwrap_or(0),
                l.contains("DISPFlagDefinition"),
            )
        })
        .collect();
    want.sort();
    assert_eq!(subprograms, want, "{file}: subprograms");
    let files: BTreeSet<(String, String)> = std::iter::once(m.metadata.view())
        .chain(m.bodies.iter().map(|b| b.metadata(m)))
        .flat_map(|v| v.files())
        .map(|f| (s(f.filename), s(f.directory)))
        .collect();
    let want: BTreeSet<(String, String)> = ll
        .metadata
        .values()
        .filter(|l| l.starts_with("!DIFile(") || l.starts_with("distinct !DIFile("))
        .map(|l| {
            (
                field_str(l, "filename").unwrap(),
                field_str(l, "directory").unwrap_or_default(),
            )
        })
        .collect();
    assert_eq!(files, want, "{file}: files");
}

#[test]
fn modules_match_llvm_dis() {
    for file in FIXTURES {
        let data = read(file);
        let bc = Bitcode::parse(&data).unwrap();
        assert_eq!(bc.modules.len(), 1, "{file}");
        let text = String::from_utf8(read(&format!("{file}.ll"))).unwrap();
        let ll = parse_ll(&text);
        check_module(file, &bc.modules[0], &ll);
    }
}

/// `<FUNCTION_BLOCK NumWords=N` lines of a dump, at the module's level.
fn dump_function_words(dump: &str) -> Vec<u32> {
    dump.lines()
        .filter_map(|l| l.strip_prefix("  <FUNCTION_BLOCK NumWords="))
        .map(|r| r.split(' ').next().unwrap().parse().unwrap())
        .collect()
}

#[test]
fn function_blocks_match_llvm_bcanalyzer() {
    for file in FIXTURES.iter().chain(&["multi.bc"]) {
        let data = read(file);
        let bc = Bitcode::parse(&data).unwrap();
        let dump = String::from_utf8(read(&format!("{file}.dump.txt"))).unwrap();
        let want = dump_function_words(&dump);
        let mut have: Vec<(u64, u32)> = bc
            .modules
            .iter()
            .flat_map(|m| m.bodies.iter().map(|b| (b.bit_offset, b.num_words)))
            .collect();
        have.sort_unstable();
        let words: Vec<u32> = have.iter().map(|h| h.1).collect();
        assert_eq!(words, want, "{file}: function blocks");
        for m in &bc.modules {
            for b in &m.bodies {
                // ENTER_SUBBLOCK (the module's width), the block ID (VBR8),
                // the abbreviation width (VBR4), alignment, the length, and
                // the words.
                assert_eq!(b.bits % 32, 0, "{file}: ends aligned");
                let words = u64::from(b.num_words) * 32;
                assert!(
                    b.bits > words && b.bits <= words + 64 + 32,
                    "{file}: {} bits for {} words",
                    b.bits,
                    b.num_words
                );
                assert!(b.instruction_bits() < b.bits);
            }
        }
    }
}

fn symtab_text(bc: &Bitcode<'_>, file: &str) -> String {
    let st = bc.symtab.expect(file);
    let s = |b: &[u8]| String::from_utf8_lossy(b).into_owned();
    let mut out = String::new();
    out += &format!("version: {}\n", st.version().unwrap());
    if st.version() == Some(4) {
        out += &format!("producer: {}\n", s(st.producer()));
    }
    let triple = s(st.target_triple());
    out += &format!("target triple: {triple}\n");
    out += &format!("source filename: {}\n", s(st.source_filename()));
    let coff = triple.contains("windows") || triple.contains("win32");
    let macho = triple.contains("apple") || triple.contains("darwin") || triple.contains("macos");
    let wasm = triple.starts_with("wasm");
    if coff {
        out += &format!("linker opts: {}\n", s(st.coff_linker_opts()));
    } else if !macho && !wasm {
        out += "dependent libraries:";
        for l in st.dependent_libraries() {
            out += &format!(" \"{}\"", s(l));
        }
        out += "\n";
    }
    let comdats = st.comdats();
    for sym in st.symbols() {
        if !sym.has(flags::GLOBAL) || sym.has(flags::FORMAT_SPECIFIC) {
            continue;
        }
        out.push(match sym.visibility() {
            1 => 'H',
            2 => 'P',
            _ => 'D',
        });
        for (c, f) in [
            ('U', flags::UNDEFINED),
            ('C', flags::COMMON),
            ('W', flags::WEAK),
            ('I', flags::INDIRECT),
            ('O', flags::MAY_OMIT),
            ('T', flags::TLS),
            ('X', flags::EXECUTABLE),
        ] {
            out.push(if sym.has(f) { c } else { '-' });
        }
        out += &format!(" {}\n", s(sym.name));
        if sym.has(flags::COMMON) {
            out += &format!(
                "         size {} align {}\n",
                sym.common_size, sym.common_align
            );
        }
        if let Some(c) = sym.comdat {
            let c = comdats[c as usize];
            let kind =
                ["any", "exactmatch", "largest", "nodeduplicate", "samesize"][c.selection as usize];
            out += &format!("         comdat {kind} {}\n", s(c.name));
        }
        if coff && sym.has(flags::WEAK) && sym.has(flags::INDIRECT) {
            out += &format!("         fallback {}\n", s(sym.coff_weak_fallback));
        }
        if !sym.section.is_empty() {
            out += &format!("         section {}\n", s(sym.section));
        }
    }
    out += "\n";
    out
}

#[test]
fn symtabs_match_llvm_lto2() {
    for file in ["kinds-thin.bc", "every.bc", "embed-darwin.bc"] {
        let data = read(file);
        let bc = Bitcode::parse(&data).unwrap();
        let want = String::from_utf8(read(&format!("{file}.symtab.txt"))).unwrap();
        assert_eq!(symtab_text(&bc, file), want, "{file}");
    }
}
