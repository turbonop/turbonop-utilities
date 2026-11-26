//! How long reading a library's members takes, by step, on one thread:
//! `readbench <lib>`.
use std::time::Instant;
use turbonop_global_alloc as _;

fn main() {
    let path = std::env::args().nth(1).unwrap();
    let data = std::fs::read(&path).unwrap();
    let archive = turbonop_archive::Archive::parse(&data).unwrap();
    let members: Vec<_> = archive.files().collect();
    for _round in 0..2 {
        let t = Instant::now();
        let objects: Vec<_> = members
            .iter()
            .filter_map(|m| match turbonop_coff::File::parse(m.data) {
                Ok(turbonop_coff::File::Object(o)) => Some(o),
                _ => None,
            })
            .collect();
        let parse = t.elapsed();
        let t = Instant::now();
        let mut units = 0;
        for o in &objects {
            if let Ok(f) = turbonop_fold::rewrite::units::find(o) {
                units += f.units.len();
            }
        }
        let find = t.elapsed();
        let symbols: usize = objects.iter().map(|o| o.symbols.len()).sum();
        println!(
            "{} members, {} symbols, {} units: parse {:.3} s, find {:.3} s",
            objects.len(),
            symbols,
            units,
            parse.as_secs_f64(),
            find.as_secs_f64()
        );
    }
}
