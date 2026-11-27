//! ELF: `turbolib compare-images` for two ELF programs ([`compare`]), and
//! `turbolib verify` for GNU and BSD archives of ELF objects
//! ([`library`]).
//!
//! **`compare`** reads the two programs alone (they carry their symbols
//! and DWARF). Folding moves COMDAT groups between members, so the
//! linker lays code and data out in another order and every address
//! changes; everything is compared by what addresses name (see
//! [`program`]):
//!
//! - **Functions and data**: every symbol of `.symtab` with a size (one
//!   per extent: aliases share it) is paired with its counterpart by its
//!   key (its name; a local's qualified by its file and an ordinal), and
//!   their bytes compared as for COFF (`image::bytes`): x86-64 and ARM64
//!   PC-relative operands by what their targets name, dynamic
//!   relocations in them by what they name, and other differing 4-byte
//!   words read as addresses naming the same thing. Bytes outside every
//!   sized symbol are counted, not compared.
//! - **Tables** ([`tables`]): the headers, program headers and sections
//!   (types and flags, in order), `.symtab` as a set, the dynamic section,
//!   dynamic symbols with their versions, dynamic relocations, the
//!   initializer arrays in order, notes, and `.eh_frame` by function.
//! - **DWARF** ([`dwarf`]): every live subprogram with its subtree, its
//!   unit's attributes and line rows, variables at fixed addresses, and
//!   named types, by content.

pub(crate) mod dwarf;
pub mod library;
pub(crate) mod program;
mod tables;

use std::borrow::Cow;
use std::collections::BTreeMap;

use program::{Program, Region};

use crate::image::bytes::{self, Fields, Isa, Side};
use crate::report::{Report, show};

/// Whether `data` is an ELF file.
pub fn is_elf(data: &[u8]) -> bool {
    data.starts_with(b"\x7fELF")
}

/// Compares two linked ELF programs (see the module's documentation), on
/// every core.
pub fn compare(a: &[u8], b: &[u8]) -> Result<Report, String> {
    let (pa, pb) = turbonop_par::join(
        || Program::read(a, "the first program"),
        || Program::read(b, "the second program"),
    );
    let (pa, pb) = (pa?, pb?);
    let mut report = Report::new();
    tables::headers(&pa, &pb, &mut report);
    let (mut first, second) = turbonop_par::join(
        || {
            let mut r = Report::new();
            contents(&pa, &pb, &mut r);
            tables::symbols(&pa, &pb, &mut r);
            tables::dynamic(&pa, &pb, &mut r);
            tables::initializers(&pa, &pb, &mut r);
            tables::notes(&pa, &pb, &mut r);
            tables::unwind(&pa, &pb, &mut r);
            r
        },
        || dwarf::compare(&pa, &pb),
    );
    first.merge(second);
    report.merge(first);
    Ok(report)
}

/// Compares every function's and datum's bytes.
fn contents(a: &Program<'_>, b: &Program<'_>, rep: &mut Report) {
    let by_key: BTreeMap<&[u8], &Region> = b.regions.iter().map(|r| (&*r.key, r)).collect();
    let keys_a: std::collections::BTreeSet<&[u8]> = a.regions.iter().map(|r| &*r.key).collect();
    let mut pairs: Vec<(&Region, &Region)> = Vec::with_capacity(a.regions.len());
    for r in &a.regions {
        match by_key.get(&*r.key) {
            Some(s) => pairs.push((r, s)),
            None => rep.fail("functions or data only in the first program", || {
                show(&r.key)
            }),
        }
    }
    for r in &b.regions {
        if !keys_a.contains(&*r.key) {
            rep.fail("functions or data only in the second program", || {
                show(&r.key)
            });
        }
    }
    let reports: Vec<Report> = turbonop_par::map_balanced(&pairs, 256, |&(ra, rb)| {
        let mut r = Report::new();
        region(a, b, ra, rb, &mut r);
        r
    });
    for r in reports {
        rep.merge(r);
    }
    rep.checked(
        "bytes outside every sized symbol (first program; not compared)",
        a.uncovered(),
    );
}

/// The bytes of a region with its dynamic relocations' places zeroed, and
/// those relocations as (offset, type, target).
fn masked<'a>(p: &Program<'a>, r: &Region, bytes: &'a [u8]) -> (Cow<'a, [u8]>, Vec<String>) {
    let rels = p.dynrel_in(r.addr, r.size);
    if rels.is_empty() {
        return (Cow::Borrowed(bytes), Vec::new());
    }
    let mut out = bytes.to_vec();
    let mut list = Vec::with_capacity(rels.len());
    for x in rels {
        let at = (x.place - r.addr) as usize;
        let end = (at + usize::from(x.size)).min(out.len());
        out[at..end].fill(0);
        let kind = if x.relr {
            "RELR".to_string()
        } else {
            x.kind.to_string()
        };
        list.push(format!("+{at:#x} {kind} {}", p.target(x)));
    }
    (Cow::Owned(out), list)
}

fn region(a: &Program<'_>, b: &Program<'_>, ra: &Region, rb: &Region, rep: &mut Report) {
    rep.checked("functions and data", 1);
    if ra.size != rb.size {
        rep.fail("functions or data of another size", || {
            format!("{}: {:#x} and {:#x}", show(&ra.key), ra.size, rb.size)
        });
        return;
    }
    if ra.nobits || rb.nobits {
        if ra.nobits != rb.nobits {
            rep.fail("data uninitialized in only one program", || show(&ra.key));
        }
        return;
    }
    let (Some(ba), Some(bb)) = (a.bytes(ra.addr, ra.size), b.bytes(rb.addr, rb.size)) else {
        rep.unchecked("functions or data outside sections with contents", || {
            show(&ra.key)
        });
        return;
    };
    rep.checked("bytes of functions and data", ra.size);
    let (ma, la) = masked(a, ra, ba);
    let (mb, lb) = masked(b, rb, bb);
    rep.checked("dynamic relocations in functions and data", la.len() as u64);
    if la != lb {
        rep.fail(
            "functions or data whose dynamic relocations name other things",
            || {
                let at = la.iter().zip(&lb).position(|(x, y)| x != y).unwrap_or(0);
                format!(
                    "{}: {} and {}",
                    show(&ra.key),
                    la.get(at).map_or("nothing", String::as_str),
                    lb.get(at).map_or("nothing", String::as_str)
                )
            },
        );
        return;
    }
    let isa = match (ra.code, a.machine()) {
        (true, 62) => Isa::X64,
        (true, 183) => Isa::Arm64,
        _ => Isa::Data,
    };
    let sa = Side {
        image: a,
        rva: ra.addr as u32,
        bytes: &ma,
    };
    let sb = Side {
        image: b,
        rva: rb.addr as u32,
        bytes: &mb,
    };
    let f: Fields = bytes::compare(&sa, &sb, isa, &ra.key, rep);
    rep.checked("PC-relative operands matched", f.relative);
    rep.checked("other words read as addresses, matched", f.inferred);
    rep.checked("thread-pointer offsets naming the same TLS variable", f.tls);
}

#[cfg(test)]
mod tests;
