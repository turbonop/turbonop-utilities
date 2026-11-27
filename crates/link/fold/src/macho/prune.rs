//! A giver's DWARF without what described the code it lost.
//!
//! After its units are cut out (and [`super::adjust`] has moved what no
//! relocation holds), a giver's compile unit still holds the moved
//! copies' DIEs, their addresses tombstoned. This removes them, and what
//! only they reached, so that the giver's DWARF describes only what it
//! still holds:
//!
//! - **Entities.** A DIE whose parent is the unit or a namespace is an
//!   entity, kept or removed whole (its subtree). A namespace is a scope,
//!   not an entity: removed only when it had children, lost them all, and
//!   nothing kept refers to it.
//! - **Dead**: a subprogram or variable entity some of whose debug
//!   relocations named lost code (tombstoned, or naming a moved name) and
//!   none anything kept, with no `DW_AT_const_value`, whose range and
//!   location lists are all empty now.
//! - **Reached**: the entities the dead ones refer to, transitively
//!   (references, location expressions' DIEs), that are types (but base
//!   types, which dsymutil always keeps) or subprograms without addresses
//!   (declarations and abstract instances), with no debug relocations.
//! - **Kept**: every other entity, and everything a kept entity refers to,
//!   to a fixpoint (a dead subprogram a kept call site names stays, its
//!   address tombstoned, and is counted). The rest is removed. Everything
//!   dsymutil keeps of the giver (DIEs with linked addresses, base types,
//!   imported entities, and what they reach) is thus kept, unchanged.
//! - **Rewriting.** The removed subtrees are cut out of `__debug_info`
//!   ([`Object::cut`], which moves its relocations); every kept reference
//!   (`DW_FORM_ref*`, `DW_AT_sibling` to where its sibling was), string
//!   offset, and list offset is set to where its target went; the unit's
//!   length shrinks. `__debug_str` loses the strings nothing kept names
//!   (the first string stays: offset 0 ends an accelerator table's chain),
//!   `__debug_ranges` and `__debug_loc` the lists nothing kept names and
//!   the entries the cut emptied, and Apple's accelerator tables
//!   (`__apple_names`, `__apple_types`, `__apple_namespac`, `__apple_objc`)
//!   the entries of removed DIEs; they are written again as LLVM's
//!   `AppleAccelTableWriter` writes them (its bucket count for the hashes
//!   left, each bucket's hashes in order, a name's entries in their
//!   order). The abbreviations and the line program stay as they are.
//!
//! Handled: one compile unit of DWARF 4, 32-bit, the sections above (and
//! `__debug_macinfo`), references within the unit, no reference in an
//! expression, no `DW_FORM_ref_udata`, `DW_FORM_indirect`, or DWARF 5
//! forms. Anything else is refused, and the giver keeps its DWARF as it
//! is (the dead DIEs tombstoned).

use std::collections::BTreeSet;
use std::ops::Range;

use turbonop_dwarf::consts::*;
use turbonop_dwarf::ops::{self, Operand};
use turbonop_dwarf::{Abbrevs, Reader, Sections, Unit, Value};
use turbonop_macho_object::{Contents, Object};

type R<T> = Result<T, &'static str>;

/// What [`prune`] removed.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(super) struct Pruned {
    /// Dead entities removed, and those kept because a kept DIE refers to
    /// them.
    pub dead: usize,
    pub dead_kept: usize,
    /// Other entities removed (what only the dead reached) and namespaces
    /// emptied.
    pub reached: usize,
    /// Bytes of debug sections removed.
    pub bytes: u64,
}

const NONE: u32 = u32::MAX;

/// The debug sections handled, by name.
const KNOWN: [&[u8]; 11] = [
    b"__debug_info",
    b"__debug_abbrev",
    b"__debug_str",
    b"__debug_line",
    b"__debug_ranges",
    b"__debug_loc",
    b"__debug_macinfo",
    b"__apple_names",
    b"__apple_types",
    b"__apple_namespac",
    b"__apple_objc",
];
const INFO: usize = 0;
const ABBREV: usize = 1;
const STR: usize = 2;
const RANGES: usize = 4;
const LOC: usize = 5;
const APPLE: Range<usize> = 7..11;

struct Die {
    offset: u32,
    /// Where its subtree ends.
    end: u32,
    parent: u32,
    tag: u16,
    /// The entity holding it (itself for an entity; `NONE` for a scope).
    ent: u32,
    /// A `DW_AT_low_pc`, `DW_AT_entry_pc`, or `DW_AT_ranges`.
    has_addr: bool,
    const_value: bool,
}

/// A value of a DIE to set once what it names has moved.
#[derive(Clone, Copy, Debug)]
enum Fix {
    /// A reference (unit-relative, or `DW_FORM_ref_addr`'s section
    /// offset), to the DIE at `target` (a section offset).
    Ref {
        target: u32,
        sibling: bool,
    },
    Strp(u32),
    Ranges(u32),
    Loc(u32),
}

struct At {
    pos: u32,
    size: u8,
    die: u32,
    fix: Fix,
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

/// Attributes whose `DW_FORM_sec_offset` is a location list.
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

fn read_le(b: &[u8], at: usize, n: usize) -> Option<u64> {
    let s = b.get(at..at.checked_add(n)?)?;
    let mut v = [0u8; 8];
    v[..n].copy_from_slice(s);
    Some(u64::from_le_bytes(v))
}

fn write_le(b: &mut [u8], at: usize, n: usize, v: u64) {
    b[at..at + n].copy_from_slice(&v.to_le_bytes()[..n]);
}

/// Refuses an expression that names a DIE (its operand can't be moved).
fn check_expr(e: &[u8], address_size: u8) -> R<()> {
    let ops = ops::decode(e, turbonop_core::Endian::Little, address_size, 4, 4)
        .map_err(|_| "an expression that doesn't decode")?;
    for op in &ops {
        for x in &op.operands {
            match *x {
                Operand::InfoOffset { .. } | Operand::UnitDie { .. } => {
                    return Err("an expression that names a DIE");
                }
                Operand::Expression(n) => check_expr(n, address_size)?,
                _ => {}
            }
        }
    }
    Ok(())
}

/// Offsets after removing sorted, disjoint `ranges`: an offset in a
/// removed range is where the range was.
pub(super) struct Shift {
    ranges: Vec<Range<u32>>,
    /// Bytes removed up to each range's end.
    removed: Vec<u32>,
}

impl Shift {
    pub(super) fn new(ranges: Vec<Range<u32>>) -> Self {
        let mut removed = Vec::with_capacity(ranges.len());
        let mut n = 0;
        for r in &ranges {
            n += r.end - r.start;
            removed.push(n);
        }
        Shift { ranges, removed }
    }

    pub(super) fn map(&self, off: u32) -> u32 {
        let k = self.ranges.partition_point(|r| r.end <= off);
        let before = k.checked_sub(1).map_or(0, |k| self.removed[k]);
        match self.ranges.get(k) {
            Some(r) if off >= r.start => r.start - before,
            _ => off - before,
        }
    }

    pub(super) fn removed(&self, off: u32) -> bool {
        let k = self.ranges.partition_point(|r| r.end <= off);
        self.ranges.get(k).is_some_and(|r| off >= r.start)
    }

    fn total(&self) -> u32 {
        self.removed.last().copied().unwrap_or(0)
    }
}

/// A list entry's kind: a range that holds something, an empty one, a
/// base address selection.
const RANGE: u8 = 0;
const EMPTY: u8 = 1;
const SELECT: u8 = 2;

/// Every DIE of `u` (with its parent, extent, and entity), and the values
/// of its attributes that name what may move.
fn walk(u: &Unit<'_>, abbrevs: &Abbrevs) -> R<(Vec<Die>, Vec<At>)> {
    let bad = "a DIE that doesn't read";
    let data = u.sections().info;
    let mut r = Reader::new(data, u.dies as usize, u.sections().endian);
    let mut dies: Vec<Die> = Vec::new();
    let mut ats: Vec<At> = Vec::new();
    // (Open DIEs with children, and whether each DIE is a scope.)
    let mut stack: Vec<u32> = Vec::new();
    let mut scope: Vec<bool> = Vec::new();
    let end = u.end as usize;
    while r.pos < end {
        let at = r.pos as u32;
        let code = r.uleb().ok_or(bad)?;
        if code == 0 {
            if let Some(p) = stack.pop() {
                dies[p as usize].end = r.pos as u32;
            } else if dies.is_empty() {
                return Err(bad);
            }
            continue;
        }
        let a = abbrevs.get(code).ok_or(bad)?;
        let i = dies.len() as u32;
        let parent = stack.last().copied().unwrap_or(NONE);
        if parent == NONE && !dies.is_empty() {
            return Err("DIEs after the unit's DIE");
        }
        if matches!(
            a.tag,
            DW_TAG_MODULE | DW_TAG_PARTIAL_UNIT | DW_TAG_TYPE_UNIT | DW_TAG_SKELETON_UNIT
        ) || (parent != NONE && a.tag == DW_TAG_COMPILE_UNIT)
        {
            return Err("a DIE of a tag not handled");
        }
        let in_scope = parent == NONE || scope[parent as usize];
        let is_scope = parent == NONE || (a.tag == DW_TAG_NAMESPACE && in_scope);
        let ent = if is_scope {
            NONE
        } else if in_scope {
            i
        } else {
            dies[parent as usize].ent
        };
        let mut die = Die {
            offset: at,
            end: 0,
            parent,
            tag: a.tag,
            ent,
            has_addr: false,
            const_value: false,
        };
        for spec in &a.attrs {
            let pos = r.pos as u32;
            let (name, form) = (spec.name, spec.form);
            let v = u.read_value(&mut r, form, spec.implicit).ok_or(bad)?;
            let mut push = |size: u8, fix: Fix| {
                ats.push(At {
                    pos,
                    size,
                    die: i,
                    fix,
                });
            };
            // (A reference or section offset: 32 bits.)
            let n = match v {
                Value::Uint(n) => u32::try_from(n).unwrap_or(u32::MAX),
                _ => 0,
            };
            match form {
                DW_FORM_REF1 | DW_FORM_REF2 | DW_FORM_REF4 | DW_FORM_REF8 => {
                    let size = match form {
                        DW_FORM_REF1 => 1,
                        DW_FORM_REF2 => 2,
                        DW_FORM_REF4 => 4,
                        _ => 8,
                    };
                    push(
                        size,
                        Fix::Ref {
                            target: n,
                            sibling: name == DW_AT_SIBLING,
                        },
                    );
                }
                DW_FORM_REF_ADDR => push(
                    4,
                    Fix::Ref {
                        target: n,
                        sibling: name == DW_AT_SIBLING,
                    },
                ),
                DW_FORM_STRP => {
                    let Value::StrOffset(_, x) = v else {
                        return Err(bad);
                    };
                    push(4, Fix::Strp(u32::try_from(x).map_err(|_| bad)?));
                }
                DW_FORM_SEC_OFFSET => match name {
                    DW_AT_RANGES => push(4, Fix::Ranges(n)),
                    DW_AT_STMT_LIST | DW_AT_MACRO_INFO => {}
                    x if is_loclist(x) => push(4, Fix::Loc(n)),
                    _ => return Err("a section offset of an attribute not known"),
                },
                DW_FORM_EXPRLOC => {
                    let Value::Block(b) = v else { return Err(bad) };
                    check_expr(b, u.address_size)?;
                }
                DW_FORM_BLOCK1 | DW_FORM_BLOCK2 | DW_FORM_BLOCK4 | DW_FORM_BLOCK
                    if is_loclist(name) =>
                {
                    let Value::Block(b) = v else { return Err(bad) };
                    check_expr(b, u.address_size)?;
                }
                DW_FORM_ADDR
                | DW_FORM_BLOCK1
                | DW_FORM_BLOCK2
                | DW_FORM_BLOCK4
                | DW_FORM_BLOCK
                | DW_FORM_DATA1
                | DW_FORM_DATA2
                | DW_FORM_DATA4
                | DW_FORM_DATA8
                | DW_FORM_DATA16
                | DW_FORM_STRING
                | DW_FORM_FLAG
                | DW_FORM_FLAG_PRESENT
                | DW_FORM_SDATA
                | DW_FORM_UDATA
                | DW_FORM_IMPLICIT_CONST => {}
                _ => return Err("a form not handled"),
            }
            match name {
                DW_AT_LOW_PC | DW_AT_ENTRY_PC | DW_AT_RANGES => die.has_addr = true,
                DW_AT_CONST_VALUE => die.const_value = true,
                _ => {}
            }
        }
        if !a.children {
            die.end = r.pos as u32;
        }
        dies.push(die);
        scope.push(is_scope);
        if a.children {
            stack.push(i);
        }
    }
    for p in stack {
        dies[p as usize].end = end as u32;
    }
    Ok((dies, ats))
}

/// A list of `__debug_ranges` or `__debug_loc`: where it starts and ends,
/// and its entries (offset, length, and [`RANGE`], [`EMPTY`], or
/// [`SELECT`]).
struct List {
    start: u32,
    end: u32,
    entries: Vec<(u32, u32, u8)>,
}

/// The lists of a DWARF 4 list section, in order (each to its end-of-list
/// entry).
fn lists(b: &[u8], address_size: u8, loc: bool) -> R<Vec<List>> {
    let bad = "a list that doesn't read";
    let n = usize::from(address_size);
    let max = if n == 8 {
        u64::MAX
    } else {
        u64::from(u32::MAX)
    };
    let mut out = Vec::new();
    let mut at = 0;
    let mut cur = List {
        start: 0,
        end: 0,
        entries: Vec::new(),
    };
    while at < b.len() {
        let s = read_le(b, at, n).ok_or(bad)?;
        let e = read_le(b, at + n, n).ok_or(bad)?;
        let mut len = 2 * n;
        if s == 0 && e == 0 {
            at += len;
            cur.end = at as u32;
            out.push(core::mem::replace(
                &mut cur,
                List {
                    start: at as u32,
                    end: 0,
                    entries: Vec::new(),
                },
            ));
            continue;
        }
        let select = s == max;
        if loc && !select {
            let x = read_le(b, at + len, 2).ok_or(bad)? as usize;
            let expr = b.get(at + len + 2..at + len + 2 + x).ok_or(bad)?;
            check_expr(expr, address_size)?;
            len += 2 + x;
        }
        cur.entries.push((
            at as u32,
            len as u32,
            if select {
                SELECT
            } else if s == e {
                EMPTY
            } else {
                RANGE
            },
        ));
        at += len;
    }
    if !cur.entries.is_empty() || at != b.len() {
        return Err(bad);
    }
    Ok(out)
}

/// An Apple accelerator table: its header (with its header data), the
/// offset of the DIE-offset atom in an entry, its base, and its
/// names in order: (hash, string offset, entries).
struct Apple {
    header: Vec<u8>,
    buckets: usize,
    die_at: usize,
    base: u32,
    names: Vec<(u32, u32, Vec<Vec<u8>>)>,
}

/// `HASH`.
const APPLE_MAGIC: u32 = 0x4841_5348;

fn apple(b: &[u8]) -> R<Apple> {
    let bad = "an accelerator table that doesn't read";
    let u32_at = |at: usize| read_le(b, at, 4).map(|v| v as u32).ok_or(bad);
    let u16_at = |at: usize| read_le(b, at, 2).map(|v| v as u16).ok_or(bad);
    if u32_at(0)? != APPLE_MAGIC || u16_at(4)? != 1 {
        return Err(bad);
    }
    let buckets = u32_at(8)? as usize;
    let hashes = u32_at(12)? as usize;
    let header_data = u32_at(16)? as usize;
    let base = u32_at(20)?;
    let atoms = u32_at(24)? as usize;
    if header_data != 8 + 4 * atoms {
        return Err(bad);
    }
    let mut die_at = None;
    let mut entry = 0;
    for k in 0..atoms {
        let kind = u16_at(28 + 4 * k)?;
        let form = u16_at(30 + 4 * k)?;
        let size = match form {
            DW_FORM_DATA1 | DW_FORM_FLAG => 1,
            DW_FORM_DATA2 => 2,
            DW_FORM_DATA4 => 4,
            DW_FORM_DATA8 => 8,
            _ => return Err("an accelerator table atom of a form not known"),
        };
        if kind == 1 {
            // DW_ATOM_die_offset.
            if size != 4 || die_at.is_some() {
                return Err(bad);
            }
            die_at = Some(entry);
        }
        entry += size;
    }
    let die_at = die_at.ok_or("an accelerator table without DIE offsets")?;
    let head = 20 + header_data;
    let hashes_at = head + 4 * buckets;
    let offsets_at = hashes_at + 4 * hashes;
    let mut names = Vec::new();
    let mut end = offsets_at + 4 * hashes;
    for h in 0..hashes {
        let hash = u32_at(hashes_at + 4 * h)?;
        let mut at = u32_at(offsets_at + 4 * h)? as usize;
        loop {
            let s = u32_at(at)?;
            at += 4;
            if s == 0 {
                break;
            }
            let n = u32_at(at)? as usize;
            at += 4;
            let mut v = Vec::with_capacity(n);
            for _ in 0..n {
                v.push(b.get(at..at + entry).ok_or(bad)?.to_vec());
                at += entry;
            }
            names.push((hash, s, v));
        }
        end = end.max(at);
    }
    if end != b.len() {
        return Err(bad);
    }
    Ok(Apple {
        header: b[..head].to_vec(),
        buckets,
        die_at,
        base,
        names,
    })
}

/// LLVM's bucket count for `n` distinct hashes.
fn bucket_count(n: usize) -> usize {
    if n > 1024 {
        n / 4
    } else if n > 16 {
        n / 2
    } else {
        n.max(1)
    }
}

/// Writes `t` with `names` (each its hash, string offset, and entries),
/// as LLVM's `AppleAccelTableWriter` lays a table out: with `buckets`
/// buckets, or LLVM's count for the hashes.
fn write_apple(t: &Apple, names: &[(u32, u32, Vec<Vec<u8>>)], buckets: Option<usize>) -> Vec<u8> {
    let distinct: BTreeSet<u32> = names.iter().map(|n| n.0).collect();
    let nb = buckets
        .unwrap_or_else(|| bucket_count(distinct.len()))
        .max(1);
    // Each bucket's names: by hash, then in their order.
    let mut order: Vec<usize> = (0..names.len()).collect();
    order.sort_by_key(|&i| (names[i].0 as usize % nb, names[i].0, i));
    let mut groups: Vec<(u32, Vec<usize>)> = Vec::new();
    for i in order {
        match groups.last_mut() {
            Some((h, v)) if *h == names[i].0 => v.push(i),
            _ => groups.push((names[i].0, vec![i])),
        }
    }
    let mut out = t.header.clone();
    out[8..12].copy_from_slice(&(nb as u32).to_le_bytes());
    out[12..16].copy_from_slice(&(groups.len() as u32).to_le_bytes());
    let mut buckets = vec![u32::MAX; nb];
    for (g, (h, _)) in groups.iter().enumerate().rev() {
        buckets[*h as usize % nb] = g as u32;
    }
    for b in buckets {
        out.extend_from_slice(&b.to_le_bytes());
    }
    for (h, _) in &groups {
        out.extend_from_slice(&h.to_le_bytes());
    }
    let mut data = Vec::new();
    let data_at = out.len() + 4 * groups.len();
    let mut offsets = Vec::with_capacity(groups.len());
    for (_, v) in &groups {
        offsets.push((data_at + data.len()) as u32);
        for &i in v {
            let (_, s, e) = &names[i];
            data.extend_from_slice(&s.to_le_bytes());
            data.extend_from_slice(&(e.len() as u32).to_le_bytes());
            for x in e {
                data.extend_from_slice(x);
            }
        }
        data.extend_from_slice(&0u32.to_le_bytes());
    }
    for o in offsets {
        out.extend_from_slice(&o.to_le_bytes());
    }
    out.extend_from_slice(&data);
    out
}

/// Prunes `o`'s DWARF (cut, its addresses moved): `gone` are the places
/// in `__debug_info` whose relocations into lost code were removed,
/// `gone_syms` the symbols of the lost units (a relocation naming one
/// names lost code). `None`: nothing to remove. On an error `o` may be
/// half-changed: the caller prunes a copy.
pub(super) fn prune(
    o: &mut Object<'_>,
    gone: &BTreeSet<u32>,
    gone_syms: &BTreeSet<u32>,
) -> R<Option<Pruned>> {
    if o.endian() != turbonop_core::Endian::Little {
        return Err("big-endian DWARF");
    }
    let mut idx: [Option<usize>; KNOWN.len()] = [None; KNOWN.len()];
    for (i, s) in o.sections().enumerate() {
        if s.segment_name() != b"__DWARF" {
            continue;
        }
        let k = KNOWN
            .iter()
            .position(|n| *n == s.name())
            .ok_or("a debug section not handled")?;
        if idx[k].is_some() {
            return Err("two debug sections of one name");
        }
        idx[k] = Some(i);
        if matches!(k, ABBREV | STR | 7..=10) && !s.relocations.is_empty() {
            return Err("relocations in a debug section that has none");
        }
    }
    let Some(info_i) = idx[INFO] else {
        return Ok(None);
    };
    let bytes = |k: usize| -> &[u8] {
        idx[k]
            .and_then(|i| o.section(i))
            .and_then(|s| s.bytes())
            .unwrap_or(&[])
    };
    let endian = turbonop_core::Endian::Little;
    let mut d = Sections::new(endian);
    d.info = bytes(INFO);
    d.abbrev = bytes(ABBREV);
    d.str = bytes(STR);
    d.ranges = bytes(RANGES);
    d.loc = bytes(LOC);
    let units: Vec<Unit<'_>> = Unit::iter(d, turbonop_dwarf::UnitSection::Info, 0).collect();
    let [u] = units.as_slice() else {
        return Err("not one compile unit");
    };
    if u.dwarf64 || u.version != 4 || u.offset != 0 || u.end as usize != d.info.len() {
        return Err("not one 32-bit DWARF 4 unit");
    }
    if d.info.len() > u32::MAX as usize / 2 {
        return Err("a __debug_info too large");
    }
    let abbrevs = Abbrevs::parse(d.abbrev, u.abbrev_offset, endian)
        .ok_or("an abbreviation table that doesn't read")?;
    // 1. The DIEs, and the values to set.
    let (dies, ats) = walk(u, &abbrevs)?;
    let die_at = |off: u32| -> Option<usize> {
        let i = dies.partition_point(|x| x.offset < off);
        dies.get(i).filter(|x| x.offset == off).map(|_| i)
    };
    // The DIE whose attributes hold `pos`.
    let die_of =
        |pos: u32| -> usize { dies.partition_point(|x| x.offset <= pos).saturating_sub(1) };
    let n = dies.len();
    // 2. Each entity's relocations: into lost code, or into what stays.
    let mut gone_n = vec![0u32; n];
    let mut live_n = vec![0u32; n];
    let info_s = o.section(info_i).ok_or("no __debug_info")?;
    for &p in gone {
        let e = dies[die_of(p)].ent;
        if e != NONE {
            gone_n[e as usize] += 1;
        }
    }
    let mut live_places = Vec::new();
    for r in &info_s.relocations {
        let e = dies[die_of(r.address)].ent;
        let dead = r.external && gone_syms.contains(&r.symbolnum);
        if !dead {
            live_places.push(r.address);
        }
        if e == NONE {
            continue;
        }
        if dead {
            gone_n[e as usize] += 1;
        } else {
            live_n[e as usize] += 1;
        }
    }
    // The lists.
    let range_lists = lists(d.ranges, u.address_size, false)?;
    let loc_lists = lists(d.loc, u.address_size, true)?;
    let list_at = |v: &[List], off: u32| -> R<usize> {
        let i = v.partition_point(|l| l.start < off);
        match v.get(i) {
            Some(l) if l.start == off => Ok(i),
            _ => Err("a list offset not at a list's start"),
        }
    };
    let mut nonempty = vec![false; n];
    for a in &ats {
        let (v, off) = match a.fix {
            Fix::Ranges(off) => (&range_lists, off),
            Fix::Loc(off) => (&loc_lists, off),
            _ => continue,
        };
        let l = &v[list_at(v, off)?];
        let e = dies[a.die as usize].ent;
        if e != NONE && l.entries.iter().any(|x| x.2 == RANGE) {
            nonempty[e as usize] = true;
        }
    }
    // 3. References between entities (and from scopes).
    let mut edges: Vec<Vec<u32>> = vec![Vec::new(); n];
    let mut from_scopes: Vec<u32> = Vec::new();
    let mut ns_refs: Vec<(u32, u32)> = Vec::new(); // (source DIE, namespace)
    for a in &ats {
        let Fix::Ref { target, sibling } = a.fix else {
            continue;
        };
        if sibling {
            continue;
        }
        let t = die_at(target).ok_or("a reference to no DIE")? as u32;
        let src = dies[a.die as usize].ent;
        let te = dies[t as usize].ent;
        if te == NONE {
            ns_refs.push((a.die, t));
            continue;
        }
        if src == NONE {
            from_scopes.push(te);
        } else if src != te {
            edges[src as usize].push(te);
        }
    }
    // 4. Dead entities, what they reach, and what stays.
    let is_ent = |i: usize| dies[i].ent == i as u32;
    let dead: Vec<bool> = (0..n)
        .map(|i| {
            is_ent(i)
                && matches!(dies[i].tag, DW_TAG_SUBPROGRAM | DW_TAG_VARIABLE)
                && gone_n[i] > 0
                && live_n[i] == 0
                && !(dies[i].tag == DW_TAG_VARIABLE && dies[i].const_value)
                && !nonempty[i]
        })
        .collect();
    if !dead.iter().any(|&x| x) {
        return Ok(None);
    }
    let mut candidate = dead.clone();
    let mut seen = dead.clone();
    let mut work: Vec<u32> = (0..n as u32).filter(|&i| dead[i as usize]).collect();
    while let Some(e) = work.pop() {
        for &t in &edges[e as usize] {
            let t = t as usize;
            if seen[t] {
                continue;
            }
            seen[t] = true;
            work.push(t as u32);
            let x = &dies[t];
            candidate[t] = gone_n[t] == 0
                && live_n[t] == 0
                && !nonempty[t]
                && (is_type(x.tag) || (x.tag == DW_TAG_SUBPROGRAM && !x.has_addr));
        }
    }
    let mut kept: Vec<bool> = (0..n).map(|i| is_ent(i) && !candidate[i]).collect();
    let mut work: Vec<u32> = (0..n as u32).filter(|&i| kept[i as usize]).collect();
    for &t in &from_scopes {
        if !kept[t as usize] {
            kept[t as usize] = true;
            work.push(t);
        }
    }
    while let Some(e) = work.pop() {
        for &t in &edges[e as usize] {
            if !kept[t as usize] {
                kept[t as usize] = true;
                work.push(t);
            }
        }
    }
    let mut stats = Pruned::default();
    let mut removed = vec![false; n];
    for i in 0..n {
        if is_ent(i) && !kept[i] {
            removed[i] = true;
            if dead[i] {
                stats.dead += 1;
            } else {
                stats.reached += 1;
            }
        } else if dead[i] {
            stats.dead_kept += 1;
        }
    }
    if stats.dead == 0 {
        return Ok(None);
    }
    // Namespaces that lost every child, and that nothing kept names.
    let mut ns_named = vec![false; n];
    for &(src, t) in &ns_refs {
        let e = dies[src as usize].ent;
        if e == NONE || kept[e as usize] {
            ns_named[t as usize] = true;
        }
    }
    let mut children = vec![0u32; n];
    let mut alive = vec![0u32; n];
    for i in (1..n).rev() {
        let x = &dies[i];
        if x.tag == DW_TAG_NAMESPACE && x.ent == NONE {
            removed[i] = children[i] > 0 && alive[i] == 0 && !ns_named[i];
            if removed[i] {
                stats.reached += 1;
            }
        }
        if x.ent == NONE || x.ent == i as u32 {
            let p = x.parent as usize;
            children[p] += 1;
            if !removed[i] {
                alive[p] += 1;
            }
        }
    }
    // 5. What `__debug_info` loses.
    let mut cut_info: Vec<Range<u32>> = Vec::new();
    for i in 0..n {
        if !removed[i] {
            continue;
        }
        let r = dies[i].offset..dies[i].end;
        if cut_info.last().is_some_and(|l| l.end > r.start) {
            continue; // (Inside a namespace removed.)
        }
        match cut_info.last_mut() {
            Some(l) if l.end == r.start => l.end = r.end,
            _ => cut_info.push(r),
        }
    }
    let info = Shift::new(cut_info);
    if live_places.iter().any(|&p| info.removed(p)) {
        return Err("a relocation to what stays in a DIE removed");
    }
    let kept_die = |i: usize| !info.removed(dies[i].offset);
    // 6. Strings, lists, and accelerator tables: what kept DIEs name.
    let str_b = d.str;
    let mut strings: Vec<Range<u32>> = Vec::new();
    {
        let mut at = 0;
        while at < str_b.len() {
            let z = str_b[at..]
                .iter()
                .position(|&c| c == 0)
                .ok_or("a __debug_str that doesn't end a string")?;
            strings.push(at as u32..(at + z + 1) as u32);
            at += z + 1;
        }
    }
    let string_of = |off: u32| -> R<usize> {
        let k = strings.partition_point(|r| r.end <= off);
        match strings.get(k) {
            Some(r) if r.start <= off => Ok(k),
            _ => Err("a string offset past __debug_str"),
        }
    };
    let mut str_used = vec![false; strings.len()];
    if let Some(x) = str_used.first_mut() {
        *x = true;
    }
    let mut ranges_used = vec![false; range_lists.len()];
    let mut loc_used = vec![false; loc_lists.len()];
    for a in &ats {
        if !kept_die(a.die as usize) {
            continue;
        }
        match a.fix {
            Fix::Strp(s) => str_used[string_of(s)?] = true,
            Fix::Ranges(off) => ranges_used[list_at(&range_lists, off)?] = true,
            Fix::Loc(off) => loc_used[list_at(&loc_lists, off)?] = true,
            Fix::Ref { target, sibling } => {
                if !sibling && info.removed(target) {
                    return Err("a kept DIE names a removed one");
                }
            }
        }
    }
    // (Each table's names: hash, string offset, entries.)
    type Names = Vec<(u32, u32, Vec<Vec<u8>>)>;
    let mut pending: Vec<(usize, Apple, Names, usize)> = Vec::new();
    for k in APPLE {
        let Some(si) = idx[k] else { continue };
        let b = bytes(k);
        let t = apple(b)?;
        let mut names: Names = Vec::with_capacity(t.names.len());
        for (hash, s, entries) in &t.names {
            let mut v = Vec::with_capacity(entries.len());
            for e in entries {
                let off = read_le(e, t.die_at, 4).unwrap_or(0) as u32;
                let off = off
                    .checked_add(t.base)
                    .ok_or("an accelerator table entry past __debug_info")?;
                let i = die_at(off).ok_or("an accelerator table entry naming no DIE")?;
                if !kept_die(i) {
                    continue;
                }
                let mut e = e.clone();
                let new = info
                    .map(off)
                    .checked_sub(t.base)
                    .ok_or("an accelerator table entry before its base")?;
                write_le(&mut e, t.die_at, 4, u64::from(new));
                v.push(e);
            }
            if !v.is_empty() {
                str_used[string_of(*s)?] = true;
                names.push((*hash, *s, v));
            }
        }
        pending.push((si, t, names, b.len()));
    }
    let mut str_cuts: Vec<Range<u32>> = Vec::new();
    for (r, _) in strings.iter().zip(&str_used).filter(|(_, u)| !**u) {
        match str_cuts.last_mut() {
            Some(l) if l.end == r.start => l.end = r.end,
            _ => str_cuts.push(r.clone()),
        }
    }
    let strs = Shift::new(str_cuts);
    let list_cuts = |v: &[List], used: &[bool]| -> Vec<Range<u32>> {
        let mut out: Vec<Range<u32>> = Vec::new();
        let mut push = |r: Range<u32>| match out.last_mut() {
            Some(l) if l.end == r.start => l.end = r.end,
            _ => out.push(r),
        };
        for (l, &u) in v.iter().zip(used) {
            if !u {
                push(l.start..l.end);
                continue;
            }
            for &(at, len, kind) in &l.entries {
                if kind == EMPTY {
                    push(at..at + len);
                }
            }
        }
        out
    };
    let ranges = Shift::new(list_cuts(&range_lists, &ranges_used));
    let locs = Shift::new(list_cuts(&loc_lists, &loc_used));
    // 7. The values to set, at their new places in `__debug_info`.
    let mut patches: Vec<(u32, u8, u64)> = Vec::new();
    for a in &ats {
        if !kept_die(a.die as usize) {
            continue;
        }
        let v = match a.fix {
            Fix::Ref { target, .. } => {
                // (Unit-relative, the unit at 0: the same as a section offset.)
                u64::from(info.map(target))
            }
            Fix::Strp(s) => u64::from(strs.map(s)),
            Fix::Ranges(off) => u64::from(ranges.map(off)),
            Fix::Loc(off) => u64::from(locs.map(off)),
        };
        if a.size < 8 && v >> (8 * u32::from(a.size)) != 0 {
            return Err("a reference too large for its form");
        }
        patches.push((info.map(a.pos), a.size, v));
    }
    let info_len = d.info.len() as u32 - info.total();
    patches.push((0, 4, u64::from(info_len - 4)));
    // The accelerator tables, their string offsets moved.
    let mut apple_out: Vec<(usize, Vec<u8>, usize)> = Vec::new();
    for (si, t, names, old_len) in pending {
        let names: Names = names
            .into_iter()
            .map(|(h, s, e)| (h, strs.map(s), e))
            .collect();
        let mut b = write_apple(&t, &names, None);
        if b.len() > old_len {
            // (LLVM's count for fewer hashes may be larger: 16 buckets for
            // 16 hashes, 8 for 17. The original count, then.)
            b = write_apple(&t, &names, Some(t.buckets));
        }
        if b.len() > old_len {
            return Err("an accelerator table that grew");
        }
        apple_out.push((si, b, old_len));
    }
    // 8. Cut, then set.
    let before: u64 = o
        .sections()
        .filter(|s| s.segment_name() == b"__DWARF")
        .map(turbonop_macho_object::Section::size)
        .sum();
    let mut cuts: Vec<(usize, Vec<Range<u64>>)> = Vec::new();
    let wide = |s: &Shift| -> Vec<Range<u64>> {
        s.ranges
            .iter()
            .map(|r| u64::from(r.start)..u64::from(r.end))
            .collect()
    };
    cuts.push((info_i, wide(&info)));
    for (k, s) in [(STR, &strs), (RANGES, &ranges), (LOC, &locs)] {
        if let Some(si) = idx[k]
            && !s.ranges.is_empty()
        {
            cuts.push((si, wide(s)));
        }
    }
    for (si, b, old_len) in &apple_out {
        if b.len() < *old_len {
            cuts.push((
                *si,
                std::iter::once(b.len() as u64..*old_len as u64).collect(),
            ));
        }
    }
    cuts.sort_by_key(|c| c.0);
    let c = o
        .cut(&cuts)
        .map_err(|_| "a cut of debug sections refused")?;
    if !c.dangling.is_empty() || !c.unadjusted.is_empty() || !c.orphaned_symbols.is_empty() {
        return Err("a cut of debug sections that moved what it couldn't");
    }
    let s = o.sections_mut().nth(info_i).ok_or("no __debug_info")?;
    let Contents::Bytes(b) = &mut s.contents else {
        return Err("a __debug_info without bytes");
    };
    let b = b.to_mut();
    for (at, size, v) in patches {
        write_le(b, at as usize, usize::from(size), v);
    }
    for (si, bytes, _) in apple_out {
        let s = o.sections_mut().nth(si).ok_or("no section")?;
        s.contents = Contents::Bytes(std::borrow::Cow::Owned(bytes));
    }
    let after: u64 = o
        .sections()
        .filter(|s| s.segment_name() == b"__DWARF")
        .map(turbonop_macho_object::Section::size)
        .sum();
    stats.bytes = before - after;

    Ok(Some(stats))
}

/// Each Apple accelerator table of `o` (by section name), and whether it
/// reads and [`write_apple`] writes it again as the same bytes (with
/// LLVM's bucket count): for checking the writer against LLVM's on a
/// corpus.
pub fn apple_round_trip(o: &Object<'_>) -> Vec<(Vec<u8>, R<bool>)> {
    let mut out = Vec::new();
    for s in o.sections() {
        if s.segment_name() != b"__DWARF" || !KNOWN[APPLE].contains(&s.name()) {
            continue;
        }
        let b = s.bytes().unwrap_or(&[]);
        let r = apple(b).map(|t| write_apple(&t, &t.names, None) == b);
        out.push((s.name().to_vec(), r));
    }
    out
}

#[cfg(test)]
mod tests;
