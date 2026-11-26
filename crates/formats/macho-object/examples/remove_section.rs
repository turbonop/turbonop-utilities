//! Removes a section from an object, as folding does (its symbols, then
//! the section, then the addresses laid out again and every encoded
//! address moved with them), and checks the result is a reference object
//! byte for byte: the same source built without the section (see
//! `corpus/build.sh`, whose `pad-*` objects have a section `__TEXT,__pad`
//! the `tricky-*` ones don't).
//!
//!     cargo run --release -p turbonop-macho-object --example remove_section -- <object> <segment>,<section> <reference>...
//!
//! Several triples may follow one another. A result that differs is
//! written to `<object>.removed`.

use turbonop_global_alloc as _;

use turbonop_macho_object::Object;

fn remove(path: &str, which: &str, reference: &str) -> Result<(), String> {
    let data = std::fs::read(path).map_err(|e| format!("{path}: {e}"))?;
    let want = std::fs::read(reference).map_err(|e| format!("{reference}: {e}"))?;
    let (seg, sect) = which
        .split_once(',')
        .ok_or_else(|| format!("{which}: not <segment>,<section>"))?;
    let mut o = Object::parse(&data).map_err(|e| format!("{path}: {e}"))?;
    let index = o
        .sections()
        .position(|s| s.segment_name() == seg.as_bytes() && s.name() == sect.as_bytes())
        .ok_or_else(|| format!("{path}: no section {which}"))?;
    let ordinal = u8::try_from(index + 1).map_err(|_| "a section past 255".to_string())?;
    let keep_symbols: Vec<bool> = o
        .symbols
        .iter()
        .map(|s| !(s.in_section() && s.n_sect == ordinal))
        .collect();
    o.retain_symbols(&keep_symbols);
    let mut keep = vec![true; o.section_count()];
    keep[index] = false;
    o.retain_sections(&keep);
    let unadjusted = o.assign_addresses();
    // arm64's section-start labels (ltmp0, ltmp1, ...), which LLVM numbers
    // in section order: renumbered as it would without the section.
    let mut n = 0;
    for s in &mut o.symbols {
        if s.name.starts_with(b"ltmp") && s.name[4..].iter().all(u8::is_ascii_digit) {
            s.name = format!("ltmp{n}").into_bytes().into();
            n += 1;
        }
    }
    let out = o.write().map_err(|e| format!("{path}: {e}"))?;
    let at = out
        .iter()
        .zip(&want)
        .position(|(a, b)| a != b)
        .or_else(|| (out.len() != want.len()).then(|| out.len().min(want.len())));
    match at {
        None => {
            println!(
                "{path} without {which}: {reference}, byte for byte ({} relocations unadjusted)",
                unadjusted.len()
            );
            Ok(())
        }
        Some(at) => {
            let _ = std::fs::write(format!("{path}.removed"), &out);
            Err(format!(
                "{path} without {which}: differs from {reference} at {at} ({} and {} bytes; {} relocations unadjusted)",
                out.len(),
                want.len(),
                unadjusted.len()
            ))
        }
    }
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let mut failed = false;
    for triple in args.chunks(3) {
        let [path, which, reference] = triple else {
            eprintln!("usage: remove_section <object> <segment>,<section> <reference>...");
            std::process::exit(2);
        };
        if let Err(e) = remove(path, which, reference) {
            println!("{e}");
            failed = true;
        }
    }
    if failed {
        std::process::exit(1);
    }
}
