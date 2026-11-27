//! Moving sections and cutting byte ranges out of them: one map from each
//! old address to its new one, applied to everything that holds an
//! address.

use alloc::collections::BTreeSet;
use alloc::vec;
use alloc::vec::Vec;
use core::fmt;
use core::ops::Range;

use crate::reloc::{self, AddressRule, arm64, generic, x86_64};
use crate::{Contents, Object, Relocation, Unadjusted, ehframe, write};
use turbonop_macho::consts::{CPU_TYPE_ARM64, CPU_TYPE_ARM64_32, CPU_TYPE_X86, CPU_TYPE_X86_64};

/// What [`Object::cut`] left for the caller.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Cut {
    /// Symbols whose address was in a removed range, by index: left as
    /// they were, their `n_value`s the old ones (the caller removes them or
    /// makes them undefined); and the address the removed range's start
    /// has now (where the symbol would be if it were kept at the cut, as
    /// arm64's section-start labels, `ltmp0` and on, are by LLVM).
    pub orphaned_symbols: Vec<(u32, u64)>,
    /// Relocations whose encoded target (or scattered `r_value`) was in a
    /// removed range: left as they were, as (section index, relocation
    /// index after the cut).
    pub dangling: Vec<(usize, usize)>,
    /// Relocations (and `__eh_frame` sections) whose bytes it couldn't
    /// rewrite though something they involve moved.
    pub unadjusted: Vec<Unadjusted>,
    /// Sections (by index) where bytes after a cut moved back by other
    /// than a multiple of the section's alignment: what follows may have
    /// lost its alignment (cut only moves bytes; it doesn't pad).
    pub misaligned: Vec<usize>,
}

/// Why [`Object::cut`] refused the ranges it was given (nothing changed).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CutError {
    /// A section index past the last section.
    NoSection(usize),
    /// A section's ranges overlap, or one runs past its end.
    BadRanges(usize),
}

impl fmt::Display for CutError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            CutError::NoSection(i) => write!(f, "no section {i}"),
            CutError::BadRanges(i) => write!(f, "section {i}'s ranges overlap or pass its end"),
        }
    }
}

impl core::error::Error for CutError {}

/// A section offset after the section's cuts (`None`: in a removed
/// range). Offsets before the section or past its end move as its start
/// and its end do.
fn map_offset(ranges: &Cuts<'_>, off: i128) -> Option<i128> {
    // (The ranges before `off`, by binary search: a section may have
    // thousands of cuts and many more relocations.)
    let k = ranges.ranges.partition_point(|r| i128::from(r.end) <= off);
    if ranges
        .ranges
        .get(k)
        .is_some_and(|r| off >= i128::from(r.start))
    {
        return None;
    }
    let removed = k.checked_sub(1).map_or(0, |k| ranges.removed[k]);
    Some(off - i128::from(removed))
}

/// A section's cut ranges (sorted, disjoint), and the bytes removed up to
/// each one's end.
struct Cuts<'r> {
    ranges: &'r [Range<u64>],
    removed: Vec<u64>,
}

impl<'r> Cuts<'r> {
    fn new(ranges: &'r [Range<u64>]) -> Self {
        let mut sum = 0;
        let removed = ranges
            .iter()
            .map(|r| {
                sum += r.end - r.start;
                sum
            })
            .collect();
        Cuts { ranges, removed }
    }
}

/// Each old address's new one.
struct Map<'m> {
    /// Each section's old address and size.
    old: &'m [(u64, u64)],
    new: &'m [u64],
    cuts: &'m [Vec<Range<u64>>],
    sums: &'m [Cuts<'m>],
}

impl Map<'_> {
    /// The section an old address is in (an end address counts as its
    /// section's, for labels at the end).
    fn section_of(&self, a: u64) -> Option<usize> {
        let old = self.old;
        old.iter()
            .position(|&(lo, size)| a >= lo && a - lo < size)
            .or_else(|| old.iter().position(|&(lo, size)| a >= lo && a - lo == size))
    }

    /// Old address `a`, taken as section `t`'s, after the move.
    fn in_section(&self, t: usize, a: u64) -> Option<u64> {
        let (lo, _) = *self.old.get(t)?;
        let off = i128::from(a) - i128::from(lo);
        let n = map_offset(&self.sums[t], off)?;
        Some((i128::from(self.new[t]) + n) as u64)
    }

    /// Old address `a` after the move (an address in no section stays).
    fn address(&self, a: u64) -> Option<u64> {
        match self.section_of(a) {
            Some(t) => self.in_section(t, a),
            None => Some(a),
        }
    }

    /// Section `t`'s move, if every address in it moves by the same.
    fn constant(&self, t: usize) -> Option<i64> {
        let cuts = self.cuts.get(t)?;
        cuts.is_empty()
            .then(|| self.new[t].wrapping_sub(self.old[t].0) as i64)
    }

    /// Whether anything in section `t` moves.
    fn changed(&self, t: usize) -> bool {
        self.cuts.get(t).is_some_and(|c| !c.is_empty())
            || self
                .new
                .get(t)
                .zip(self.old.get(t))
                .is_some_and(|(n, o)| *n != o.0)
    }
}

/// Why a relocation's bytes weren't rewritten.
enum Problem {
    /// Its target was removed.
    Dangling,
    /// Its encoding isn't one [`reloc`] knows.
    Unknown,
}

fn signed(v: u64, length: u8) -> i64 {
    let bits = 8u32 << length.min(3);
    if bits == 64 {
        v as i64
    } else {
        ((v << (64 - bits)) as i64) >> (64 - bits)
    }
}

/// Whether a relocation's `r_address` is a place in its section (not a
/// 32-bit target's `PAIR`'s, which holds something else).
fn has_place(cputype: u32, r: &Relocation) -> bool {
    matches!(
        cputype,
        CPU_TYPE_X86_64 | CPU_TYPE_ARM64 | CPU_TYPE_ARM64_32
    ) || r.kind != generic::PAIR
}

/// The section a plain local relocation names (`None` for `R_ABS`).
fn target(r: &Relocation, n: usize) -> Option<usize> {
    let t = r.symbolnum.checked_sub(1)? as usize;
    (t < n).then_some(t)
}

/// Moves every section from `old` to `new` (by index), removing `cuts`
/// (each section's sorted, disjoint offset ranges) from them, and moves
/// everything that holds an address with them.
pub(crate) fn remap(
    o: &mut Object<'_>,
    old: &[(u64, u64)],
    new: &[u64],
    cuts: &[Vec<Range<u64>>],
) -> Cut {
    let sums: Vec<Cuts<'_>> = cuts.iter().map(|c| Cuts::new(c)).collect();
    let map = Map {
        old,
        new,
        cuts,
        sums: &sums,
    };
    let n = old.len();
    let mut out = Cut::default();
    if (0..n).all(|t| !map.changed(t)) {
        return out;
    }
    let cputype = o.header.cputype;
    let endian = o.endian();
    let wide = o.is_64();
    for (i, s) in o.sections_mut().enumerate() {
        if i >= n {
            break;
        }
        let ranges = &cuts[i];
        let sum = &sums[i];
        let old_addr = old[i].0;
        let alignment = 1u64 << s.align.min(32);
        let mut removed_before = 0u64;
        for r in ranges {
            removed_before += r.end - r.start;
            if r.end < old[i].1 && !removed_before.is_multiple_of(alignment) {
                out.misaligned.push(i);
                break;
            }
        }
        let bytes = s.bytes();
        // Edits to the old bytes: offset, length (log2), amount.
        let mut edits: Vec<(u32, u8, i64)> = Vec::new();
        let mut kept: Vec<Relocation> = Vec::with_capacity(s.relocations.len());
        for g in reloc::fixups(cputype, &s.relocations) {
            let group = &s.relocations[g];
            let first = group[0];
            let Some(new_off) = map_offset(sum, i128::from(first.address)) else {
                continue;
            };
            let old_place = old_addr.wrapping_add(u64::from(first.address));
            let new_place = (i128::from(new[i]) + new_off) as u64;
            let place_delta = new_place.wrapping_sub(old_place) as i64;
            let mut group: Vec<Relocation> = group.to_vec();
            let mut values_set = false;
            let value = |r: &Relocation| -> Option<u64> {
                reloc::value_at(bytes?, r.address, r.length, endian)
            };
            // The move of section `t`'s address `a` (or of the whole
            // section, when it moves as one).
            let moved = |t: Option<usize>, a: Option<u64>| -> Result<i64, Problem> {
                let Some(t) = t else { return Ok(0) };
                if let Some(d) = map.constant(t) {
                    return Ok(d);
                }
                let a = a.ok_or(Problem::Unknown)?;
                let b = map.in_section(t, a).ok_or(Problem::Dangling)?;
                Ok(b.wrapping_sub(a) as i64)
            };
            let wide_pair = matches!(
                cputype,
                CPU_TYPE_X86_64 | CPU_TYPE_ARM64 | CPU_TYPE_ARM64_32
            );
            let subtractor = if cputype == CPU_TYPE_X86_64 {
                x86_64::SUBTRACTOR
            } else {
                arm64::SUBTRACTOR
            };
            let result: Result<i64, Problem> = if wide_pair
                && group.len() == 2
                && first.kind == subtractor
                && first.value.is_none()
                && group[1].value.is_none()
            {
                // SUBTRACTOR (B) then UNSIGNED (A): the bytes hold A - B
                // plus an addend; a local half's address is in them.
                let (sub, add) = (group[0], group[1]);
                let v = value(&add);
                let delta_a = if add.external {
                    Ok(0)
                } else if sub.external {
                    moved(target(&add, n), v)
                } else {
                    moved(target(&add, n), None)
                };
                let delta_b = if sub.external {
                    Ok(0)
                } else if add.external {
                    moved(
                        target(&sub, n),
                        v.map(|v| signed(v, add.length).wrapping_neg() as u64),
                    )
                } else {
                    moved(target(&sub, n), None)
                };
                match (delta_a, delta_b) {
                    (Ok(a), Ok(b)) => Ok(a.wrapping_sub(b)),
                    (Err(Problem::Dangling), _) | (_, Err(Problem::Dangling)) => {
                        Err(Problem::Dangling)
                    }
                    _ => Err(Problem::Unknown),
                }
            } else if cputype == CPU_TYPE_X86
                && group.len() == 2
                && matches!(first.kind, generic::SECTDIFF | generic::LOCAL_SECTDIFF)
                && first.value.is_some()
                && group[1].value.is_some()
            {
                let (a, b) = (first.value.unwrap_or(0), group[1].value.unwrap_or(0));
                match (map.address(u64::from(a)), map.address(u64::from(b))) {
                    (Some(na), Some(nb)) => {
                        group[0].value = Some(na as u32);
                        group[1].value = Some(nb as u32);
                        values_set = true;
                        Ok((na.wrapping_sub(u64::from(a)) as i64)
                            .wrapping_sub(nb.wrapping_sub(u64::from(b)) as i64))
                    }
                    _ => Err(Problem::Dangling),
                }
            } else {
                // One entry, or a pair whose first decides, or
                // ARM64_RELOC_ADDEND then the entry it is the addend of.
                let r = if first.value.is_none() && first.kind == arm64::ADDEND && wide_pair {
                    group[group.len() - 1]
                } else {
                    first
                };
                match reloc::address_rule(cputype, &r) {
                    AddressRule::None => Ok(0),
                    AddressRule::Pc => Ok(place_delta.wrapping_neg()),
                    AddressRule::Target { sign: 1, pcrel } => {
                        let t = target(&r, n);
                        let a = value(&r).map(|v| {
                            if !pcrel {
                                return v;
                            }
                            let bias = if cputype == CPU_TYPE_X86_64 {
                                match r.kind {
                                    x86_64::SIGNED_1 => 5,
                                    x86_64::SIGNED_2 => 6,
                                    x86_64::SIGNED_4 => 8,
                                    _ => 4,
                                }
                            } else {
                                1u64 << r.length.min(3)
                            };
                            old_place
                                .wrapping_add(bias)
                                .wrapping_add(signed(v, r.length) as u64)
                        });
                        moved(t, a).map(|d| d.wrapping_sub(if pcrel { place_delta } else { 0 }))
                    }
                    AddressRule::Target { sign, pcrel } => {
                        match target(&r, n).map(|t| map.constant(t)) {
                            None => Ok(if pcrel { place_delta.wrapping_neg() } else { 0 }),
                            Some(Some(d)) => Ok(sign.wrapping_mul(d).wrapping_sub(if pcrel {
                                place_delta
                            } else {
                                0
                            })),
                            Some(None) => Err(Problem::Unknown),
                        }
                    }
                    AddressRule::Value => {
                        let a = r.value.unwrap_or(0);
                        match map.address(u64::from(a)) {
                            Some(na) => {
                                group[0].value = Some(na as u32);
                                values_set = true;
                                Ok(na.wrapping_sub(u64::from(a)) as i64)
                            }
                            None => Err(Problem::Dangling),
                        }
                    }
                    AddressRule::Difference | AddressRule::Unknown => {
                        let target_changed = match r.value {
                            Some(v) => map.section_of(u64::from(v)).is_some_and(|t| map.changed(t)),
                            None => !r.external && target(&r, n).is_some_and(|t| map.changed(t)),
                        };
                        if place_delta != 0 || target_changed {
                            Err(Problem::Unknown)
                        } else {
                            Ok(0)
                        }
                    }
                }
            };
            // The scattered values not set above move with their sections.
            if !values_set {
                for r in &mut group {
                    if let Some(v) = r.value {
                        r.value = Some(map.address(u64::from(v)).unwrap_or(u64::from(v)) as u32);
                    }
                }
            }
            let index = kept.len();
            let last = group[group.len() - 1];
            for r in &mut group {
                if has_place(cputype, r)
                    && let Some(off) = map_offset(sum, i128::from(r.address))
                {
                    r.address = off as u32;
                }
            }
            match result {
                Ok(0) => {}
                Ok(d) => edits.push((first.address, last.length, d)),
                Err(Problem::Dangling) => out.dangling.push((i, index)),
                Err(Problem::Unknown) => out.unadjusted.push(Unadjusted {
                    section: i,
                    relocation: Some(index),
                }),
            }
            kept.extend(group);
        }
        // `__eh_frame`'s pointers that no relocation covers.
        if s.segment_name() == b"__TEXT" && s.name() == b"__eh_frame" {
            let covered: BTreeSet<u32> = s.relocations.iter().map(|r| r.address).collect();
            match bytes.and_then(|b| ehframe::pointers(b, wide, endian).map(|p| (b, p))) {
                None if bytes.is_some() => out.unadjusted.push(Unadjusted {
                    section: i,
                    relocation: None,
                }),
                None => {}
                Some((b, pointers)) => {
                    let mut ok = true;
                    for p in pointers {
                        let Ok(at) = u32::try_from(p.offset) else {
                            ok = false;
                            continue;
                        };
                        let Some(new_off) = map_offset(sum, i128::from(at)) else {
                            continue;
                        };
                        if covered.contains(&at) {
                            continue;
                        }
                        let length = p.size.trailing_zeros() as u8;
                        let Some(v) = reloc::value_at(b, at, length, endian) else {
                            ok = false;
                            continue;
                        };
                        let old_place = old_addr.wrapping_add(u64::from(at));
                        let new_place = (i128::from(new[i]) + new_off) as u64;
                        let (t, place_delta) = match p.encoding & 0x70 {
                            0 => (v, 0),
                            ehframe::DW_EH_PE_PCREL => (
                                old_place.wrapping_add(signed(v, length) as u64),
                                new_place.wrapping_sub(old_place) as i64,
                            ),
                            _ => {
                                ok = false;
                                continue;
                            }
                        };
                        let Some(nt) = map.address(t) else {
                            ok = false;
                            continue;
                        };
                        let d = (nt.wrapping_sub(t) as i64).wrapping_sub(place_delta);
                        if d != 0 {
                            edits.push((at, length, d));
                        }
                    }
                    // Each FDE's distance back to its CIE.
                    for (field, cie) in ehframe::cie_links(b, endian).unwrap_or_default() {
                        let Some(nf) = map_offset(sum, field as i128) else {
                            continue;
                        };
                        let Some(nc) = map_offset(sum, cie as i128) else {
                            ok = false;
                            continue;
                        };
                        let d = ((nf - nc) - (field as i128 - cie as i128)) as i64;
                        if d != 0 {
                            edits.push((field as u32, 2, d));
                        }
                    }
                    if !ok {
                        out.unadjusted.push(Unadjusted {
                            section: i,
                            relocation: None,
                        });
                    }
                }
            }
        }
        // The bytes: rewritten, then cut.
        let removed: u64 = ranges.iter().map(|r| r.end - r.start).sum();
        match &mut s.contents {
            Contents::Bytes(b) if !edits.is_empty() || removed != 0 => {
                let v = b.to_mut();
                for (at, length, d) in edits {
                    if !reloc::add_at(v, at, length, d, endian) {
                        out.unadjusted.push(Unadjusted {
                            section: i,
                            relocation: None,
                        });
                    }
                }
                if removed != 0 {
                    let mut cut = Vec::with_capacity(v.len() - removed as usize);
                    let mut at = 0usize;
                    for r in ranges {
                        cut.extend_from_slice(&v[at..r.start as usize]);
                        at = r.end as usize;
                    }
                    cut.extend_from_slice(&v[at..]);
                    *v = cut;
                }
            }
            Contents::Zerofill(size) => *size -= removed,
            Contents::Bytes(_) => {}
        }
        s.relocations = kept;
        s.addr = new[i];
    }
    // Symbols.
    for (k, sym) in o.symbols.iter_mut().enumerate() {
        if sym.n_sect == 0 || !(sym.in_section() || sym.is_stab()) {
            continue;
        }
        let t = usize::from(sym.n_sect) - 1;
        if t >= n {
            continue;
        }
        match map.in_section(t, sym.n_value) {
            Some(a) => sym.n_value = a,
            None => {
                let off = sym.n_value.wrapping_sub(old[t].0);
                let start = cuts[t]
                    .iter()
                    .find(|r| off >= r.start && off < r.end)
                    .map_or(off, |r| r.start);
                let before: u64 = cuts[t]
                    .iter()
                    .filter(|r| r.end <= start)
                    .map(|r| r.end - r.start)
                    .sum();
                let at = new[t].wrapping_add(start.wrapping_sub(before));
                out.orphaned_symbols.push((k as u32, at));
            }
        }
    }
    o.edit_linkedit(|a| map.address(a));
    o.original = None;
    out
}

/// Checks and gathers `cuts` by section: each section's ranges sorted,
/// empty ones dropped.
pub(crate) fn gather(
    o: &Object<'_>,
    cuts: &[(usize, Vec<Range<u64>>)],
) -> Result<Vec<Vec<Range<u64>>>, CutError> {
    let sizes: Vec<u64> = o.sections().map(crate::Section::size).collect();
    let mut per = vec![Vec::new(); sizes.len()];
    for (i, ranges) in cuts {
        let slot = per.get_mut(*i).ok_or(CutError::NoSection(*i))?;
        slot.extend(ranges.iter().filter(|r| r.start < r.end).cloned());
    }
    for (i, ranges) in per.iter_mut().enumerate() {
        ranges.sort_by_key(|r| r.start);
        if ranges.windows(2).any(|w| w[0].end > w[1].start)
            || ranges.last().is_some_and(|r| r.end > sizes[i])
        {
            return Err(CutError::BadRanges(i));
        }
    }
    Ok(per)
}

/// Each section's address after the cuts, as LLVM lays them out.
pub(crate) fn new_addresses(o: &Object<'_>, cuts: &[Vec<Range<u64>>]) -> Vec<u64> {
    let secs: Vec<(u32, u64, bool)> = o
        .sections()
        .zip(cuts)
        .map(|(s, c)| {
            let removed: u64 = c.iter().map(|r| r.end - r.start).sum();
            (s.align, s.size() - removed, s.is_zerofill_kind())
        })
        .collect();
    write::llvm_offsets(&secs)
}
