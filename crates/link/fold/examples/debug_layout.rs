//! Where compilers put CodeView in an object: `debug_layout <dir or file>...`
//! reads every object (and library member) and counts, separately for
//! MSVC's and clang's objects (by `S_COMPILE3`), the subsections and symbol
//! records of the per-object `.debug$S` (not a COMDAT) and of the
//! per-function ones (COMDATs, associated with a function or data), and
//! the relocations from per-object `.debug$S` to symbols defined in
//! COMDATs (which a fold would move).
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use turbonop_codeview::c13;
use turbonop_codeview::symbols::kind;
use turbonop_coff::{File, Object, SectionNumber};
use turbonop_global_alloc as _;

#[derive(Default)]
struct Counts {
    objects: u64,
    /// (where, subsection kind) -> count.
    subsections: BTreeMap<(&'static str, u32), u64>,
    /// (where, symbol kind) -> count.
    symbols: BTreeMap<(&'static str, &'static str), u64>,
    /// (target storage class) -> relocations from per-object .debug$S to
    /// a symbol defined in a COMDAT.
    to_comdat: BTreeMap<u8, u64>,
    /// Per-function .debug$S sections, and those holding no symbols.
    per_function: u64,
}

impl Counts {
    fn add(&mut self, o: Counts) {
        self.objects += o.objects;
        self.per_function += o.per_function;
        for (k, v) in o.subsections {
            *self.subsections.entry(k).or_default() += v;
        }
        for (k, v) in o.symbols {
            *self.symbols.entry(k).or_default() += v;
        }
        for (k, v) in o.to_comdat {
            *self.to_comdat.entry(k).or_default() += v;
        }
    }
}

const COMDAT: u32 = 0x1000;

fn producer(o: &Object<'_>) -> &'static str {
    for s in &o.sections {
        if s.name.as_bytes() != b".debug$S" || s.characteristics & COMDAT != 0 {
            continue;
        }
        let Ok(subs) = c13::section_subsections(s.bytes()) else {
            continue;
        };
        for sub in subs {
            if sub.kind != turbonop_codeview::lines::subsection::DEBUG_S_SYMBOLS {
                continue;
            }
            for r in c13::symbol_records(sub.data).unwrap_or_default() {
                if r.kind == kind::S_COMPILE3 {
                    let text = &r.data[22.min(r.data.len())..];
                    return if text.starts_with(b"clang") {
                        "clang"
                    } else {
                        "msvc"
                    };
                }
            }
        }
    }
    "none"
}

fn object(o: &Object<'_>) -> (&'static str, Counts) {
    let mut c = Counts {
        objects: 1,
        ..Default::default()
    };
    for s in &o.sections {
        if s.name.as_bytes() != b".debug$S" {
            continue;
        }
        let comdat = s.characteristics & COMDAT != 0;
        let place = if comdat { "function" } else { "object" };
        if comdat {
            c.per_function += 1;
        } else {
            for r in &s.relocations {
                if let Some(t) = o.symbol(r.symbol)
                    && let SectionNumber::Section(id) = t.section
                    && o.sections[id.index()].characteristics & COMDAT != 0
                {
                    *c.to_comdat.entry(t.storage_class).or_default() += 1;
                }
            }
        }
        let Ok(subs) = c13::section_subsections(s.bytes()) else {
            *c.subsections.entry((place, u32::MAX)).or_default() += 1;
            continue;
        };
        for sub in subs {
            *c.subsections.entry((place, sub.kind)).or_default() += 1;
            if sub.kind == turbonop_codeview::lines::subsection::DEBUG_S_SYMBOLS {
                for r in c13::symbol_records(sub.data).unwrap_or_default() {
                    *c.symbols.entry((place, kind::name(r.kind))).or_default() += 1;
                }
            }
        }
    }
    (producer(o), c)
}

fn files(path: &Path, out: &mut Vec<PathBuf>) {
    if path.is_dir() {
        for e in std::fs::read_dir(path).unwrap().flatten() {
            files(&e.path(), out);
        }
    } else if path
        .extension()
        .is_some_and(|e| e.eq_ignore_ascii_case("lib") || e.eq_ignore_ascii_case("obj"))
    {
        out.push(path.to_path_buf());
    }
}

fn main() {
    let mut paths = Vec::new();
    for a in std::env::args().skip(1) {
        files(Path::new(&a), &mut paths);
    }
    let results = turbonop_par::map_balanced(&paths, 1, |p| {
        let data = std::fs::read(p).unwrap();
        let mut out: BTreeMap<&'static str, Counts> = BTreeMap::new();
        let mut one = |d: &[u8]| {
            if let Ok(File::Object(o)) = File::parse(d) {
                let (who, c) = object(&o);
                out.entry(who).or_default().add(c);
            }
        };
        if turbonop_archive::Archive::is_archive(&data) {
            if let Ok(a) = turbonop_archive::Archive::parse(&data) {
                for m in a.files() {
                    one(m.data);
                }
            }
        } else {
            one(&data);
        }
        out
    });
    let mut all: BTreeMap<&'static str, Counts> = BTreeMap::new();
    for r in results {
        for (k, v) in r {
            all.entry(k).or_default().add(v);
        }
    }
    for (who, c) in &all {
        println!(
            "== {who}: {} objects, {} per-function .debug$S",
            c.objects, c.per_function
        );
        for ((place, k), n) in &c.subsections {
            println!("  subsection {place:8} {k:#x}: {n}");
        }
        for ((place, k), n) in &c.symbols {
            println!("  symbol {place:8} {k}: {n}");
        }
        for (class, n) in &c.to_comdat {
            println!(
                "  per-object .debug$S relocation to a COMDAT's symbol, storage class {class}: {n}"
            );
        }
    }
}
