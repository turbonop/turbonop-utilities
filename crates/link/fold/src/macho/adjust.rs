//! A giver's DWARF after its units are cut out: what holds addresses that
//! no relocation does, which [`Object::cut`] can't move.
//!
//! LLVM writes a Mach-O object's DWARF with local relocations only where a
//! DIE (or a line sequence's `DW_LNE_set_address`) names an address; the
//! rest is relative, resolved by the assembler: a line program's address
//! advances, DWARF 4's range and location list entries (offsets from the
//! unit's base address, `DW_AT_low_pc`, or from a base address selection
//! entry), and the unit's `DW_AT_high_pc` (its length). When code is cut
//! out of a section, everything after the cut moves back, and these must
//! move with it:
//!
//! - **The line program** is decoded into rows (old addresses), the rows
//!   of code cut out dropped, the rest moved, and encoded again as LLVM
//!   encodes it ([`crate::dwarf::line`]); each sequence's
//!   `DW_LNE_set_address` keeps its local relocation, with the new address.
//! - **List entries**, each end of each range, move to where their
//!   addresses went; a range of code cut out becomes empty (`1, 1` if it
//!   would be `0, 0`, which ends a list). Base address selection entries
//!   move too.
//! - **The unit's `DW_AT_high_pc`** (a length) shrinks by the bytes cut out
//!   of its range.
//!
//! Handled: one compile unit of DWARF 2 to 4 (`__debug_line`,
//! `__debug_ranges`, `__debug_loc`), no relocations in the list sections,
//! no `__debug_aranges`. Anything else is refused, and the member is kept
//! whole (its copies are then still held by synthesized members too, which
//! is allowed: they are identical weak definitions).

use std::collections::BTreeMap;
use std::ops::Range;

use turbonop_dwarf::consts::{DW_AT_HIGH_PC, DW_AT_LOW_PC, DW_AT_STMT_LIST};
use turbonop_dwarf::{Abbrevs, Sections, Unit, Value};
use turbonop_macho_object::{Contents, Object, Relocation};

use crate::dwarf::line::{self, Row, Table};

type R<T> = Result<T, &'static str>;

/// Where each old address of a cut object went.
pub(super) struct AddressMap {
    /// Each section's old address and size, and new address.
    pub old: Vec<(u64, u64)>,
    pub new: Vec<u64>,
    /// Each section's cut ranges (offsets), sorted.
    pub cuts: Vec<Vec<Range<u64>>>,
}

impl AddressMap {
    /// The section holding old address `a` (one that ends at `a` if none
    /// holds it).
    fn section_of(&self, a: u64) -> Option<usize> {
        self.old
            .iter()
            .position(|&(lo, size)| a >= lo && a - lo < size)
            .or_else(|| {
                self.old
                    .iter()
                    .position(|&(lo, size)| a >= lo && a - lo == size)
            })
    }

    /// Whether old address `a` of section `s` was cut out.
    fn cut_out(&self, s: usize, a: u64) -> bool {
        let off = a.wrapping_sub(self.old[s].0);
        self.cuts[s].iter().any(|r| r.contains(&off))
    }

    /// Where old address `a` (taken as section `s`'s) is now: an address
    /// that was cut out is where its cut was.
    fn place(&self, s: usize, a: u64) -> u64 {
        let off = a.wrapping_sub(self.old[s].0);
        let mut removed = 0;
        for r in &self.cuts[s] {
            if off >= r.end {
                removed += r.end - r.start;
            } else if off > r.start {
                removed += off - r.start;
            }
        }
        self.new[s] + off - removed
    }

    /// [`place`](Self::place) for an address of no known section: its own
    /// section's, or unchanged.
    fn place_any(&self, a: u64) -> u64 {
        match self.section_of(a) {
            Some(s) => self.place(s, a),
            None => a,
        }
    }
}

/// A field of a list entry.
#[derive(Clone, Copy, Debug)]
enum Field {
    /// A range's start and end (relative to `base`, an old address), at
    /// `at` (two fields of `size` bytes).
    Range {
        at: usize,
        start: u64,
        end: u64,
        base: Base,
        /// The section its start's relocation names, if it has one (then
        /// its end has one too, and the base is 0).
        sec: Option<usize>,
    },
    /// A base address selection entry's address, at `at`.
    Select { at: usize, address: u64 },
}

/// What a list entry's addresses are relative to.
#[derive(Clone, Copy, Debug)]
enum Base {
    /// The unit's `DW_AT_low_pc`.
    Unit,
    /// A base address selection entry's (an old address).
    Selected(u64),
}

/// `__debug_line`'s index, its table, and its sequences (each with its
/// section, if known).
type LinePlan = (usize, Table, Vec<(line::Sequence, Option<usize>)>);

/// What is to be adjusted, read before the cut.
pub(super) struct Plan {
    info: usize,
    /// The unit's `DW_AT_low_pc`: its place in `__debug_info` and size, and
    /// its old value; its `DW_AT_high_pc` (a length): place, size, value.
    low_pc: Option<(usize, usize, u64)>,
    high_pc: Option<(usize, usize, u64)>,
    /// `__debug_line`: its index, its table, and the table's sequences
    /// (each with its section, if known).
    line: Option<LinePlan>,
    /// List sections: their index, address size, and fields.
    lists: Vec<(usize, u8, Vec<Field>)>,
    /// The relocations of the list sections and of `__debug_line`, taken
    /// out before the cut (which can't move what they hold, or would take
    /// the end of a range that ends where a cut starts for the start of
    /// what was cut) and put back by [`Plan::apply`]: (section, relocations).
    stripped: Vec<(usize, Vec<Relocation>)>,
    /// The unit's `DW_AT_low_pc` relocation, taken out the same way: it
    /// names its section's start (the base of the unit's lists), not the
    /// atom there, which may go.
    low_reloc: Option<Relocation>,
}

fn read_le(b: &[u8], at: usize, n: usize) -> Option<u64> {
    let s = b.get(at..at.checked_add(n)?)?;
    let mut v = [0u8; 8];
    v[..n].copy_from_slice(s);
    Some(u64::from_le_bytes(v))
}

fn write_le(b: &mut [u8], at: usize, n: usize, v: u64) {
    b[at..at + n].copy_from_slice(&v.to_le_bytes()[..n]);
}

impl Plan {
    /// Reads `o`'s DWARF (before anything is cut). `None`: there is none.
    pub(super) fn read(o: &Object<'_>) -> R<Option<Plan>> {
        if o.endian() != turbonop_core::Endian::Little {
            return Err("big-endian DWARF");
        }
        let mut by_name: BTreeMap<&[u8], usize> = BTreeMap::new();
        for (i, s) in o.sections().enumerate() {
            if s.segment_name() == b"__DWARF" {
                by_name.insert(s.name(), i);
            }
        }
        let Some(&info) = by_name.get(&b"__debug_info"[..]) else {
            return Ok(None);
        };
        for name in [
            &b"__debug_aranges"[..],
            b"__debug_rnglists",
            b"__debug_loclists",
            b"__debug_addr",
        ] {
            if by_name.contains_key(name) {
                return Err("DWARF sections whose addresses aren't moved");
            }
        }
        let sec = |name: &[u8]| -> &[u8] {
            by_name
                .get(name)
                .and_then(|&i| o.section(i))
                .and_then(|s| s.bytes())
                .unwrap_or(&[])
        };
        let endian = turbonop_core::Endian::Little;
        let mut d = Sections::new(endian);
        d.info = sec(b"__debug_info");
        d.abbrev = sec(b"__debug_abbrev");
        d.str = sec(b"__debug_str");
        d.line = sec(b"__debug_line");
        d.ranges = sec(b"__debug_ranges");
        d.loc = sec(b"__debug_loc");
        let units: Vec<Unit<'_>> = Unit::iter(d, turbonop_dwarf::UnitSection::Info, 0).collect();
        let [u] = units.as_slice() else {
            return Err(if units.is_empty() {
                "a __debug_info that doesn't read"
            } else {
                "more than one compile unit"
            });
        };
        if u.dwarf64 || u.version >= 5 {
            return Err("64-bit DWARF or DWARF 5");
        }
        if u.end as usize != d.info.len() {
            return Err("a __debug_info with more than its unit");
        }
        let abbrevs = Abbrevs::parse(d.abbrev, u.abbrev_offset, endian)
            .ok_or("an abbreviation table that doesn't read")?;
        let (_, top) = crate::dwarf::read_die(u, &abbrevs, u.dies)?;
        let mut plan = Plan {
            info,
            low_pc: None,
            high_pc: None,
            line: None,
            lists: Vec::new(),
            stripped: Vec::new(),
            low_reloc: None,
        };
        let mut stmt = None;
        for a in &top {
            let size = (a.end - a.pos) as usize;
            match (a.name, a.value) {
                (DW_AT_LOW_PC, Value::Addr(v)) => plan.low_pc = Some((a.pos as usize, size, v)),
                (DW_AT_HIGH_PC, Value::Addr(_)) => {} // (An address: relocated.)
                (DW_AT_HIGH_PC, v) => {
                    let len = u.uint(&v).ok_or("a DW_AT_high_pc of a form not known")?;
                    plan.high_pc = Some((a.pos as usize, size, len));
                }
                (DW_AT_STMT_LIST, v) => stmt = u.uint(&v),
                _ => {}
            }
        }
        let base = plan.low_pc.map_or(0, |l| l.2);
        let sections: Vec<_> = o.sections().collect();
        if let Some((at, _, _)) = plan.low_pc {
            plan.low_reloc = sections[info]
                .relocations
                .iter()
                .find(|r| r.address as usize == at)
                .copied();
            if plan
                .low_reloc
                .is_some_and(|r| r.external || r.value.is_some() || r.symbolnum == 0)
            {
                return Err("a DW_AT_low_pc relocation not known");
            }
        }
        // The line table: one, the whole section.
        if let (Some(off), Some(&li)) = (stmt, by_name.get(&b"__debug_line"[..])) {
            let bytes = d.line;
            let t = line::table(bytes, off as usize, u.address_size, endian)?;
            if t.range != (0..bytes.len()) {
                return Err("a __debug_line with more than its unit's table");
            }
            let rels: BTreeMap<u32, Relocation> = sections[li]
                .relocations
                .iter()
                .map(|r| (r.address, *r))
                .collect();
            let read = |at: usize, n: u8| read_le(bytes, at, usize::from(n));
            let seqs = line::decode(bytes, &t, endian, &read)?;
            let mut out = Vec::with_capacity(seqs.len());
            for s in seqs {
                let section = match s.set_address.and_then(|a| rels.get(&(a as u32))) {
                    Some(r) if !r.external && r.value.is_none() && r.symbolnum > 0 => {
                        Some(r.symbolnum as usize - 1)
                    }
                    Some(_) => return Err("a line sequence's relocation not known"),
                    None => None,
                };
                out.push((s, section));
            }
            plan.stripped.push((li, Vec::new()));
            plan.line = Some((li, t, out));
        }
        // The lists (DWARF 4's, every entry relative to a base).
        for name in [&b"__debug_ranges"[..], b"__debug_loc"] {
            let Some(&li) = by_name.get(name) else {
                continue;
            };
            // Each relocated field's section (plain local addresses).
            let mut rel_sec: BTreeMap<usize, usize> = BTreeMap::new();
            for r in &sections[li].relocations {
                if r.external
                    || r.value.is_some()
                    || r.pcrel
                    || r.kind != 0
                    || r.symbolnum == 0
                    || 1usize << r.length != usize::from(u.address_size)
                {
                    return Err("a list section's relocation not known");
                }
                rel_sec.insert(r.address as usize, r.symbolnum as usize - 1);
            }
            if !rel_sec.is_empty() && base != 0 {
                return Err("relocated list entries relative to a base");
            }
            plan.stripped.push((li, sections[li].relocations.clone()));
            let loc = name == b"__debug_loc";
            let b = sections[li].bytes().unwrap_or(&[]);
            let n = usize::from(u.address_size);
            let max = if n == 8 {
                u64::MAX
            } else {
                u64::from(u32::MAX)
            };
            let mut fields = Vec::new();
            let mut at = 0;
            let mut cur = Base::Unit;
            let bad = "a list that doesn't read";
            while at < b.len() {
                let start = read_le(b, at, n).ok_or(bad)?;
                let end = read_le(b, at + n, n).ok_or(bad)?;
                if start == 0 && end == 0 {
                    at += 2 * n;
                    cur = Base::Unit;
                    continue;
                }
                if start == max {
                    fields.push(Field::Select {
                        at: at + n,
                        address: end,
                    });
                    cur = Base::Selected(end);
                    at += 2 * n;
                    continue;
                }
                fields.push(Field::Range {
                    at,
                    start,
                    end,
                    base: cur,
                    sec: rel_sec.get(&at).copied(),
                });
                at += 2 * n;
                if loc {
                    let len = read_le(b, at, 2).ok_or(bad)? as usize;
                    at += 2 + len;
                }
            }
            if at != b.len() {
                return Err(bad);
            }
            plan.lists.push((li, u.address_size, fields));
        }
        Ok(Some(plan))
    }

    /// Takes out the relocations [`Plan::apply`] puts back (see
    /// `stripped`).
    pub(super) fn strip(&self, o: &mut Object<'_>) {
        for &(si, _) in &self.stripped {
            if let Some(s) = o.sections_mut().nth(si) {
                s.relocations.clear();
            }
        }
        if let Some(r) = self.low_reloc
            && let Some(s) = o.sections_mut().nth(self.info)
        {
            s.relocations.retain(|x| *x != r);
        }
    }

    /// Adjusts `o` (cut, its addresses mapped by `map`). How many list
    /// entries became empty.
    pub(super) fn apply(&self, o: &mut Object<'_>, map: &AddressMap) -> R<usize> {
        let mut emptied = 0;
        // The unit's base: where its section's start (or whatever it names)
        // is now, its relocation put back.
        let old_base = self.low_pc.map_or(0, |l| l.2);
        let new_base = match (self.low_pc, self.low_reloc) {
            (Some((at, n, _)), Some(r)) => {
                let b = map.place(r.symbolnum as usize - 1, old_base);
                let s = o.sections_mut().nth(self.info).ok_or("no __debug_info")?;
                if let Contents::Bytes(x) = &mut s.contents {
                    write_le(x.to_mut(), at, n, b);
                }
                s.relocations.push(r);
                s.relocations.sort_by_key(|x| core::cmp::Reverse(x.address));
                b
            }
            _ => old_base,
        };
        // DW_AT_high_pc.
        if let Some((at, n, len)) = self.high_pc {
            let s = map.section_of(old_base);
            let (lo, hi) = match s {
                Some(s) => (
                    map.place(s, old_base),
                    map.place(s, old_base.wrapping_add(len)),
                ),
                None => (old_base, old_base.wrapping_add(len)),
            };
            if let Some(Contents::Bytes(b)) =
                o.sections_mut().nth(self.info).map(|s| &mut s.contents)
            {
                write_le(b.to_mut(), at, n, hi.wrapping_sub(lo));
            }
        }
        // The lists.
        for (li, size, fields) in &self.lists {
            let n = usize::from(*size);
            let Some(Contents::Bytes(b)) = o.sections_mut().nth(*li).map(|s| &mut s.contents)
            else {
                continue;
            };
            let b = b.to_mut();
            for f in fields {
                match *f {
                    Field::Select { at, address } => {
                        write_le(b, at, n, map.place_any(address));
                    }
                    Field::Range {
                        at,
                        start,
                        end,
                        base,
                        sec,
                    } => {
                        let (old, new) = match (base, sec) {
                            // (Relocated: addresses, as the base is 0.)
                            (_, Some(_)) => (0, 0),
                            (Base::Unit, None) => (old_base, new_base),
                            (Base::Selected(a), None) => (a, map.place_any(a)),
                        };
                        let a = old.wrapping_add(start);
                        let e = old.wrapping_add(end);
                        let (na, ne) = match sec.or_else(|| map.section_of(a)) {
                            Some(s) => (map.place(s, a), map.place(s, e)),
                            None => (a, e),
                        };
                        let (mut x, mut y) = (na.wrapping_sub(new), ne.wrapping_sub(new));
                        if na == ne && a != e {
                            emptied += 1;
                        }
                        if x == 0 && y == 0 {
                            (x, y) = (1, 1);
                        }
                        write_le(b, at, n, x);
                        write_le(b, at + n, n, y);
                    }
                }
            }
        }
        // The list sections' relocations back (their bytes are set).
        for (si, rels) in &self.stripped {
            if !rels.is_empty()
                && let Some(s) = o.sections_mut().nth(*si)
            {
                s.relocations.clone_from(rels);
            }
        }
        // The line program.
        if let Some((li, t, seqs)) = &self.line {
            let mut moved: Vec<Vec<Row>> = Vec::with_capacity(seqs.len());
            let mut sections: Vec<Option<usize>> = Vec::with_capacity(seqs.len());
            for (s, section) in seqs {
                let Some(first) = s.rows.first() else {
                    continue;
                };
                let sec = section.or_else(|| map.section_of(first.address));
                let mut rows = Vec::with_capacity(s.rows.len());
                for r in &s.rows {
                    let Some(sec) = sec else {
                        rows.push(*r);
                        continue;
                    };
                    if !r.end_sequence && map.cut_out(sec, r.address) {
                        continue;
                    }
                    rows.push(Row {
                        address: map.place(sec, r.address),
                        ..*r
                    });
                }
                if rows.len() < 2 {
                    continue; // Only its end: all its code went.
                }
                moved.push(rows);
                sections.push(*section);
            }
            let refs: Vec<&[Row]> = moved.iter().map(Vec::as_slice).collect();
            let (program, at) = line::encode(&t.params, &refs, turbonop_core::Endian::Little)?;
            let s = o.sections_mut().nth(*li).ok_or("no __debug_line")?;
            let old = s.bytes().unwrap_or(&[]);
            let mut bytes = old[t.range.start..t.program.start].to_vec();
            bytes.extend_from_slice(&program);
            let len = u32::try_from(bytes.len() - 4).map_err(|_| "a line table too large")?;
            bytes[..4].copy_from_slice(&len.to_le_bytes());
            let base = t.program.start - t.range.start;
            let mut rels = Vec::new();
            for (k, &a) in at.iter().enumerate() {
                if let Some(sec) = sections[k] {
                    rels.push(Relocation::plain(
                        (base + a) as u32,
                        sec as u32 + 1,
                        false,
                        0,
                        if t.params.address_size == 8 { 3 } else { 2 },
                    ));
                }
            }
            rels.reverse();
            s.contents = Contents::Bytes(std::borrow::Cow::Owned(bytes));
            s.relocations = rels;
        }
        Ok(emptied)
    }
}

/// A line program's bytes, and those encoded again.
pub type Programs = (Vec<u8>, Vec<u8>);

/// Decodes and encodes again every line table of `o` (one per object, as
/// [`Plan::read`] reads it): the program's bytes and the encoded ones. For checking
/// the encoder against LLVM's on a corpus.
pub fn line_round_trip(o: &Object<'_>) -> Result<Option<Programs>, &'static str> {
    let Some(plan) = Plan::read(o)? else {
        return Ok(None);
    };
    let Some((li, t, seqs)) = &plan.line else {
        return Ok(None);
    };
    let rows: Vec<&[Row]> = seqs.iter().map(|(s, _)| s.rows.as_slice()).collect();
    let (program, _) = line::encode(&t.params, &rows, turbonop_core::Endian::Little)?;
    let old = o.section(*li).and_then(|s| s.bytes()).unwrap_or(&[]);
    Ok(Some((old[t.program.clone()].to_vec(), program)))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn addresses_move_back_past_cuts() {
        // A section at 0x100 of 0x100 bytes, 0x10..0x30 and 0x80..0x90 cut;
        // the next at 0x200 now at 0x1D0.
        let map = AddressMap {
            old: vec![(0x100, 0x100), (0x200, 0x10)],
            new: vec![0x100, 0x1D0],
            cuts: vec![vec![0x10..0x30, 0x80..0x90], Vec::new()],
        };
        assert_eq!(map.place(0, 0x100), 0x100);
        assert_eq!(map.place(0, 0x10F), 0x10F);
        // In a cut: where the cut was.
        assert!(map.cut_out(0, 0x110) && map.cut_out(0, 0x12F));
        assert_eq!(map.place(0, 0x120), 0x110);
        assert_eq!(map.place(0, 0x130), 0x110);
        assert!(!map.cut_out(0, 0x130));
        assert_eq!(map.place(0, 0x190), 0x160);
        // The section's end, and the next section.
        assert_eq!(map.place(0, 0x200), 0x1D0);
        assert_eq!(map.section_of(0x200), Some(1));
        assert_eq!(map.place_any(0x208), 0x1D8);
        assert_eq!(map.place_any(0x500), 0x500);
    }
}
