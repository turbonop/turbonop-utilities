use std::collections::HashSet;
use turbonop_pe::ltcg::*;
fn main() {
    let mut bad = 0;
    let mut n = 0;
    let (mut listed_ok, mut listed_bad) = (0, 0);
    for path in std::env::args().skip(1) {
        let data = std::fs::read(&path).unwrap();
        let o = LtcgObject::parse(&data).unwrap();
        let g = o.globals().unwrap();
        let l = o.locals().unwrap();
        let i = o.initializers().unwrap();
        let u = o.il_units();
        n += 1;
        let mut why = Vec::new();
        if g.walked.is_err() { why.push(format!("gl {:?}", g.walked)); }
        if l.walked.is_err() { why.push(format!("sy {:?}", l.walked)); }
        if i.walked.is_err() { why.push(format!("in {:?}", i.walked)); }
        if !u.agrees_with(&l) { why.push(format!("units {} sy {}", u.len(), l.functions.len())); }
        for s in &g.symbols {
            if let Some(il) = s.il {
                let k = u.index_of(il.il_offset);
                if k.is_none() || l.functions.get(k.unwrap()).map(|f| f.offset) != Some(il.symbols_offset as usize) {
                    why.push(format!("unit/sy pairing {}", String::from_utf8_lossy(s.name)));
                    break;
                }
            }
        }
        if let Ok(idx) = std::fs::read_to_string(format!("{path}.idx")) {
            let idx: HashSet<&str> = idx.split_whitespace().collect();
            for (s, li) in g.symbols.iter().zip(g.lib_listed()) {
                if !s.storage().is_public_definition() { continue; }
                let nm = String::from_utf8_lossy(s.name);
                let inn = idx.contains(&*nm) || idx.contains(&*format!("_{nm}")) || idx.contains(&*format!("@{nm}"));
                if inn == li { listed_ok += 1 } else { listed_bad += 1; if listed_bad < 10 { println!("listed mismatch {path} {nm} idx={inn} ours={li}"); } }
            }
        }
        if !why.is_empty() { bad += 1; println!("{path}: {why:?}"); }
    }
    println!("{n} objects, {bad} with problems; lib index: {listed_ok} agree, {listed_bad} disagree");
}
