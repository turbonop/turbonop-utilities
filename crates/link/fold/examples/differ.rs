//! Why copies of one name differ: `differ <lib>...` compares, for every name
//! defined by units of more than one identity, the first two identities,
//! and counts what differs between them (which section's data,
//! characteristics, relocations, or symbols, or the environment), with the
//! bytes at stake.
use std::collections::{BTreeMap, HashMap};
use turbonop_coff::{Object, SectionData};
use turbonop_fold::rewrite::units::{self, Unit};
use turbonop_global_alloc as _;

fn class(name: &[u8]) -> String {
    let n = String::from_utf8_lossy(name);
    let base = n.split('$').next().unwrap_or("");
    let base = if base.is_empty() { n.as_ref() } else { base };
    base.to_string()
}

type Copy<'a> = (&'a Object<'a>, &'a units::Facts<'a>, &'a Unit<'a>);

/// What differs between two copies of one unit, in words.
fn what(a: Copy<'_>, b: Copy<'_>) -> String {
    if a.1.environment != b.1.environment {
        if a.1.feat00 != b.1.feat00 {
            return "environment: @feat.00".into();
        }
        if a.1.mismatches != b.1.mismatches {
            return "environment: /FAILIFMISMATCH".into();
        }
        return "environment: S_COMPILE3".into();
    }
    let (oa, _, ua) = a;
    let (ob, _, ub) = b;
    let keep = |o: &Object<'_>, s: &&turbonop_coff::SectionId| {
        !units::is_debug(o.sections[s.index()].name.as_bytes())
    };
    let sa: Vec<_> = ua.sections.iter().filter(|s| keep(oa, s)).collect();
    let sb: Vec<_> = ub.sections.iter().filter(|s| keep(ob, s)).collect();
    if sa.len() != sb.len() {
        return "number of sections".into();
    }
    for (x, y) in sa.iter().zip(&sb) {
        let (p, q) = (&oa.sections[x.index()], &ob.sections[y.index()]);
        let name = class(p.name.as_bytes());
        if p.name.as_bytes() != q.name.as_bytes() {
            return format!("section names ({name})");
        }
        if p.characteristics != q.characteristics {
            return format!("{name}: characteristics");
        }
        let same_data = match (&p.data, &q.data) {
            (SectionData::Bytes(u), SectionData::Bytes(v)) => u == v,
            (SectionData::Uninitialized(u), SectionData::Uninitialized(v)) => u == v,
            _ => false,
        };
        if !same_data {
            return format!("{name}: data");
        }
        if p.relocations.len() != q.relocations.len() {
            return format!("{name}: number of relocations");
        }
        for (r, t) in p.relocations.iter().zip(&q.relocations) {
            if r.offset != t.offset || r.kind != t.kind {
                return format!("{name}: relocation offsets or kinds");
            }
            let (ta, tb) = (&oa.symbols[r.symbol.index()], &ob.symbols[t.symbol.index()]);
            if ta.is_external() && ta.name.as_bytes() != tb.name.as_bytes() {
                return format!("{name}: relocation targets' names");
            }
        }
    }
    if ua.symbols.len() != ub.symbols.len() {
        return "number of symbols".into();
    }
    for (x, y) in ua.symbols.iter().zip(&ub.symbols) {
        let (p, q) = (&oa.symbols[x.index()], &ob.symbols[y.index()]);
        if p.value != q.value || p.storage_class != q.storage_class || p.kind != q.kind {
            return format!("symbols' values or classes ({})", class(p.name.as_bytes()));
        }
        if p.is_external() && p.name.as_bytes() != q.name.as_bytes() {
            return "external symbols' names".into();
        }
    }
    if ua.selection != ub.selection {
        return "selection".into();
    }
    "checksum, or relocations inside the unit".into()
}

fn main() {
    let mut causes: BTreeMap<String, (u64, u64)> = BTreeMap::new();
    let mut examples: BTreeMap<String, String> = BTreeMap::new();
    for path in std::env::args().skip(1) {
        let data = std::fs::read(&path).unwrap();
        let archive = turbonop_archive::Archive::parse(&data).unwrap();
        let members: Vec<_> = archive.files().collect();
        let mut objects = Vec::new();
        for m in &members {
            if let Ok(turbonop_coff::File::Object(o)) = turbonop_coff::File::parse(m.data)
                && let Ok(f) = units::find(&o)
            {
                objects.push((o, f));
            }
        }
        let mut by_name: HashMap<&[u8], Vec<(usize, usize)>> = HashMap::new();
        for (k, (_, f)) in objects.iter().enumerate() {
            for (u, unit) in f.units.iter().enumerate() {
                if let Some(n) = unit.externals.first() {
                    by_name.entry(n.as_ref()).or_default().push((k, u));
                }
            }
        }
        for (name, copies) in &by_name {
            let first = copies[0];
            let id = |&(k, u): &(usize, usize)| objects[k].1.units[u].identity;
            let Some(&second) = copies.iter().find(|c| id(c) != id(&first)) else {
                continue;
            };
            let copy = |(k, u): (usize, usize)| -> Copy<'_> {
                (&objects[k].0, &objects[k].1, &objects[k].1.units[u])
            };
            let (a, b) = (copy(first), copy(second));
            let w = what(a, b);
            let e = causes.entry(w.clone()).or_default();
            e.0 += 1;
            e.1 += a.2.size * (copies.len() as u64 - 1);
            examples
                .entry(w)
                .or_insert_with(|| String::from_utf8_lossy(name).chars().take(100).collect());
        }
    }
    let mut v: Vec<_> = causes.into_iter().collect();
    v.sort_by_key(|x| std::cmp::Reverse(x.1.1));
    for (w, (n, bytes)) in v {
        println!(
            "{:>8} names {:>9.1} MiB  {}   e.g. {}",
            n,
            bytes as f64 / 1_048_576.0,
            w,
            examples[&w]
        );
    }
}
