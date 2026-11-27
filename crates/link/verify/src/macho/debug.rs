//! A rewritten member's DWARF against its original's, where folding
//! changes it (see `turbonop_fold::macho`): the givers' line programs lose
//! the rows of the code they lost and the rest move with it, list entries
//! move with what they name (those of lost code become empty), the unit's
//! `DW_AT_high_pc` (a length) shrinks, and a debug relocation into a lost
//! atom goes, its place all ones. Everything is compared by what each
//! address names (an atom's key and the offset in it, or a section's end),
//! read with `turbonop_dwarf`'s line reader, apart from the engine's.

use std::collections::BTreeSet;
use std::ops::Range;

use turbonop_dwarf::consts::{DW_AT_HIGH_PC, DW_AT_LOW_PC, DW_AT_STMT_LIST};
use turbonop_dwarf::{Abbrevs, Sections, Unit, Value};
use turbonop_macho_object::{Object, Section};

use super::{atom_keys, split};
use crate::report::{Report, show};

/// What an object's addresses name: atoms by key, and sections' ends.
pub(super) struct Locator {
    atoms: Vec<Vec<(Range<u64>, Vec<usize>)>>,
    keys: Vec<Vec<Vec<u8>>>,
    /// Each section's address, size, and name.
    sections: Vec<(u64, u64, Vec<u8>)>,
}

/// A place an address names.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub(super) enum Place {
    /// An atom (its key), and the offset in it.
    Atom(Vec<u8>, u64),
    /// A section's end (its name).
    End(Vec<u8>),
    /// In no section.
    Nowhere(u64),
}

impl Locator {
    pub(super) fn new(o: &Object<'_>) -> Self {
        let atoms = split(o);
        let keys = atom_keys(o, &atoms);
        let sections = o
            .sections()
            .map(|s: &Section<'_>| {
                (
                    s.addr,
                    s.size(),
                    [s.segment_name(), b",", s.name()].concat(),
                )
            })
            .collect();
        Locator {
            atoms,
            keys,
            sections,
        }
    }

    /// What address `a` names: `hint`, a section it is taken as (one that
    /// a relocation names), or the section that holds it (one that ends at
    /// it if none holds it).
    pub(super) fn place(&self, a: u64, hint: Option<usize>) -> Place {
        let si = hint.filter(|&h| h < self.sections.len()).or_else(|| {
            self.sections
                .iter()
                .position(|&(lo, size, _)| a >= lo && a - lo < size)
                .or_else(|| {
                    self.sections
                        .iter()
                        .position(|&(lo, size, _)| a >= lo && a - lo == size)
                })
        });
        let Some(si) = si else {
            return Place::Nowhere(a);
        };
        let (lo, size, ref name) = self.sections[si];
        let Some(off) = a.checked_sub(lo) else {
            return Place::Nowhere(a);
        };
        if off == size {
            return Place::End(name.clone());
        }
        let list = &self.atoms[si];
        let k = list.partition_point(|(r, _)| r.start <= off);
        match k.checked_sub(1) {
            Some(k) if off < list[k].0.end => {
                Place::Atom(self.keys[si][k].clone(), off - list[k].0.start)
            }
            _ => Place::Nowhere(a),
        }
    }
}

impl Locator {
    /// Where `p` is once the atoms `lost` are gone: in a lost atom, the
    /// start of the next atom kept (or its section's end).
    pub(super) fn kept(&self, p: Place, lost: &BTreeSet<&[u8]>) -> Place {
        let Place::Atom(k, _) = &p else { return p };
        if !lost.contains(k.as_slice()) {
            return p;
        }
        for (si, keys) in self.keys.iter().enumerate() {
            if let Some(j) = keys.iter().position(|x| x == k) {
                return keys[j + 1..]
                    .iter()
                    .find(|x| !lost.contains(x.as_slice()))
                    .map_or_else(
                        || Place::End(self.sections[si].2.clone()),
                        |x| Place::Atom(x.clone(), 0),
                    );
            }
        }
        p
    }
}

fn read_le(b: &[u8], at: usize, n: usize) -> Option<u64> {
    let s = b.get(at..at.checked_add(n)?)?;
    let mut v = [0u8; 8];
    v[..n].copy_from_slice(s);
    Some(u64::from_le_bytes(v))
}

/// The DWARF of an object: its sections, and its unit's `DW_AT_low_pc`
/// (place and value), `DW_AT_high_pc` (a length: place, size, value), and
/// line table offset.
struct Dwarf<'a> {
    s: Sections<'a>,
    low: Option<(usize, usize, u64)>,
    high: Option<(usize, usize, u64)>,
    stmt: Option<u64>,
    address_size: u8,
}

fn dwarf<'a>(o: &'a Object<'a>) -> Option<Dwarf<'a>> {
    let mut s = Sections::new(turbonop_core::Endian::Little);
    for x in o.sections() {
        if x.segment_name() == b"__DWARF"
            && let Some(n) = x.name().strip_prefix(b"__debug_")
        {
            s.set(n, x.bytes().unwrap_or(&[]));
        }
    }
    let u = Unit::iter(s, turbonop_dwarf::UnitSection::Info, 0).next()?;
    let abbrevs = Abbrevs::parse(s.abbrev, u.abbrev_offset, s.endian)?;
    let mut r = turbonop_dwarf::Reader::new(s.info, u.dies as usize, s.endian);
    let code = r.uleb()?;
    let a = abbrevs.get(code)?;
    let mut d = Dwarf {
        s,
        low: None,
        high: None,
        stmt: None,
        address_size: u.address_size,
    };
    for spec in &a.attrs {
        let pos = r.pos;
        let v = u.read_value(&mut r, spec.form, spec.implicit)?;
        let n = r.pos - pos;
        match (spec.name, v) {
            (DW_AT_LOW_PC, Value::Addr(x)) => d.low = Some((pos, n, x)),
            (DW_AT_HIGH_PC, Value::Addr(_)) => {}
            (DW_AT_HIGH_PC, v) => d.high = u.uint(&v).map(|x| (pos, n, x)),
            (DW_AT_STMT_LIST, v) => d.stmt = u.uint(&v),
            _ => {}
        }
    }
    Some(d)
}

/// The rows of a line program, each address by what it names (in the
/// section its sequence's relocation names, if any).
fn rows(o: &Object<'_>, d: &Dwarf<'_>, loc: &Locator) -> Option<Vec<(Place, String)>> {
    let p = turbonop_dwarf::line_program(&d.s, d.stmt?, d.address_size)?;
    // The sequences' sections, by their `DW_LNE_set_address` relocations
    // (each sequence's section, in order).
    let line = o
        .sections()
        .find(|x| x.segment_name() == b"__DWARF" && x.name() == b"__debug_line")?;
    let mut secs: Vec<Option<usize>> = line
        .relocations
        .iter()
        .map(|r| {
            (
                r.address,
                (!r.external)
                    .then(|| (r.symbolnum as usize).checked_sub(1))
                    .flatten(),
            )
        })
        .collect::<std::collections::BTreeMap<u32, Option<usize>>>()
        .into_values()
        .collect();
    secs.reverse();
    let mut out = Vec::with_capacity(p.rows.len());
    let mut hint = secs.pop().flatten();
    for r in &p.rows {
        // (A sequence's end says nothing but where it is.)
        if r.end_sequence {
            out.push((loc.place(r.address, hint), "end".to_string()));
            hint = secs.pop().flatten();
            continue;
        }
        out.push((
            loc.place(r.address, hint),
            format!(
                "{} {} {} {}{}{}{} {}",
                r.file,
                r.line,
                r.column,
                u8::from(r.is_stmt),
                u8::from(r.end_sequence),
                u8::from(r.prologue_end),
                u8::from(r.epilogue_begin),
                r.discriminator
            ),
        ));
        if r.end_sequence {
            hint = secs.pop().flatten();
        }
    }
    Some(out)
}

/// The ranges of DWARF 4's list section `name` (each entry's place and the
/// places it names, or that it is empty), and its other bytes (list ends,
/// expressions).
#[allow(clippy::type_complexity)]
fn lists(
    o: &Object<'_>,
    d: &Dwarf<'_>,
    loc: &Locator,
    name: &[u8],
) -> Option<(Vec<(usize, Option<(Place, Place)>)>, Vec<u8>)> {
    let x = o
        .sections()
        .find(|x| x.segment_name() == b"__DWARF" && x.name() == name)?;
    let b = x.bytes()?;
    let rel: std::collections::BTreeMap<u32, usize> = x
        .relocations
        .iter()
        .filter(|r| !r.external)
        .map(|r| (r.address, r.symbolnum as usize - 1))
        .collect();
    let n = usize::from(d.address_size);
    let max = if n == 8 {
        u64::MAX
    } else {
        u64::from(u32::MAX)
    };
    let loc_list = name == b"__debug_loc";
    let base0 = d.low.map_or(0, |l| l.2);
    let mut out = Vec::new();
    let mut rest = Vec::new();
    let mut at = 0;
    let mut base = base0;
    while at + 2 * n <= b.len() {
        let s = read_le(b, at, n)?;
        let e = read_le(b, at + n, n)?;
        if s == 0 && e == 0 {
            rest.extend_from_slice(&b[at..at + 2 * n]);
            at += 2 * n;
            base = base0;
            continue;
        }
        if s == max {
            out.push((
                at,
                Some((
                    loc.place(e, rel.get(&((at + n) as u32)).copied()),
                    Place::Nowhere(0),
                )),
            ));
            base = e;
            at += 2 * n;
            continue;
        }
        let hint = rel.get(&(at as u32)).copied();
        let b0 = if hint.is_some() { 0 } else { base };
        let (a, z) = (b0.wrapping_add(s), b0.wrapping_add(e));
        let entry = if a == z {
            None
        } else {
            let pa = loc.place(a, hint);
            let hint = hint.or(match &pa {
                Place::Atom(..) => loc
                    .sections
                    .iter()
                    .position(|&(lo, size, _)| a >= lo && a - lo < size),
                _ => None,
            });
            Some((pa, loc.place(z, hint)))
        };
        out.push((at, entry));
        at += 2 * n;
        if loc_list {
            let len = read_le(b, at, 2)? as usize;
            rest.extend_from_slice(b.get(at..at + 2 + len)?);
            at += 2 + len;
        }
    }
    Some((out, rest))
}

/// The entries of the DWARF 4 list at `offset` of section `name` that hold
/// something: each range by the places its ends name (taken with
/// [`Locator::kept`] when `lost` is given, and left out if that empties
/// it), and a location list entry's expression. `base` is the unit's
/// `DW_AT_low_pc`.
pub(super) fn list_at(
    o: &Object<'_>,
    loc: &Locator,
    name: &[u8],
    offset: u64,
    address_size: u8,
    base: u64,
    lost: Option<&BTreeSet<&[u8]>>,
) -> Option<Vec<(Place, Place, Vec<u8>)>> {
    let x = o
        .sections()
        .find(|x| x.segment_name() == b"__DWARF" && x.name() == name)?;
    let b = x.bytes()?;
    let rel: std::collections::BTreeMap<u32, usize> = x
        .relocations
        .iter()
        .filter(|r| !r.external)
        .map(|r| (r.address, r.symbolnum as usize - 1))
        .collect();
    let n = usize::from(address_size);
    let max = if n == 8 {
        u64::MAX
    } else {
        u64::from(u32::MAX)
    };
    let loc_list = name == b"__debug_loc";
    let mut out = Vec::new();
    let mut at = usize::try_from(offset).ok()?;
    let mut cur = base;
    loop {
        let entry = at;
        let s = read_le(b, at, n)?;
        let e = read_le(b, at + n, n)?;
        at += 2 * n;
        if s == 0 && e == 0 {
            return Some(out);
        }
        if s == max {
            cur = e;
            continue;
        }
        let expr = if loc_list {
            let len = read_le(b, at, 2)? as usize;
            let x = b.get(at + 2..at + 2 + len)?.to_vec();
            at += 2 + len;
            x
        } else {
            Vec::new()
        };
        let hint = rel.get(&(entry as u32)).copied();
        let b0 = if hint.is_some() { 0 } else { cur };
        let (a, z) = (b0.wrapping_add(s), b0.wrapping_add(e));
        if a == z {
            continue;
        }
        let pa = loc.place(a, hint);
        let hz = hint.or(match &pa {
            Place::Atom(..) => loc
                .sections
                .iter()
                .position(|&(lo, size, _)| a >= lo && a - lo < size),
            _ => None,
        });
        let pz = loc.place(z, hz);
        let (pa, pz) = match lost {
            Some(l) => (loc.kept(pa, l), loc.kept(pz, l)),
            None => (pa, pz),
        };
        if pa != pz {
            out.push((pa, pz, expr));
        }
    }
}

/// Checks a rewritten member's line program, lists (by place, unless its
/// DWARF was pruned: then each kept DIE's lists are compared by the
/// `pruned` module), and unit length against the original's (`lost`: the
/// keys of the atoms it lost). The places in `__debug_info` the caller
/// should leave out of its byte comparison (the unit's `DW_AT_high_pc`).
pub(super) fn check(
    o: &Object<'_>,
    n: &Object<'_>,
    lost: &BTreeSet<&[u8]>,
    pruned: bool,
    what: &[u8],
    rep: &mut Report,
) -> Vec<Range<usize>> {
    let (lo, ln) = (Locator::new(o), Locator::new(n));
    let (Some(dold), Some(dnew)) = (dwarf(o), dwarf(n)) else {
        return Vec::new();
    };
    let gone = |p: &Place| matches!(p, Place::Atom(k, _) if lost.contains(k.as_slice()));
    // Line rows: the original's but the lost atoms', each naming the same.
    if let (Some(ro), Some(rn)) = (rows(o, &dold, &lo), rows(n, &dnew, &ln)) {
        // (A sequence's end in lost code is where what follows it is now.)
        let expect: Vec<(Place, String)> = ro
            .iter()
            .filter(|r| r.1 == "end" || !gone(&r.0))
            .map(|r| {
                if r.1 == "end" {
                    (lo.kept(r.0.clone(), lost), r.1.clone())
                } else {
                    r.clone()
                }
            })
            .collect();
        let same = expect.len() == rn.len() && expect.iter().zip(&rn).all(|(a, b)| *a == *b);
        rep.checked("line rows of rewritten members", rn.len() as u64);
        if !same {
            rep.fail(
                "line rows that aren't the original's but the lost code's",
                || {
                    let k = expect
                        .iter()
                        .zip(&rn)
                        .position(|(a, b)| *a != *b)
                        .unwrap_or(expect.len().min(rn.len()));
                    format!(
                        "{}: row {k}: {:?} and {:?}",
                        show(what),
                        expect.get(k),
                        rn.get(k)
                    )
                },
            );
        }
    } else if dold.stmt.is_some() {
        rep.fail("line tables that don't read", || show(what));
    }
    // Lists.
    for name in [&b"__debug_ranges"[..], b"__debug_loc"] {
        if pruned {
            break;
        }
        let (Some((eo, bo)), Some((en, bn))) =
            (lists(o, &dold, &lo, name), lists(n, &dnew, &ln, name))
        else {
            continue;
        };
        rep.checked("list entries of rewritten members", en.len() as u64);
        let ok = bo == bn
            && eo.len() == en.len()
            && eo.iter().zip(&en).all(|(a, b)| {
                a.0 == b.0
                    && match (&a.1, &b.1) {
                        // (What the original's range holds that stays.)
                        (Some((s, z)), y) => {
                            let (s, z) = (lo.kept(s.clone(), lost), lo.kept(z.clone(), lost));
                            match y {
                                None => s == z,
                                Some(y) => (s, z) == *y,
                            }
                        }
                        (x, y) => x == y,
                    }
            });
        if !ok {
            rep.fail(
                "list entries that don't name what the original's do (or aren't empty for lost code)",
                || format!("{}: {}", show(what), show(name)),
            );
        }
    }
    // The unit's length: what its range holds now.
    let mut skip = Vec::new();
    if let Some((at, size, _)) = dnew.low {
        skip.push(at..at + size);
    }
    if let (Some((at, size, len)), Some((at2, size2, len2))) = (dold.high, dnew.high) {
        skip.push(at2..at2 + size2);
        let low = dold.low.map_or(0, |l| l.2);
        let new_low = dnew.low.map_or(0, |l| l.2);
        // The original's range, in the new object: from where its start is
        // to where its end is.
        // (The end in the start's section: the next one may start there.)
        let si = lo
            .sections
            .iter()
            .position(|&(a, size, _)| low >= a && low - a < size);
        let pa = lo.place(low, si);
        let pz = lo.place(low.wrapping_add(len), si);
        let find = |p: &Place| -> Option<u64> {
            match p {
                Place::End(name) => ln.sections.iter().find(|s| s.2 == *name).map(|s| s.0 + s.1),
                Place::Atom(k, off) => {
                    for (si, keys) in ln.keys.iter().enumerate() {
                        if let Some(j) = keys.iter().position(|x| x == k) {
                            return Some(ln.sections[si].0 + ln.atoms[si][j].0.start + off);
                        }
                    }
                    None
                }
                Place::Nowhere(a) => Some(*a),
            }
        };
        let expected = match (find(&pa), find(&pz)) {
            (Some(a), Some(z)) => Some(z.wrapping_sub(a)),
            // (The range starts in lost code: from the new start.)
            (None, Some(z)) => Some(z.wrapping_sub(new_low)),
            _ => None,
        };
        let _ = (at, size);
        if expected.is_some_and(|e| e != len2) {
            rep.fail(
                "units' lengths (DW_AT_high_pc) that don't cover what's left",
                || {
                    format!(
                        "{}: {len2:#x}, not {:#x}",
                        show(what),
                        expected.unwrap_or(0)
                    )
                },
            );
        }
    }
    skip
}
