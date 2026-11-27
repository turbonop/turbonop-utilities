//! How much memory reading a library's members into the object model and
//! finding their units takes, by part (from the vectors' capacities and
//! the types' sizes; the allocator's own overhead not counted):
//! `modelsize <lib>`.
use std::mem::size_of;
use turbonop_coff::{Aux, Object, Relocation, Section, SectionData, Symbol};
use turbonop_fold::rewrite::units;
use turbonop_global_alloc as _;

fn mib(n: usize) -> f64 {
    n as f64 / 1_048_576.0
}

fn main() {
    let path = std::env::args().nth(1).expect("a library");
    let data = std::fs::read(&path).expect("readable");
    let archive = turbonop_archive::Archive::parse(&data).expect("a library");
    let members: Vec<_> = archive.files().collect();
    let objects: Vec<Object<'_>> = members
        .iter()
        .filter_map(|m| match turbonop_coff::File::parse(m.data) {
            Ok(turbonop_coff::File::Object(o)) => Some(o),
            _ => None,
        })
        .collect();
    let (mut sym, mut sym_owned, mut sec, mut sec_owned, mut rel, mut nsym, mut nsec, mut nrel) =
        (0, 0, 0, 0, 0, 0, 0, 0);
    for o in &objects {
        sym += o.symbols.capacity() * size_of::<Symbol<'_>>();
        nsym += o.symbols.len();
        for s in &o.symbols {
            if let Aux::Raw(std::borrow::Cow::Owned(b)) = &s.aux {
                sym_owned += b.capacity();
            }
        }
        sec += o.sections.capacity() * size_of::<Section<'_>>();
        nsec += o.sections.len();
        for s in &o.sections {
            rel += s.relocations.capacity() * size_of::<Relocation>();
            nrel += s.relocations.len();
            if let SectionData::Bytes(std::borrow::Cow::Owned(b)) = &s.data {
                sec_owned += b.capacity();
            }
        }
    }
    println!(
        "{} objects: {nsym} symbols ({} B each) {:.1} MiB (+{:.1} owned); {nsec} sections ({} B each) {:.1} MiB (+{:.1} owned); {nrel} relocations {:.1} MiB",
        objects.len(),
        size_of::<Symbol<'_>>(),
        mib(sym),
        mib(sym_owned),
        size_of::<Section<'_>>(),
        mib(sec),
        mib(sec_owned),
        mib(rel)
    );
    let facts: Vec<units::Facts> = objects.iter().filter_map(|o| units::find(o).ok()).collect();
    let (mut u, mut usec, mut usym, mut uext, mut uext_bytes, mut nunits, mut other, mut rest) =
        (0, 0, 0, 0, 0, 0, 0, 0);
    for f in &facts {
        u += f.units.capacity() * size_of::<units::Unit>();
        nunits += f.units.len();
        for x in &f.units {
            usec += x.sections.capacity() * 4;
            usym += x.symbols.capacity() * 4;
            uext += x.externals.capacity() * 24;
            uext_bytes += x.externals.iter().map(Vec::capacity).sum::<usize>();
            rest += x.edges.capacity() * 16
                + x.referrers.capacity() * 4
                + x.closure.capacity() * 8
                + x.detail.as_ref().map_or(0, String::capacity);
        }
        other += f.other_definitions.capacity() * 24
            + f.other_definitions
                .iter()
                .map(Vec::capacity)
                .sum::<usize>();
    }
    println!(
        "{nunits} units ({} B each) {:.1} MiB; their sections {:.1}, symbols {:.1}, externals {:.1} + names {:.1}, the rest {:.1}; other definitions {:.1} MiB",
        size_of::<units::Unit>(),
        mib(u),
        mib(usec),
        mib(usym),
        mib(uext),
        mib(uext_bytes),
        mib(rest),
        mib(other)
    );
}
