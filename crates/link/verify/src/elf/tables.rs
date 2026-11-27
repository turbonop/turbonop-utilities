//! The ELF programs' tables, each entry by what it names: headers,
//! program headers and sections, symbols, the dynamic section, dynamic
//! symbols and relocations, the initializer arrays, notes, `.eh_frame`
//! and `.eh_frame_hdr`.

use std::collections::BTreeMap;

use turbonop_dwarf::cfi::{Cfi, CfiKind, Entry};
use turbonop_elf::consts::*;

use super::program::Program;
use crate::report::{Report, show};

/// The prefix of turbolib's anchors (absolute symbols that only folded
/// libraries' synthesized members define).
pub(crate) const ANCHOR: &[u8] = b"__turbolib.anchor.";

/// Compares two lists in order; failures under `what`.
pub(crate) fn lists(a: &[String], b: &[String], what: &str, rep: &mut Report) {
    rep.checked(what, a.len() as u64);
    if a == b {
        return;
    }
    let at = a
        .iter()
        .zip(b)
        .position(|(x, y)| x != y)
        .unwrap_or(a.len().min(b.len()));
    rep.fail(&format!("{what} that differ"), || {
        format!(
            "entry {at} of {} and {}: {} and {}",
            a.len(),
            b.len(),
            a.get(at).map_or("nothing", String::as_str),
            b.get(at).map_or("nothing", String::as_str)
        )
    });
}

/// Compares two multisets of strings; failures under `what`, with what
/// only one side has.
pub(crate) fn sets(a: &[String], b: &[String], what: &str, rep: &mut Report) {
    fn count(v: &[String]) -> BTreeMap<&str, i64> {
        let mut m: BTreeMap<&str, i64> = BTreeMap::new();
        for x in v {
            *m.entry(x.as_str()).or_default() += 1;
        }
        m
    }
    let (ma, mb) = (count(a), count(b));
    rep.checked(what, a.len() as u64);
    for (k, &n) in &ma {
        let m = mb.get(k).copied().unwrap_or(0);
        if n > m {
            rep.fail(&format!("{what} only in the first program"), || {
                (*k).to_string()
            });
        }
    }
    for (k, &n) in &mb {
        let m = ma.get(k).copied().unwrap_or(0);
        if n > m {
            rep.fail(&format!("{what} only in the second program"), || {
                (*k).to_string()
            });
        }
    }
}

pub(crate) fn headers(a: &Program<'_>, b: &Program<'_>, rep: &mut Report) {
    let (ha, hb) = (a.file.header(), b.file.header());
    let fields = |p: &Program<'_>| {
        let h = p.file.header();
        vec![
            format!("type {}", h.kind),
            format!("machine {}", h.machine),
            format!("flags {:#x}", h.flags),
            format!("OS ABI {}", h.os_abi),
            format!("class {:?}", h.class),
            format!("entry {}", p.render(h.entry)),
        ]
    };
    let _ = (ha, hb);
    lists(&fields(a), &fields(b), "header fields", rep);
    let phdrs = |p: &Program<'_>| -> Vec<String> {
        p.file
            .segments()
            .map(|s| format!("type {:#x} flags {:#x}", s.kind, s.flags))
            .collect()
    };
    lists(&phdrs(a), &phdrs(b), "program headers", rep);
    let sizes = |p: &Program<'_>| -> Vec<(u64, u64)> {
        p.file.segments().map(|s| (s.filesz, s.memsz)).collect()
    };
    if sizes(a) != sizes(b) {
        rep.expect("segments of other sizes", String::new);
    }
    // Debug sections apart: their contents are compared as DWARF, and a
    // folded program has some the original keeps in `.dwo` files (the
    // synthesized members' DWARF is unsplit).
    let debug =
        |s: &&turbonop_elf::Section<'_>| s.name.starts_with(b".debug_") && s.flags & SHF_ALLOC == 0;
    let sections = |p: &Program<'_>| -> Vec<String> {
        p.sections
            .iter()
            .skip(1)
            .filter(|s| !debug(s))
            .map(|s| format!("{} type {:#x} flags {:#x}", show(s.name), s.kind, s.flags))
            .collect()
    };
    lists(&sections(a), &sections(b), "sections", rep);
    let debug_names = |p: &Program<'_>| -> std::collections::BTreeSet<String> {
        p.sections
            .iter()
            .filter(debug)
            .map(|s| show(s.name))
            .collect()
    };
    let (da, db) = (debug_names(a), debug_names(b));
    for n in da.difference(&db) {
        rep.expect("debug sections only in the first program", || n.clone());
    }
    for n in db.difference(&da) {
        rep.expect("debug sections only in the second program", || n.clone());
    }
    for (x, y) in a.sections.iter().zip(&b.sections) {
        if x.name == y.name && x.size != y.size {
            rep.expect("sections of another size", || {
                format!("{}: {:#x} and {:#x}", show(x.name), x.size, y.size)
            });
        }
    }
}

fn is_anchor(name: &[u8]) -> bool {
    name.starts_with(ANCHOR)
}

/// `.symtab`, as a set of each symbol's key, type, binding, visibility,
/// size, and section name; anchors only in the second are expected.
pub(crate) fn symbols(a: &Program<'_>, b: &Program<'_>, rep: &mut Report) {
    let desc = |p: &Program<'_>| -> BTreeMap<Vec<u8>, Vec<String>> {
        let mut m: BTreeMap<Vec<u8>, Vec<String>> = BTreeMap::new();
        for (s, k) in p.symbols.iter().zip(&p.keys) {
            let Some(k) = k else { continue };
            // mold writes an absolute local for each input's COMDAT group
            // signature, discarded copies' too: compared by name, as a set.
            if s.bind == STB_LOCAL && s.shndx == u32::from(SHN_ABS) && s.value == 0 && s.size == 0 {
                let e = m.entry(s.name.to_vec()).or_default();
                let d = "local absolute 0".to_string();
                if !e.contains(&d) {
                    e.push(d);
                }
                continue;
            }
            let section = match s.shndx {
                0 => "undefined".to_string(),
                x if x == u32::from(SHN_ABS) => "absolute".to_string(),
                x => p
                    .sections
                    .get(x as usize)
                    .map_or_else(|| format!("{x:#x}"), |s| show(s.name)),
            };
            m.entry(k.to_vec()).or_default().push(format!(
                "type {} binding {} visibility {} size {:#x} in {section}",
                s.kind, s.bind, s.visibility, s.size
            ));
        }
        m
    };
    let (da, db) = (desc(a), desc(b));
    rep.checked("symbols", da.len() as u64);
    for (k, v) in &da {
        match db.get(k) {
            None => rep.fail("symbols only in the first program", || show(k)),
            Some(w) if w != v => rep.fail("symbols that differ", || {
                format!("{}: {} and {}", show(k), v.join(", "), w.join(", "))
            }),
            Some(_) => {}
        }
    }
    for k in db.keys() {
        if !da.contains_key(k) {
            if is_anchor(k) {
                rep.expect("turbolib's anchors, only in the second program", || show(k));
            } else {
                rep.fail("symbols only in the second program", || show(k));
            }
        }
    }
}

/// The dynamic section, dynamic symbols, and dynamic relocations.
pub(crate) fn dynamic(a: &Program<'_>, b: &Program<'_>, rep: &mut Report) {
    let entries = |p: &Program<'_>| -> Vec<String> {
        let dynstr = p
            .sections
            .iter()
            .find(|s| s.kind == SHT_DYNAMIC)
            .and_then(|d| p.sections.get(d.link as usize))
            .map_or(&[][..], |s| p.file.section_data(s));
        let string = |off: u64| -> String {
            let rest = usize::try_from(off)
                .ok()
                .and_then(|o| dynstr.get(o..))
                .unwrap_or(&[]);
            let n = rest.iter().position(|&c| c == 0).unwrap_or(rest.len());
            show(&rest[..n])
        };
        p.file
            .dynamic()
            .iter()
            .map(|d| match d.tag {
                DT_NEEDED | DT_SONAME | DT_RPATH | DT_RUNPATH => {
                    format!("tag {:#x} {}", d.tag, string(d.value))
                }
                DT_FLAGS | DT_FLAGS_1 | DT_PLTREL | DT_BIND_NOW | DT_TEXTREL | DT_SYMBOLIC => {
                    format!("tag {:#x} {:#x}", d.tag, d.value)
                }
                DT_INIT | DT_FINI => format!("tag {:#x} {}", d.tag, p.render(d.value)),
                t => format!("tag {t:#x}"),
            })
            .collect()
    };
    lists(&entries(a), &entries(b), "dynamic section entries", rep);

    let dynsyms = |p: &Program<'_>| -> Vec<String> {
        let Some(t) = p.sections.iter().find(|s| s.kind == SHT_DYNSYM) else {
            return Vec::new();
        };
        let versions = p.file.symbol_versions();
        p.file
            .symbols(t)
            .iter()
            .enumerate()
            .skip(1)
            .map(|(i, s)| {
                let v = versions
                    .get(i)
                    .copied()
                    .flatten()
                    .map_or(String::new(), |v| {
                        format!(
                            "@{}{}{}",
                            if v.hidden { "" } else { "@" },
                            show(v.name),
                            v.file.map_or(String::new(), |f| format!(" ({})", show(f)))
                        )
                    });
                let at = if s.shndx == 0 {
                    "undefined".to_string()
                } else if s.kind == STT_TLS {
                    format!("tls:{}", p.render_tls(s.value))
                } else {
                    p.render(s.value)
                };
                format!(
                    "{}{v} type {} binding {} visibility {} size {:#x} at {at}",
                    show(s.name),
                    s.kind,
                    s.bind,
                    s.visibility,
                    s.size
                )
            })
            .collect()
    };
    sets(&dynsyms(a), &dynsyms(b), "dynamic symbols", rep);

    let relocs = |p: &Program<'_>| -> Vec<String> {
        p.dynrel
            .iter()
            .map(|r| {
                format!(
                    "{} at {}: {}",
                    if r.relr {
                        "RELR".to_string()
                    } else {
                        format!("type {}", r.kind)
                    },
                    p.render(r.place),
                    p.target(r)
                )
            })
            .collect()
    };
    sets(&relocs(a), &relocs(b), "dynamic relocations", rep);
}

/// `.init_array`, `.fini_array`, `.preinit_array` (and `.ctors`,
/// `.dtors`), entry by entry, in order.
pub(crate) fn initializers(a: &Program<'_>, b: &Program<'_>, rep: &mut Report) {
    let table = |p: &Program<'_>, name: &[u8]| -> Vec<String> {
        let Some(s) = p.sections.iter().find(|s| s.name == name) else {
            return Vec::new();
        };
        let w = p.word();
        let mut out = Vec::new();
        let mut at = s.addr;
        while at + u64::from(w) <= s.addr + s.size {
            let rel = p.dynrel_in(at, u64::from(w)).first();
            out.push(match rel {
                Some(r) => p.target(r),
                None => p.render(p.read_word(at, w).unwrap_or(0)),
            });
            at += u64::from(w);
        }
        out
    };
    for name in [
        &b".preinit_array"[..],
        b".init_array",
        b".fini_array",
        b".ctors",
        b".dtors",
    ] {
        lists(
            &table(a, name),
            &table(b, name),
            &format!("{} entries", show(name)),
            rep,
        );
    }
}

/// The notes of note sections; build IDs may differ (expected).
pub(crate) fn notes(a: &Program<'_>, b: &Program<'_>, rep: &mut Report) {
    let notes = |p: &Program<'_>| -> (Vec<String>, Vec<String>) {
        let mut out = Vec::new();
        let mut ids = Vec::new();
        for s in p.sections.iter().filter(|s| s.kind == SHT_NOTE) {
            for n in p.file.notes(p.file.section_data(s)) {
                let text = format!(
                    "{}: {} type {:#x}: {}",
                    show(s.name),
                    show(n.name),
                    n.kind,
                    n.desc
                        .iter()
                        .map(|b| format!("{b:02x}"))
                        .collect::<String>()
                );
                if n.name == b"GNU" && n.kind == 3 {
                    ids.push(text);
                } else {
                    out.push(text);
                }
            }
        }
        (out, ids)
    };
    let ((na, ia), (nb, ib)) = (notes(a), notes(b));
    sets(&na, &nb, "notes", rep);
    if ia != ib {
        rep.expect("build IDs that differ", String::new);
    }
}

/// One FDE, as text: what it describes and its rules.
fn fde_text(p: &Program<'_>, cfi: &Cfi<'_>, e: &turbonop_dwarf::cfi::Fde<'_>) -> (String, String) {
    // Instructions without the trailing DW_CFA_nops that pad the entry
    // (its padding follows where the linker put it).
    let hex = |b: &[u8]| {
        let n = b.iter().rposition(|&x| x != 0).map_or(0, |k| k + 1);
        b[..n]
            .iter()
            .map(|x| format!("{x:02x}"))
            .collect::<String>()
    };
    let cie = match cfi.entry_at(e.cie as usize) {
        Some((Entry::Cie(c), _)) => format!(
            "CIE {} code {} data {} ra {} personality {} signal {} instructions {}",
            show(c.augmentation),
            c.code_alignment,
            c.data_alignment,
            c.return_address_register,
            c.personality.map_or("none".into(), |x| p.render(x)),
            c.signal_frame,
            hex(c.instructions)
        ),
        _ => "no CIE".into(),
    };
    (
        p.render(e.pc_begin),
        format!(
            "range {:#x} LSDA {} instructions {} {cie}",
            e.pc_range,
            e.lsda.map_or("none".into(), |x| lsda_text(p, x, e.pc_begin)
                .unwrap_or_else(|| format!("{} (not read)", p.render(x)))),
            hex(e.instructions)
        ),
    )
}

/// `.eh_frame`'s FDEs by the function each describes, and
/// `.eh_frame_hdr`'s table as a set.
pub(crate) fn unwind(a: &Program<'_>, b: &Program<'_>, rep: &mut Report) {
    let fdes = |p: &Program<'_>| -> BTreeMap<String, Vec<String>> {
        let mut m: BTreeMap<String, Vec<String>> = BTreeMap::new();
        let Some(s) = p.sections.iter().find(|s| s.name == b".eh_frame") else {
            return m;
        };
        let cfi = Cfi {
            data: p.file.section_data(s),
            kind: CfiKind::EhFrame,
            endian: p.file.endian(),
            address: s.addr,
            address_size: p.word(),
            text_base: None,
            data_base: None,
        };
        for e in cfi.entries() {
            if let Entry::Fde(f) = e {
                let (k, v) = fde_text(p, &cfi, &f);
                m.entry(k).or_default().push(v);
            }
        }
        m
    };
    let (fa, fb) = (fdes(a), fdes(b));
    rep.checked("FDEs", fa.values().map(|v| v.len() as u64).sum());
    for (k, v) in &fa {
        match fb.get(k) {
            None => rep.fail("FDEs only in the first program", || k.clone()),
            Some(w) if w != v => rep.fail("FDEs that differ", || {
                format!("{k}: {} and {}", v.join("; "), w.join("; "))
            }),
            _ => {}
        }
    }
    for k in fb.keys() {
        if !fa.contains_key(k) {
            rep.fail("FDEs only in the second program", || k.clone());
        }
    }
    let hdr = |p: &Program<'_>| -> Vec<String> {
        let Some(s) = p.sections.iter().find(|s| s.name == b".eh_frame_hdr") else {
            return Vec::new();
        };
        p.file
            .eh_frame_hdr(s)
            .iter()
            .map(|e| p.render(e.pc))
            .collect()
    };
    sets(&hdr(a), &hdr(b), ".eh_frame_hdr entries", rep);
}

/// Reads a pointer of `.eh_frame` encoding `enc` at `at` of `data` (which
/// lies at `base`): its value, `None` for `DW_EH_PE_omit` or what can't be
/// read. Relative encodings other than `pcrel` are read as their offsets.
fn encoded(data: &[u8], at: &mut usize, enc: u8, base: u64, word: u8) -> Option<u64> {
    if enc == 0xFF {
        return None;
    }
    let start = *at;
    let mut r = turbonop_dwarf::Reader::new(data, start, turbonop_core::Endian::Little);
    let v = match enc & 0x0F {
        0x00 => r.uint(usize::from(word))?,
        0x01 => r.uleb()?,
        0x02 => r.uint(2)?,
        0x03 => r.uint(4)?,
        0x04 => r.uint(8)?,
        0x09 => r.sleb()? as u64,
        0x0A => (((r.uint(2)? << 48) as i64) >> 48) as u64,
        0x0B => (((r.uint(4)? << 32) as i64) >> 32) as u64,
        0x0C => r.uint(8)?,
        _ => return None,
    };
    *at = r.pos;
    Some(if enc & 0x70 == 0x10 {
        v.wrapping_add(base + start as u64)
    } else {
        v
    })
}

/// A GCC LSDA (`.gcc_except_table`) as text, by content: its call sites
/// (relative to the function's start, `func`), their action chains, and
/// the type and exception-specification entries they reach by what they
/// name. `None` if it doesn't parse.
fn lsda_text(p: &Program<'_>, addr: u64, func: u64) -> Option<String> {
    use std::fmt::Write as _;
    let s = p.section_at(addr)?;
    let data = p.file.section_data(s);
    let base = s.addr;
    let mut at = usize::try_from(addr - base).ok()?;
    let word = p.word();
    let get = |at: &mut usize| -> Option<u8> {
        let b = *data.get(*at)?;
        *at += 1;
        Some(b)
    };
    let uleb = |at: &mut usize| -> Option<u64> {
        let mut r = turbonop_dwarf::Reader::new(data, *at, turbonop_core::Endian::Little);
        let v = r.uleb()?;
        *at = r.pos;
        Some(v)
    };
    let mut out = String::new();
    let lp_enc = get(&mut at)?;
    let lpstart = encoded(data, &mut at, lp_enc, base, word).unwrap_or(func);
    let tt_enc = get(&mut at)?;
    let tt_end = if tt_enc == 0xFF {
        None
    } else {
        let off = uleb(&mut at)?;
        Some(at + usize::try_from(off).ok()?)
    };
    let cs_enc = get(&mut at)?;
    let cs_len = usize::try_from(uleb(&mut at)?).ok()?;
    let cs_end = at.checked_add(cs_len)?;
    let _ = write!(out, "lpstart {:+} call sites", lpstart as i64 - func as i64);
    let mut actions: Vec<u64> = Vec::new();
    while at < cs_end {
        let start = encoded(data, &mut at, cs_enc, base, word)?;
        let len = encoded(data, &mut at, cs_enc, base, word)?;
        let lp = encoded(data, &mut at, cs_enc, base, word)?;
        let action = uleb(&mut at)?;
        let _ = write!(out, " ({start:#x} {len:#x} {lp:#x} {action})");
        if action != 0 {
            actions.push(action);
        }
    }
    // The action chains, and the largest type filter they use.
    let mut max_filter = 0i64;
    let mut specs = Vec::new();
    out.push_str(" actions");
    let mut seen = std::collections::BTreeSet::new();
    while let Some(a) = actions.pop() {
        let mut rec = cs_end + usize::try_from(a - 1).ok()?;
        let mut chain = String::new();
        let mut steps = 0;
        loop {
            if !seen.insert(rec) && chain.is_empty() {
                break;
            }
            let mut r = turbonop_dwarf::Reader::new(data, rec, turbonop_core::Endian::Little);
            let filter = r.sleb()?;
            let next_at = r.pos;
            let next = r.sleb()?;
            let _ = write!(chain, " {filter}");
            if filter > 0 {
                max_filter = max_filter.max(filter);
            } else if filter < 0 {
                specs.push(filter);
            }
            steps += 1;
            if next == 0 || steps > 1000 {
                break;
            }
            rec = usize::try_from(next_at as i64 + next).ok()?;
        }
        let _ = write!(out, " [{}]", chain.trim());
    }
    if let Some(end) = tt_end {
        let size = match tt_enc & 0x0F {
            0x00 => usize::from(word),
            0x02 | 0x0A => 2,
            0x03 | 0x0B => 4,
            0x04 | 0x0C => 8,
            _ => return Some(out),
        };
        out.push_str(" types");
        for k in 1..=max_filter {
            let mut e = end.checked_sub(size * usize::try_from(k).ok()?)?;
            let v = encoded(data, &mut e, tt_enc, base, word)?;
            let name = if v == 0 {
                "catch-all".into()
            } else {
                p.render(v)
            };
            let _ = write!(out, " {name}");
        }
        for f in specs {
            let mut e = end + usize::try_from(-f - 1).ok()?;
            out.push_str(" spec");
            loop {
                let i = uleb(&mut e)?;
                if i == 0 {
                    break;
                }
                let mut t = end.checked_sub(size * usize::try_from(i).ok()?)?;
                let v = encoded(data, &mut t, tt_enc, base, word)?;
                let _ = write!(out, " {}", p.render(v));
            }
        }
    }
    Some(out)
}
