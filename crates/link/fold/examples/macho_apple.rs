//! Reads the Apple accelerator tables of each Mach-O object given (files,
//! or directories searched for `.o` files) and writes them again as
//! `turbonop_fold`'s pruning writes them, and says how many came back the
//! same bytes: `macho_apple <path>...`.
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
            Ok(o) => turbonop_fold::macho::prune::apple_round_trip(&o)
                .into_iter()
                .map(|(name, r)| {
                    let name = String::from_utf8_lossy(&name).into_owned();
                    match r {
                        Ok(true) => format!("{name}: the same"),
                        Ok(false) => format!("{name}: different ({})", p.display()),
                        Err(e) => format!("{name}: not read: {e}"),
                    }
                })
                .collect(),
            Err(_) => vec!["not an object".to_string()],
        }
    });
    let mut tally: BTreeMap<String, usize> = BTreeMap::new();
    let mut shown = 0;
    for r in results.into_iter().flatten() {
        let key = match r.split_once(" (") {
            Some((k, _)) => {
                if shown < 5 {
                    println!("{r}");
                    shown += 1;
                }
                k.to_string()
            }
            None => r,
        };
        *tally.entry(key).or_default() += 1;
    }
    for (k, v) in tally {
        println!("{v:8} {k}");
    }
}
