//! A survivor member's DWARF, for the synthesized members it gives units
//! to ([`crate::dwarf`] builds each one's compile unit, as for ELF), and
//! the debug sections that makes, written as Mach-O ones.
//!
//! The DWARF module sees a Mach-O member's code as its atoms (each atom of
//! a section that holds units one of its "sections"), its debug sections'
//! relocations as addresses (a local relocation's: its section and the
//! address the bytes hold; an external one's: its symbol's section and
//! value plus the bytes, or an undefined name), and each line sequence as
//! rows by section, from which a moved atom's make a sequence of their own
//! (clang writes one sequence per section, not per function).
//!
//! In the synthesized member, as LLVM writes a Mach-O object's DWARF: an
//! address of moved code is a local `UNSIGNED` relocation of its section,
//! the bytes its address; an offset into another debug section is the
//! offset alone (Mach-O's DWARF has no relocations between its sections);
//! an address of anything else (a name the member doesn't define) is the
//! tombstone, all ones, without a relocation.

use std::collections::BTreeMap;

use turbonop_macho_object::consts::{S_ATTR_PURE_INSTRUCTIONS, S_ATTR_SOME_INSTRUCTIONS};
use turbonop_macho_object::{Contents, Relocation, Section, name_field, reloc};

use super::{Parsed, atoms, unit_section};
use crate::dwarf::{self, Out, Sect, Source, Sym, Target};

/// A member's DWARF as [`dwarf`] reads it, and each of its units' atom.
pub(super) struct Debug<'p> {
    pub src: Source<'p>,
    /// Each unit's atom (its index among the source's sections).
    pub unit_atom: Vec<usize>,
}

type R<T> = Result<T, &'static str>;

/// Reads member `p`'s DWARF.
pub(super) fn read<'p>(p: &Parsed<'p>) -> R<Debug<'p>> {
    let o = p.obj.as_ref().ok_or("no object")?;
    if o.endian() != turbonop_core::Endian::Little {
        return Err("a big-endian object");
    }
    let cputype = o.header.cputype;
    let sections: Vec<&Section<'_>> = o.sections().collect();
    // The atoms of the sections that hold units.
    let mut by_section: Vec<Vec<(u64, u64, usize)>> = vec![Vec::new(); sections.len()];
    let mut sects: Vec<Sect> = Vec::new();
    let mut at_start: BTreeMap<(usize, u64), usize> = BTreeMap::new();
    for (si, s) in sections.iter().enumerate() {
        if !unit_section(s) {
            continue;
        }
        let code = s.flags & (S_ATTR_PURE_INSTRUCTIONS | S_ATTR_SOME_INSTRUCTIONS) != 0;
        for (range, _) in atoms(o, si, s) {
            let id = sects.len();
            sects.push(Sect {
                code,
                size: range.end - range.start,
            });
            at_start.insert((si, range.start), id);
            by_section[si].push((range.start, range.end, id));
        }
    }
    let mut owner = vec![None; sects.len()];
    let mut unit_atom = Vec::with_capacity(p.units.len());
    for (k, u) in p.units.iter().enumerate() {
        let a = u.main();
        let id = *at_start
            .get(&(a.section, a.range.start))
            .ok_or("a unit that isn't an atom")?;
        owner[id] = Some(k);
        unit_atom.push(id);
    }
    // Symbols: the object's, then one for each real section.
    let n = o.symbols.len();
    let mut symbols: Vec<Sym> = o
        .symbols
        .iter()
        .map(|y| Sym {
            global: y.is_external(),
            section: None,
            value: y.n_value,
            section_symbol: false,
        })
        .collect();
    for si in 0..sections.len() {
        symbols.push(Sym {
            global: false,
            section: Some(si),
            value: 0,
            section_symbol: true,
        });
    }
    // The debug sections, their relocations as addresses.
    let mut debug = Vec::new();
    for s in &sections {
        if s.segment_name() != b"__DWARF" {
            continue;
        }
        let bytes = s.bytes().unwrap_or(&[]);
        let mut rels = Vec::with_capacity(s.relocations.len());
        for r in &s.relocations {
            let size: u8 = match r.length {
                2 => 4,
                3 => 8,
                _ => return Err("a debug relocation of a size not known"),
            };
            let plain = r.value.is_none()
                && !r.pcrel
                && r.kind
                    == if cputype == turbonop_macho_object::consts::CPU_TYPE_X86_64 {
                        reloc::x86_64::UNSIGNED
                    } else {
                        reloc::arm64::UNSIGNED
                    };
            if !plain {
                return Err("a debug relocation not known");
            }
            let raw = bytes
                .get(r.address as usize..r.address as usize + usize::from(size))
                .map(|b| {
                    let mut v = [0u8; 8];
                    v[..b.len()].copy_from_slice(b);
                    u64::from_le_bytes(v)
                })
                .ok_or("a debug relocation past its section")?;
            let (sym, addend) = if r.external {
                let y = o
                    .symbols
                    .get(r.symbolnum as usize)
                    .ok_or("a debug relocation naming no symbol")?;
                if y.in_section() {
                    let t = usize::from(y.n_sect) - 1;
                    let base = sections.get(t).ok_or("no section")?.addr;
                    (
                        (n + t) as u32,
                        y.n_value.wrapping_sub(base).wrapping_add(raw) as i64,
                    )
                } else {
                    (r.symbolnum, raw as i64)
                }
            } else {
                let t = (r.symbolnum as usize)
                    .checked_sub(1)
                    .ok_or("a debug relocation naming no section")?;
                let ts = sections.get(t).ok_or("no section")?;
                if ts.segment_name() == b"__DWARF" {
                    return Err("a debug relocation to a debug section");
                }
                ((n + t) as u32, raw.wrapping_sub(ts.addr) as i64)
            };
            rels.push((u64::from(r.address), size, sym, addend));
        }
        debug.push((s.name(), bytes.to_vec(), rels));
    }
    let src = Source::macho(debug, symbols, by_section, sects, &owner)?;
    Ok(Debug { src, unit_atom })
}

/// A debug section's references to fill in: (offset, size, target,
/// addend).
pub(super) type Refs = Vec<(u64, usize, Target, i64)>;

/// The debug sections of `out`, to add to a synthesized member: each
/// section, and its references (offset, size, target, addend) to fill in
/// once the member's sections have addresses ([`fill`]). `like` gives a
/// section of the survivor member by name, whose flags and alignment a
/// section copies.
#[allow(clippy::type_complexity)]
pub(super) fn sections(
    out: &Out,
    like: &dyn Fn(&[u8]) -> Option<(u32, u32)>,
) -> Vec<(Section<'static>, Refs)> {
    let mut v = Vec::new();
    for s in &out.sections {
        if s.bytes.is_empty() && s.rels.is_empty() {
            continue;
        }
        let name: Vec<u8> = [b"__".as_slice(), &s.name[1..]].concat();
        let name = &name[..name.len().min(16)];
        let (align, flags) = like(name).unwrap_or((0, turbonop_macho_object::consts::S_ATTR_DEBUG));
        let refs = s
            .rels
            .iter()
            .map(|r| {
                (
                    r.offset,
                    if r.rtype == 10 { 4 } else { 8 },
                    r.target.clone(),
                    r.addend,
                )
            })
            .collect();
        v.push((
            Section {
                sectname: name_field(name),
                segname: name_field(b"__DWARF"),
                addr: 0,
                align,
                flags,
                reserved1: 0,
                reserved2: 0,
                reserved3: 0,
                contents: Contents::Bytes(std::borrow::Cow::Owned(s.bytes.clone())),
                relocations: Vec::new(),
            },
            refs,
        ));
    }
    v
}

/// Fills a debug section's references in: `debug` gives each debug
/// section of `out`'s place among the member's sections (`usize::MAX`: not
/// written), `atom` an atom's section and address in the member. The
/// section's relocations, and how many addresses were written as the
/// tombstone.
pub(super) fn fill(
    bytes: &mut [u8],
    refs: &[(u64, usize, Target, i64)],
    atom: &dyn Fn(usize) -> Option<(usize, u64)>,
) -> R<(Vec<Relocation>, usize)> {
    let mut rels = Vec::new();
    let mut tombstoned = 0;
    for &(at, n, ref t, addend) in refs {
        let at = at as usize;
        let place = bytes
            .get_mut(at..at + n)
            .ok_or("a debug reference past its section")?;
        let v = match *t {
            Target::Debug(_) => addend as u64,
            Target::Section(id) => match atom(id) {
                Some((sec, a)) => {
                    rels.push(Relocation::plain(
                        at as u32,
                        sec as u32 + 1,
                        false,
                        0,
                        if n == 4 { 2 } else { 3 },
                    ));
                    a.wrapping_add(addend as u64)
                }
                None => {
                    tombstoned += 1;
                    u64::MAX
                }
            },
            Target::Global(_) => {
                tombstoned += 1;
                u64::MAX
            }
        };
        place.copy_from_slice(&v.to_le_bytes()[..n]);
    }
    rels.sort_by_key(|r| core::cmp::Reverse(r.address));
    Ok((rels, tombstoned))
}

/// Builds the DWARF of a synthesized member holding `units` of `d`'s
/// member (unit indexes); `supplied`, the classes its givers define (see
/// [`dwarf::build`]).
pub(super) fn build(
    d: &Debug<'_>,
    units: &[usize],
    supplied: Option<&turbonop_hash::Set<u128>>,
) -> R<Option<Out>> {
    let moved = units.iter().map(|&k| d.unit_atom[k]).collect();
    dwarf::build(&d.src, &moved, supplied)
}
