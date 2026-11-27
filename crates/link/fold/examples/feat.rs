//! The @feat.00 values of libraries' objects, counted, with an example member.
use std::collections::BTreeMap;
use turbonop_global_alloc as _;
fn main() {
    let mut seen: BTreeMap<u32, (u64, String)> = BTreeMap::new();
    for path in std::env::args().skip(1) {
        let data = std::fs::read(&path).unwrap();
        let ar = turbonop_archive::Archive::parse(&data).unwrap();
        for m in ar.files() {
            let Ok(turbonop_coff::File::Object(o)) = turbonop_coff::File::parse(m.data) else {
                continue;
            };
            if let Some(s) = o.symbols.iter().find(|s| s.name.as_bytes() == b"@feat.00") {
                let e = seen
                    .entry(s.value)
                    .or_insert((0, String::from_utf8_lossy(&m.name).into_owned()));
                e.0 += 1;
            }
        }
    }
    for (v, (n, ex)) in seen {
        println!("{v:#010x} {n:>6}  e.g. {ex}");
    }
}
