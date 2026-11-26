//! Folding a Mach-O static library's duplicate weak definitions
//! (`--fold=identical`).
//!
//! Mach-O has no COMDAT groups. What the linker coalesces are **weak
//! definitions** (`N_WEAK_DEF`; inline functions, template instantiations,
//! vtables, typeinfo, and static data of inline functions), each the atom
//! it starts in a section of an object marked
//! `MH_SUBSECTIONS_VIA_SYMBOLS`: an atom runs from its symbol to the next
//! symbol of the section that isn't an `N_ALT_ENTRY` (or the section's
//! end), as ld64 and ld64.lld split sections.
//!
//! **Units.** A unit is such an atom whose external symbols are all weak
//! definitions, in a section that isn't a literal, pointer, unwind, debug,
//! or initializer section: its bytes (or zero-fill size), the relocations
//! whose place is in it (whole pairs), the symbols in it (locals too), and
//! its `__LD,__compact_unwind` entries (those whose function address is
//! in it). Its identity hash covers the section's names, flags, and
//! alignment, the bytes, each relocation as (offset in the atom, type,
//! length, PC-relativeness, target: an external name, or the offset of a
//! local symbol of the atom), the symbols (names, types, descriptions,
//! offsets), the unwind entries (offset of the function, length,
//! encoding, personality by name), and the member's environment.
//!
//! **The environment** is what a synthesized member must carry for its
//! units: the CPU type and subtype, the header's flags, and the
//! `LC_BUILD_VERSION` (or `LC_VERSION_MIN_*`) command's bytes. Only units
//! of one environment fold together, and each synthesized member copies
//! its environment's.
//!
//! **What stops a unit folding**: a relocation in it reaching a local
//! symbol outside it (a string literal's `l_.str`, a `static` function) or
//! naming a section (a local relocation: x86-64's references to `L`
//! labels); something outside it reaching in, but for its unwind entries
//! and the debug sections (a local relocation into it, an external one to
//! a local symbol of it); an unwind entry with an LSDA; an `__eh_frame`
//! FDE for it; a data-in-code entry or an optimization hint in it; a
//! relocation into its section whose address isn't decoded.
//!
//! **What folds** follows the ELF engine ([`crate::elf`]): the copies of
//! a unit fold when every definition of each of their names in the
//! library is such a copy, and, under `--pull=preserve`, the member the
//! archive index sends each name to (its first definer) has no side
//! effects: initializer or terminator sections (`S_MOD_INIT_FUNC_POINTERS`,
//! `S_MOD_TERM_FUNC_POINTERS`, `S_INIT_FUNC_OFFSETS`,
//! `S_THREAD_LOCAL_INIT_FUNCTION_POINTERS`), Objective-C class, category,
//! and protocol lists, `LC_LINKER_OPTION` commands (auto-linked
//! libraries), or a section of a name the linkers give no meaning to
//! (which `section$start$` symbols could collect). A unit naming a name
//! its member defines and keeps must stay too, to a fixpoint (a
//! synthesized member's reference to such a name could be resolved from
//! another library's member). Initializers themselves never move: their
//! entries are atoms of `__mod_init_func` with no symbol, which stay in
//! their member, so a member's initializers keep their order.
//!
//! **Rewriting.** The first copy survives into a synthesized member (one
//! per set of members that held the copies, and environment), with its
//! symbols and unwind entries. Each member that held a copy loses the
//! atoms (cut out of their sections, everything after them moved back:
//! [`turbonop_macho_object::Object::cut`]) and their unwind entries; its
//! definitions become weak undefined references, and it gains a strong
//! undefined reference to the synthesized member's anchor (a private
//! external absolute symbol named for the synthesized member's contents
//! and the library, so that two libraries' never collide), which the
//! symbol table's order (LLVM's: undefined names sorted) puts before every
//! other undefined name. ld64.lld resolves an object's undefined names in
//! that order as it loads the object, loading archive members at once,
//! and a weak reference loads a member as a strong one does: with the
//! anchor first, the synthesized member is loaded, and the moved names
//! defined, before the moved names are looked up, so they aren't taken
//! from another library's copy. Synthesized members come first in the
//! archive, so their names are in the linker's table before a member of
//! the same archive loaded while the archive is read looks for them. A
//! synthesized member refers to names moved into another the same way.
//!
//! **Debug information.** A member that gave a copy up loses each debug
//! section's local relocation to it (and its place is zeroed), so
//! `dsymutil`, which reads the DWARF from the member through the
//! executable's debug map, finds no address for the copy's DIEs and
//! drops them, as it does for every copy but the linked one. Synthesized
//! members carry no DWARF yet: a folded function has none in the linked
//! program's dSYM ([`Report::debug_dropped`]).

use std::borrow::Cow;
use std::collections::{BTreeMap, BTreeSet};
use std::ops::Range;

use turbonop_macho_object::consts::{
    LC_BUILD_VERSION, LC_LINKER_OPTION, LC_VERSION_MIN_IPHONEOS, LC_VERSION_MIN_MACOSX,
    LC_VERSION_MIN_TVOS, LC_VERSION_MIN_WATCHOS, MH_OBJECT, N_ABS, N_EXT, N_PEXT, N_SECT, N_TYPE,
    N_UNDF, S_ATTR_DEBUG, S_INIT_FUNC_OFFSETS, S_MOD_INIT_FUNC_POINTERS, S_MOD_TERM_FUNC_POINTERS,
    S_THREAD_LOCAL_INIT_FUNCTION_POINTERS,
};
use turbonop_macho_object::{
    Command, Contents, Header, MH_SUBSECTIONS_VIA_SYMBOLS, Object, Relocation, Section, Segment,
    Symbol, name_field, reloc,
};

use crate::hash::H128;

/// `N_WEAK_REF`: an undefined name that may stay undefined.
pub const N_WEAK_REF: u16 = 0x40;
/// `N_WEAK_DEF`: a definition the linker may coalesce.
pub const N_WEAK_DEF: u16 = 0x80;
/// `N_ALT_ENTRY`: a symbol that doesn't start an atom.
pub const N_ALT_ENTRY: u16 = 0x200;
/// The prefix of anchors' names: `!` sorts before the names compilers
/// make (`_`, letters, `l`, `L`), so an anchor is the first undefined name
/// of a member that refers to it.
pub const ANCHOR_PREFIX: &[u8] = b"!turbolib.anchor.";

/// The local label of an alignment gap between a synthesized member's
/// units (an atom of its own).
pub const PAD: &[u8] = b"l_turbolib.pad";

/// `--pull`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Pull {
    /// Fold no unit whose first definer has side effects.
    Preserve,
    /// Fold regardless.
    Relaxed,
}

/// How to fold.
#[derive(Clone, Copy, Debug)]
pub struct Options<'a> {
    pub pull: Pull,
    /// What tells this library's anchors from another's (its file name):
    /// part of each anchor's name.
    pub salt: &'a [u8],
}

/// A member of the library.
#[derive(Clone, Copy, Debug)]
pub struct Input<'a> {
    pub name: &'a [u8],
    pub data: &'a [u8],
}

/// A member of the library written.
#[derive(Clone, Debug)]
pub struct Output {
    pub name: Vec<u8>,
    /// Its bytes: `None` for an input unchanged (`input`'s).
    pub data: Option<Vec<u8>>,
    /// The input it is, if not synthesized.
    pub input: Option<usize>,
}

/// What folding did.
#[derive(Clone, Debug, Default)]
pub struct Report {
    pub members: usize,
    /// Members passed through, by why.
    pub passed: BTreeMap<String, usize>,
    pub units: usize,
    /// Copies that stayed, by why.
    pub kept: BTreeMap<&'static str, usize>,
    /// Distinct units with copies in two or more members.
    pub duplicated: usize,
    /// Distinct units folded, and the copies they had.
    pub folded: usize,
    pub copies: usize,
    pub synthesized: usize,
    /// Bytes of the units removed from members, and written in synthesized
    /// members' sections.
    pub removed_bytes: u64,
    pub synthesized_bytes: u64,
    /// Debug relocations of givers removed (their places zeroed).
    pub tombstoned: usize,
    /// Distinct folded units with debug information, which synthesized
    /// members don't carry yet.
    pub debug_dropped: usize,
    /// Members whose rewriting failed, kept whole (their copies are still
    /// held by synthesized members too, which is allowed: the copies are
    /// identical weak definitions).
    pub kept_whole: BTreeMap<String, usize>,
}

/// Sections no unit is taken from.
fn unit_section(s: &Section<'_>) -> bool {
    use turbonop_macho_object::consts::{S_REGULAR, S_ZEROFILL};
    if s.is_debug() || s.flags & S_ATTR_DEBUG != 0 {
        return false;
    }
    let name = s.name();
    if matches!(
        name,
        b"__compact_unwind" | b"__eh_frame" | b"__gcc_except_tab" | b"__llvm_addrsig"
    ) {
        return false;
    }
    matches!(s.kind(), S_REGULAR | S_ZEROFILL)
}

/// Sections the linkers give a meaning (others could be collected by
/// `section$start$` symbols).
fn known_section(seg: &[u8], name: &[u8]) -> bool {
    if seg == b"__DWARF" {
        return true;
    }
    matches!(
        name,
        b"__text"
            | b"__data"
            | b"__const"
            | b"__cstring"
            | b"__bss"
            | b"__common"
            | b"__literal4"
            | b"__literal8"
            | b"__literal16"
            | b"__gcc_except_tab"
            | b"__eh_frame"
            | b"__compact_unwind"
            | b"__thread_vars"
            | b"__thread_data"
            | b"__thread_bss"
            | b"__thread_ptrs"
            | b"__StaticInit"
            | b"__cfstring"
            | b"__ustring"
            | b"__llvm_addrsig"
            | b"__mod_init_func"
            | b"__mod_term_func"
            | b"__init_offsets"
            | b"__la_symbol_ptr"
            | b"__nl_symbol_ptr"
            | b"__got"
            | b"__stubs"
            | b"__textcoal_nt"
            | b"__datacoal_nt"
            | b"__const_coal"
            | b"__text_cold"
            | b"__objc_imageinfo"
    )
}

/// A member's side effects of loading (`--pull=preserve`), if any.
fn side_effects(o: &Object<'_>) -> Option<&'static str> {
    for c in &o.commands {
        if c.cmd() == LC_LINKER_OPTION {
            return Some("LC_LINKER_OPTION");
        }
    }
    for s in o.sections() {
        if matches!(
            s.kind(),
            S_MOD_INIT_FUNC_POINTERS
                | S_MOD_TERM_FUNC_POINTERS
                | S_INIT_FUNC_OFFSETS
                | S_THREAD_LOCAL_INIT_FUNCTION_POINTERS
        ) {
            return Some("initializers");
        }
        if s.name().starts_with(b"__objc_") && s.name() != b"__objc_imageinfo" {
            return Some("Objective-C metadata");
        }
        if !known_section(s.segment_name(), s.name()) {
            return Some("a section of its own");
        }
    }
    None
}

/// A unit: an atom and what goes with it.
#[derive(Clone, Debug)]
struct Unit {
    section: usize,
    /// Offsets in the section.
    range: Range<u64>,
    /// Its symbols (indexes), external and local.
    symbols: Vec<u32>,
    /// Its external names.
    names: Vec<Vec<u8>>,
    /// Its relocations (indexes into the section's).
    relocations: Vec<usize>,
    /// Its unwind entries: offsets in `__compact_unwind`.
    unwind: Vec<u64>,
    /// External names its relocations (and unwind entries) name.
    refs: BTreeSet<Vec<u8>>,
    hash: u128,
    /// Why it can't fold, if it can't.
    blocked: Option<&'static str>,
    /// Whether debug sections refer to it.
    debug: bool,
}

/// A member read.
struct Parsed<'a> {
    obj: Option<Object<'a>>,
    why_not: Option<String>,
    env: u128,
    side_effects: Option<&'static str>,
    units: Vec<Unit>,
    /// Each external definition: its name and unit (`None`: not a unit's).
    defs: Vec<(Vec<u8>, Option<usize>)>,
    /// `__compact_unwind`'s index, if any.
    unwind_section: Option<usize>,
}

/// The size of a compact unwind entry (64-bit).
const UNWIND_ENTRY: u64 = 32;

fn h128(h: &H128) -> u128 {
    h.finish()
}

/// The environment of `o`.
fn environment(o: &Object<'_>) -> (u128, Vec<Command<'static>>) {
    let mut h = H128::new();
    h.u64(u64::from(o.header.cputype));
    h.u64(u64::from(o.header.cpusubtype));
    h.u64(u64::from(o.header.flags));
    let mut keep = Vec::new();
    for c in &o.commands {
        if let Command::Other { cmd, body } = c
            && matches!(
                *cmd,
                LC_BUILD_VERSION
                    | LC_VERSION_MIN_MACOSX
                    | LC_VERSION_MIN_IPHONEOS
                    | LC_VERSION_MIN_TVOS
                    | LC_VERSION_MIN_WATCHOS
            )
        {
            h.u64(u64::from(*cmd));
            h.bytes(body);
            keep.push(Command::Other {
                cmd: *cmd,
                body: Cow::Owned(body.to_vec()),
            });
        }
    }
    (h128(&h), keep)
}

/// Whether `y` is one of LLVM's section-start labels (`ltmp0` and on,
/// arm64's local symbols at each section's start): not an atom's, it stays
/// at its section's start when atoms are cut.
fn section_label(y: &Symbol<'_>) -> bool {
    !y.is_external() && y.name.starts_with(b"ltmp") && y.name[4..].iter().all(u8::is_ascii_digit)
}

/// The atoms of section `si` (ordinal `si + 1`): each range of offsets
/// and the symbols in it.
fn atoms(o: &Object<'_>, si: usize, s: &Section<'_>) -> Vec<(Range<u64>, Vec<u32>)> {
    let ordinal = si + 1;
    let mut syms: Vec<(u64, u32)> = o
        .symbols
        .iter()
        .enumerate()
        .filter(|(_, y)| y.in_section() && usize::from(y.n_sect) == ordinal && !section_label(y))
        .map(|(i, y)| (y.n_value.wrapping_sub(s.addr), i as u32))
        .collect();
    syms.sort_unstable();
    let size = s.size();
    let mut out: Vec<(Range<u64>, Vec<u32>)> = Vec::new();
    let mut starts: Vec<u64> = syms
        .iter()
        .filter(|&&(_, i)| o.symbols[i as usize].n_desc & N_ALT_ENTRY == 0)
        .map(|&(a, _)| a)
        .collect();
    starts.dedup();
    if starts.first() != Some(&0) {
        starts.insert(0, 0);
    }
    for (k, &a) in starts.iter().enumerate() {
        let end = starts.get(k + 1).copied().unwrap_or(size).min(size);
        if a >= end && !(a == end && a == size && k + 1 == starts.len()) {
            continue;
        }
        out.push((a..end, Vec::new()));
    }
    for &(a, i) in &syms {
        // The atom holding `a` (a symbol at the end goes to the last).
        let k = out.partition_point(|(r, _)| r.start <= a).saturating_sub(1);
        if let Some(atom) = out.get_mut(k) {
            atom.1.push(i);
        }
    }
    out
}

/// Reads a member.
fn parse<'a>(input: &Input<'a>) -> Parsed<'a> {
    let mut p = Parsed {
        obj: None,
        why_not: None,
        env: 0,
        side_effects: None,
        units: Vec::new(),
        defs: Vec::new(),
        unwind_section: None,
    };
    let Ok(o) = Object::parse(input.data) else {
        p.why_not = Some("not a Mach-O object turbolib reads".into());
        return p;
    };
    if o.header.filetype != MH_OBJECT {
        p.why_not = Some("not an object (MH_OBJECT)".into());
        return p;
    }
    let cputype = o.header.cputype;
    let arm64 = cputype == turbonop_macho_object::consts::CPU_TYPE_ARM64;
    let x86_64 = cputype == turbonop_macho_object::consts::CPU_TYPE_X86_64;
    let subsections = o.header.flags & MH_SUBSECTIONS_VIA_SYMBOLS != 0;
    // Definitions (every member's: they decide what folds).
    let defs_of = |o: &Object<'_>| -> Vec<(Vec<u8>, Option<usize>)> {
        o.symbols
            .iter()
            .filter(|y| {
                y.is_external()
                    && !y.is_undefined()
                    && y.n_type & N_TYPE != turbonop_macho_object::N_INDR
            })
            .map(|y| (y.name.to_vec(), None))
            .chain(
                o.symbols
                    .iter()
                    .filter(|y| y.is_external() && y.n_type & N_TYPE == N_UNDF && y.n_value != 0)
                    .map(|y| (y.name.to_vec(), None)),
            )
            .collect()
    };
    if !(arm64 || x86_64) || !o.is_64() || !subsections {
        p.defs = defs_of(&o);
        p.why_not = Some(if subsections {
            "not arm64 or x86-64".into()
        } else {
            "not split into atoms (no MH_SUBSECTIONS_VIA_SYMBOLS)".into()
        });
        return p;
    }
    let (env, _) = environment(&o);
    p.env = env;
    p.side_effects = side_effects(&o);
    let endian = o.endian();
    let sections: Vec<&Section<'_>> = o.sections().collect();
    p.unwind_section = sections
        .iter()
        .position(|s| s.segment_name() == b"__LD" && s.name() == b"__compact_unwind");

    // Units, by section and offset.
    let mut units: Vec<Unit> = Vec::new();
    for (si, s) in sections.iter().enumerate() {
        if !unit_section(s) {
            continue;
        }
        for (range, syms) in atoms(&o, si, s) {
            let ext: Vec<u32> = syms
                .iter()
                .copied()
                .filter(|&i| o.symbols[i as usize].is_external())
                .collect();
            if ext.is_empty()
                || !ext
                    .iter()
                    .all(|&i| o.symbols[i as usize].n_desc & N_WEAK_DEF != 0)
            {
                continue;
            }
            units.push(Unit {
                section: si,
                range,
                names: ext
                    .iter()
                    .map(|&i| o.symbols[i as usize].name.to_vec())
                    .collect(),
                symbols: syms,
                relocations: Vec::new(),
                unwind: Vec::new(),
                refs: BTreeSet::new(),
                hash: 0,
                blocked: None,
                debug: false,
            });
        }
    }
    let unit_at = |units: &[Unit], si: usize, off: u64| -> Option<usize> {
        units
            .iter()
            .position(|u| u.section == si && u.range.start <= off && off < u.range.end)
    };
    // The unit each symbol is in.
    let mut sym_unit: Vec<Option<usize>> = vec![None; o.symbols.len()];
    for (k, u) in units.iter().enumerate() {
        for &i in &u.symbols {
            sym_unit[i as usize] = Some(k);
        }
    }
    let section_of_addr = |a: u64| -> Option<(usize, u64)> {
        sections
            .iter()
            .position(|s| a >= s.addr && a - s.addr < s.size().max(1))
            .map(|i| (i, a - sections[i].addr))
    };
    let block = |units: &mut Vec<Unit>, k: usize, why: &'static str| {
        if units[k].blocked.is_none() {
            units[k].blocked = Some(why);
        }
    };
    // Sections a relocation reaches without a decoded address.
    let mut opaque: BTreeSet<usize> = BTreeSet::new();

    // Every relocation: which unit its place is in, and what it reaches.
    for (si, s) in sections.iter().enumerate() {
        let is_unwind = Some(si) == p.unwind_section;
        let debug = s.segment_name() == b"__DWARF";
        for fx in reloc::fixups(cputype, &s.relocations) {
            let first = s.relocations[fx.start];
            let place = u64::from(first.address);
            let owner = if unit_section(s) {
                unit_at(&units, si, place)
            } else {
                None
            };
            if let Some(k) = owner {
                units[k].relocations.extend(fx.clone());
            }
            for ri in fx.clone() {
                let r = s.relocations[ri];
                if r.value.is_some() {
                    // Scattered: 32-bit targets only; not read here.
                    if let Some(k) = owner {
                        block(&mut units, k, "a scattered relocation");
                    }
                    continue;
                }
                if arm64 && r.kind == reloc::arm64::ADDEND {
                    continue;
                }
                if r.external {
                    let Some(y) = o.symbols.get(r.symbolnum as usize) else {
                        if let Some(k) = owner {
                            block(&mut units, k, "a relocation naming no symbol");
                        }
                        continue;
                    };
                    let target = sym_unit[r.symbolnum as usize];
                    if y.is_external() {
                        if let Some(k) = owner {
                            units[k].refs.insert(y.name.to_vec());
                        }
                        continue;
                    }
                    // A local symbol.
                    match (owner, target) {
                        (Some(k), Some(t)) if k == t => {}
                        (Some(k), _) => block(&mut units, k, "reaches a local symbol outside it"),
                        (None, Some(t)) if debug => units[t].debug = true,
                        (None, Some(t)) => {
                            block(&mut units, t, "a local symbol of it named from outside");
                        }
                        (None, None) => {}
                    }
                    continue;
                }
                // A local relocation.
                if let Some(k) = owner {
                    block(&mut units, k, "a section-relative relocation");
                }
                if !reloc::names_section(cputype, &r) {
                    continue;
                }
                let Some(addr) =
                    turbonop_macho_object::reloc::local_address(cputype, endian, s, ri)
                else {
                    opaque.insert(r.symbolnum as usize - 1);
                    continue;
                };
                // (In the section it names: an address at a section's end is
                // that section's, not the next one's.)
                let ts = r.symbolnum as usize - 1;
                let Some(toff) = sections.get(ts).and_then(|x| addr.checked_sub(x.addr)) else {
                    continue;
                };
                let Some(t) = unit_at(&units, ts, toff) else {
                    continue;
                };
                if is_unwind && place % UNWIND_ENTRY == 0 && fx.len() == 1 {
                    // An unwind entry's function: the unit's own.
                    if toff == units[t].range.start || units[t].range.contains(&toff) {
                        units[t].unwind.push(place);
                    }
                } else if debug {
                    units[t].debug = true;
                } else if owner != Some(t) {
                    block(&mut units, t, "reached from outside by a local relocation");
                }
            }
        }
    }
    for k in 0..units.len() {
        if opaque.contains(&units[k].section) {
            block(&mut units, k, "a relocation into its section isn't decoded");
        }
    }
    // Unwind entries: no LSDA, personality by name.
    if let Some(ui) = p.unwind_section {
        let s = sections[ui];
        for k in 0..units.len() {
            let entries = units[k].unwind.clone();
            for e in entries {
                for r in &s.relocations {
                    let a = u64::from(r.address);
                    if a == e + 24 {
                        block(&mut units, k, "an LSDA");
                    } else if a == e + 16 {
                        match o.symbols.get(r.symbolnum as usize) {
                            Some(y) if r.external && y.is_external() => {
                                units[k].refs.insert(y.name.to_vec());
                            }
                            _ => block(&mut units, k, "a personality that isn't named"),
                        }
                    }
                }
                // The entry's function must start the unit, and its length
                // stay in it.
                let bytes = s.bytes().unwrap_or(&[]);
                let len = bytes
                    .get(e as usize + 8..e as usize + 12)
                    .map_or(0, |b| u32::from_le_bytes([b[0], b[1], b[2], b[3]]));
                let start = bytes
                    .get(e as usize..e as usize + 8)
                    .map_or(0, |b| u64::from_le_bytes(b.try_into().unwrap_or([0; 8])));
                let off = start.wrapping_sub(sections[units[k].section].addr);
                if off < units[k].range.start || off + u64::from(len) > units[k].range.end {
                    block(&mut units, k, "an unwind entry reaching outside it");
                }
            }
        }
    }
    // FDEs, data in code, and optimization hints in a unit.
    for (si, s) in sections.iter().enumerate() {
        if !(s.segment_name() == b"__TEXT" && s.name() == b"__eh_frame") {
            continue;
        }
        let Some(ptrs) =
            turbonop_macho_object::ehframe::pointers(s.bytes().unwrap_or(&[]), true, endian)
        else {
            for k in 0..units.len() {
                block(&mut units, k, "an __eh_frame that doesn't decode");
            }
            continue;
        };
        for ptr in ptrs {
            if ptr.kind != turbonop_macho_object::ehframe::PointerKind::PcBegin {
                continue;
            }
            let bytes = s.bytes().unwrap_or(&[]);
            let at = ptr.offset;
            let raw = match ptr.size {
                4 => bytes
                    .get(at..at + 4)
                    .map(|b| i64::from(i32::from_le_bytes([b[0], b[1], b[2], b[3]]))),
                8 => bytes
                    .get(at..at + 8)
                    .map(|b| i64::from_le_bytes(b.try_into().unwrap_or([0; 8]))),
                _ => None,
            };
            let covered = s
                .relocations
                .iter()
                .any(|r| u64::from(r.address) == ptr.offset as u64);
            if covered {
                // A relocated pc_begin: its target by its relocation.
                for r in s
                    .relocations
                    .iter()
                    .filter(|r| u64::from(r.address) == ptr.offset as u64)
                {
                    if r.external
                        && let Some(Some(t)) = sym_unit.get(r.symbolnum as usize)
                    {
                        block(&mut units, *t, "an FDE");
                    }
                }
                continue;
            }
            let Some(raw) = raw else { continue };
            let target = s
                .addr
                .wrapping_add(ptr.offset as u64)
                .wrapping_add(raw as u64);
            if let Some((ts, toff)) = section_of_addr(target)
                && let Some(t) = unit_at(&units, ts, toff)
            {
                block(&mut units, t, "an FDE");
            }
            let _ = si;
        }
    }
    for c in &o.commands {
        if let Command::LinkeditData { cmd, data } = c {
            let addrs: Vec<u64> = match *cmd {
                turbonop_macho_object::consts::LC_DATA_IN_CODE => {
                    turbonop_macho_object::linkedit::decode_data_in_code(data, endian)
                        .map(|v| v.iter().map(|e| u64::from(e.offset)).collect())
                        .unwrap_or_default()
                }
                turbonop_macho_object::consts::LC_LINKER_OPTIMIZATION_HINT => {
                    turbonop_macho_object::linkedit::decode_hints(data)
                        .map(|v| v.iter().flat_map(|h| h.args.clone()).collect())
                        .unwrap_or_default()
                }
                _ => continue,
            };
            for a in addrs {
                if let Some((ts, toff)) = section_of_addr(a)
                    && let Some(t) = unit_at(&units, ts, toff)
                {
                    block(&mut units, t, "data in code or an optimization hint");
                }
            }
        }
    }

    // Identity hashes.
    for u in &mut units {
        let s = sections[u.section];
        let mut h = H128::new();
        h.u128(env);
        h.bytes(s.segment_name());
        h.bytes(s.name());
        h.u64(u64::from(s.flags));
        h.u64(u64::from(s.align));
        h.u64(u.range.end - u.range.start);
        if let Some(b) = s.bytes() {
            h.bytes(&b[u.range.start as usize..u.range.end as usize]);
        }
        let mut rels: Vec<usize> = u.relocations.clone();
        rels.sort_by_key(|&i| (s.relocations[i].address, i));
        for &i in &rels {
            let r = s.relocations[i];
            h.u64(u64::from(r.address) - u.range.start);
            h.u64(
                u64::from(r.kind)
                    | u64::from(r.length) << 8
                    | u64::from(r.pcrel) << 16
                    | u64::from(r.external) << 17,
            );
            if r.external {
                match o.symbols.get(r.symbolnum as usize) {
                    Some(y) if y.is_external() => h.bytes(&y.name),
                    Some(y) => {
                        h.u64(1);
                        h.u64(y.n_value.wrapping_sub(s.addr) - u.range.start);
                    }
                    None => h.u64(2),
                }
            } else {
                h.u64(u64::from(r.symbolnum));
            }
        }
        for &i in &u.symbols {
            let y = &o.symbols[i as usize];
            h.bytes(&y.name);
            h.u64(u64::from(y.n_type) | u64::from(y.n_desc) << 8);
            h.u64(y.n_value.wrapping_sub(s.addr) - u.range.start);
        }
        if let Some(ui) = p.unwind_section {
            let us = sections[ui];
            let b = us.bytes().unwrap_or(&[]);
            for &e in &u.unwind {
                let e = e as usize;
                if let Some(entry) = b.get(e..e + 32) {
                    let start = u64::from_le_bytes(entry[..8].try_into().unwrap_or([0; 8]));
                    h.u64(start.wrapping_sub(s.addr).wrapping_sub(u.range.start));
                    h.bytes(&entry[8..16]);
                }
                for r in us
                    .relocations
                    .iter()
                    .filter(|r| u64::from(r.address) == e as u64 + 16)
                {
                    if let Some(y) = o.symbols.get(r.symbolnum as usize) {
                        h.bytes(&y.name);
                    }
                }
            }
        }
        u.hash = h128(&h);
    }
    let mut defs = defs_of(&o);
    for (name, unit) in &mut defs {
        *unit = units.iter().position(|u| u.names.iter().any(|n| n == name));
    }
    p.defs = defs;
    p.units = units;
    p.obj = Some(o);
    p
}

/// A synthesized member being built.
struct Synth {
    /// Its givers (members), in order.
    givers: Vec<usize>,
    env: u128,
    /// Its units: (member, unit), the survivors.
    units: Vec<(usize, usize)>,
    anchor: Vec<u8>,
    name: Vec<u8>,
}

/// Folds the library `inputs`.
pub fn fold(inputs: &[Input<'_>], options: &Options<'_>) -> (Vec<Output>, Report) {
    let mut report = Report {
        members: inputs.len(),
        ..Report::default()
    };
    let parsed: Vec<Parsed<'_>> = turbonop_par::map(inputs, 1, parse);
    for p in &parsed {
        if let Some(w) = &p.why_not {
            *report.passed.entry(w.clone()).or_default() += 1;
        }
        report.units += p.units.len();
    }
    let unchanged = |inputs: &[Input<'_>]| -> Vec<Output> {
        inputs
            .iter()
            .enumerate()
            .map(|(i, m)| Output {
                name: m.name.to_vec(),
                data: None,
                input: Some(i),
            })
            .collect()
    };
    // Every definition of each name, in member order.
    let mut definers: BTreeMap<&[u8], Vec<(usize, Option<usize>)>> = BTreeMap::new();
    for (m, p) in parsed.iter().enumerate() {
        for (name, u) in &p.defs {
            definers.entry(name.as_slice()).or_default().push((m, *u));
        }
    }
    // The copies of each unit, by hash, in member order.
    let mut by_hash: BTreeMap<u128, Vec<(usize, usize)>> = BTreeMap::new();
    for (m, p) in parsed.iter().enumerate() {
        for (k, u) in p.units.iter().enumerate() {
            by_hash.entry(u.hash).or_default().push((m, k));
        }
    }
    // Whether each copy folds (per member, per unit), and why not.
    let mut folds: Vec<Vec<bool>> = parsed.iter().map(|p| vec![false; p.units.len()]).collect();
    let mut why: Vec<Vec<Option<&'static str>>> =
        parsed.iter().map(|p| vec![None; p.units.len()]).collect();
    for copies in by_hash.values() {
        let members: BTreeSet<usize> = copies.iter().map(|c| c.0).collect();
        if members.len() < 2 {
            for &(m, k) in copies {
                why[m][k] = Some("no copy in another member");
            }
            continue;
        }
        report.duplicated += 1;
        let (m0, k0) = copies[0];
        let names = &parsed[m0].units[k0].names;
        // Every definition of its names is a copy of this unit.
        let others = names.iter().any(|n| {
            definers.get(n.as_slice()).is_none_or(|ds| {
                ds.iter().any(|&(m, u)| {
                    u.is_none_or(|u| parsed[m].units[u].hash != parsed[m0].units[k0].hash)
                })
            })
        });
        // The first definer of each name.
        let first_has_side_effects = options.pull == Pull::Preserve
            && names.iter().any(|n| {
                definers
                    .get(n.as_slice())
                    .and_then(|ds| ds.first())
                    .is_some_and(|&(m, _)| parsed[m].side_effects.is_some())
            });
        for &(m, k) in copies {
            let u = &parsed[m].units[k];
            why[m][k] = if let Some(b) = u.blocked {
                Some(b)
            } else if others {
                Some("its names have other definitions")
            } else if first_has_side_effects {
                Some("its first definer has side effects (--pull=preserve)")
            } else {
                None
            };
            folds[m][k] = why[m][k].is_none();
        }
    }
    // A unit naming a name its member keeps must stay; a unit with fewer
    // than two folding copies stays. To a fixpoint.
    loop {
        let mut changed = false;
        for (m, p) in parsed.iter().enumerate() {
            for (k, u) in p.units.iter().enumerate() {
                if !folds[m][k] {
                    continue;
                }
                let stays = u.refs.iter().any(|r| {
                    p.defs
                        .iter()
                        .any(|(n, du)| n == r && du.is_none_or(|du| !folds[m][du]))
                });
                if stays {
                    folds[m][k] = false;
                    why[m][k] = Some("calls a name of its member that stays");
                    changed = true;
                }
            }
        }
        for copies in by_hash.values() {
            let n: BTreeSet<usize> = copies
                .iter()
                .filter(|&&(m, k)| folds[m][k])
                .map(|c| c.0)
                .collect();
            if n.len() == 1 {
                for &(m, k) in copies {
                    if folds[m][k] {
                        folds[m][k] = false;
                        why[m][k] = Some("its other copies stayed");
                        changed = true;
                    }
                }
            }
        }
        if !changed {
            break;
        }
    }
    for w in why.iter().flatten().flatten() {
        *report.kept.entry(w).or_default() += 1;
    }

    // Synthesized members: by givers and environment.
    let mut synth: Vec<Synth> = Vec::new();
    let mut synth_of: BTreeMap<(Vec<usize>, u128), usize> = BTreeMap::new();
    // Where each folded copy's unit went: (member, unit) -> synthesized.
    let mut moved_to: BTreeMap<(usize, usize), usize> = BTreeMap::new();
    // Where each moved name went.
    let mut name_to: BTreeMap<Vec<u8>, usize> = BTreeMap::new();
    for copies in by_hash.values() {
        let live: Vec<(usize, usize)> = copies
            .iter()
            .copied()
            .filter(|&(m, k)| folds[m][k])
            .collect();
        if live.len() < 2 {
            continue;
        }
        let givers: Vec<usize> = live
            .iter()
            .map(|c| c.0)
            .collect::<BTreeSet<_>>()
            .into_iter()
            .collect();
        let env = parsed[live[0].0].env;
        let s = *synth_of.entry((givers.clone(), env)).or_insert_with(|| {
            synth.push(Synth {
                givers: givers.clone(),
                env,
                units: Vec::new(),
                anchor: Vec::new(),
                name: Vec::new(),
            });
            synth.len() - 1
        });
        synth[s].units.push(live[0]);
        for &c in &live {
            moved_to.insert(c, s);
        }
        for n in &parsed[live[0].0].units[live[0].1].names {
            name_to.insert(n.clone(), s);
        }
        report.folded += 1;
        report.copies += live.len();
        if parsed[live[0].0].units[live[0].1].debug {
            report.debug_dropped += 1;
        }
    }
    if synth.is_empty() {
        return (unchanged(inputs), report);
    }
    // Synthesized members in the order of their first giver, then their
    // first unit (deterministic, whatever the threads).
    let mut order: Vec<usize> = (0..synth.len()).collect();
    order.sort_by_key(|&s| (synth[s].givers.clone(), synth[s].units[0]));
    let mut renum = vec![0usize; synth.len()];
    for (new, &old) in order.iter().enumerate() {
        renum[old] = new;
    }
    let mut synth: Vec<Synth> = {
        let mut slots: Vec<Option<Synth>> = synth.into_iter().map(Some).collect();
        order
            .iter()
            .map(|&o| slots[o].take().expect("each once"))
            .collect()
    };
    for v in moved_to.values_mut() {
        *v = renum[*v];
    }
    for v in name_to.values_mut() {
        *v = renum[*v];
    }
    for (i, s) in synth.iter_mut().enumerate() {
        let mut h = H128::new();
        h.u128(s.env);
        h.bytes(options.salt);
        for &(m, k) in &s.units {
            h.u128(parsed[m].units[k].hash);
        }
        let v = h128(&h);
        s.anchor = [ANCHOR_PREFIX, format!("{v:032x}").as_bytes()].concat();
        s.name = format!("__turbolib.{i}.o").into_bytes();
    }

    // Write the synthesized members and rewrite the givers, on every core.
    let built: Vec<Result<Vec<u8>, String>> = turbonop_par::map_range(synth.len(), 1, |i| {
        build_synth(&parsed, &synth, i, &name_to)
    });
    let rewritten: Vec<Option<Rewritten>> = turbonop_par::map_range(parsed.len(), 1, |m| {
        let gone: Vec<usize> = (0..parsed[m].units.len())
            .filter(|&k| folds[m][k])
            .collect();
        if gone.is_empty() {
            return None;
        }
        let anchors: BTreeSet<usize> = gone.iter().map(|&k| moved_to[&(m, k)]).collect();
        Some(rewrite_giver(&parsed[m], &gone, &anchors, &synth))
    });
    let mut out = Vec::with_capacity(inputs.len() + synth.len());
    let mut failed_synth = false;
    for (i, b) in built.into_iter().enumerate() {
        match b {
            Ok(data) => {
                report.synthesized_bytes += data.len() as u64;
                out.push(Output {
                    name: synth[i].name.clone(),
                    data: Some(data),
                    input: None,
                });
            }
            Err(e) => {
                *report
                    .kept_whole
                    .entry(format!("a synthesized member: {e}"))
                    .or_default() += 1;
                failed_synth = true;
            }
        }
    }
    if failed_synth {
        // (Givers would refer to anchors no member defines.)
        report.synthesized = 0;
        return (unchanged(inputs), report);
    }
    report.synthesized = synth.len();
    for (m, r) in rewritten.into_iter().enumerate() {
        match r {
            None => out.push(Output {
                name: inputs[m].name.to_vec(),
                data: None,
                input: Some(m),
            }),
            Some(Ok((data, tomb, removed))) => {
                report.tombstoned += tomb;
                report.removed_bytes += removed;
                out.push(Output {
                    name: inputs[m].name.to_vec(),
                    data: Some(data),
                    input: Some(m),
                });
            }
            Some(Err(e)) => {
                *report.kept_whole.entry(e).or_default() += 1;
                out.push(Output {
                    name: inputs[m].name.to_vec(),
                    data: None,
                    input: Some(m),
                });
            }
        }
    }
    (out, report)
}

/// A rewritten giver: its bytes, the debug relocations removed, and the
/// bytes of units removed; or why it couldn't be rewritten.
type Rewritten = Result<(Vec<u8>, usize, u64), String>;

/// An unwind entry of a synthesized member: its function's offset in its
/// section, that section, the entry's bytes, and its personality's name.
type UnwindEntry = (u64, usize, Vec<u8>, Option<Vec<u8>>);

fn put_u64(b: &mut [u8], at: usize, v: u64) {
    b[at..at + 8].copy_from_slice(&v.to_le_bytes());
}

/// The bytes of synthesized member `i`.
fn build_synth(
    parsed: &[Parsed<'_>],
    synth: &[Synth],
    i: usize,
    name_to: &BTreeMap<Vec<u8>, usize>,
) -> Result<Vec<u8>, String> {
    let s = &synth[i];
    let (m0, _) = s.units[0];
    let base = parsed[m0].obj.as_ref().ok_or("no object")?;
    let (_, env_commands) = environment(base);
    let header = Header {
        reserved: 0,
        ..base.header
    };
    // Sections, in the order units first need them.
    let mut sections: Vec<Section<'static>> = Vec::new();
    let mut key_of: Vec<([u8; 16], [u8; 16], u32)> = Vec::new();
    let mut symbols: Vec<Symbol<'static>> = Vec::new();
    let mut by_name: BTreeMap<Vec<u8>, u32> = BTreeMap::new();
    let mut anchors_needed: BTreeSet<usize> = BTreeSet::new();
    // (Relocations per section, with old symbol references to remap.)
    let mut unwind: Vec<UnwindEntry> = Vec::new();
    let undefined = |symbols: &mut Vec<Symbol<'static>>,
                     by_name: &mut BTreeMap<Vec<u8>, u32>,
                     n: &[u8],
                     weak: bool|
     -> u32 {
        // (A weak reference only if every reference is: as the link of
        // the givers would have it.)
        if let Some(&i) = by_name.get(n) {
            let y = &mut symbols[i as usize];
            if !weak && y.is_undefined() {
                y.n_desc &= !N_WEAK_REF;
            }
            return i;
        }
        symbols.push(Symbol {
            name: Cow::Owned(n.to_vec()),
            name_offset: 0,
            n_type: N_UNDF | N_EXT,
            n_sect: 0,
            n_desc: if weak { N_WEAK_REF } else { 0 },
            n_value: 0,
            indirect: None,
        });
        let k = (symbols.len() - 1) as u32;
        by_name.insert(n.to_vec(), k);
        k
    };
    // First: every unit's own symbols defined, so references resolve to them.
    struct Placed {
        member: usize,
        unit: usize,
        section: usize,
        offset: u64,
        /// Old symbol index -> new.
        syms: BTreeMap<u32, u32>,
    }
    let mut placed: Vec<Placed> = Vec::new();
    let mut pads: Vec<(usize, u64)> = Vec::new();
    for &(m, k) in &s.units {
        let o = parsed[m].obj.as_ref().ok_or("no object")?;
        let u = &parsed[m].units[k];
        let src = o.section(u.section).ok_or("no section")?;
        let key = (src.segname, src.sectname, src.flags);
        let si = match key_of.iter().position(|k| *k == key) {
            Some(i) => i,
            None => {
                key_of.push(key);
                sections.push(Section {
                    sectname: src.sectname,
                    segname: src.segname,
                    addr: 0,
                    align: src.align,
                    flags: src.flags,
                    reserved1: 0,
                    reserved2: 0,
                    reserved3: 0,
                    contents: if src.bytes().is_some() {
                        Contents::Bytes(Cow::Owned(Vec::new()))
                    } else {
                        Contents::Zerofill(0)
                    },
                    relocations: Vec::new(),
                });
                sections.len() - 1
            }
        };
        let sec = &mut sections[si];
        sec.align = sec.align.max(src.align);
        let a = 1u64 << src.align.min(15);
        let end = sec.size();
        let at = end.div_ceil(a) * a;
        let len = u.range.end - u.range.start;
        if at > end && end > 0 {
            // The alignment gap is an atom of its own (a local label), so
            // that the unit before it keeps its size.
            pads.push((si, end));
        }
        match (&mut sec.contents, src.bytes()) {
            (Contents::Bytes(b), Some(sb)) => {
                let b = b.to_mut();
                b.resize(at as usize, 0);
                b.extend_from_slice(&sb[u.range.start as usize..u.range.end as usize]);
            }
            (Contents::Zerofill(n), None) => *n = at + len,
            _ => return Err("a unit's section kind differs".into()),
        }
        placed.push(Placed {
            member: m,
            unit: k,
            section: si,
            offset: at,
            syms: BTreeMap::new(),
        });
    }
    // Section addresses: LLVM's (laid out below); symbols take offsets now
    // and addresses once the sections are placed.
    for p in &mut placed {
        let o = parsed[p.member].obj.as_ref().ok_or("no object")?;
        let u = &parsed[p.member].units[p.unit];
        let src = o.section(u.section).ok_or("no section")?;
        for &yi in &u.symbols {
            let y = &o.symbols[yi as usize];
            let off = y.n_value.wrapping_sub(src.addr) - u.range.start + p.offset;
            symbols.push(Symbol {
                name: Cow::Owned(y.name.to_vec()),
                name_offset: 0,
                n_type: y.n_type,
                n_sect: (p.section + 1) as u8,
                n_desc: y.n_desc,
                // (An offset until the section's address is known.)
                n_value: off,
                indirect: None,
            });
            let ni = (symbols.len() - 1) as u32;
            p.syms.insert(yi, ni);
            if y.is_external() {
                by_name.insert(y.name.to_vec(), ni);
            }
        }
    }
    for &(si, off) in &pads {
        symbols.push(Symbol {
            name: Cow::Borrowed(PAD),
            name_offset: 0,
            n_type: N_SECT,
            n_sect: (si + 1) as u8,
            n_desc: 0,
            n_value: off,
            indirect: None,
        });
    }
    // Relocations, and unwind entries.
    let mut relocs: Vec<Vec<Relocation>> = vec![Vec::new(); sections.len()];
    for p in &placed {
        let o = parsed[p.member].obj.as_ref().ok_or("no object")?;
        let u = &parsed[p.member].units[p.unit];
        let src = o.section(u.section).ok_or("no section")?;
        for &ri in &u.relocations {
            let mut r = src.relocations[ri];
            r.address = u32::try_from(u64::from(r.address) - u.range.start + p.offset)
                .map_err(|_| "a section too large")?;
            if r.external {
                let y = &o.symbols[r.symbolnum as usize];
                r.symbolnum = if y.is_external() {
                    if let Some(&t) = name_to.get(y.name.as_ref())
                        && t != i
                    {
                        anchors_needed.insert(t);
                    }
                    undefined(
                        &mut symbols,
                        &mut by_name,
                        &y.name,
                        y.is_undefined() && y.n_desc & N_WEAK_REF != 0,
                    )
                } else {
                    *p.syms
                        .get(&r.symbolnum)
                        .ok_or("a local symbol outside the unit")?
                };
            }
            relocs[p.section].push(r);
        }
        if let Some(ui) = parsed[p.member].unwind_section {
            let us = o.section(ui).ok_or("no unwind section")?;
            let b = us.bytes().unwrap_or(&[]);
            for &e in &u.unwind {
                let e = e as usize;
                let mut entry = b
                    .get(e..e + 32)
                    .ok_or("an unwind entry past its section")?
                    .to_vec();
                let start = u64::from_le_bytes(entry[..8].try_into().unwrap_or([0; 8]));
                let off = start.wrapping_sub(src.addr) - u.range.start + p.offset;
                put_u64(&mut entry, 0, off);
                let personality = us
                    .relocations
                    .iter()
                    .find(|r| u64::from(r.address) == e as u64 + 16)
                    .map(|r| o.symbols[r.symbolnum as usize].name.to_vec());
                unwind.push((off, p.section, entry, personality));
            }
        }
    }
    let anchor_names: Vec<Vec<u8>> = anchors_needed
        .iter()
        .map(|&t| synth[t].anchor.clone())
        .collect();
    for a in &anchor_names {
        undefined(&mut symbols, &mut by_name, a, false);
    }
    // The anchor.
    symbols.push(Symbol {
        name: Cow::Owned(s.anchor.clone()),
        name_offset: 0,
        n_type: N_ABS | N_EXT | N_PEXT,
        n_sect: 0,
        n_desc: 0,
        n_value: 0,
        indirect: None,
    });
    // The unwind section.
    let unwind_si = if unwind.is_empty() {
        None
    } else {
        let us = parsed[m0]
            .unwind_section
            .and_then(|ui| base.section(ui))
            .or_else(|| {
                s.units.iter().find_map(|&(m, _)| {
                    parsed[m]
                        .unwind_section
                        .and_then(|ui| parsed[m].obj.as_ref()?.section(ui))
                })
            })
            .ok_or("no unwind section to copy")?;
        sections.push(Section {
            sectname: us.sectname,
            segname: us.segname,
            addr: 0,
            align: us.align,
            flags: us.flags,
            reserved1: 0,
            reserved2: 0,
            reserved3: 0,
            contents: Contents::Bytes(Cow::Owned(Vec::new())),
            relocations: Vec::new(),
        });
        relocs.push(Vec::new());
        Some(sections.len() - 1)
    };
    // Zero-fill sections last, as LLVM orders them (renumbering symbols).
    let mut order: Vec<usize> = (0..sections.len()).collect();
    order.sort_by_key(|&k| (sections[k].bytes().is_none(), k));
    let mut new_of = vec![0usize; sections.len()];
    for (n, &k) in order.iter().enumerate() {
        new_of[k] = n;
    }
    for y in &mut symbols {
        if y.n_type & N_TYPE == N_SECT {
            y.n_sect = (new_of[usize::from(y.n_sect) - 1] + 1) as u8;
        }
    }
    let mut slots: Vec<Option<(Section<'static>, Vec<Relocation>)>> =
        sections.into_iter().zip(relocs).map(Some).collect();
    let mut sections: Vec<Section<'static>> = Vec::with_capacity(slots.len());
    let mut relocs: Vec<Vec<Relocation>> = Vec::with_capacity(slots.len());
    for &k in &order {
        let (s, r) = slots[k].take().expect("each once");
        sections.push(s);
        relocs.push(r);
    }
    let unwind_si = unwind_si.map(|k| new_of[k]);
    for (_, sec, _, _) in &mut unwind {
        *sec = new_of[*sec];
    }
    // Addresses, as LLVM lays them out.
    let mut addr = 0u64;
    for s in sections.iter_mut().filter(|s| s.bytes().is_some()) {
        let a = 1u64 << s.align.min(15);
        addr = addr.div_ceil(a) * a;
        s.addr = addr;
        addr += s.size();
    }
    for s in sections.iter_mut().filter(|s| s.bytes().is_none()) {
        let a = 1u64 << s.align.min(15);
        addr = addr.div_ceil(a) * a;
        s.addr = addr;
        addr += s.size();
    }
    for y in &mut symbols {
        if y.n_type & N_TYPE == N_SECT {
            y.n_value += sections[usize::from(y.n_sect) - 1].addr;
        }
    }
    // Unwind entries: the function's address, a local relocation to its
    // section; the personality by name.
    if let Some(ui) = unwind_si {
        unwind.sort_by_key(|e| (e.1, e.0));
        let mut bytes = Vec::with_capacity(unwind.len() * 32);
        let mut rs = Vec::new();
        for (off, sec, mut entry, personality) in unwind {
            let at = bytes.len() as u32;
            put_u64(&mut entry, 0, sections[sec].addr + off);
            if personality.is_some() {
                put_u64(&mut entry, 16, 0);
            }
            bytes.extend_from_slice(&entry);
            rs.push(Relocation::plain(at, (sec + 1) as u32, false, 0, 3));
            if let Some(p) = personality {
                let k = undefined(&mut symbols, &mut by_name, &p, false);
                rs.push(Relocation::plain(at + 16, k, true, 0, 3));
            }
        }
        // (LLVM writes a section's relocations last to first.)
        rs.reverse();
        relocs[ui] = rs;
        sections[ui].contents = Contents::Bytes(Cow::Owned(bytes));
    }
    // Local relocations inside the units' bytes encode addresses: none here
    // (a unit with one doesn't fold).
    for (s, r) in sections.iter_mut().zip(relocs) {
        let mut r = r;
        r.sort_by_key(|r| core::cmp::Reverse(r.address));
        s.relocations = r;
    }
    let segment = Segment {
        segname: name_field(b""),
        vmaddr: 0,
        maxprot: 7,
        initprot: 7,
        flags: 0,
        sections,
    };
    let mut commands = vec![Command::Segment(segment)];
    commands.extend(env_commands);
    commands.push(Command::Symtab);
    commands.push(Command::Dysymtab(Default::default()));
    let mut obj = Object::new(header, commands);
    obj.symbols = symbols;
    obj.sort_symbols();
    obj.update_dysymtab();
    obj.relayout();
    obj.write().map_err(|e| e.to_string())
}

/// Member `p` without its units `gone`, referring to the synthesized
/// members `anchors`: its bytes, the debug relocations removed, and the
/// bytes of units removed.
fn rewrite_giver(
    p: &Parsed<'_>,
    gone: &[usize],
    anchors: &BTreeSet<usize>,
    synth: &[Synth],
) -> Result<(Vec<u8>, usize, u64), String> {
    let mut o = p.obj.clone().ok_or("no object")?;
    let cputype = o.header.cputype;
    let endian = o.endian();
    let mut removed = 0u64;
    // The address ranges going.
    let ranges: Vec<(usize, Range<u64>, Range<u64>)> = gone
        .iter()
        .map(|&k| {
            let u = &p.units[k];
            let a = o.section(u.section).map_or(0, |s| s.addr);
            (
                u.section,
                u.range.clone(),
                a + u.range.start..a + u.range.end,
            )
        })
        .collect();
    let in_gone = |a: u64| ranges.iter().any(|(_, _, r)| r.contains(&a));
    let gone_syms: BTreeSet<u32> = gone
        .iter()
        .flat_map(|&k| p.units[k].symbols.iter().copied())
        .collect();
    // 1. Debug sections' relocations to what goes: removed, places zeroed.
    let mut tomb = 0;
    let nsec = o.section_count();
    for si in 0..nsec {
        let Some(s) = o.section(si) else { continue };
        if s.segment_name() != b"__DWARF" {
            continue;
        }
        let mut drop = BTreeSet::new();
        for (ri, r) in s.relocations.iter().enumerate() {
            let hit = if r.external {
                gone_syms.contains(&r.symbolnum) && !o.symbols[r.symbolnum as usize].is_external()
            } else if reloc::names_section(cputype, r) {
                reloc::local_address(cputype, endian, s, ri).is_some_and(in_gone)
            } else {
                false
            };
            if hit {
                drop.insert(ri);
            }
        }
        if drop.is_empty() {
            continue;
        }
        // (Whole fixups.)
        let fx = reloc::fixups(cputype, &s.relocations);
        let mut all = BTreeSet::new();
        for f in fx {
            if f.clone().any(|i| drop.contains(&i)) {
                all.extend(f);
            }
        }
        let s = o.sections_mut().nth(si).ok_or("no section")?;
        let places: Vec<(u32, u8)> = all
            .iter()
            .map(|&i| (s.relocations[i].address, s.relocations[i].length))
            .collect();
        if let Contents::Bytes(b) = &mut s.contents {
            let b = b.to_mut();
            for (at, len) in places {
                let n = 1usize << len.min(3);
                if let Some(x) = b.get_mut(at as usize..at as usize + n) {
                    x.fill(0);
                }
            }
        }
        let mut i = 0;
        s.relocations.retain(|_| {
            let keep = !all.contains(&i);
            i += 1;
            keep
        });
        tomb += all.len();
    }
    // 2. Cut the units and their unwind entries.
    let mut cuts: BTreeMap<usize, Vec<Range<u64>>> = BTreeMap::new();
    for (si, r, _) in &ranges {
        removed += r.end - r.start;
        cuts.entry(*si).or_default().push(r.clone());
    }
    if let Some(ui) = p.unwind_section {
        for &k in gone {
            for &e in &p.units[k].unwind {
                cuts.entry(ui).or_default().push(e..e + UNWIND_ENTRY);
            }
        }
    }
    let cuts: Vec<(usize, Vec<Range<u64>>)> = cuts
        .into_iter()
        .map(|(s, mut v)| {
            v.sort_by_key(|r| r.start);
            (s, v)
        })
        .collect();
    let cut = o.cut(&cuts).map_err(|e| e.to_string())?;
    if !cut.dangling.is_empty() {
        return Err("a relocation still reaches what was cut".into());
    }
    if !cut.unadjusted.is_empty() {
        return Err("an address the cut couldn't move".into());
    }
    // 3. Symbols: moved names become weak references, locals of the units
    // go; the anchors are added.
    let mut keep = vec![true; o.symbols.len()];
    for &(i, now) in &cut.orphaned_symbols {
        let y = &mut o.symbols[i as usize];
        if section_label(y) {
            y.n_value = now;
            continue;
        }
        if !gone_syms.contains(&i) {
            return Err("a symbol in a cut range outside the units".into());
        }
        if y.is_external() {
            y.n_type = N_UNDF | N_EXT;
            y.n_sect = 0;
            y.n_value = 0;
            y.n_desc = N_WEAK_REF;
        } else {
            keep[i as usize] = false;
        }
    }
    o.retain_symbols(&keep);
    for &a in anchors {
        o.symbols.push(Symbol {
            name: Cow::Owned(synth[a].anchor.clone()),
            name_offset: 0,
            n_type: N_UNDF | N_EXT,
            n_sect: 0,
            n_desc: 0,
            n_value: 0,
            indirect: None,
        });
    }
    o.sort_symbols();
    o.update_dysymtab();
    o.relayout();
    let data = o.write().map_err(|e| e.to_string())?;
    Ok((data, tomb, removed))
}

#[cfg(test)]
mod tests;
