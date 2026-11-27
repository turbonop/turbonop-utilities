//! A rewritten member's pruned DWARF against its original's: folding may
//! remove the DIEs that described the code a giver lost, and what only
//! they named (see `turbonop_fold::macho`'s `prune` module). Read apart
//! from the engine, with `turbonop_dwarf`'s reader:
//!
//! - **The DIEs left are the original's**: the two trees are aligned, each
//!   new DIE matched to an original one in order; below the unit and
//!   namespaces whole subtrees may be missing, nowhere else. A matched
//!   DIE has the original's abbreviation and every attribute's value: the
//!   same bytes (relocations' places aside, each relocation the
//!   original's, or, for one into a lost atom, gone with its place all
//!   ones), strings by their contents, range and location lists by what
//!   their entries name (those emptied by the lost code left out), and
//!   references to the DIEs matched to the original's targets
//!   (`DW_AT_sibling` to where its subtree ends). Every relocation of the
//!   new `__debug_info` is one of these.
//! - **What was removed** is, at the unit's or a namespace's level: a
//!   subprogram or variable whose debug relocations all named lost code
//!   (one at least), without `DW_AT_const_value`, whose lists name only
//!   lost code; an unnamed type (not a base type) or a subprogram without
//!   an address, without relocations; or a namespace of such. Nothing kept
//!   refers into it (references are matched). So what dsymutil keeps of
//!   the member (DIEs with linked addresses, base types, imported
//!   entities, constants, named types, and what they reach) is all there,
//!   unchanged.
//! - **Apple's accelerator tables** hold the original's entries whose DIEs
//!   are kept, each naming its DIE's match (and no others), every hash the
//!   DJB hash of its name, findable through its bucket.

use std::collections::{BTreeMap, BTreeSet};

use turbonop_dwarf::consts::*;
use turbonop_dwarf::{Abbrevs, Reader, Sections, Unit, Value};
use turbonop_macho_object::Object;

use super::debug::{Locator, list_at};
use crate::hash::H128;
use crate::report::{Report, show};

const NONE: u32 = u32::MAX;

struct A<'d> {
    name: u16,
    form: u16,
    pos: u32,
    end: u32,
    value: Value<'d>,
}

struct D<'d> {
    offset: u32,
    /// Where its subtree ends.
    end: u32,
    code: u64,
    tag: u16,
    parent: u32,
    attrs: Vec<A<'d>>,
    children: Vec<u32>,
}

struct Tree<'d> {
    info: &'d [u8],
    str: &'d [u8],
    address_size: u8,
    low: u64,
    dies: Vec<D<'d>>,
}

fn debug_bytes<'a>(o: &'a Object<'a>, name: &[u8]) -> &'a [u8] {
    o.sections()
        .find(|s| s.segment_name() == b"__DWARF" && s.name() == name)
        .and_then(|s| s.bytes())
        .unwrap_or(&[])
}

fn tree<'a>(o: &'a Object<'a>) -> Option<Tree<'a>> {
    let mut s = Sections::new(turbonop_core::Endian::Little);
    for x in o.sections() {
        if x.segment_name() == b"__DWARF"
            && let Some(n) = x.name().strip_prefix(b"__debug_")
        {
            s.set(n, x.bytes().unwrap_or(&[]));
        }
    }
    let mut units = Unit::iter(s, turbonop_dwarf::UnitSection::Info, 0);
    let u = units.next()?;
    if units.next().is_some() || u.offset != 0 || u.dwarf64 {
        return None;
    }
    let abbrevs = Abbrevs::parse(s.abbrev, u.abbrev_offset, s.endian)?;
    let mut r = Reader::new(s.info, u.dies as usize, s.endian);
    let mut dies: Vec<D<'_>> = Vec::new();
    let mut stack: Vec<u32> = Vec::new();
    let end = u.end as usize;
    while r.pos < end {
        let at = r.pos as u32;
        let code = r.uleb()?;
        if code == 0 {
            if let Some(p) = stack.pop() {
                dies[p as usize].end = r.pos as u32;
            }
            continue;
        }
        let a = abbrevs.get(code)?;
        let parent = stack.last().copied().unwrap_or(NONE);
        if parent == NONE && !dies.is_empty() {
            return None;
        }
        let mut attrs = Vec::with_capacity(a.attrs.len());
        for spec in &a.attrs {
            let pos = r.pos as u32;
            if spec.form == DW_FORM_INDIRECT {
                return None;
            }
            let value = u.read_value(&mut r, spec.form, spec.implicit)?;
            attrs.push(A {
                name: spec.name,
                form: spec.form,
                pos,
                end: r.pos as u32,
                value,
            });
        }
        let i = dies.len() as u32;
        if parent != NONE {
            dies[parent as usize].children.push(i);
        }
        dies.push(D {
            offset: at,
            end: r.pos as u32,
            code,
            tag: a.tag,
            parent,
            attrs,
            children: Vec::new(),
        });
        if a.children {
            stack.push(i);
        }
    }
    for p in stack {
        dies[p as usize].end = end as u32;
    }
    let low = dies.first().and_then(|d| {
        d.attrs.iter().find_map(|a| match (a.name, a.value) {
            (DW_AT_LOW_PC, Value::Addr(x)) => Some(x),
            _ => None,
        })
    });
    Some(Tree {
        info: s.info,
        str: s.str,
        address_size: u.address_size,
        low: low.unwrap_or(0),
        dies,
    })
}

impl Tree<'_> {
    fn die_at(&self, off: u64) -> Option<usize> {
        let i = self.dies.partition_point(|d| u64::from(d.offset) < off);
        self.dies
            .get(i)
            .filter(|d| u64::from(d.offset) == off)
            .map(|_| i)
    }

    fn string(&self, off: u64) -> Option<&[u8]> {
        let s = self.str.get(usize::try_from(off).ok()?..)?;
        Some(&s[..s.iter().position(|&c| c == 0)?])
    }

    fn scope(&self, i: u32) -> bool {
        i == 0
            || (self.dies[i as usize].tag == DW_TAG_NAMESPACE
                && self.scope(self.dies[i as usize].parent))
    }
}

fn is_ref(form: u16) -> bool {
    matches!(
        form,
        DW_FORM_REF1
            | DW_FORM_REF2
            | DW_FORM_REF4
            | DW_FORM_REF8
            | DW_FORM_REF_ADDR
            | DW_FORM_REF_UDATA
    )
}

fn is_loclist(name: u16) -> bool {
    matches!(
        name,
        DW_AT_LOCATION
            | DW_AT_STRING_LENGTH
            | DW_AT_RETURN_ADDR
            | DW_AT_DATA_MEMBER_LOCATION
            | DW_AT_FRAME_BASE
            | DW_AT_SEGMENT
            | DW_AT_STATIC_LINK
            | DW_AT_USE_LOCATION
            | DW_AT_VTABLE_ELEM_LOCATION
    )
}

fn is_type(tag: u16) -> bool {
    matches!(
        tag,
        DW_TAG_ARRAY_TYPE
            | DW_TAG_CLASS_TYPE
            | DW_TAG_ENUMERATION_TYPE
            | DW_TAG_POINTER_TYPE
            | DW_TAG_REFERENCE_TYPE
            | DW_TAG_STRING_TYPE
            | DW_TAG_STRUCTURE_TYPE
            | DW_TAG_SUBROUTINE_TYPE
            | DW_TAG_TYPEDEF
            | DW_TAG_UNION_TYPE
            | DW_TAG_PTR_TO_MEMBER_TYPE
            | DW_TAG_SET_TYPE
            | DW_TAG_SUBRANGE_TYPE
            | DW_TAG_CONST_TYPE
            | DW_TAG_PACKED_TYPE
            | DW_TAG_VOLATILE_TYPE
            | DW_TAG_RESTRICT_TYPE
            | DW_TAG_INTERFACE_TYPE
            | DW_TAG_UNSPECIFIED_TYPE
            | DW_TAG_SHARED_TYPE
            | DW_TAG_RVALUE_REFERENCE_TYPE
            | DW_TAG_TEMPLATE_ALIAS
            | DW_TAG_ATOMIC_TYPE
            | DW_TAG_IMMUTABLE_TYPE
    )
}

/// A debug relocation: its place, canonical form, width, and whether it
/// named lost code.
type Rel = (u32, u128, u8, bool);

fn in_range(rels: &[Rel], lo: u32, hi: u32) -> &[Rel] {
    let a = rels.partition_point(|r| r.0 < lo);
    let b = rels.partition_point(|r| r.0 < hi);
    &rels[a..b]
}

struct Cmp<'t, 'd> {
    o: &'t Tree<'d>,
    n: &'t Tree<'d>,
    orel: &'t [Rel],
    nrel: &'t [Rel],
    oobj: &'t Object<'d>,
    nobj: &'t Object<'d>,
    lo: &'t Locator,
    ln: &'t Locator,
    lost: &'t BTreeSet<&'t [u8]>,
    /// Each original DIE's match.
    map: Vec<u32>,
    /// Each DIE's [`signature`], by tree.
    osig: Vec<Option<u128>>,
    nsig: Vec<Option<u128>>,
    /// New relocations compared.
    seen_rels: usize,
}

/// Hashes DIE `i`'s strings.
fn strings(t: &Tree<'_>, i: usize, h: &mut H128) {
    for x in &t.dies[i].attrs {
        match x.value {
            Value::StrOffset(_, off) => h.bytes(t.string(off).unwrap_or(b"?")),
            Value::Str(s) => h.bytes(s),
            _ => {}
        }
    }
}

/// How deep [`signature`] follows references.
const SIG_DEPTH: u8 = 3;

/// A DIE's signature, for aligning the trees: its abbreviation, tag, and
/// the values pruning doesn't change (strings by content, constants,
/// flags), its ancestors' names, and its references' targets' signatures,
/// `depth` deep.
fn signature(t: &Tree<'_>, memo: &mut Vec<Option<u128>>, i: usize, depth: u8) -> u128 {
    if depth == SIG_DEPTH
        && let Some(s) = memo[i]
    {
        return s;
    }
    let d = &t.dies[i];
    let mut h = H128::new();
    h.u64(d.code);
    h.u64(u64::from(d.tag));
    // (Its scope: its ancestors' names, and their specifications' and
    // abstract origins': a local class's function may have none of its own.)
    let mut p = d.parent;
    while p != NONE {
        let a = &t.dies[p as usize];
        h.u64(u64::from(a.tag));
        strings(t, p as usize, &mut h);
        for x in &a.attrs {
            if matches!(x.name, DW_AT_SPECIFICATION | DW_AT_ABSTRACT_ORIGIN)
                && let Value::Uint(v) = x.value
                && let Some(j) = t.die_at(v)
            {
                strings(t, j, &mut h);
            }
        }
        p = a.parent;
    }
    for a in &d.attrs {
        match a.value {
            Value::StrOffset(_, off) => h.bytes(t.string(off).unwrap_or(b"?")),
            Value::Str(s) => h.bytes(s),
            Value::Data(b) => h.bytes(b),
            Value::Sint(v) => h.u64(v as u64),
            Value::Uint(v) if !is_ref(a.form) && a.form != DW_FORM_SEC_OFFSET => h.u64(v),
            Value::Uint(v) if is_ref(a.form) && depth > 0 => {
                // (The unit at 0: unit-relative references are section offsets.)
                match t.die_at(v) {
                    Some(j) if a.name != DW_AT_SIBLING => h.u128(signature(t, memo, j, depth - 1)),
                    _ => h.u64(1),
                }
            }
            _ => {}
        }
    }
    let s = h.finish();
    if depth == SIG_DEPTH {
        memo[i] = Some(s);
    }
    s
}

impl Cmp<'_, '_> {
    /// Whether attribute `k` of original DIE `c` and new DIE `d` hold the
    /// same (references aside); counts the new relocations compared.
    fn same_attr(&mut self, c: usize, d: usize, k: usize) -> bool {
        let (oa, na) = (&self.o.dies[c].attrs[k], &self.n.dies[d].attrs[k]);
        if is_ref(oa.form) {
            return true;
        }
        if c == 0 && matches!(oa.name, DW_AT_LOW_PC | DW_AT_HIGH_PC) {
            // (The unit's extent: checked by `debug::check`.)
            self.seen_rels += in_range(self.nrel, na.pos, na.end).len();
            return true;
        }
        match (oa.value, na.value) {
            (Value::StrOffset(_, x), Value::StrOffset(_, y)) => {
                return self.o.string(x).is_some() && self.o.string(x) == self.n.string(y);
            }
            (Value::Uint(x), Value::Uint(y)) if oa.form == DW_FORM_SEC_OFFSET => {
                let name = match oa.name {
                    DW_AT_RANGES => &b"__debug_ranges"[..],
                    n if is_loclist(n) => b"__debug_loc",
                    _ => return x == y,
                };
                let a = list_at(
                    self.oobj,
                    self.lo,
                    name,
                    x,
                    self.o.address_size,
                    self.o.low,
                    Some(self.lost),
                );
                let b = list_at(
                    self.nobj,
                    self.ln,
                    name,
                    y,
                    self.n.address_size,
                    self.n.low,
                    None,
                );
                return a.is_some() && a == b;
            }
            _ => {}
        }
        // Bytes, relocations' places aside.
        let (Some(ob), Some(nb)) = (
            self.o.info.get(oa.pos as usize..oa.end as usize),
            self.n.info.get(na.pos as usize..na.end as usize),
        ) else {
            return false;
        };
        if ob.len() != nb.len() {
            return false;
        }
        let (mut ob, mut nb) = (ob.to_vec(), nb.to_vec());
        let nr = in_range(self.nrel, na.pos, na.end);
        let mut used = 0;
        for &(at, canon, w, lost) in in_range(self.orel, oa.pos, oa.end) {
            let k = (at - oa.pos) as usize;
            let w = usize::from(w);
            let theirs = nr.iter().find(|r| r.0 - na.pos == at - oa.pos);
            if lost {
                if theirs.is_some()
                    || nb
                        .get(k..k + w)
                        .is_none_or(|x| x.iter().any(|&b| b != 0xFF))
                {
                    return false;
                }
            } else {
                match theirs {
                    Some(r) if r.1 == canon && usize::from(r.2) == w => used += 1,
                    _ => return false,
                }
            }
            for b in [&mut ob, &mut nb] {
                if let Some(x) = b.get_mut(k..k + w) {
                    x.fill(0);
                }
            }
        }
        if used != nr.len() {
            return false;
        }
        self.seen_rels += used;
        ob == nb
    }

    /// Whether DIE `c` and `d` hold the same (references aside), and if
    /// `whole`, their subtrees too (then matched, as `self.map` records).
    fn same(&mut self, c: usize, d: usize, whole: bool) -> bool {
        let (od, nd) = (&self.o.dies[c], &self.n.dies[d]);
        if od.code != nd.code || od.attrs.len() != nd.attrs.len() {
            return false;
        }
        let seen = self.seen_rels;
        for k in 0..od.attrs.len() {
            if !self.same_attr(c, d, k) {
                self.seen_rels = seen;
                return false;
            }
        }
        if !whole {
            return true;
        }
        let (oc, nc) = (od.children.clone(), nd.children.clone());
        if oc.len() != nc.len() {
            self.seen_rels = seen;
            return false;
        }
        for (&x, &y) in oc.iter().zip(&nc) {
            if !self.same(x as usize, y as usize, true) {
                self.unmap(c);
                self.seen_rels = seen;
                return false;
            }
        }
        self.map[c] = d as u32;
        true
    }

    fn unmap(&mut self, c: usize) {
        self.map[c] = NONE;
        let ch = self.o.dies[c].children.clone();
        for x in ch {
            self.unmap(x as usize);
        }
    }

    /// Aligns the children of scope `c` with those of `d`.
    fn align(&mut self, c: usize, d: usize) -> bool {
        let (oc, nc) = (
            self.o.dies[c].children.clone(),
            self.n.dies[d].children.clone(),
        );
        let mut j = 0;
        for &x in &oc {
            let Some(&y) = nc.get(j) else { break };
            let (x, y) = (x as usize, y as usize);
            let s1 = signature(self.o, &mut self.osig, x, SIG_DEPTH);
            let s2 = signature(self.n, &mut self.nsig, y, SIG_DEPTH);
            if s1 != s2 {
                continue;
            }
            let scope = self.o.dies[x].tag == DW_TAG_NAMESPACE;
            if scope {
                if self.same(x, y, false) {
                    self.map[x] = y as u32;
                    if !self.align(x, y) {
                        return false;
                    }
                    j += 1;
                }
            } else if self.same(x, y, true) {
                j += 1;
            }
        }
        j == nc.len()
    }

    /// Why original DIE `c` (unmatched, its parent a scope) couldn't have
    /// been removed, if it couldn't: (A) dead code's, (B) a type or
    /// declaration nothing kept names, (C) a namespace of such.
    fn removable(
        &self,
        c: usize,
        rels_in: &dyn Fn(u32, u32) -> (usize, usize),
    ) -> Result<bool, ()> {
        let d = &self.o.dies[c];
        let (lost, kept) = rels_in(d.offset, d.end);
        let has = |name: u16| d.attrs.iter().any(|a| a.name == name);
        if d.tag == DW_TAG_NAMESPACE {
            if d.children.is_empty() || kept > 0 {
                return Err(());
            }
            let mut dead = false;
            for &x in &d.children {
                dead |= self.removable(x as usize, rels_in)?;
            }
            return Ok(dead);
        }
        if matches!(d.tag, DW_TAG_SUBPROGRAM | DW_TAG_VARIABLE)
            && lost > 0
            && kept == 0
            && !(d.tag == DW_TAG_VARIABLE && has(DW_AT_CONST_VALUE))
        {
            // Its lists name only lost code.
            for i in c..self.o.dies.len() {
                let x = &self.o.dies[i];
                if x.offset >= d.end {
                    break;
                }
                for a in &x.attrs {
                    if a.form != DW_FORM_SEC_OFFSET {
                        continue;
                    }
                    let name = match a.name {
                        DW_AT_RANGES => &b"__debug_ranges"[..],
                        n if is_loclist(n) => b"__debug_loc",
                        _ => continue,
                    };
                    let Value::Uint(off) = a.value else {
                        return Err(());
                    };
                    let l = list_at(
                        self.oobj,
                        self.lo,
                        name,
                        off,
                        self.o.address_size,
                        self.o.low,
                        Some(self.lost),
                    );
                    if l.is_none_or(|l| !l.is_empty()) {
                        return Err(());
                    }
                }
            }
            return Ok(true);
        }
        let addressless = d.tag == DW_TAG_SUBPROGRAM
            && !has(DW_AT_LOW_PC)
            && !has(DW_AT_RANGES)
            && !has(DW_AT_ENTRY_PC);
        if ((is_type(d.tag) && !has(DW_AT_NAME)) || addressless) && lost == 0 && kept == 0 {
            return Ok(false);
        }
        Err(())
    }
}

/// An accelerator table entry: name, hash, DIE offset, the entry's other
/// atoms.
type Entry = (Vec<u8>, u32, u32, Vec<u8>);

/// An accelerator table's entries, or why it doesn't read.
fn apple_entries(b: &[u8], str: &[u8]) -> Result<Vec<Entry>, &'static str> {
    let bad = "doesn't read";
    let u32_at = |at: usize| -> Result<u32, &'static str> {
        b.get(at..at + 4)
            .map(|x| u32::from_le_bytes([x[0], x[1], x[2], x[3]]))
            .ok_or(bad)
    };
    if u32_at(0)? != 0x4841_5348 {
        return Err(bad);
    }
    let nb = u32_at(8)? as usize;
    let nh = u32_at(12)? as usize;
    let hdl = u32_at(16)? as usize;
    let base = u32_at(20)?;
    let natoms = u32_at(24)? as usize;
    let mut size = 0;
    let mut die_at = None;
    for k in 0..natoms {
        let x = u32_at(28 + 4 * k)?;
        let (kind, form) = (x & 0xFFFF, x >> 16);
        let w = match form as u16 {
            DW_FORM_DATA1 | DW_FORM_FLAG => 1,
            DW_FORM_DATA2 => 2,
            DW_FORM_DATA4 => 4,
            DW_FORM_DATA8 => 8,
            _ => return Err("an atom of a form not known"),
        };
        if kind == 1 {
            die_at = Some(size);
        }
        size += w;
    }
    let die_at = die_at.ok_or("no DIE offsets")?;
    let buckets = 20 + hdl;
    let hashes = buckets + 4 * nb;
    let offsets = hashes + 4 * nh;
    let mut out = Vec::new();
    let mut prev_bucket = 0;
    for h in 0..nh {
        let hash = u32_at(hashes + 4 * h)?;
        // Findable: its bucket starts at or before it, and every hash from
        // there to it is of that bucket.
        let bucket = hash as usize % nb.max(1);
        if bucket < prev_bucket {
            return Err("hashes out of bucket order");
        }
        prev_bucket = bucket;
        let first = u32_at(buckets + 4 * bucket)? as usize;
        if first > h {
            return Err("a hash its bucket doesn't reach");
        }
        for g in first..h {
            if u32_at(hashes + 4 * g)? as usize % nb != bucket {
                return Err("a hash its bucket doesn't reach");
            }
        }
        let mut at = u32_at(offsets + 4 * h)? as usize;
        loop {
            let s = u32_at(at)?;
            at += 4;
            if s == 0 {
                break;
            }
            let n = u32_at(at)? as usize;
            at += 4;
            let name = str
                .get(s as usize..)
                .and_then(|x| x.iter().position(|&c| c == 0).map(|z| &x[..z]));
            let name = name.ok_or("a name past __debug_str")?;
            let djb = name.iter().fold(5381u32, |h, &c| {
                h.wrapping_mul(33).wrapping_add(u32::from(c))
            });
            if djb != hash {
                return Err("a hash that isn't its name's");
            }
            for _ in 0..n {
                let e = b.get(at..at + size).ok_or(bad)?;
                let off =
                    u32::from_le_bytes([e[die_at], e[die_at + 1], e[die_at + 2], e[die_at + 3]]);
                let mut rest = e.to_vec();
                rest[die_at..die_at + 4].fill(0);
                out.push((name.to_vec(), hash, off.wrapping_add(base), rest));
                at += size;
            }
        }
    }
    Ok(out)
}

/// What the rewritten member's relocations in `__debug_info` are, for
/// [`check`]: (place, canonical form, width).
pub(super) type Relocs<'r> = &'r [(u32, u128, Option<Vec<u8>>, u8)];

/// Checks member `n`'s pruned DWARF against its original `o`'s (`orel`
/// and `nrel`: their `__debug_info` relocations; `lost`: the keys of the
/// atoms it lost; `lost_names`: the lost units' names, which a relocation
/// may name).
#[allow(clippy::too_many_arguments)]
pub(super) fn check(
    o: &Object<'_>,
    n: &Object<'_>,
    orel: Relocs<'_>,
    nrel: Relocs<'_>,
    lost: &BTreeSet<&[u8]>,
    lost_names: &BTreeSet<Vec<u8>>,
    what: &[u8],
    rep: &mut Report,
) {
    let (Some(ot), Some(nt)) = (tree(o), tree(n)) else {
        rep.fail("pruned DWARF that doesn't read", || show(what));
        return;
    };
    if debug_bytes(o, b"__debug_abbrev") != debug_bytes(n, b"__debug_abbrev")
        || ot.info.get(4..11) != nt.info.get(4..11)
    {
        rep.fail(
            "pruned DWARF whose unit's header or abbreviations changed",
            || show(what),
        );
        return;
    }
    // Which original relocations named lost code: an atom lost, or a lost
    // unit's name.
    let info_s = o
        .sections()
        .find(|s| s.segment_name() == b"__DWARF" && s.name() == b"__debug_info");
    let names: BTreeMap<u32, &[u8]> = info_s
        .map(|s| {
            s.relocations
                .iter()
                .filter(|r| r.external)
                .filter_map(|r| {
                    o.symbols
                        .get(r.symbolnum as usize)
                        .map(|y| (r.address, y.name.as_ref()))
                })
                .collect()
        })
        .unwrap_or_default();
    let orels: Vec<Rel> = orel
        .iter()
        .map(|r| {
            let gone = r.2.as_deref().is_some_and(|k| lost.contains(k))
                || names.get(&r.0).is_some_and(|y| lost_names.contains(*y));
            (r.0, r.1, r.3, gone)
        })
        .collect();
    let nrels: Vec<Rel> = nrel.iter().map(|r| (r.0, r.1, r.3, false)).collect();
    let (lo, ln) = (Locator::new(o), Locator::new(n));
    let mut c = Cmp {
        o: &ot,
        n: &nt,
        orel: &orels,
        nrel: &nrels,
        oobj: o,
        nobj: n,
        lo: &lo,
        ln: &ln,
        lost,
        map: vec![NONE; ot.dies.len()],
        osig: vec![None; ot.dies.len()],
        nsig: vec![None; nt.dies.len()],
        seen_rels: 0,
    };
    if ot.dies.is_empty() || nt.dies.is_empty() || !c.same(0, 0, false) {
        rep.fail("pruned DWARF whose unit's DIE changed", || show(what));
        return;
    }
    c.map[0] = 0;
    if !c.align(0, 0) {
        rep.fail("pruned DWARF with DIEs not the original's", || show(what));
        return;
    }
    rep.checked("DIEs of pruned members", nt.dies.len() as u64);
    // References: to the matches of the original's targets.
    let mut bad_refs = 0;
    let mut first_bad = None;
    for (i, &m) in c.map.iter().enumerate() {
        if m == NONE {
            continue;
        }
        let (od, nd) = (&ot.dies[i], &nt.dies[m as usize]);
        for (oa, na) in od.attrs.iter().zip(&nd.attrs) {
            if !is_ref(oa.form) {
                continue;
            }
            let (Value::Uint(x), Value::Uint(y)) = (oa.value, na.value) else {
                bad_refs += 1;
                continue;
            };
            let ok = if oa.name == DW_AT_SIBLING {
                y == u64::from(nd.end)
            } else {
                ot.die_at(x)
                    .map(|t| c.map[t])
                    .is_some_and(|t| t != NONE && u64::from(nt.dies[t as usize].offset) == y)
            };
            if !ok {
                bad_refs += 1;
                first_bad.get_or_insert((od.offset, oa.name, x, y));
            }
        }
    }
    if bad_refs > 0 {
        rep.fail(
            "pruned DWARF's references that don't name what the original's do",
            || {
                let (at, name, x, y) = first_bad.unwrap_or_default();
                format!(
                    "{}: {bad_refs}, the first in the DIE at {at:#x}: attribute {name:#x}, {x:#x} and {y:#x}",
                    show(what)
                )
            },
        );
    }
    // Every new relocation compared (the unit's own among them).
    if c.seen_rels != nrels.len() {
        rep.fail("pruned DWARF's relocations not the original's", || {
            format!("{}: {} of {}", show(what), c.seen_rels, nrels.len())
        });
    }
    // What was removed.
    let rels_in = |a: u32, b: u32| -> (usize, usize) {
        let v = in_range(&orels, a, b);
        let lost = v.iter().filter(|r| r.3).count();
        (lost, v.len() - lost)
    };
    let (mut dead, mut named) = (0u64, 0u64);
    let mut bad = Vec::new();
    for (i, d) in ot.dies.iter().enumerate() {
        if c.map[i] != NONE || d.parent == NONE || c.map[d.parent as usize] == NONE {
            continue;
        }
        if !ot.scope(d.parent) {
            bad.push(i);
            continue;
        }
        match c.removable(i, &rels_in) {
            Ok(true) => dead += 1,
            Ok(false) => named += 1,
            Err(()) => bad.push(i),
        }
    }
    rep.checked("DIEs removed from pruned members: dead code's", dead);
    rep.checked(
        "DIEs removed from pruned members: types and declarations nothing kept names",
        named,
    );
    if !bad.is_empty() {
        rep.fail("pruned DWARF without DIEs that describe what stays", || {
            format!(
                "{}: the DIE at {:#x} (tag {:#x}), and {} more",
                show(what),
                ot.dies[bad[0]].offset,
                ot.dies[bad[0]].tag,
                bad.len() - 1
            )
        });
    }
    // The accelerator tables.
    for name in [
        &b"__apple_names"[..],
        b"__apple_types",
        b"__apple_namespac",
        b"__apple_objc",
    ] {
        let (ob, nb) = (debug_bytes(o, name), debug_bytes(n, name));
        if ob.is_empty() && nb.is_empty() {
            continue;
        }
        let (Ok(oe), Ok(ne)) = (apple_entries(ob, ot.str), apple_entries(nb, nt.str)) else {
            rep.fail("pruned members' accelerator tables that don't read", || {
                format!("{}: {}", show(what), show(name))
            });
            continue;
        };
        let mut expect: Vec<Entry> = oe
            .into_iter()
            .filter_map(|(s, h, off, rest)| {
                let t = ot.die_at(u64::from(off))?;
                let m = c.map[t];
                (m != NONE).then(|| (s, h, nt.dies[m as usize].offset, rest))
            })
            .collect();
        let mut got = ne;
        expect.sort();
        got.sort();
        rep.checked(
            "accelerator table entries of pruned members",
            got.len() as u64,
        );
        if expect != got {
            rep.fail(
                "pruned members' accelerator tables without the original's entries of kept DIEs (or with others)",
                || format!("{}: {}", show(what), show(name)),
            );
        }
    }
}
