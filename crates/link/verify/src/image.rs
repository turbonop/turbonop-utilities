//! `turbolib compare-images`: whether two programs, linked from the same
//! inputs except that one used turbolib's rewritten libraries, are
//! equivalent.
//!
//! Folding moves COMDATs between members, and so changes the order the
//! linker lays them out in, and every address. The two images are
//! compared structurally, through each one's PDB:
//!
//! - **Functions and data** ([`bytes`]): every section contribution (one
//!   COFF section of one object, as the PDB lists them) is matched with
//!   its counterpart by the label at its start (a public, procedure, or
//!   data symbol; else the contribution's own label, see [`program`]),
//!   and their bytes compared with every address in them mapped through
//!   each image's labels.
//! - **Tables** ([`tables`]): imports, exports, the C runtime's
//!   initializer and terminator tables, TLS callbacks, the load
//!   configuration, the `/guard` tables, and the unwind data, function by
//!   function, each entry by what it names.
//! - **Headers**: machine, characteristics, subsystem, DLL
//!   characteristics, image base, the entry point, and the sections'
//!   names and characteristics, in order.
//! - **PDBs**, by content ([`crate::pdb`]).
//!
//! What this proves, and what it doesn't, is in `docs/turbolib.md`
//! ("Verification").

pub(crate) mod bytes;
pub(crate) mod program;
pub(crate) mod tables;

use bytes::{Fields, Isa, Side};
use program::{Contribution, Linked, priority};
use turbonop_hash::Map;
use turbonop_pe::consts::section_characteristics as scn;

use crate::report::{Report, show};

/// A program: its image and its PDB, as bytes.
#[derive(Clone, Copy, Debug)]
pub struct Program<'a> {
    pub image: &'a [u8],
    pub pdb: &'a [u8],
}

/// Compares two programs (see the module's documentation), on every core.
pub fn compare(a: Program<'_>, b: Program<'_>) -> Result<Report, String> {
    let (la, lb) = turbonop_par::join(
        || Linked::read(a.image, a.pdb, "the first program"),
        || Linked::read(b.image, b.pdb, "the second program"),
    );
    let (mut la, mut lb) = (la?, lb?);
    let mut report = Report::new();
    let named = name_by_content(&mut la, &mut lb);
    if named > 0 {
        report.expect(
            "unlabeled read-only data named by its content (a copy in another module)",
            || format!("{named} contributions"),
        );
    }
    headers(&la, &lb, &mut report);
    let (contents, pdbs) = turbonop_par::join(
        || {
            let mut r = Report::new();
            contents(&la, &lb, &mut r);
            tables::imports(&la, &lb, &mut r);
            tables::exports(&la, &lb, &mut r);
            tables::initializers(&la, &lb, &mut r);
            tables::load_config(&la, &lb, &mut r);
            tables::unwind(&la, &lb, &mut r);
            r
        },
        || crate::pdb::compare(&la, &lb),
    );
    report.merge(contents);
    report.merge(pdbs);
    Ok(report)
}

fn headers(a: &Linked<'_>, b: &Linked<'_>, rep: &mut Report) {
    let field = |rep: &mut Report, name: &str, x: u64, y: u64| {
        rep.checked("header fields", 1);
        if x != y {
            rep.fail("header fields that differ", || {
                format!("{name}: {x:#x} and {y:#x}")
            });
        }
    };
    field(rep, "Machine", u64::from(a.machine), u64::from(b.machine));
    field(
        rep,
        "Characteristics",
        u64::from(a.pe.file_header().characteristics.get()),
        u64::from(b.pe.file_header().characteristics.get()),
    );
    let (ha, hb) = (a.pe.optional_header(), b.pe.optional_header());
    if let (Some(ha), Some(hb)) = (ha, hb) {
        field(
            rep,
            "Subsystem",
            u64::from(ha.subsystem()),
            u64::from(hb.subsystem()),
        );
        field(
            rep,
            "DllCharacteristics",
            u64::from(ha.dll_characteristics()),
            u64::from(hb.dll_characteristics()),
        );
        field(rep, "ImageBase", a.image_base, b.image_base);
        let entry = |p: &Linked<'_>, h: &turbonop_pe::OptionalHeader<'_>| {
            p.name(h.address_of_entry_point())
                .map(|(n, o)| (n.to_vec(), o))
        };
        rep.checked("header fields", 1);
        if entry(a, ha) != entry(b, hb) {
            rep.fail("header fields that differ", || "the entry point".into());
        }
        if ha.size_of_image() != hb.size_of_image() {
            rep.expect("images of another size", || {
                format!("{:#x} and {:#x}", ha.size_of_image(), hb.size_of_image())
            });
        }
    }
    let sections = |p: &Linked<'_>| -> Vec<(Vec<u8>, u32)> {
        p.sections
            .iter()
            .map(|s| (s.name.clone(), s.characteristics))
            .collect()
    };
    rep.checked("sections", a.sections.len() as u64);
    if sections(a) == sections(b) {
        for (x, y) in a.sections.iter().zip(&b.sections) {
            if x.virtual_size != y.virtual_size {
                rep.expect("sections of another size", || {
                    format!(
                        "{}: {:#x} and {:#x}",
                        show(&x.name),
                        x.virtual_size,
                        y.virtual_size
                    )
                });
            }
        }
    } else {
        rep.fail(
            "sections that differ in name, characteristics, or order",
            || {
                let show = |v: Vec<(Vec<u8>, u32)>| {
                    v.iter()
                        .map(|(n, c)| format!("{} {c:#x}", show(n)))
                        .collect::<Vec<_>>()
                        .join(", ")
                };
                format!("{} and {}", show(sections(a)), show(sections(b)))
            },
        );
    }
    // Each image's PDB is its own.
    let own = |p: &Linked<'_>| {
        let info = p.pdb.info();
        p.pe.codeview().is_some_and(|cv| match cv {
            turbonop_pe::CodeView::Rsds { guid, age, .. } => {
                guid == info.guid() && age <= info.age()
            }
            _ => false,
        })
    };
    for (ok, which) in [(own(a), "first"), (own(b), "second")] {
        if !ok {
            rep.fail("programs whose PDB doesn't match the image", || {
                format!("the {which}")
            });
        }
    }
    if a.pdb.info().guid() != b.pdb.info().guid() {
        rep.expect("PDBs with other signatures (GUIDs)", || {
            "each link makes its own".into()
        });
    }
}

/// A contribution's key: the symbol labeling its start, or its own label;
/// and whether that symbol was one of turbolib's renamed copies.
fn key<'p>(p: &'p Linked<'_>, c: &'p Contribution) -> (&'p [u8], bool) {
    match p.label_at(c.rva) {
        Some(l) if l.priority < priority::CONTRIBUTION => (&l.name, l.renamed),
        _ => (&c.label, false),
    }
}

/// The ISA a contribution's bytes are decoded with.
fn isa(p: &Linked<'_>, c: &Contribution) -> Isa {
    if c.characteristics & scn::CNT_CODE == 0 {
        return Isa::Data;
    }
    match p.machine {
        turbonop_pe::consts::machine::AMD64 => Isa::X64,
        turbonop_pe::consts::machine::ARM64 => Isa::Arm64,
        _ => Isa::Data,
    }
}

/// A contribution's content, its addresses read as names: the key that
/// pairs the contributions whose labels matched nothing (those that moved
/// to another module, say). Every base-relocated address stands for its
/// name, and so does every 4-byte aligned word that is the RVA of a
/// label's start.
fn content(p: &Linked<'_>, c: &Contribution) -> Vec<u8> {
    let data = p.bytes(c.rva, c.size);
    let mut out = Vec::with_capacity(data.len() + 16);
    out.extend_from_slice(&c.characteristics.to_le_bytes());
    let relocations = p.relocations_in(c.rva, c.size);
    let mut r = 0;
    let mut at = 0;
    while at < data.len() {
        if let Some(&(rva, size)) = relocations.get(r)
            && rva - c.rva == at as u32
        {
            let size = usize::from(size);
            let mut v = [0u8; 8];
            let n = size.min(data.len() - at);
            v[..n].copy_from_slice(&data[at..at + n]);
            let va = u64::from_le_bytes(v);
            // An address in the contribution itself, by its offset: its own
            // label names the module that holds it.
            let own = va
                .checked_sub(p.image_base + u64::from(c.rva))
                .filter(|&o| o < u64::from(c.size));
            match own {
                Some(o) => out.extend_from_slice(format!("<self+{o:#x}>").as_bytes()),
                None => {
                    let name = p.name_va(va);
                    out.extend_from_slice(format!("{name:?}").as_bytes());
                }
            }
            at += size;
            r += 1;
            continue;
        }
        if (c.rva as usize + at) & 3 == 0 && at + 4 <= data.len() {
            let w = u32::from_le_bytes([data[at], data[at + 1], data[at + 2], data[at + 3]]);
            // Only a word naming the start of something: any small integer
            // (a size, an offset, four characters of a string) is an RVA in
            // some section, and would name a function's middle by its
            // address (which follows the layout).
            if let Some(n) = p.section_of(w).and(p.name(w)).filter(|n| n.1 == 0) {
                out.extend_from_slice(format!("{n:?}").as_bytes());
                at += 4;
                continue;
            }
        }
        out.push(data[at]);
        at += 1;
    }
    out
}

/// Names the unlabeled read-only data whose labels match nothing in the
/// other program by its content, in both programs, so that references to
/// it compare by what it holds rather than by the module that held it.
///
/// Folding moves a function's associative read-only data (MSVC's `/RTC1`
/// variable descriptions and their names, say) with it, into another
/// module, and nothing labels it but its contribution (module,
/// characteristics, size, CRCs). That data refers to more such data, so a
/// name by content is found in rounds: each round names every such
/// contribution by its content with the previous round's names in it (see
/// [`content`]), until a round tells no two contributions apart that the
/// one before didn't, and joins none it had told apart (at most 16).
///
/// Two contributions of the same content then have one name, and are
/// interchangeable: sound for data nothing writes (an address compared as
/// a number is the exception, as everywhere in this comparison). Code and
/// writable data keep their labels. Returns how many contributions were
/// named.
fn name_by_content(a: &mut Linked<'_>, b: &mut Linked<'_>) -> usize {
    fn candidates(p: &Linked<'_>, other: &Linked<'_>) -> Vec<usize> {
        let theirs: turbonop_hash::Set<&[u8]> = other
            .contributions
            .iter()
            .map(|c| key(other, c).0)
            .collect();
        p.contributions
            .iter()
            .enumerate()
            .filter(|(_, c)| {
                c.characteristics & (scn::CNT_CODE | scn::MEM_WRITE | scn::CNT_UNINITIALIZED_DATA)
                    == 0
                    && c.size > 0
                    && !p.in_tables(c.rva, c.size)
                    && p.label_at(c.rva)
                        .is_none_or(|l| l.priority >= priority::CONTRIBUTION)
                    && !theirs.contains(&*c.label)
            })
            .map(|(i, _)| i)
            .collect()
    }
    let (ca, cb) = (candidates(a, b), candidates(b, a));
    if ca.is_empty() || cb.is_empty() {
        return 0;
    }
    // Each candidate's class in the round before (its label at first).
    let mut previous: Vec<usize> = (0..ca.len() + cb.len()).collect();
    for round in 0..16 {
        let (xa, xb) = turbonop_par::join(
            || turbonop_par::map(&ca, 64, |&i| content(a, &a.contributions[i])),
            || turbonop_par::map(&cb, 64, |&j| content(b, &b.contributions[j])),
        );
        let mut ids: Map<&[u8], usize> = Map::default();
        let classes: Vec<usize> = xa
            .iter()
            .chain(&xb)
            .map(|c| {
                let n = ids.len();
                *ids.entry(c.as_slice()).or_insert(n)
            })
            .collect();
        // Stable when the two rounds' classes correspond one to one.
        let mut pairs: turbonop_hash::Set<(usize, usize)> = turbonop_hash::Set::default();
        let mut olds: turbonop_hash::Set<usize> = turbonop_hash::Set::default();
        for (&o, &n) in previous.iter().zip(&classes) {
            pairs.insert((o, n));
            olds.insert(o);
        }
        let stable = round > 0 && pairs.len() == ids.len() && pairs.len() == olds.len();
        drop(xa);
        drop(xb);
        fn apply(p: &mut Linked<'_>, which: &[usize], classes: &[usize]) {
            for (&i, &class) in which.iter().zip(classes) {
                let c = &p.contributions[i];
                let name: Box<[u8]> = format!(
                    "[read-only data {:x} {:x} by content {class}]",
                    c.characteristics, c.size
                )
                .into_bytes()
                .into();
                let rva = c.rva;
                p.rename(rva, name.clone());
                p.contributions[i].label = name;
            }
        }
        apply(a, &ca, &classes[..ca.len()]);
        apply(b, &cb, &classes[ca.len()..]);
        previous = classes;
        if stable {
            break;
        }
    }
    ca.len() + cb.len()
}

fn contents(a: &Linked<'_>, b: &Linked<'_>, rep: &mut Report) {
    // Pairs by key, in address order within a key.
    let mut by_key: Map<&[u8], Vec<usize>> = Map::default();
    for (j, c) in b.contributions.iter().enumerate() {
        by_key.entry(key(b, c).0).or_default().push(j);
    }
    let mut used: Map<&[u8], usize> = Map::default();
    let mut pairs = Vec::new();
    let mut lone_a = Vec::new();
    // (Contributions holding the linker's tables are compared as tables.)
    let mut paired_b: Vec<bool> = b
        .contributions
        .iter()
        .map(|c| b.in_tables(c.rva, c.size))
        .collect();
    let mut tables = 0;
    for (i, c) in a.contributions.iter().enumerate() {
        if a.in_tables(c.rva, c.size) {
            tables += 1;
            continue;
        }
        let k = key(a, c).0;
        let n = used.entry(k).or_insert(0);
        match by_key
            .get(k)
            .and_then(|v| v[(*n).min(v.len())..].iter().find(|&&j| !paired_b[j]))
        {
            Some(&j) => {
                *n += 1;
                paired_b[j] = true;
                pairs.push((i, j));
            }
            None => lone_a.push(i),
        }
    }
    // The rest, paired by content.
    let lone_b: Vec<usize> = (0..b.contributions.len())
        .filter(|&j| !paired_b[j])
        .collect();
    if !lone_a.is_empty() && !lone_b.is_empty() {
        let ca = turbonop_par::map(&lone_a, 64, |&i| content(a, &a.contributions[i]));
        let cb = turbonop_par::map(&lone_b, 64, |&j| content(b, &b.contributions[j]));
        let mut by_content: Map<&[u8], Vec<usize>> = Map::default();
        for (k, c) in cb.iter().enumerate().rev() {
            by_content.entry(c.as_slice()).or_default().push(k);
        }
        let mut still = Vec::new();
        for (k, c) in ca.iter().enumerate() {
            match by_content.get_mut(c.as_slice()).and_then(Vec::pop) {
                Some(m) => {
                    paired_b[lone_b[m]] = true;
                    pairs.push((lone_a[k], lone_b[m]));
                    rep.expect(
                        "functions or data matched by content (moved to another module, or unlabeled)",
                        || show(&a.contributions[lone_a[k]].label),
                    );
                }
                None => still.push(lone_a[k]),
            }
        }
        lone_a = still;
    }
    for i in lone_a {
        let c = &a.contributions[i];
        rep.fail("functions or data only in the first program", || {
            show(key(a, c).0)
        });
    }
    for (j, c) in b.contributions.iter().enumerate() {
        if paired_b[j] {
            continue;
        }
        let (k, renamed) = key(b, c);
        if renamed {
            rep.expect(
                "turbolib's renamed copies only in the second program (unreferenced, kept under /OPT:NOREF)",
                || show(k),
            );
        } else {
            rep.fail("functions or data only in the second program", || show(k));
        }
    }
    // Each pair, in parallel.
    pairs.sort_unstable();
    let results = turbonop_par::map_balanced(&pairs, 256, |&(i, j)| {
        let (x, y) = (&a.contributions[i], &b.contributions[j]);
        let mut r = Report::new();
        let name = key(a, x).0;
        if x.size != y.size || x.characteristics != y.characteristics {
            r.fail("functions or data of another size or kind", || {
                format!("{}: {:#x} and {:#x} bytes", show(name), x.size, y.size)
            });
            return (r, Fields::default(), 0);
        }
        if x.characteristics & scn::CNT_UNINITIALIZED_DATA != 0 {
            return (r, Fields::default(), u64::from(x.size));
        }
        let (bx, by) = (a.bytes(x.rva, x.size), b.bytes(y.rva, y.size));
        let f = bytes::compare(
            &Side {
                image: a,
                rva: x.rva,
                bytes: &bx,
            },
            &Side {
                image: b,
                rva: y.rva,
                bytes: &by,
            },
            isa(a, x),
            name,
            &mut r,
        );
        (r, f, u64::from(x.size))
    });
    let mut fields = Fields::default();
    let mut compared = 0;
    for (r, f, n) in results {
        rep.merge(r);
        fields.absolute += f.absolute;
        fields.relative += f.relative;
        fields.inferred += f.inferred;
        compared += n;
    }
    rep.checked(
        "functions and data (section contributions) compared",
        pairs.len() as u64,
    );
    rep.checked(
        "contributions holding the linker's tables (compared as tables)",
        tables,
    );
    rep.checked("bytes of them compared", compared);
    rep.checked(
        "absolute addresses (base relocations) matched by name",
        fields.absolute,
    );
    rep.checked("PC-relative operands matched by name", fields.relative);
    rep.checked(
        "image-relative words matched by name (inferred)",
        fields.inferred,
    );
    // What no contribution covers.
    let uncovered = |p: &Linked<'_>| {
        let covered: u64 = p.contributions.iter().map(|c| u64::from(c.size)).sum();
        let total: u64 = p.sections.iter().map(|s| u64::from(s.virtual_size)).sum();
        total.saturating_sub(covered)
    };
    for (n, which) in [(uncovered(a), "first"), (uncovered(b), "second")] {
        rep.checked(
            &format!("section bytes in the {which} program no contribution covers (not compared)"),
            n,
        );
    }
}

#[cfg(test)]
mod tests;
