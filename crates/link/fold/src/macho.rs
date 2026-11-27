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
//! in it), its `__eh_frame` FDEs (those whose function is in it: clang
//! writes one for every x86-64 function, its pointers PC-relative with no
//! relocation), and its LSDAs (the atoms of `__gcc_except_tab` its unwind
//! entries and FDEs name, when nothing else reaches them: they move with
//! it as atoms of its own). Its identity hash covers, for each atom, the
//! section's names, flags, and alignment, the bytes, each relocation as
//! (offset in the atom, type, length, PC-relativeness, target: an external
//! name, or the offset of a local symbol of the atom), the symbols (names,
//! types, descriptions, offsets); the unwind entries (offset of the
//! function, length, encoding, personality by name, LSDA by atom and
//! offset); the FDEs (their bytes with their pointers and CIE pointer out,
//! the pointers' targets by atom and offset, the CIE's bytes and
//! relocations by name); and the member's environment. A function's
//! alignment padding (the bytes after its end by its one unwind entry, up
//! to the next atom, with no relocation or symbol in them) isn't part of
//! it: x86-64's code is aligned to 16 bytes, so a copy that ends its
//! section and one that doesn't differ only there.
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
//! a local symbol of it); an LSDA something else reaches (or two units
//! share), or one outside `__gcc_except_tab`; an FDE with relocations (not
//! what LLVM writes for x86-64), or one reaching outside it; a data-in-code
//! entry or an optimization hint in it; a relocation into its section
//! whose address isn't decoded.
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
//! symbols, unwind entries (their LSDA fields local relocations to its
//! LSDAs), and FDEs (after copies of their CIEs, their pointers set to
//! where the atoms are; alignment gaps in code are NOPs, as LLVM pads).
//! Each member that held a copy loses the atoms (cut out of their
//! sections, everything after them moved back:
//! [`turbonop_macho_object::Object::cut`]), their unwind entries, and
//! their FDEs; its
//! definitions become weak undefined references, and it gains a strong
//! undefined reference to the synthesized member's anchor (a private
//! external absolute symbol named for the synthesized member's contents
//! and the library, so that two libraries' never collide).
//!
//! **The order of the undefined names** decides what ld64.lld loads, and
//! when: it resolves an object's undefined names in symbol-table order as
//! it loads the object, loading an archive member the moment a name meets
//! one of the archive's symbols (recursively), and a weak reference loads
//! a member as a strong one does. So a giver's undefined names are its
//! original ones first, in LLVM's order (sorted), which load what they
//! loaded, in the same order; then the anchors (`~` sorts after the names
//! compilers make), which load the synthesized members, whose own
//! undefined names were all the givers' too and are resolved by then; and
//! last the moved names (out of LLVM's order), defined by then, so they
//! aren't taken from another library's copy. A synthesized member's own
//! undefined names are in the same three runs: names moved into another
//! synthesized member come after the anchors that load it. Synthesized
//! members come
//! first in the archive, so their names are in the linker's table before a
//! member of the same archive loaded while the archive is read looks for
//! them. A synthesized member refers to names moved into another the same
//! way.
//!
//! **Debug information.** A member that gave a copy up loses each debug
//! section's local relocation to it (its place set to all ones, the
//! tombstone: 0 is an address of the object), so `dsymutil`, which reads
//! the DWARF from the member through the executable's debug map, finds no
//! address for the copy's DIEs and drops them, as it does for every copy
//! but the linked one; and what in its DWARF holds addresses no relocation
//! does (its line program, its list entries, its unit's length) moves with
//! its code (see [`adjust`]). Then the DIEs that described what it lost, and what
//! only they reached, are removed from it (see [`prune`]: where that can't
//! be done, they stay, tombstoned, counted in
//! [`Report::debug_unpruned_why`]). Each synthesized member gets a compile unit
//! of its own from the survivors' member's (see the `debug` module, and
//! [`crate::dwarf`]): the moved functions' DIEs and line rows, the types
//! they name (classes as declarations), their lists. Where that can't be
//! done, the units' DWARF is dropped, and counted
//! ([`Report::debug_dropped_why`]).

use std::borrow::Cow;
use std::collections::{BTreeMap, BTreeSet, HashMap};
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
/// The prefix of anchors' names: `~` sorts after the names compilers make
/// (`_`, letters, `l`, `L`), so a member's anchors come after its other
/// undefined names in LLVM's order.
pub const ANCHOR_PREFIX: &[u8] = b"~turbolib.anchor.";

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
    /// Debug relocations of givers removed (their places set to all ones).
    pub tombstoned: usize,
    /// Distinct folded units with debug information: those whose
    /// synthesized member describes them, and those whose doesn't (by
    /// why).
    pub debug_moved: usize,
    pub debug_dropped: usize,
    pub debug_dropped_why: BTreeMap<&'static str, usize>,
    /// In the synthesized members' DWARF: the root DIEs copied, the
    /// addresses written as tombstones (they named nothing moved), and the
    /// bytes of debug sections.
    pub debug_roots: usize,
    pub debug_tombstoned: usize,
    pub debug_bytes: u64,
    /// Givers whose DWARF lost what described their lost code (see the
    /// `prune` module): the givers, the dead subprograms and variables
    /// removed, those kept because a kept DIE names them, the other
    /// entities removed (what only the dead reached, namespaces
    /// emptied), and the debug bytes removed; and the givers whose DWARF
    /// stayed as it was, by why.
    pub debug_pruned: usize,
    pub debug_dead_removed: usize,
    pub debug_dead_kept: usize,
    pub debug_reached_removed: usize,
    pub debug_pruned_bytes: u64,
    pub debug_unpruned_why: BTreeMap<&'static str, usize>,
    /// How long each phase took, in order (for `--stats`).
    pub phases: Vec<(&'static str, std::time::Duration)>,
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

/// An atom that moves with a unit: the unit's own (the first), or an LSDA
/// of it (an atom of `__gcc_except_tab` only its unwind information names).
#[derive(Clone, Debug)]
struct Atom {
    section: usize,
    /// Offsets in the section.
    range: Range<u64>,
    /// Its symbols (indexes), external and local.
    symbols: Vec<u32>,
    /// Its relocations (indexes into the section's).
    relocations: Vec<usize>,
}

/// A pointer field of an FDE: its offset in the record, its size and
/// encoding (`DW_EH_PE_pcrel`, no relocation), and what it points to: an
/// atom of the unit (its index) and the offset in it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct EhPointer {
    at: u64,
    size: u8,
    encoding: u8,
    atom: usize,
    offset: u64,
}

/// An `__eh_frame` FDE of a unit's function.
#[derive(Clone, Debug)]
struct Fde {
    /// Its record (offsets in `__eh_frame`), and its CIE's.
    record: Range<u64>,
    cie: Range<u64>,
    /// The offset of its CIE pointer in the record.
    id_at: u64,
    pc: EhPointer,
    lsda: Option<EhPointer>,
}

/// A unit: an atom and what goes with it.
#[derive(Clone, Debug)]
struct Unit {
    /// Its atoms: its own first, then its LSDAs.
    atoms: Vec<Atom>,
    /// Its external names.
    names: Vec<Vec<u8>>,
    /// Its unwind entries: offsets in `__compact_unwind`, and the LSDA each
    /// names (an atom of the unit, and the offset in it).
    unwind: Vec<(u64, Option<(usize, u64)>)>,
    /// Its FDEs.
    fdes: Vec<Fde>,
    /// External names its relocations (and unwind information) name.
    refs: BTreeSet<Vec<u8>>,
    hash: u128,
    /// Why it can't fold, if it can't.
    blocked: Option<&'static str>,
    /// Whether debug sections refer to it.
    debug: bool,
}

impl Unit {
    fn main(&self) -> &Atom {
        &self.atoms[0]
    }
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
    /// `__compact_unwind`'s index, `__eh_frame`'s, and
    /// `__gcc_except_tab`'s, if any.
    unwind_section: Option<usize>,
    eh_section: Option<usize>,
    lsda_section: Option<usize>,
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
/// and the symbols in it, in order.
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

/// The index of the atom of `atoms` (in order) holding offset `off`.
fn atom_at(atoms: &[(Range<u64>, Vec<u32>)], off: u64) -> Option<usize> {
    let k = atoms
        .partition_point(|(r, _)| r.start <= off)
        .checked_sub(1)?;
    (off < atoms[k].0.end).then_some(k)
}

fn le(bytes: &[u8], at: u64, n: u8) -> Option<u64> {
    let b = bytes.get(at as usize..at as usize + usize::from(n))?;
    let mut v = [0u8; 8];
    v[..b.len()].copy_from_slice(b);
    Some(u64::from_le_bytes(v))
}

/// A pointer's value as its encoding's format reads it (sign-extended for
/// the signed formats).
fn eh_value(bytes: &[u8], at: u64, size: u8, encoding: u8) -> Option<i64> {
    let v = le(bytes, at, size)?;
    let bits = 8 * u32::from(size);
    Some(if encoding & 0x08 != 0 && bits < 64 {
        ((v << (64 - bits)) as i64) >> (64 - bits)
    } else {
        v as i64
    })
}

/// The length of atom `a` that is what it is: a function's padding (the
/// bytes after its end, by its one unwind entry, up to the next atom: the
/// next function's alignment, which x86-64's code has and copies at a
/// section's end don't) isn't, when nothing (a relocation, a symbol) is
/// in it. Its whole length otherwise.
fn function_length(
    o: &Object<'_>,
    s: &Section<'_>,
    a: &Atom,
    unwind: &[(u64, Option<(usize, u64)>)],
    unwind_bytes: &[u8],
) -> u64 {
    use turbonop_macho_object::consts::{S_ATTR_PURE_INSTRUCTIONS, S_ATTR_SOME_INSTRUCTIONS};
    let full = a.range.end - a.range.start;
    if s.flags & (S_ATTR_PURE_INSTRUCTIONS | S_ATTR_SOME_INSTRUCTIONS) == 0 {
        return full;
    }
    let [(e, _)] = unwind else { return full };
    let (Some(start), Some(len)) = (le(unwind_bytes, *e, 8), le(unwind_bytes, *e + 8, 4)) else {
        return full;
    };
    if start.wrapping_sub(s.addr) != a.range.start || len == 0 || len >= full {
        return full;
    }
    let end = a.range.start + len;
    let in_tail = a
        .relocations
        .iter()
        .any(|&i| u64::from(s.relocations[i].address) >= end)
        || a.symbols
            .iter()
            .any(|&i| o.symbols[i as usize].n_value.wrapping_sub(s.addr) >= end);
    if in_tail { full } else { len }
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
        eh_section: None,
        lsda_section: None,
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
    let find = |seg: &[u8], name: &[u8]| {
        sections
            .iter()
            .position(|s| s.segment_name() == seg && s.name() == name)
    };
    p.unwind_section = find(b"__LD", b"__compact_unwind");
    p.eh_section = find(b"__TEXT", b"__eh_frame");
    p.lsda_section = find(b"__TEXT", b"__gcc_except_tab");
    let ls = p.lsda_section;

    // Atoms of the unit sections and of `__gcc_except_tab`; units.
    let mut atom_list: Vec<Vec<(Range<u64>, Vec<u32>)>> = vec![Vec::new(); sections.len()];
    let mut atom_unit: Vec<Vec<Option<usize>>> = vec![Vec::new(); sections.len()];
    let mut units: Vec<Unit> = Vec::new();
    for (si, s) in sections.iter().enumerate() {
        let lsda = Some(si) == ls;
        if !unit_section(s) && !lsda {
            continue;
        }
        let list = atoms(&o, si, s);
        let mut owners = vec![None; list.len()];
        if !lsda {
            for (k, (range, syms)) in list.iter().enumerate() {
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
                owners[k] = Some(units.len());
                units.push(Unit {
                    atoms: vec![Atom {
                        section: si,
                        range: range.clone(),
                        symbols: syms.clone(),
                        relocations: Vec::new(),
                    }],
                    names: ext
                        .iter()
                        .map(|&i| o.symbols[i as usize].name.to_vec())
                        .collect(),
                    unwind: Vec::new(),
                    fdes: Vec::new(),
                    refs: BTreeSet::new(),
                    hash: 0,
                    blocked: None,
                    debug: false,
                });
            }
        }
        atom_list[si] = list;
        atom_unit[si] = owners;
    }
    let unit_at = |si: usize, off: u64| -> Option<usize> {
        atom_unit
            .get(si)?
            .get(atom_at(atom_list.get(si)?, off)?)
            .copied()
            .flatten()
    };
    // The LSDA atom an offset of `__gcc_except_tab` is in.
    let lsda_at = |off: u64| -> Option<usize> { atom_at(&atom_list[ls?], off) };
    let n_lsda = ls.map_or(0, |l| atom_list[l].len());
    // Each LSDA atom's relocations, the names they name, and whether
    // anything but a unit's unwind information reaches it (or it reaches
    // a local symbol): then it can't move.
    let mut lsda_relocs: Vec<Vec<usize>> = vec![Vec::new(); n_lsda];
    let mut lsda_refs: Vec<BTreeSet<Vec<u8>>> = vec![BTreeSet::new(); n_lsda];
    let mut lsda_bad = vec![false; n_lsda];
    let mut lsda_users: Vec<BTreeSet<usize>> = vec![BTreeSet::new(); n_lsda];
    // The unit (or LSDA atom) each symbol is in.
    let mut sym_unit: Vec<Option<usize>> = vec![None; o.symbols.len()];
    for (k, u) in units.iter().enumerate() {
        for &i in &u.main().symbols {
            sym_unit[i as usize] = Some(k);
        }
    }
    let mut sym_lsda: Vec<Option<usize>> = vec![None; o.symbols.len()];
    if let Some(l) = ls {
        for (k, (_, syms)) in atom_list[l].iter().enumerate() {
            for &i in syms {
                sym_lsda[i as usize] = Some(k);
            }
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

    // Every relocation: which unit (or LSDA) its place is in, and what it
    // reaches. (An unwind entry's personality and LSDA fields are read
    // below, with the entry.)
    for (si, s) in sections.iter().enumerate() {
        let is_unwind = Some(si) == p.unwind_section;
        let is_lsda = Some(si) == ls;
        let debug = s.segment_name() == b"__DWARF";
        let units_here = unit_section(s);
        for fx in reloc::fixups(cputype, &s.relocations) {
            let first = s.relocations[fx.start];
            let place = u64::from(first.address);
            if is_unwind && place % UNWIND_ENTRY >= 16 {
                continue;
            }
            let owner = if units_here { unit_at(si, place) } else { None };
            let lsda_owner = if is_lsda { lsda_at(place) } else { None };
            if let Some(k) = owner {
                units[k].atoms[0].relocations.extend(fx.clone());
            }
            if let Some(l) = lsda_owner {
                lsda_relocs[l].extend(fx.clone());
            }
            for ri in fx.clone() {
                let r = s.relocations[ri];
                if r.value.is_some() {
                    // Scattered: 32-bit targets only; not read here.
                    if let Some(k) = owner {
                        block(&mut units, k, "a scattered relocation");
                    }
                    if let Some(l) = lsda_owner {
                        lsda_bad[l] = true;
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
                        if let Some(l) = lsda_owner {
                            lsda_bad[l] = true;
                        }
                        continue;
                    };
                    if y.is_external() {
                        if let Some(k) = owner {
                            units[k].refs.insert(y.name.to_vec());
                        }
                        if let Some(l) = lsda_owner {
                            lsda_refs[l].insert(y.name.to_vec());
                        }
                        continue;
                    }
                    // A local symbol.
                    if let Some(l) = sym_lsda[r.symbolnum as usize] {
                        if !debug && lsda_owner != Some(l) {
                            lsda_bad[l] = true;
                        }
                    } else if lsda_owner.is_some() {
                        lsda_bad[lsda_owner.unwrap_or(0)] = true;
                    }
                    let target = sym_unit[r.symbolnum as usize];
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
                if let Some(l) = lsda_owner {
                    lsda_bad[l] = true;
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
                if Some(ts) == ls {
                    if let Some(l) = lsda_at(toff)
                        && !debug
                    {
                        lsda_bad[l] = true;
                    }
                    continue;
                }
                let Some(t) = unit_at(ts, toff) else {
                    continue;
                };
                if is_unwind && place % UNWIND_ENTRY == 0 && fx.len() == 1 {
                    // An unwind entry's function: the unit's own.
                    units[t].unwind.push((place, None));
                } else if debug {
                    units[t].debug = true;
                } else if owner != Some(t) {
                    block(&mut units, t, "reached from outside by a local relocation");
                }
            }
        }
    }
    for u in &mut units {
        if opaque.contains(&u.main().section) && u.blocked.is_none() {
            u.blocked = Some("a relocation into its section isn't decoded");
        }
    }
    if ls.is_some_and(|x| opaque.contains(&x)) {
        lsda_bad.fill(true);
    }
    // Unwind entries: the personality by name, the LSDA in an atom of
    // `__gcc_except_tab`; the entry's function must start in the unit and
    // its length stay in it.
    if let Some(ui) = p.unwind_section {
        let s = sections[ui];
        let at: BTreeMap<u64, usize> = s
            .relocations
            .iter()
            .enumerate()
            .map(|(i, r)| (u64::from(r.address), i))
            .collect();
        let bytes = s.bytes().unwrap_or(&[]);
        for k in 0..units.len() {
            let mut entries = std::mem::take(&mut units[k].unwind);
            for (e, lsda) in &mut entries {
                if let Some(&ri) = at.get(&(*e + 16)) {
                    let r = s.relocations[ri];
                    match o.symbols.get(r.symbolnum as usize) {
                        Some(y) if r.external && y.is_external() => {
                            units[k].refs.insert(y.name.to_vec());
                        }
                        _ => block(&mut units, k, "a personality that isn't named"),
                    }
                }
                if let Some(&ri) = at.get(&(*e + 24)) {
                    let r = s.relocations[ri];
                    let target = if r.external {
                        o.symbols.get(r.symbolnum as usize).and_then(|y| {
                            let l = sym_lsda.get(r.symbolnum as usize).copied().flatten()?;
                            let base = sections[ls?].addr + atom_list[ls?][l].0.start;
                            let addend = le(bytes, *e + 24, 8)?;
                            Some((l, y.n_value.wrapping_sub(base).wrapping_add(addend)))
                        })
                    } else {
                        turbonop_macho_object::reloc::local_address(cputype, endian, s, ri)
                            .and_then(|a| {
                                let lsec = ls?;
                                if r.symbolnum as usize != lsec + 1 {
                                    return None;
                                }
                                let off = a.checked_sub(sections[lsec].addr)?;
                                let l = lsda_at(off)?;
                                Some((l, off - atom_list[lsec][l].0.start))
                            })
                    };
                    match target {
                        Some((l, off)) => {
                            lsda_users[l].insert(k);
                            *lsda = Some((l, off));
                        }
                        None => block(&mut units, k, "an LSDA outside __gcc_except_tab"),
                    }
                }
                let len = le(bytes, *e + 8, 4).unwrap_or(0);
                let start = le(bytes, *e, 8).unwrap_or(0);
                let u = units[k].main();
                let off = start.wrapping_sub(sections[u.section].addr);
                if off < u.range.start || off + len > u.range.end {
                    block(&mut units, k, "an unwind entry reaching outside it");
                }
            }
            units[k].unwind = entries;
        }
    }
    // FDEs: each unit's, with its LSDA; the CIE's personality by name.
    if let Some(ei) = p.eh_section {
        let s = sections[ei];
        let bytes = s.bytes().unwrap_or(&[]);
        let records = crate::elf::eh_frame::records(bytes, false);
        let ptrs = turbonop_macho_object::ehframe::pointers(bytes, true, endian);
        match (records, ptrs) {
            (Some(records), Some(ptrs)) => {
                // Relocations by record.
                let mut rels_of: BTreeMap<usize, Vec<usize>> = BTreeMap::new();
                for (ri, r) in s.relocations.iter().enumerate() {
                    let a = u64::from(r.address) as usize;
                    let k = records.partition_point(|x| x.start <= a).saturating_sub(1);
                    rels_of.entry(k).or_default().push(ri);
                }
                let mut by_record: BTreeMap<usize, Vec<&turbonop_macho_object::ehframe::Pointer>> =
                    BTreeMap::new();
                for ptr in &ptrs {
                    by_record.entry(ptr.record.0).or_default().push(ptr);
                }
                for (k, rec) in records.iter().enumerate() {
                    let Some(cie_start) = rec.cie else { continue };
                    let fields = by_record.get(&rec.start).cloned().unwrap_or_default();
                    let Some(pc) = fields
                        .iter()
                        .find(|f| f.kind == turbonop_macho_object::ehframe::PointerKind::PcBegin)
                    else {
                        continue;
                    };
                    if rels_of.contains_key(&k) {
                        // A relocated FDE (not what LLVM writes for x86-64):
                        // its function's unit, if a relocation names it,
                        // stays; any other unit might be its, so all do.
                        for &ri in &rels_of[&k] {
                            let r = s.relocations[ri];
                            match sym_unit.get(r.symbolnum as usize) {
                                Some(Some(t)) if r.external => {
                                    block(&mut units, *t, "an FDE with relocations");
                                }
                                _ => {
                                    for t in 0..units.len() {
                                        block(&mut units, t, "an FDE with relocations");
                                    }
                                }
                            }
                        }
                        continue;
                    }
                    let at = pc.offset as u64;
                    let Some(v) = eh_value(bytes, at, pc.size, pc.encoding) else {
                        continue;
                    };
                    let target = match pc.encoding & 0x70 {
                        turbonop_macho_object::ehframe::DW_EH_PE_PCREL => {
                            s.addr.wrapping_add(at).wrapping_add(v as u64)
                        }
                        0 => v as u64,
                        _ => {
                            for t in 0..units.len() {
                                block(&mut units, t, "an FDE pointer of an encoding not known");
                            }
                            continue;
                        }
                    };
                    let Some((ts, toff)) = section_of_addr(target) else {
                        continue;
                    };
                    let Some(t) = unit_at(ts, toff) else { continue };
                    let len = le(bytes, at + u64::from(pc.size), pc.size).unwrap_or(u64::MAX);
                    let u = units[t].main();
                    if pc.encoding & 0x70 != turbonop_macho_object::ehframe::DW_EH_PE_PCREL
                        || pc.encoding & 0x80 != 0
                        || toff.checked_add(len).is_none_or(|e| e > u.range.end)
                    {
                        block(&mut units, t, "an FDE reaching outside it");
                        continue;
                    }
                    let pcp = EhPointer {
                        at: at - rec.start as u64,
                        size: pc.size,
                        encoding: pc.encoding,
                        atom: 0,
                        offset: toff - u.range.start,
                    };
                    // Its LSDA.
                    let mut lsda = None;
                    if let Some(lp) = fields
                        .iter()
                        .find(|f| f.kind == turbonop_macho_object::ehframe::PointerKind::Lsda)
                    {
                        let at = lp.offset as u64;
                        let v = eh_value(bytes, at, lp.size, lp.encoding).unwrap_or(0);
                        let target = s.addr.wrapping_add(at).wrapping_add(v as u64);
                        let found = ls.and_then(|l| {
                            let off = target.checked_sub(sections[l].addr)?;
                            let a = lsda_at(off)?;
                            Some((a, off - atom_list[l][a].0.start))
                        });
                        match found {
                            Some((a, off))
                                if lp.encoding & 0x70
                                    == turbonop_macho_object::ehframe::DW_EH_PE_PCREL
                                    && lp.encoding & 0x80 == 0 =>
                            {
                                lsda_users[a].insert(t);
                                lsda = Some(EhPointer {
                                    at: at - rec.start as u64,
                                    size: lp.size,
                                    encoding: lp.encoding,
                                    atom: a,
                                    offset: off,
                                });
                            }
                            _ if v == 0 => {}
                            _ => block(&mut units, t, "an FDE's LSDA outside __gcc_except_tab"),
                        }
                    }
                    // Its CIE: its relocations (the personality) by name.
                    let Some(ci) = crate::elf::eh_frame::index_of(&records, cie_start) else {
                        block(&mut units, t, "an FDE whose CIE doesn't read");
                        continue;
                    };
                    for &ri in rels_of.get(&ci).map_or(&[][..], Vec::as_slice) {
                        let r = s.relocations[ri];
                        match o.symbols.get(r.symbolnum as usize) {
                            Some(y) if r.external && y.is_external() => {
                                units[t].refs.insert(y.name.to_vec());
                            }
                            _ => block(&mut units, t, "a CIE's relocation that isn't named"),
                        }
                    }
                    let c = &records[ci];
                    units[t].fdes.push(Fde {
                        record: rec.start as u64..rec.end() as u64,
                        cie: c.start as u64..c.end() as u64,
                        id_at: rec.id_at as u64,
                        pc: pcp,
                        lsda,
                    });
                }
            }
            _ if !bytes.is_empty() => {
                for k in 0..units.len() {
                    block(&mut units, k, "an __eh_frame that doesn't decode");
                }
            }
            _ => {}
        }
    }
    // Each unit's LSDAs become atoms of it, when nothing else reaches them.
    let mut lsda_atom: BTreeMap<(usize, usize), usize> = BTreeMap::new();
    if let Some(l) = ls {
        let mut wanted: BTreeMap<usize, BTreeSet<usize>> = BTreeMap::new();
        for (a, users) in lsda_users.iter().enumerate() {
            for &k in users {
                wanted.entry(k).or_default().insert(a);
            }
        }
        for (k, set) in wanted {
            for a in set {
                if lsda_bad[a] || lsda_users[a].len() > 1 {
                    block(&mut units, k, "an LSDA reached from outside its unit");
                    continue;
                }
                let (range, syms) = atom_list[l][a].clone();
                let idx = units[k].atoms.len();
                units[k].atoms.push(Atom {
                    section: l,
                    range,
                    symbols: syms,
                    relocations: lsda_relocs[a].clone(),
                });
                let refs = lsda_refs[a].clone();
                units[k].refs.extend(refs);
                lsda_atom.insert((k, a), idx);
            }
        }
    }
    for k in 0..units.len() {
        let mut ok = true;
        for (_, lsda) in &mut units[k].unwind {
            if let Some((a, off)) = *lsda {
                match lsda_atom.get(&(k, a)) {
                    Some(&i) => *lsda = Some((i, off)),
                    None => ok = false,
                }
            }
        }
        for f in &mut units[k].fdes {
            if let Some(lp) = &mut f.lsda {
                match lsda_atom.get(&(k, lp.atom)) {
                    Some(&i) => lp.atom = i,
                    None => ok = false,
                }
            }
        }
        if !ok {
            block(&mut units, k, "an LSDA reached from outside its unit");
        }
    }
    // Data in code and optimization hints in a unit.
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
                    && let Some(t) = unit_at(ts, toff)
                {
                    block(&mut units, t, "data in code or an optimization hint");
                }
            }
        }
    }

    // Identity hashes.
    let unwind_rel: BTreeMap<u64, usize> = p
        .unwind_section
        .map(|ui| {
            sections[ui]
                .relocations
                .iter()
                .enumerate()
                .map(|(i, r)| (u64::from(r.address), i))
                .collect()
        })
        .unwrap_or_default();
    let unwind_bytes = p
        .unwind_section
        .and_then(|ui| sections[ui].bytes())
        .unwrap_or(&[]);
    for u in &mut units {
        let mut h = H128::new();
        h.u128(env);
        for (ai, a) in u.atoms.iter().enumerate() {
            let s = sections[a.section];
            h.u64(ai as u64);
            h.bytes(s.segment_name());
            h.bytes(s.name());
            h.u64(u64::from(s.flags));
            h.u64(u64::from(s.align));
            let len = if ai == 0 {
                function_length(&o, s, a, &u.unwind, unwind_bytes)
            } else {
                a.range.end - a.range.start
            };
            h.u64(len);
            if let Some(b) = s.bytes() {
                h.bytes(&b[a.range.start as usize..(a.range.start + len) as usize]);
            }
            let mut rels: Vec<usize> = a.relocations.clone();
            rels.sort_by_key(|&i| (s.relocations[i].address, i));
            for &i in &rels {
                let r = s.relocations[i];
                h.u64(u64::from(r.address) - a.range.start);
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
                            h.u64(y.n_value.wrapping_sub(s.addr) - a.range.start);
                        }
                        None => h.u64(2),
                    }
                } else {
                    h.u64(u64::from(r.symbolnum));
                }
            }
            for &i in &a.symbols {
                let y = &o.symbols[i as usize];
                // (An LSDA's local label, `GCC_except_tableN`, is numbered
                // by its object: not part of what it is.)
                if ai == 0 || y.is_external() {
                    h.bytes(&y.name);
                }
                h.u64(u64::from(y.n_type) | u64::from(y.n_desc) << 8);
                h.u64(y.n_value.wrapping_sub(s.addr) - a.range.start);
            }
        }
        let main = u.main();
        let s = sections[main.section];
        if let Some(ui) = p.unwind_section {
            let us = sections[ui];
            let b = us.bytes().unwrap_or(&[]);
            for &(e, lsda) in &u.unwind {
                let e = e as usize;
                if let Some(entry) = b.get(e..e + 32) {
                    let start = u64::from_le_bytes(entry[..8].try_into().unwrap_or([0; 8]));
                    h.u64(start.wrapping_sub(s.addr).wrapping_sub(main.range.start));
                    h.bytes(&entry[8..16]);
                }
                if let Some(&ri) = unwind_rel.get(&(e as u64 + 16))
                    && let Some(y) = o.symbols.get(us.relocations[ri].symbolnum as usize)
                {
                    h.bytes(&y.name);
                }
                if let Some((a, off)) = lsda {
                    h.u64(3);
                    h.u64(a as u64);
                    h.u64(off);
                }
            }
        }
        if let Some(ei) = p.eh_section {
            let es = sections[ei];
            let b = es.bytes().unwrap_or(&[]);
            for f in &u.fdes {
                let mut rec = b[f.record.start as usize..f.record.end as usize].to_vec();
                rec[f.id_at as usize..f.id_at as usize + 4].fill(0);
                for q in std::iter::once(&f.pc).chain(&f.lsda) {
                    rec[q.at as usize..q.at as usize + usize::from(q.size)].fill(0);
                    h.u64(q.atom as u64);
                    h.u64(q.offset);
                }
                h.bytes(&rec);
                h.bytes(&b[f.cie.start as usize..f.cie.end as usize]);
                for r in &es.relocations {
                    let a = u64::from(r.address);
                    if f.cie.contains(&a) {
                        h.u64(a - f.cie.start);
                        h.u64(u64::from(r.kind) | u64::from(r.length) << 8);
                        if let Some(y) = o.symbols.get(r.symbolnum as usize) {
                            h.bytes(&y.name);
                        }
                    }
                }
            }
        }
        u.hash = h128(&h);
    }
    let unit_of_name: BTreeMap<&[u8], usize> = units
        .iter()
        .enumerate()
        .flat_map(|(k, u)| u.names.iter().map(move |n| (n.as_slice(), k)))
        .collect();
    let mut defs = defs_of(&o);
    for (name, unit) in &mut defs {
        *unit = unit_of_name.get(name.as_slice()).copied();
    }
    drop(unit_of_name);
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
    let mut clock = std::time::Instant::now();
    let mut lap = |report: &mut Report, what: &'static str| {
        let now = std::time::Instant::now();
        report.phases.push((what, now - clock));
        clock = now;
    };
    // (Members' costs differ widely: threads take them as they finish.)
    let parsed: Vec<Parsed<'_>> = turbonop_par::map_balanced(inputs, 1, parse);
    lap(&mut report, "reading");
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
    let mut definers: HashMap<&[u8], Vec<(usize, Option<usize>)>> = HashMap::new();
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
    // (Each member's definitions by name: the unit, if a unit's.)
    // (Lookups only: hash maps.)
    let defs_by_name: Vec<HashMap<&[u8], Vec<Option<usize>>>> = parsed
        .iter()
        .map(|p| {
            let mut m: HashMap<&[u8], Vec<Option<usize>>> = HashMap::new();
            for (n, u) in &p.defs {
                m.entry(n.as_slice()).or_default().push(*u);
            }
            m
        })
        .collect();
    // (A worklist: a unit that stays makes stay the units of its member
    // that name its names, and its copies stay when one member is left.)
    let users: Vec<HashMap<&[u8], Vec<usize>>> = parsed
        .iter()
        .map(|p| {
            let mut m: HashMap<&[u8], Vec<usize>> = HashMap::new();
            for (k, u) in p.units.iter().enumerate() {
                for r in &u.refs {
                    m.entry(r.as_slice()).or_default().push(k);
                }
            }
            m
        })
        .collect();
    let groups: Vec<&Vec<(usize, usize)>> = by_hash.values().collect();
    let mut group_of: Vec<Vec<usize>> = parsed.iter().map(|p| vec![0; p.units.len()]).collect();
    for (g, copies) in groups.iter().enumerate() {
        for &(m, k) in *copies {
            group_of[m][k] = g;
        }
    }
    let mut work: Vec<(usize, usize)> = Vec::new();
    for (m, p) in parsed.iter().enumerate() {
        for (k, u) in p.units.iter().enumerate() {
            if !folds[m][k] {
                work.push((m, k));
                continue;
            }
            // Names a definition that isn't a unit's.
            let stays = u.refs.iter().any(|r| {
                defs_by_name[m]
                    .get(r.as_slice())
                    .is_some_and(|ds| ds.iter().any(Option::is_none))
            });
            if stays {
                folds[m][k] = false;
                why[m][k] = Some("calls a name of its member that stays");
                work.push((m, k));
            }
        }
    }
    while let Some((m, k)) = work.pop() {
        for n in &parsed[m].units[k].names {
            for &k2 in users[m].get(n.as_slice()).map_or(&[][..], Vec::as_slice) {
                if folds[m][k2] {
                    folds[m][k2] = false;
                    why[m][k2] = Some("calls a name of its member that stays");
                    work.push((m, k2));
                }
            }
        }
        let copies = groups[group_of[m][k]];
        let mut live = copies.iter().filter(|&&(m, k)| folds[m][k]).map(|c| c.0);
        if let Some(first) = live.next()
            && live.all(|x| x == first)
        {
            for &(m2, k2) in copies {
                if folds[m2][k2] {
                    folds[m2][k2] = false;
                    why[m2][k2] = Some("its other copies stayed");
                    work.push((m2, k2));
                }
            }
        }
    }
    if std::env::var_os("TURBOLIB_MACHO_WHY").is_some() {
        // (A diagnostic: each copy that stays, and why.)
        for (m, p) in parsed.iter().enumerate() {
            for (k, u) in p.units.iter().enumerate() {
                if let Some(w) = why[m][k] {
                    eprintln!(
                        "{}: {}: {w}",
                        String::from_utf8_lossy(inputs[m].name),
                        String::from_utf8_lossy(&u.names[0])
                    );
                }
            }
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
    }
    if synth.is_empty() {
        return (unchanged(inputs), report);
    }
    // Synthesized members in the order of their first giver, then their
    // first unit (deterministic, whatever the threads).
    let mut order: Vec<usize> = (0..synth.len()).collect();
    order.sort_by(|&a, &b| {
        (&synth[a].givers, synth[a].units[0]).cmp(&(&synth[b].givers, synth[b].units[0]))
    });
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

    lap(&mut report, "choosing");
    // The survivors' members' DWARF, read once each, on every core.
    let debug_members: Vec<usize> = synth
        .iter()
        .filter(|s| s.units.iter().any(|&(m, k)| parsed[m].units[k].debug))
        .map(|s| s.units[0].0)
        .collect::<BTreeSet<usize>>()
        .into_iter()
        .collect();
    // Then the givers rewritten and that DWARF read, together, on every
    // core (threads take jobs as they finish: their costs differ widely).
    enum Job {
        Source(usize),
        Giver(usize),
    }
    enum Done<'p> {
        Source(Box<Result<debug::Debug<'p>, &'static str>>),
        Giver(Option<Rewritten>),
    }
    let jobs: Vec<Job> = debug_members
        .iter()
        .map(|&m| Job::Source(m))
        .chain((0..parsed.len()).map(Job::Giver))
        .collect();
    let done: Vec<Done<'_>> = turbonop_par::map_balanced(&jobs, 1, |j| match *j {
        Job::Source(m) => Done::Source(Box::new(debug::read(&parsed[m]))),
        Job::Giver(m) => {
            let gone: Vec<usize> = (0..parsed[m].units.len())
                .filter(|&k| folds[m][k])
                .collect();
            if gone.is_empty() {
                return Done::Giver(None);
            }
            let anchors: BTreeSet<usize> = gone.iter().map(|&k| moved_to[&(m, k)]).collect();
            Done::Giver(Some(rewrite_giver(&parsed[m], &gone, &anchors, &synth)))
        }
    });
    let mut sources = Vec::with_capacity(debug_members.len());
    let mut rewritten: Vec<Option<Rewritten>> = Vec::with_capacity(parsed.len());
    for d in done {
        match d {
            Done::Source(s) => sources.push(*s),
            Done::Giver(g) => rewritten.push(g),
        }
    }
    lap(&mut report, "rewriting and reading DWARF");
    let source_of: BTreeMap<usize, &Result<debug::Debug<'_>, &'static str>> =
        debug_members.iter().copied().zip(sources.iter()).collect();
    // The synthesized members, on every core.
    let built: Vec<Result<(Vec<u8>, SynthDebug), String>> =
        turbonop_par::map_balanced(&(0..synth.len()).collect::<Vec<usize>>(), 1, |&i| {
            let src = synth[i]
                .units
                .first()
                .and_then(|&(m, _)| source_of.get(&m).copied());
            // The classes all its givers define in every link that loads
            // them (see `reach`): a link that loads it through a giver
            // has their definitions.
            let mut sets: Vec<&Supplies> = Vec::with_capacity(synth[i].givers.len());
            for &g in &synth[i].givers {
                match rewritten.get(g) {
                    Some(Some(Ok((.., Some(s))))) => sets.push(s),
                    _ => {
                        sets.clear();
                        break;
                    }
                }
            }
            sets.sort_by_key(|s| s.len());
            let supplied: Option<Supplies> = sets.split_first().map(|(first, rest)| {
                first
                    .iter()
                    .filter(|k| rest.iter().all(|s| s.contains(*k)))
                    .copied()
                    .collect()
            });
            build_synth(&parsed, &synth, i, &name_to, src, supplied.as_ref())
        });
    lap(&mut report, "synthesizing");
    let mut out = Vec::with_capacity(inputs.len() + synth.len());
    let mut failed_synth = false;
    for (i, b) in built.into_iter().enumerate() {
        match b {
            Ok((data, d)) => {
                report.synthesized_bytes += data.len() as u64;
                if d.described > 0 || d.dropped.is_some() {
                    match d.dropped {
                        None => report.debug_moved += d.described,
                        Some(why) => {
                            report.debug_dropped += d.described;
                            *report.debug_dropped_why.entry(why).or_default() += d.described;
                        }
                    }
                }
                report.debug_roots += d.roots;
                report.debug_tombstoned += d.tombstoned;
                report.debug_bytes += d.bytes;
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
            Some(Ok((data, tomb, removed, pruned, _))) => {
                report.tombstoned += tomb;
                match pruned {
                    Some(Ok(p)) => {
                        report.debug_pruned += 1;
                        report.debug_dead_removed += p.dead;
                        report.debug_dead_kept += p.dead_kept;
                        report.debug_reached_removed += p.reached;
                        report.debug_pruned_bytes += p.bytes;
                    }
                    Some(Err(why)) => *report.debug_unpruned_why.entry(why).or_default() += 1,
                    None => {}
                }
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
type Rewritten = Result<(Vec<u8>, usize, u64, Option<Pruning>, Option<Supplies>), String>;

/// The keys of the classes a giver's DWARF defines in every link that loads
/// it (see `reach`).
type Supplies = turbonop_hash::Set<u128>;

/// What pruning a giver's DWARF did, or why it didn't.
type Pruning = Result<prune::Pruned, &'static str>;

/// An unwind entry of a synthesized member: its function's place (an atom
/// placed, and the offset in it), the entry's bytes, its personality's
/// name, and its LSDA's place.
struct UnwindOut {
    func: (usize, u64),
    entry: Vec<u8>,
    personality: Option<Vec<u8>>,
    lsda: Option<(usize, u64)>,
}

/// A CIE of a synthesized member: its bytes and relocations (each its
/// offset in the record, the relocation, and its symbol's name).
type CieOut = (Vec<u8>, Vec<(u64, Relocation, Vec<u8>)>);

/// An FDE of a synthesized member: its bytes, its CIE (an index into the
/// CIEs), the offset of its CIE pointer, and its pointers (each to an atom
/// placed).
struct FdeOut {
    bytes: Vec<u8>,
    cie: usize,
    id_at: u64,
    pointers: Vec<EhPointer>,
}

fn put_u64(b: &mut [u8], at: usize, v: u64) {
    b[at..at + 8].copy_from_slice(&v.to_le_bytes());
}

/// Pads `b` to `len` bytes: with NOPs in code (as LLVM pads it: x86-64's
/// one-byte `nop`, arm64's `nop`), with zeros elsewhere.
fn pad_to(b: &mut Vec<u8>, len: usize, flags: u32, cputype: u32) {
    use turbonop_macho_object::consts::{
        CPU_TYPE_ARM64, CPU_TYPE_X86_64, S_ATTR_PURE_INSTRUCTIONS, S_ATTR_SOME_INSTRUCTIONS,
    };
    let code = flags & (S_ATTR_PURE_INSTRUCTIONS | S_ATTR_SOME_INSTRUCTIONS) != 0;
    while b.len() < len {
        match cputype {
            CPU_TYPE_X86_64 if code => b.push(0x90),
            CPU_TYPE_ARM64 if code && b.len().is_multiple_of(4) && len - b.len() >= 4 => {
                b.extend_from_slice(&[0x1f, 0x20, 0x03, 0xd5]);
            }
            _ => b.push(0),
        }
    }
}

/// An atom placed in a synthesized member: whose, where.
struct Placed {
    member: usize,
    unit: usize,
    atom: usize,
    section: usize,
    offset: u64,
}

/// What a synthesized member's DWARF describes: how many of its units with
/// debug information, unless it was dropped (why); its root DIEs, its
/// addresses written as tombstones, and its bytes.
#[derive(Clone, Copy, Debug, Default)]
struct SynthDebug {
    described: usize,
    dropped: Option<&'static str>,
    roots: usize,
    tombstoned: usize,
    bytes: u64,
}

/// The bytes of synthesized member `i`.
fn build_synth(
    parsed: &[Parsed<'_>],
    synth: &[Synth],
    i: usize,
    name_to: &BTreeMap<Vec<u8>, usize>,
    debug: Option<&Result<debug::Debug<'_>, &'static str>>,
    supplied: Option<&Supplies>,
) -> Result<(Vec<u8>, SynthDebug), String> {
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
    // Every unit's atoms placed (each unit's own, then its LSDAs).
    let mut placed: Vec<Placed> = Vec::new();
    let mut unit_placed: Vec<Vec<usize>> = Vec::with_capacity(s.units.len());
    let mut pads: Vec<(usize, u64)> = Vec::new();
    for &(m, k) in &s.units {
        let o = parsed[m].obj.as_ref().ok_or("no object")?;
        let u = &parsed[m].units[k];
        let mut here = Vec::with_capacity(u.atoms.len());
        for (ai, atom) in u.atoms.iter().enumerate() {
            let src = o.section(atom.section).ok_or("no section")?;
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
            let len = atom.range.end - atom.range.start;
            if at > end && end > 0 {
                // The alignment gap is an atom of its own (a local label), so
                // that the unit before it keeps its size.
                pads.push((si, end));
            }
            match (&mut sec.contents, src.bytes()) {
                (Contents::Bytes(b), Some(sb)) => {
                    let b = b.to_mut();
                    pad_to(b, at as usize, src.flags, base.header.cputype);
                    b.extend_from_slice(&sb[atom.range.start as usize..atom.range.end as usize]);
                }
                (Contents::Zerofill(n), None) => *n = at + len,
                _ => return Err("a unit's section kind differs".into()),
            }
            here.push(placed.len());
            placed.push(Placed {
                member: m,
                unit: k,
                atom: ai,
                section: si,
                offset: at,
            });
        }
        unit_placed.push(here);
    }
    // Symbols take offsets now and addresses once the sections are placed.
    let mut unit_syms: Vec<BTreeMap<u32, u32>> = vec![BTreeMap::new(); s.units.len()];
    for (ui, here) in unit_placed.iter().enumerate() {
        for &pi in here {
            let p = &placed[pi];
            let o = parsed[p.member].obj.as_ref().ok_or("no object")?;
            let atom = &parsed[p.member].units[p.unit].atoms[p.atom];
            let src = o.section(atom.section).ok_or("no section")?;
            for &yi in &atom.symbols {
                let y = &o.symbols[yi as usize];
                let off = y.n_value.wrapping_sub(src.addr) - atom.range.start + p.offset;
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
                unit_syms[ui].insert(yi, ni);
                if y.is_external() {
                    by_name.insert(y.name.to_vec(), ni);
                }
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
    // Relocations, unwind entries, and FDEs.
    let mut relocs: Vec<Vec<Relocation>> = vec![Vec::new(); sections.len()];
    let mut unwind: Vec<UnwindOut> = Vec::new();
    let mut cies: Vec<CieOut> = Vec::new();
    let mut fdes: Vec<FdeOut> = Vec::new();
    let mut eh_template: Option<(u32, u32)> = None;
    for (ui, &(m, k)) in s.units.iter().enumerate() {
        let o = parsed[m].obj.as_ref().ok_or("no object")?;
        let u = &parsed[m].units[k];
        for &pi in &unit_placed[ui] {
            let p = &placed[pi];
            let atom = &u.atoms[p.atom];
            let src = o.section(atom.section).ok_or("no section")?;
            for &ri in &atom.relocations {
                let mut r = src.relocations[ri];
                r.address = u32::try_from(u64::from(r.address) - atom.range.start + p.offset)
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
                        *unit_syms[ui]
                            .get(&r.symbolnum)
                            .ok_or("a local symbol outside the unit")?
                    };
                }
                relocs[p.section].push(r);
            }
        }
        let main = u.main();
        let src = o.section(main.section).ok_or("no section")?;
        if let Some(ui_sec) = parsed[m].unwind_section {
            let us = o.section(ui_sec).ok_or("no unwind section")?;
            let b = us.bytes().unwrap_or(&[]);
            for &(e, lsda) in &u.unwind {
                let e = e as usize;
                let entry = b
                    .get(e..e + 32)
                    .ok_or("an unwind entry past its section")?
                    .to_vec();
                let start = u64::from_le_bytes(entry[..8].try_into().unwrap_or([0; 8]));
                let off = start.wrapping_sub(src.addr) - main.range.start;
                let personality = us
                    .relocations
                    .iter()
                    .find(|r| u64::from(r.address) == e as u64 + 16)
                    .map(|r| o.symbols[r.symbolnum as usize].name.to_vec());
                unwind.push(UnwindOut {
                    func: (unit_placed[ui][0], off),
                    entry,
                    personality,
                    lsda: lsda.map(|(a, off)| (unit_placed[ui][a], off)),
                });
            }
        }
        if let Some(ei) = parsed[m].eh_section
            && !u.fdes.is_empty()
        {
            let es = o.section(ei).ok_or("no __eh_frame")?;
            eh_template.get_or_insert((es.align, es.flags));
            let b = es.bytes().unwrap_or(&[]);
            for f in &u.fdes {
                let cie_bytes = b[f.cie.start as usize..f.cie.end as usize].to_vec();
                let cie_rels: Vec<(u64, Relocation, Vec<u8>)> = es
                    .relocations
                    .iter()
                    .filter(|r| f.cie.contains(&u64::from(r.address)))
                    .map(|r| {
                        (
                            u64::from(r.address) - f.cie.start,
                            *r,
                            o.symbols[r.symbolnum as usize].name.to_vec(),
                        )
                    })
                    .collect();
                let key = (cie_bytes, cie_rels);
                let cie = match cies.iter().position(|c| *c == key) {
                    Some(c) => c,
                    None => {
                        cies.push(key);
                        cies.len() - 1
                    }
                };
                let mut pointers = vec![EhPointer {
                    atom: unit_placed[ui][f.pc.atom],
                    ..f.pc
                }];
                if let Some(l) = f.lsda {
                    pointers.push(EhPointer {
                        atom: unit_placed[ui][l.atom],
                        ..l
                    });
                }
                fdes.push(FdeOut {
                    bytes: b[f.record.start as usize..f.record.end as usize].to_vec(),
                    cie,
                    id_at: f.id_at,
                    pointers,
                });
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
    // `__eh_frame`: the CIEs, then the FDEs (their pointers set once the
    // sections are placed); the CIEs' relocations (personalities) by name.
    let mut eh_patches: Vec<(u64, EhPointer)> = Vec::new();
    let eh_si = match eh_template {
        Some((align, flags)) if !fdes.is_empty() => {
            let mut pieces: Vec<(&[u8], Option<usize>, usize)> = Vec::new();
            for (b, _) in &cies {
                pieces.push((b, None, 4));
            }
            for f in &fdes {
                pieces.push((&f.bytes, Some(f.cie), f.id_at as usize));
            }
            let (bytes, starts) = crate::elf::eh_frame::build(&pieces, false);
            let mut rs = Vec::new();
            for (c, (_, rels)) in cies.iter().enumerate() {
                for (off, r, name) in rels {
                    let mut r = *r;
                    r.address = u32::try_from(starts[c] as u64 + off)
                        .map_err(|_| "an __eh_frame too large")?;
                    r.symbolnum = undefined(&mut symbols, &mut by_name, name, false);
                    rs.push(r);
                }
            }
            for (j, f) in fdes.iter().enumerate() {
                let start = starts[cies.len() + j] as u64;
                for q in &f.pointers {
                    eh_patches.push((start, *q));
                }
            }
            sections.push(Section {
                sectname: name_field(b"__eh_frame"),
                segname: name_field(b"__TEXT"),
                addr: 0,
                align,
                flags,
                reserved1: 0,
                reserved2: 0,
                reserved3: 0,
                contents: Contents::Bytes(Cow::Owned(bytes)),
                relocations: Vec::new(),
            });
            relocs.push(rs);
            Some(sections.len() - 1)
        }
        _ => None,
    };
    // The units' DWARF (see the `debug` module).
    let mut stats = SynthDebug {
        described: s
            .units
            .iter()
            .filter(|&&(m, k)| parsed[m].units[k].debug)
            .count(),
        ..SynthDebug::default()
    };
    let mut debug_secs: Vec<(usize, debug::Refs)> = Vec::new();
    if stats.described > 0 {
        let built = match debug {
            _ if s.units.iter().any(|&(m, _)| m != m0) => Err("units of more than one member"),
            Some(Ok(d)) => {
                let ks: Vec<usize> = s.units.iter().map(|&(_, k)| k).collect();
                debug::build(d, &ks, supplied)
            }
            Some(Err(e)) => Err(*e),
            None => Err("no DWARF read"),
        };
        match built {
            Ok(Some(out)) => {
                stats.roots = out.roots;
                stats.tombstoned = out.tombstoned;
                let like = |name: &[u8]| {
                    base.sections()
                        .find(|x| x.segment_name() == b"__DWARF" && x.name() == name)
                        .map(|x| (x.align, x.flags))
                };
                for (sec, refs) in debug::sections(&out, &like) {
                    stats.bytes += sec.size();
                    sections.push(sec);
                    relocs.push(Vec::new());
                    debug_secs.push((sections.len() - 1, refs));
                }
            }
            Ok(None) => stats.dropped = Some("no DWARF describes them"),
            Err(e) => stats.dropped = Some(e),
        }
    }
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
    for p in &mut placed {
        p.section = new_of[p.section];
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
    let eh_si = eh_si.map(|k| new_of[k]);
    for d in &mut debug_secs {
        d.0 = new_of[d.0];
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
    let address_of = |sections: &[Section<'_>], (pi, off): (usize, u64)| -> (usize, u64) {
        let p = &placed[pi];
        (p.section, sections[p.section].addr + p.offset + off)
    };
    // Unwind entries: the function's address, a local relocation to its
    // section; the personality by name; the LSDA's address, a local
    // relocation to its section.
    if let Some(ui) = unwind_si {
        let mut entries: Vec<(usize, u64, UnwindOut)> = unwind
            .into_iter()
            .map(|e| {
                let (sec, a) = address_of(&sections, e.func);
                (sec, a, e)
            })
            .collect();
        entries.sort_by_key(|e| (e.0, e.1));
        let mut bytes = Vec::with_capacity(entries.len() * 32);
        let mut rs = Vec::new();
        for (sec, a, e) in entries {
            let at = bytes.len() as u32;
            let mut entry = e.entry;
            put_u64(&mut entry, 0, a);
            if e.personality.is_some() {
                put_u64(&mut entry, 16, 0);
            }
            rs.push(Relocation::plain(at, (sec + 1) as u32, false, 0, 3));
            if let Some(p) = e.personality {
                let k = undefined(&mut symbols, &mut by_name, &p, false);
                rs.push(Relocation::plain(at + 16, k, true, 0, 3));
            }
            if let Some(l) = e.lsda {
                let (lsec, la) = address_of(&sections, l);
                put_u64(&mut entry, 24, la);
                rs.push(Relocation::plain(at + 24, (lsec + 1) as u32, false, 0, 3));
            }
            bytes.extend_from_slice(&entry);
        }
        // (LLVM writes a section's relocations last to first.)
        rs.reverse();
        relocs[ui] = rs;
        sections[ui].contents = Contents::Bytes(Cow::Owned(bytes));
    }
    // FDEs' pointers: PC-relative, to the atoms placed.
    if let Some(ei) = eh_si {
        let eh_addr = sections[ei].addr;
        let mut writes = Vec::with_capacity(eh_patches.len());
        for (start, q) in eh_patches {
            let field = start + q.at;
            let (_, target) = address_of(&sections, (q.atom, q.offset));
            let v = target.wrapping_sub(eh_addr + field);
            writes.push((field as usize, usize::from(q.size), v));
        }
        if let Contents::Bytes(b) = &mut sections[ei].contents {
            let b = b.to_mut();
            for (at, n, v) in writes {
                b[at..at + n].copy_from_slice(&v.to_le_bytes()[..n]);
            }
        }
    }
    // The DWARF's addresses: each moved unit's atom.
    if !debug_secs.is_empty() {
        let mut atom_at: BTreeMap<usize, (usize, u64)> = BTreeMap::new();
        if let Some(Ok(d)) = debug {
            for (ui, &(_, k)) in s.units.iter().enumerate() {
                atom_at.insert(
                    d.unit_atom[k],
                    address_of(&sections, (unit_placed[ui][0], 0)),
                );
            }
        }
        for (si, refs) in std::mem::take(&mut debug_secs) {
            if let Contents::Bytes(b) = &mut sections[si].contents {
                let (rs, t) = debug::fill(b.to_mut(), &refs, &|id| atom_at.get(&id).copied())?;
                stats.tombstoned += t;
                relocs[si] = rs;
            }
        }
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
    // Names moved into other synthesized members after the anchors that
    // load those, as for givers.
    move_last(&mut obj, |y| {
        name_to.get(y.name.as_ref()).is_some_and(|&t| t != i)
    });
    obj.update_dysymtab();
    obj.relayout();
    obj.write().map(|b| (b, stats)).map_err(|e| e.to_string())
}

/// Moves the undefined names `late` says to the end of the symbol table
/// (after the anchors, which LLVM's order puts last among the rest),
/// renumbering every reference.
fn move_last(o: &mut Object<'_>, late: impl Fn(&Symbol<'_>) -> bool) {
    let mut order: Vec<usize> = (0..o.symbols.len()).collect();
    order.sort_by_key(|&i| (o.symbols[i].is_undefined() && late(&o.symbols[i]), i));
    if order.iter().enumerate().all(|(n, &i)| n == i) {
        return;
    }
    let mut map = vec![None; order.len()];
    for (new, &old) in order.iter().enumerate() {
        map[old] = Some(new as u32);
    }
    let mut slots: Vec<Option<Symbol<'_>>> = core::mem::take(&mut o.symbols)
        .into_iter()
        .map(Some)
        .collect();
    o.symbols = order.iter().filter_map(|&i| slots[i].take()).collect();
    o.renumber_symbols(&map);
}

/// Member `p` without its units `gone`, referring to the synthesized
/// members `anchors`: its bytes, the debug relocations removed, and the
/// bytes of units removed.
fn rewrite_giver(
    p: &Parsed<'_>,
    gone: &[usize],
    anchors: &BTreeSet<usize>,
    synth: &[Synth],
) -> Rewritten {
    let mut o = p.obj.clone().ok_or("no object")?;
    // What of its DWARF the cut can't move (see `adjust`), read first.
    let plan = adjust::Plan::read(&o).map_err(|e| format!("its DWARF: {e}"))?;
    if let Some(p) = &plan {
        p.strip(&mut o);
    }
    let cputype = o.header.cputype;
    let endian = o.endian();
    let mut removed = 0u64;
    // The address ranges going.
    let ranges: Vec<(usize, Range<u64>, Range<u64>)> = gone
        .iter()
        .flat_map(|&k| p.units[k].atoms.iter())
        .map(|u| {
            let a = o.section(u.section).map_or(0, |s| s.addr);
            (
                u.section,
                u.range.clone(),
                a + u.range.start..a + u.range.end,
            )
        })
        .collect();
    // (In the section the relocation names: an address at a section's end,
    // as a range list's end has, isn't the next section's first atom.)
    let mut by_section: BTreeMap<usize, Vec<Range<u64>>> = BTreeMap::new();
    for (si, _, r) in &ranges {
        by_section.entry(*si).or_default().push(r.clone());
    }
    for v in by_section.values_mut() {
        v.sort_by_key(|r| r.start);
    }
    let in_gone = |section: usize, a: u64| {
        by_section.get(&section).is_some_and(|v| {
            let k = v.partition_point(|r| r.start <= a);
            k.checked_sub(1).is_some_and(|k| v[k].contains(&a))
        })
    };
    let gone_syms: BTreeSet<u32> = gone
        .iter()
        .flat_map(|&k| {
            p.units[k]
                .atoms
                .iter()
                .flat_map(|a| a.symbols.iter().copied())
        })
        .collect();
    // 1. Debug sections' relocations to what goes: removed, their places
    // all ones (the tombstone address: 0 is an address of the object).
    let mut tomb = 0;
    let mut gone_info: BTreeSet<u32> = BTreeSet::new();
    let nsec = o.section_count();
    for si in 0..nsec {
        let Some(s) = o.section(si) else { continue };
        if s.segment_name() != b"__DWARF" || s.name() == b"__debug_line" {
            // (The line program is encoded again below, without the rows
            // of what goes.)
            continue;
        }
        let mut drop = BTreeSet::new();
        for (ri, r) in s.relocations.iter().enumerate() {
            let hit = if r.external {
                gone_syms.contains(&r.symbolnum) && !o.symbols[r.symbolnum as usize].is_external()
            } else if reloc::names_section(cputype, r) {
                reloc::local_address(cputype, endian, s, ri)
                    .is_some_and(|a| in_gone(r.symbolnum as usize - 1, a))
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
        if s.name() == b"__debug_info" {
            gone_info.extend(places.iter().map(|p| p.0));
        }
        if let Contents::Bytes(b) = &mut s.contents {
            let b = b.to_mut();
            for (at, len) in places {
                let n = 1usize << len.min(3);
                if let Some(x) = b.get_mut(at as usize..at as usize + n) {
                    x.fill(0xFF);
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
            for &(e, _) in &p.units[k].unwind {
                cuts.entry(ui).or_default().push(e..e + UNWIND_ENTRY);
            }
        }
    }
    if let Some(ei) = p.eh_section {
        for &k in gone {
            for f in &p.units[k].fdes {
                cuts.entry(ei).or_default().push(f.record.clone());
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
    let mut pruned = None;
    let mut supplies = None;
    let old: Vec<(u64, u64)> = o.sections().map(|s| (s.addr, s.size())).collect();
    let cut = o.cut(&cuts).map_err(|e| e.to_string())?;
    if !cut.dangling.is_empty() {
        return Err("a relocation still reaches what was cut".into());
    }
    if !cut.unadjusted.is_empty() {
        return Err("an address the cut couldn't move".into());
    }
    if let Some(plan) = plan {
        let mut per = vec![Vec::new(); old.len()];
        for (s, v) in &cuts {
            per[*s].clone_from(v);
        }
        let map = adjust::AddressMap {
            old,
            new: o.sections().map(|s| s.addr).collect(),
            cuts: per,
        };
        plan.apply(&mut o, &map)
            .map_err(|e| format!("its DWARF: {e}"))?;
        // The classes it still defines in every link that loads it (none,
        // where that isn't known).
        supplies = reach::reach(&o, &gone_syms).ok().flatten();
        // Its DWARF without what described the lost code (see `prune`),
        // or as it is.
        let mut t = o.clone();
        match prune::prune(&mut t, &gone_info, &gone_syms) {
            Ok(Some(st)) => {
                o = t;
                pruned = Some(Ok(st));
            }
            Ok(None) => {}
            Err(e) => pruned = Some(Err(e)),
        }
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
    // The moved names last, after the anchors (the undefined names are
    // otherwise in LLVM's order): see the module's documentation.
    let moved: BTreeSet<Vec<u8>> = gone
        .iter()
        .flat_map(|&k| p.units[k].names.iter().cloned())
        .collect();
    move_last(&mut o, |y| moved.contains(y.name.as_ref()));
    o.update_dysymtab();
    o.relayout();
    let data = o.write().map_err(|e| e.to_string())?;
    Ok((data, tomb, removed, pruned, supplies))
}

#[doc(hidden)]
pub mod adjust;
mod debug;
#[doc(hidden)]
pub mod prune;
mod reach;
#[cfg(test)]
mod tests;
