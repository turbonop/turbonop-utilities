//! The class definitions a giver's DWARF still gives every dSYM of a link
//! that loads it, which a synthesized member it gave units to may then
//! hold as declarations.
//!
//! dsymutil (the classic linker, which Apple's uses; LLVM's
//! `DWARFLinker/Classic/DWARFLinker.cpp`) decides what of an object's
//! DWARF to keep by walking every DIE of each unit in order
//! (`lookForDIEsToKeep`) from these roots (`shouldKeepDIE`):
//!
//! - subprograms and labels whose `DW_AT_low_pc` has a relocation to an
//!   address of the debug map (linked code);
//! - variables and constants outside functions with a `DW_AT_const_value`,
//!   or a location whose address is relocated to linked data;
//! - base types, `DW_TAG_imported_module`, `DW_TAG_imported_declaration`,
//!   and `DW_TAG_imported_unit`, always.
//!
//! A kept DIE keeps its children (the walk passes its "keep" flag down);
//! its ancestors (`lookForParentDIEsToKeep`), and of those the classes,
//! structures, unions, enumerations, subprograms, lexical blocks,
//! subroutine types, and common blocks with all their children
//! (`dieNeedsChildrenToBeMeaningful`); and every DIE its references name
//! (`lookForRefDIEsToKeep`: every reference attribute but
//! `DW_AT_sibling`), with their children, unless the reference is an ODR
//! one (`DW_AT_type`, `DW_AT_specification`, `DW_AT_abstract_origin`,
//! `DW_AT_import`, ...) to a type whose declaration context (see
//! [`crate::dwarf::key`]) already has a canonical definition: then the
//! reference is pointed at that one. A referenced DIE already kept isn't
//! walked again, so a namespace a `DW_TAG_imported_module` names
//! (`using namespace llvm;`) is kept with every child (classes and
//! subprograms nothing else reaches) only when nothing in it was kept
//! before the import: this is why a giver's class definition that no kept
//! DIE named was in a dSYM (7.5).
//!
//! What this computes is a part of what dsymutil puts in the dSYM of every
//! link that loads a giver with all its code (no `-dead_strip`): the roots
//! its DWARF still has once its units are cut out (DIEs whose relocations
//! name what stays), what they keep by the rules above, and, where an ODR
//! reference names a DIE that may have a declaration context, that DIE
//! and its ancestors, but nothing further. dsymutil keeps such a DIE from
//! this object (whole, with what it reaches) only if no earlier object
//! defined its context; otherwise it is the earlier object's definition
//! that is in the dSYM, with what that version reaches, which may be less
//! (a class template specialization's members are those its unit used).
//! A first version that followed such references to the end lost 12 type
//! definitions per corpus link, and changed 300 to 500 others' members.
//! A namespace's other children aren't assumed either (an import keeps
//! them only in some links). The result is the keys of the named classes,
//! structures, and unions in named scopes defined among what it reaches.
//!
//! When the defining object is the giver itself or an earlier one, it is
//! so in the original link too, so the dSYM's definition is the one it was
//! (the same object's, by the same rules), as long as the objects load in
//! the same order (which `relink.py` checks for the corpus).

use std::collections::BTreeSet;

use turbonop_dwarf::consts::*;
use turbonop_dwarf::{Abbrevs, Reader, Sections, Unit};
use turbonop_macho_object::Object;

use crate::dwarf::key;

type R<T> = Result<T, &'static str>;
type HashSet<K> = turbonop_hash::Set<K>;

const NONE: u32 = u32::MAX;

struct Die {
    offset: u32,
    end: u32,
    parent: u32,
    tag: u16,
    declaration: bool,
    const_value: bool,
    /// Inside a subprogram (dsymutil's `TF_InFunctionScope`).
    in_function: bool,
    /// Whether dsymutil may give it a declaration context (and so point an
    /// ODR reference to it at another object's DIE instead of keeping it).
    context: bool,
    /// Its key, if a class, structure, union, or namespace in named scopes
    /// (a class a debugger finds by name, or a scope of one).
    key: Option<u128>,
}

/// Whether a DIE of `tag` is kept with all its children when a DIE below it
/// is kept (dsymutil's `dieNeedsChildrenToBeMeaningful`).
fn needs_children(tag: u16) -> bool {
    matches!(
        tag,
        DW_TAG_CLASS_TYPE
            | DW_TAG_COMMON_BLOCK
            | DW_TAG_ENUMERATION_TYPE
            | DW_TAG_LEXICAL_BLOCK
            | DW_TAG_STRUCTURE_TYPE
            | DW_TAG_SUBPROGRAM
            | DW_TAG_SUBROUTINE_TYPE
            | DW_TAG_UNION_TYPE
    )
}

/// `DW_AT_LLVM_alloc_type`.
const DW_AT_LLVM_ALLOC_TYPE: u16 = 0x3E0E;

/// Whether a reference by attribute `name` is one dsymutil may point at
/// another object's DIE of the same declaration context (`isODRAttribute`).
fn is_odr(name: u16) -> bool {
    matches!(
        name,
        DW_AT_TYPE
            | DW_AT_CONTAINING_TYPE
            | DW_AT_SPECIFICATION
            | DW_AT_ABSTRACT_ORIGIN
            | DW_AT_IMPORT
            | DW_AT_LLVM_ALLOC_TYPE
    )
}

/// Whether dsymutil may give a DIE of `tag` a declaration context, when its
/// parent has one (`DeclContextTree::getChildDeclContext`; it may also
/// decline, for an artificial DIE, a local function's, or one seen twice
/// in a unit, which only makes it kept where this assumes it may not be).
fn may_have_context(tag: u16) -> bool {
    matches!(
        tag,
        DW_TAG_SUBPROGRAM
            | DW_TAG_MEMBER
            | DW_TAG_NAMESPACE
            | DW_TAG_STRUCTURE_TYPE
            | DW_TAG_CLASS_TYPE
            | DW_TAG_UNION_TYPE
            | DW_TAG_ENUMERATION_TYPE
            | DW_TAG_TYPEDEF
            | DW_TAG_MODULE
    )
}

fn aggregate(tag: u16) -> bool {
    matches!(
        tag,
        DW_TAG_CLASS_TYPE | DW_TAG_STRUCTURE_TYPE | DW_TAG_UNION_TYPE
    )
}

/// The keys of the class definitions (in named scopes) that the DWARF of
/// `o` gives the dSYM of every link that loads it whole; `gone_syms` are
/// its symbols that name moved code (x86-64's debug relocations to moved
/// functions are external). `Ok(None)` if it has no DWARF.
pub(super) fn reach(o: &Object<'_>, gone_syms: &BTreeSet<u32>) -> R<Option<HashSet<u128>>> {
    if o.endian() != turbonop_core::Endian::Little {
        return Err("big-endian DWARF");
    }
    let mut info_i = None;
    let mut d = Sections::new(turbonop_core::Endian::Little);
    for (i, s) in o.sections().enumerate() {
        if s.segment_name() != b"__DWARF" {
            continue;
        }
        let b = s.bytes().unwrap_or(&[]);
        match s.name() {
            b"__debug_info" => {
                d.info = b;
                info_i = Some(i);
            }
            b"__debug_abbrev" => d.abbrev = b,
            b"__debug_str" => d.str = b,
            b"__debug_line" => d.line = b,
            _ => {}
        }
    }
    let Some(info_i) = info_i else {
        return Ok(None);
    };
    let units: Vec<Unit<'_>> = Unit::iter(d, turbonop_dwarf::UnitSection::Info, 0).collect();
    let [u] = units.as_slice() else {
        return Err("not one compile unit");
    };
    if u.dwarf64 || !(2..=4).contains(&u.version) || u.offset != 0 {
        return Err("not one 32-bit DWARF 2 to 4 unit");
    }
    if d.info.len() > u32::MAX as usize / 2 {
        return Err("a __debug_info too large");
    }
    let abbrevs = Abbrevs::parse(d.abbrev, u.abbrev_offset, d.endian)
        .ok_or("an abbreviation table that doesn't read")?;
    // 1. The DIEs, their references, and the keys of classes and scopes.
    let bad = "a DIE that doesn't read";
    let mut r = Reader::new(d.info, u.dies as usize, d.endian);
    let mut dies: Vec<Die> = Vec::new();
    // (Each DIE's references, as offsets: `refs[ref_at[i]..ref_at[i + 1]]`.)
    let mut refs: Vec<(u32, bool)> = Vec::new();
    let mut ref_at: Vec<u32> = Vec::new();
    // Whether names inside each DIE can be looked up from anywhere.
    let mut open: Vec<bool> = Vec::new();
    let mut stack: Vec<u32> = Vec::new();
    let mut files: Option<Vec<u128>> = None;
    let end = u.end as usize;
    while r.pos < end {
        let at = r.pos as u32;
        let code = r.uleb().ok_or(bad)?;
        if code == 0 {
            if let Some(p) = stack.pop() {
                dies[p as usize].end = r.pos as u32;
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
        ref_at.push(refs.len() as u32);
        let (mut name, mut linkage) = (None, None);
        let (mut size, mut file, mut line) = (None, 0u64, 0u64);
        let (mut declaration, mut const_value) = (false, false);
        let (mut stmt, mut comp_dir) = (None, None);
        for spec in &a.attrs {
            let v = u.read_value(&mut r, spec.form, spec.implicit).ok_or(bad)?;
            match spec.form {
                DW_FORM_REF1 | DW_FORM_REF2 | DW_FORM_REF4 | DW_FORM_REF8 | DW_FORM_REF_UDATA
                | DW_FORM_REF_ADDR
                    if spec.name != DW_AT_SIBLING =>
                {
                    let t = u
                        .reference(&turbonop_dwarf::Attr {
                            name: spec.name,
                            form: spec.form,
                            value: v,
                        })
                        .ok_or("a bad reference")?;
                    let odr = spec.form != DW_FORM_REF_ADDR && is_odr(spec.name);
                    refs.push((u32::try_from(t).map_err(|_| "a bad reference")?, odr));
                }
                _ => {}
            }
            match spec.name {
                DW_AT_NAME => name = u.string(&v).map(|s| s.bytes),
                DW_AT_LINKAGE_NAME | DW_AT_MIPS_LINKAGE_NAME => {
                    linkage = u.string(&v).map(|s| s.bytes);
                }
                DW_AT_BYTE_SIZE => size = u.uint(&v),
                DW_AT_DECL_FILE => file = u.uint(&v).unwrap_or(0),
                DW_AT_DECL_LINE => line = u.uint(&v).unwrap_or(0),
                DW_AT_DECLARATION => declaration = u.uint(&v).is_none_or(|x| x != 0),
                DW_AT_CONST_VALUE => const_value = true,
                DW_AT_STMT_LIST if parent == NONE => stmt = u.uint(&v),
                DW_AT_COMP_DIR if parent == NONE => comp_dir = u.string(&v).map(|s| s.bytes),
                _ => {}
            }
        }
        if parent == NONE {
            files = stmt.and_then(|off| key::files(d.line, off, comp_dir.unwrap_or(b"")));
        }
        let parent_open = parent == NONE || open[parent as usize];
        let named = name.is_some();
        let keyed = parent != NONE
            && parent_open
            && named
            && (aggregate(a.tag) || a.tag == DW_TAG_NAMESPACE);
        let key = keyed.then(|| {
            let pk = match dies[parent as usize].key {
                Some(k) => k,
                None => key::ROOT,
            };
            let f = files
                .as_ref()
                .and_then(|v| v.get(usize::try_from(file).ok()?))
                .copied()
                .unwrap_or(0);
            let (sz, f, l) = if a.tag == DW_TAG_NAMESPACE {
                (None, 0, 0)
            } else {
                (size, f, line)
            };
            key::of(pk, a.tag, linkage.or(name).unwrap_or(b""), sz, f, l)
        });
        open.push(match a.tag {
            DW_TAG_COMPILE_UNIT => true,
            _ => key.is_some(),
        });
        let context = parent != NONE
            && may_have_context(a.tag)
            && (parent == 0 || dies[parent as usize].context);
        let in_function = parent != NONE && {
            let p = &dies[parent as usize];
            p.in_function || p.tag == DW_TAG_SUBPROGRAM
        };
        dies.push(Die {
            offset: at,
            end: if a.children { 0 } else { r.pos as u32 },
            parent,
            tag: a.tag,
            declaration,
            const_value,
            in_function,
            context,
            key,
        });
        if a.children {
            stack.push(i);
        }
    }
    for p in stack {
        dies[p as usize].end = end as u32;
    }
    ref_at.push(refs.len() as u32);
    let n = dies.len();
    let die_at = |off: u32| -> Option<usize> {
        let k = dies.partition_point(|x| x.offset < off);
        (k < n && dies[k].offset == off).then_some(k)
    };
    // 2. The DIEs holding relocations to what stays.
    let info_s = o.section(info_i).ok_or("no __debug_info")?;
    let mut live: Vec<u32> = info_s
        .relocations
        .iter()
        .filter(|r| !(r.external && gone_syms.contains(&r.symbolnum)))
        .map(|r| r.address)
        .collect();
    live.sort_unstable();
    let mut relocated = vec![false; n];
    {
        let mut k = 0;
        for p in live {
            while k + 1 < n && dies[k + 1].offset <= p {
                k += 1;
            }
            relocated[k] = true;
        }
    }
    // 3. The roots, then what they keep.
    // (`kept`: kept from this object in every such link; `defined`: in the
    // dSYM, from this object or an earlier one.)
    let mut kept = vec![false; n];
    let mut defined = vec![false; n];
    let mut work: Vec<usize> = Vec::new();
    // Keeps DIE `i` and its subtree.
    let keep_tree = |i: usize, kept: &mut Vec<bool>, work: &mut Vec<usize>| {
        let end = dies[i].end;
        let mut j = i;
        while j < n && (j == i || dies[j].offset < end) {
            if !kept[j] {
                kept[j] = true;
                work.push(j);
            }
            j += 1;
        }
    };
    for (i, x) in dies.iter().enumerate().skip(1) {
        let root = match x.tag {
            DW_TAG_SUBPROGRAM | DW_TAG_LABEL => relocated[i],
            DW_TAG_VARIABLE | DW_TAG_CONSTANT => !x.in_function && (x.const_value || relocated[i]),
            DW_TAG_BASE_TYPE
            | DW_TAG_IMPORTED_MODULE
            | DW_TAG_IMPORTED_DECLARATION
            | DW_TAG_IMPORTED_UNIT => true,
            _ => false,
        };
        if root && !kept[i] {
            keep_tree(i, &mut kept, &mut work);
        }
    }
    while let Some(i) = work.pop() {
        // Its parent (and so, in turn, its ancestors).
        let p = dies[i].parent;
        if p != NONE && p != 0 && !kept[p as usize] {
            let pi = p as usize;
            if needs_children(dies[pi].tag) {
                keep_tree(pi, &mut kept, &mut work);
            } else {
                kept[pi] = true;
                work.push(pi);
            }
        }
        // What it refers to. An ODR reference to a DIE that may have a
        // declaration context is followed only if no earlier object of the
        // link has defined that context: either way the DIE and its
        // ancestors are in the dSYM, but what it reaches in turn may be
        // another object's version's, so nothing further is assumed.
        // Otherwise the DIE is kept here, with its subtree.
        for k in ref_at[i]..ref_at[i + 1] {
            let (off, odr) = refs[k as usize];
            let t = die_at(off).ok_or("a reference to no DIE")?;
            if odr && dies[t].context {
                defined[t] = true;
            } else if !kept[t] {
                keep_tree(t, &mut kept, &mut work);
            }
        }
    }
    Ok(Some(
        (0..n)
            .filter(|&i| (kept[i] || defined[i]) && aggregate(dies[i].tag) && !dies[i].declaration)
            .filter_map(|i| dies[i].key)
            .collect(),
    ))
}
