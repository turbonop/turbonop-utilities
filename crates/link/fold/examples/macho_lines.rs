//! Decodes and encodes again the line table of each Mach-O object given
//! (files, or directories searched for `.o` files), and says how many came
//! back the same bytes: `macho_lines <path>...`.
use turbonop_global_alloc as _;

use std::collections::BTreeMap;
use std::path::PathBuf;

fn objects(p: PathBuf, out: &mut Vec<PathBuf>) {
    if p.is_dir() {
        let mut v: Vec<PathBuf> = std::fs::read_dir(&p)
            .into_iter()
            .flatten()
            .flatten()
            .map(|e| e.path())
            .collect();
        v.sort();
        for e in v {
            objects(e, out);
        }
    } else if p.extension().is_some_and(|e| e == "o") {
        out.push(p);
    }
}

fn main() {
    let mut paths = Vec::new();
    for a in std::env::args().skip(1) {
        objects(PathBuf::from(a), &mut paths);
    }
    let results = turbonop_par::map_balanced(&paths, 4, |p| {
        let data = std::fs::read(p).unwrap_or_default();
        match turbonop_macho_object::Object::parse(&data) {
            Ok(o) => match turbonop_fold::macho::adjust::line_round_trip(&o) {
                Ok(Some((a, b))) if a == b => "the same".to_string(),
                Ok(Some((a, b))) => {
                    let k = a
                        .iter()
                        .zip(&b)
                        .position(|(x, y)| x != y)
                        .unwrap_or(a.len().min(b.len()));
                    let lo = k.saturating_sub(12);
                    format!(
                        "different: {} at {k} of {} ({}): {:02x?} vs {:02x?}",
                        p.display(),
                        a.len(),
                        b.len(),
                        &a[lo..(k + 12).min(a.len())],
                        &b[lo..(k + 12).min(b.len())]
                    )
                }
                Ok(None) => "no line table".to_string(),
                Err(e) => format!("not read: {e}"),
            },
            Err(_) => "not an object".to_string(),
        }
    });
    let mut tally: BTreeMap<String, usize> = BTreeMap::new();
    for r in results {
        let key = if r.starts_with("different") {
            if tally.get("different").copied().unwrap_or(0) < 5 {
                println!("{r}");
            }
            "different".to_string()
        } else {
            r
        };
        *tally.entry(key).or_default() += 1;
    }
    for (k, v) in tally {
        println!("{v:8} {k}");
    }
}
