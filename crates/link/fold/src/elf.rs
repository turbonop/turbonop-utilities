//! Folding an ELF static library's duplicate COMDAT groups (`--fold=identical`).
//!
//! **Units.** A unit is a COMDAT group (`SHT_GROUP` with `GRP_COMDAT`): its
//! member sections (relocation sections included, listed or not), the
//! symbols defined in them, and the FDEs in the member's `.eh_frame` whose
//! `pc_begin` names one of its sections. Its identity hash covers the
//! group's flags, each member section's name, type, flags, alignment,
//! entry size, and bytes, each relocation as (offset, type, target,
//! addend) with a target outside the unit named by its symbol's name and
//! one inside it by its section's place in the unit and its value, the
//! symbols it defines, its FDEs and their CIEs (bytes and relocations, the
//! CIE pointers left out), and the member's environment.
//!
//! **The environment** is what a synthesized member must carry for its
//! units: the class, byte order, OS ABI, `e_machine`, `e_flags`, and the
//! bytes of `.note.gnu.property`, `.note.GNU-stack` (its flags: an object
//! without it makes the stack executable), `.comment`, and the
//! architectures' attribute sections (`.ARM.attributes`, `.riscv.attributes`,
//! `.MIPS.abiflags`, `.gnu.attributes`). Only units of one environment fold
//! together, and each synthesized member copies its environment's.
//!
//! **What stops a unit folding** (`--locals=keep`): a relocation reaching
//! a local symbol outside it (a string literal in `.rodata.str1.1`, a
//! `static` function); a local symbol of it named from outside but by
//! `.eh_frame` and the debug sections; a section outside it whose
//! `SHF_LINK_ORDER` names one of it; an FDE whose LSDA is outside it.
//!
//! **What folds.** The copies of a unit fold when every definition of
//! each of their names in the library is such a copy (so whichever copy a
//! link keeps is the same), and, under `--pull=preserve`, the member the
//! index sends each name to (its first definer) has no side effects of its
//! own: `.init_array`, `.fini_array`, `.preinit_array`, `.ctors`, or
//! `.dtors` outside a group, a section named like a C identifier (which
//! `__start_`/`__stop_` symbols collect), `.gnu.warning` sections, or
//! `SHF_GNU_RETAIN`. ELF has nothing like COFF's `NOLIBRARY` weak
//! externals, so a link that needed a folded name and nothing else of its
//! first definer no longer loads that member; these are the units for
//! which that could change what runs. (As for COFF, what a definer without
//! side effects loads in its turn isn't followed.)
//!
//! **Rewriting.** The first copy survives into a synthesized member (one
//! per set of members that held the copies, and environment), which keeps
//! its group (so libraries' copies still fold in a link), its FDEs and
//! their CIEs, and its environment's notes. Each member that held a copy
//! loses its sections; its definitions become weak undefined references
//! (which load nothing); its FDEs leave its `.eh_frame`; and it gains a
//! strong reference to the synthesized member's anchor (a hidden weak
//! absolute symbol, named for the synthesized member's contents), first
//! among its globals, so that whenever it is loaded, so is the code it gave
//! up, as for COFF, and the moved names come from there and not from
//! another library's copy. A synthesized member refers to names moved
//! into another one the same way.
//!
//! **Debug information** is not moved yet: in the member that gave a copy
//! up, each relocation from a `.debug_*` section to a local symbol of the
//! copy (its section's symbol) is removed and its place set to the value a
//! linker gives one to a discarded section (lld's: 1 in `.debug_loc` and
//! `.debug_ranges`, all ones in `.debug_names`, 0 elsewhere), as the link
//! of the original library would have for every copy but the one it kept.
//! The synthesized member has none: a folded function has no debug
//! information (counted in [`Report::debug_dropped`]).

use std::borrow::Cow;
use std::collections::BTreeMap;

use turbonop_elf_object::{
    Class, Data, Endian, Group, Header, Object, Relocation, SHN_ABS, SHN_UNDEF, Section, Symbol,
    shf, sht,
};

use crate::hash::H128;

mod eh_frame;

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
pub struct Options {
    pub pull: Pull,
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
    /// Units that stayed, by why.
    pub kept: BTreeMap<&'static str, usize>,
    /// Distinct units with copies in two or more members.
    pub duplicated: usize,
    /// Distinct units folded, and the copies they had.
    pub folded: usize,
    pub copies: usize,
    pub synthesized: usize,
    /// Bytes of the units' sections removed from members, and written in
    /// synthesized members.
    pub removed_bytes: u64,
    pub synthesized_bytes: u64,
    /// Debug relocations set to a tombstone, and folded units that had
    /// debug information (none of which a synthesized member has).
    pub tombstoned: usize,
    pub debug_dropped: usize,
}

/// The machines whose debug relocations can be tombstoned: the size of
/// each absolute relocation type a debug section uses.
fn debug_reloc_size(machine: u16, kind: u32) -> Option<usize> {
    match (machine, kind) {
        // R_X86_64_64, _32, _32S, _DTPOFF64, _DTPOFF32.
        (62, 1 | 17) => Some(8),
        (62, 10 | 11 | 21) => Some(4),
        // R_AARCH64_ABS64, _ABS32, _ABS16.
        (183, 257) => Some(8),
        (183, 258) => Some(4),
        (183, 259) => Some(2),
        // R_386_32, R_386_TLS_LDO_32.
        (3, 1 | 32) => Some(4),
        _ => None,
    }
}

/// The value lld gives a debug section's relocation to a discarded
/// section.
fn tombstone(section: &[u8]) -> u64 {
    match section {
        b".debug_loc" | b".debug_ranges" => 1,
        b".debug_names" => u64::MAX,
        _ => 0,
    }
}

const GRP_COMDAT: u32 = 1;

/// A unit of one member.
#[derive(Clone, Debug)]
struct Unit {
    /// The group section.
    group: usize,
    /// Its member sections (the group's list, then relocation sections for
    /// them the list leaves out).
    sections: Vec<usize>,
    /// The global names it defines.
    names: Vec<Vec<u8>>,
    hash: u128,
    stays: Option<&'static str>,
    /// Its FDEs (indexes into the member's records).
    fdes: Vec<usize>,
    /// Whether debug sections refer to it.
    debug: bool,
    bytes: u64,
}

/// One member, read.
struct Parsed<'a> {
    obj: Option<Object<'a>>,
    why_not: Option<String>,
    env: u128,
    side_effects: bool,
    units: Vec<Unit>,
    /// Every global the member defines, with its unit (`None`: outside
    /// any).
    defs: Vec<(Vec<u8>, Option<usize>)>,
    /// `.eh_frame`'s index and records, if any.
    eh: Option<(usize, Vec<eh_frame::Record>)>,
    /// Each section's unit.
    owner: Vec<Option<usize>>,
    /// An object whose definitions can't be read (another format).
    opaque: bool,
}

fn is_c_identifier(name: &[u8]) -> bool {
    !name.is_empty()
        && !name[0].is_ascii_digit()
        && name.iter().all(|&b| b.is_ascii_alphanumeric() || b == b'_')
}

fn is_debug(name: &[u8]) -> bool {
    name.starts_with(b".debug_")
}

/// The relocation sections of `obj`: (section, target section).
fn relocation_sections(obj: &Object<'_>) -> Vec<(usize, usize)> {
    obj.sections
        .iter()
        .enumerate()
        .filter(|(_, s)| s.is_relocations())
        .map(|(i, s)| (i, s.info as usize))
        .collect()
}

fn relocations<'o>(s: &'o Section<'_>) -> &'o [Relocation] {
    match &s.data {
        Data::Relocations(r) => r,
        _ => &[],
    }
}

fn section_bytes<'o>(s: &'o Section<'_>) -> &'o [u8] {
    match &s.data {
        Data::Bytes(b) => b,
        _ => &[],
    }
}

fn environment(obj: &Object<'_>) -> u128 {
    let mut h = H128::new();
    // Class, byte order, version, and ABI version; not the OS ABI, which
    // GNU as sets to GNU in objects with STB_GNU_UNIQUE or STT_GNU_IFUNC
    // symbols (a synthesized member gets it by its own symbols).
    h.bytes(&obj.header.ident[4..7]);
    h.u64(u64::from(obj.header.ident[8]));
    h.u64(u64::from(obj.header.machine));
    h.u64(u64::from(obj.header.flags));
    for name in [
        &b".note.gnu.property"[..],
        b".note.GNU-stack",
        b".comment",
        b".ARM.attributes",
        b".riscv.attributes",
        b".MIPS.abiflags",
        b".gnu.attributes",
    ] {
        match obj.sections.iter().find(|s| &*s.name == name) {
            Some(s) => {
                h.u64(1);
                h.u64(s.flags);
                h.u64(u64::from(s.kind));
                h.bytes(section_bytes(s));
            }
            None => h.u64(0),
        }
    }
    h.finish()
}

/// Reads one member and finds its units.
fn parse<'a>(input: &Input<'a>) -> Parsed<'a> {
    let mut p = Parsed {
        obj: None,
        why_not: None,
        env: 0,
        side_effects: false,
        units: Vec::new(),
        defs: Vec::new(),
        eh: None,
        owner: Vec::new(),
        opaque: false,
    };
    let data = input.data;
    if !data.starts_with(b"\x7fELF") {
        // Text and archives define nothing; bitcode, wasm, Mach-O, and
        // COFF objects, things this engine doesn't read.
        let le = |b: &[u8]| {
            b.get(..4)
                .map(|b| u32::from_le_bytes([b[0], b[1], b[2], b[3]]))
        };
        let coff = data.len() >= 20
            && data[16..18] == [0, 0]
            && matches!(
                u16::from_le_bytes([data[0], data[1]]),
                0x014C | 0x8664 | 0xAA64
            );
        p.opaque = data.starts_with(b"BC\xC0\xDE")
            || data.starts_with(b"\0asm")
            || matches!(le(data), Some(0x0B17_C0DE | 0xFEED_FACE | 0xFEED_FACF))
            || coff;
        p.why_not = Some("not ELF".into());
        return p;
    }
    let obj = match Object::parse(data) {
        Ok(o) => o,
        Err(e) => {
            // Its definitions, as an index lists them, so that no name it
            // defines folds.
            match turbonop_elf::ElfFile::parse(data) {
                Ok(f) => {
                    for s in f.all_symbols().iter().skip(1) {
                        if s.bind != 0 && s.shndx != 0 {
                            p.defs.push((s.name.to_vec(), None));
                        }
                    }
                }
                Err(_) => p.opaque = true,
            }
            p.why_not = Some(format!("not read: {e}"));
            return p;
        }
    };
    if obj.header.kind != 1 {
        p.why_not = Some("not a relocatable object".into());
        return p;
    }
    if obj.sections.iter().any(|s| s.kind == sht::CREL) {
        p.why_not = Some("CREL relocations".into());
        return p;
    }
    if obj
        .sections
        .iter()
        .filter(|s| s.kind == sht::SYMTAB)
        .count()
        != 1
    {
        p.why_not = Some("not exactly one symbol table".into());
        return p;
    }
    let symtab = obj.symtab().unwrap_or(0);
    let symbols = obj.symbols();
    let n = obj.sections.len();
    p.env = environment(&obj);

    // Each section's unit.
    let mut owner: Vec<Option<usize>> = vec![None; n];
    let mut units: Vec<Unit> = Vec::new();
    for (i, s) in obj.sections.iter().enumerate() {
        if let Data::Group(g) = &s.data
            && g.flags & GRP_COMDAT != 0
            && s.link as usize == symtab
        {
            let u = units.len();
            let mut members = Vec::with_capacity(g.members.len());
            let mut ok = true;
            for &m in &g.members {
                let m = m as usize;
                if m >= n || owner[m].is_some() {
                    ok = false;
                    break;
                }
                owner[m] = Some(u);
                members.push(m);
            }
            units.push(Unit {
                group: i,
                sections: members,
                names: Vec::new(),
                hash: 0,
                stays: (!ok).then_some("its group's members are malformed"),
                fdes: Vec::new(),
                debug: false,
                bytes: 0,
            });
            owner[i] = Some(u);
        }
    }
    // Relocation sections for a unit's sections are the unit's.
    for (r, target) in relocation_sections(&obj) {
        if owner[r].is_none()
            && let Some(&Some(u)) = owner.get(target)
        {
            owner[r] = Some(u);
            units[u].sections.push(r);
        }
    }
    // Side effects outside any group.
    for (i, s) in obj.sections.iter().enumerate() {
        if owner[i].is_some() {
            continue;
        }
        let name = &*s.name;
        let registration = matches!(
            s.kind,
            sht::INIT_ARRAY | sht::FINI_ARRAY | sht::PREINIT_ARRAY
        ) || name.starts_with(b".init_array")
            || name.starts_with(b".fini_array")
            || name.starts_with(b".preinit_array")
            || name.starts_with(b".ctors")
            || name.starts_with(b".dtors")
            || name.starts_with(b".gnu.warning");
        if registration || is_c_identifier(name) || s.flags & shf::GNU_RETAIN != 0 {
            p.side_effects = true;
        }
        if s.flags & shf::LINK_ORDER != 0
            && let Some(Some(u)) = owner.get(s.link as usize)
        {
            units[*u].stays = Some("a section outside it is linked to one of its");
        }
    }
    // Definitions.
    let sym_owner = |s: &Symbol<'_>| -> Option<usize> {
        if s.in_section() {
            owner.get(s.shndx as usize).copied().flatten()
        } else {
            None
        }
    };
    for s in symbols.iter().skip(1) {
        if s.bind() != 0 && s.shndx != SHN_UNDEF {
            let u = sym_owner(s);
            p.defs.push((s.name.to_vec(), u));
            if let Some(u) = u {
                units[u].names.push(s.name.to_vec());
            }
        }
    }
    // References: from each unit to locals outside it; to a unit's locals
    // from outside it.
    for (r, target) in relocation_sections(&obj) {
        let from = owner[r];
        let target_name = obj.sections.get(target).map_or(&b""[..], |s| &*s.name);
        let debug = is_debug(target_name) && obj.sections[target].flags & shf::ALLOC == 0;
        let eh = target_name == b".eh_frame";
        for rel in relocations(&obj.sections[r]) {
            let si = obj.reloc_symbol(rel) as usize;
            let Some(sym) = symbols.get(si) else {
                if let Some(u) = from {
                    units[u].stays = Some("a relocation names no symbol");
                }
                continue;
            };
            if sym.bind() != 0 || si == 0 {
                continue;
            }
            let to = sym_owner(sym);
            match (from, to) {
                (Some(a), Some(b)) if a == b => {}
                (Some(a), _) => units[a].stays = Some("it reaches a local symbol outside it"),
                (None, Some(b)) if debug => {
                    if debug_reloc_size(obj.header.machine, obj.reloc_type(rel)).is_none() {
                        units[b].stays =
                            Some("debug information refers to it in a way not tombstoned");
                    }
                    units[b].debug = true;
                }
                (None, Some(_)) if eh => {}
                (None, Some(b)) => {
                    units[b].stays = Some("a local symbol of it is named from outside it");
                }
                (None, None) => {}
            }
        }
    }
    // FDEs.
    if let Some(eh) = obj.sections.iter().position(|s| &*s.name == b".eh_frame") {
        let big = obj.endian() == Endian::Big;
        match eh_frame::records(section_bytes(&obj.sections[eh]), big) {
            Some(records) => {
                let rels: Vec<&Relocation> = relocation_sections(&obj)
                    .into_iter()
                    .filter(|&(_, t)| t == eh)
                    .flat_map(|(r, _)| relocations(&obj.sections[r]).iter())
                    .collect();
                for (k, rec) in records.iter().enumerate() {
                    if rec.cie.is_none() {
                        continue;
                    }
                    let mut unit = None;
                    let mut others = Vec::new();
                    for rel in &rels {
                        let at = rel.offset as usize;
                        if at < rec.start || at >= rec.end() {
                            continue;
                        }
                        let sym = symbols.get(obj.reloc_symbol(rel) as usize);
                        let to = sym.filter(|s| s.bind() == 0).and_then(&sym_owner);
                        if at == rec.pc_begin() {
                            unit = to;
                        } else {
                            others.push(to);
                        }
                    }
                    if let Some(u) = unit {
                        units[u].fdes.push(k);
                        if others.iter().any(|&o| o.is_some_and(|o| o != u)) {
                            units[u].stays = Some("its FDE's LSDA is in another group");
                        }
                    } else {
                        for b in others.into_iter().flatten() {
                            units[b].stays = Some("an FDE of another function names it");
                        }
                    }
                }
                p.eh = Some((eh, records));
            }
            None => {
                for u in &mut units {
                    u.stays = Some("the member's .eh_frame doesn't parse");
                }
            }
        }
    }
    // Hashes.
    for (ui, u) in units.iter_mut().enumerate() {
        let group = u.group;
        let unit_sections = u.sections.clone();
        let fdes = u.fdes.clone();
        let mut stays = u.stays;
        let mut h = H128::new();
        h.u128(p.env);
        if let Data::Group(g) = &obj.sections[group].data {
            h.u64(u64::from(g.flags));
        }
        let place = |sec: usize| unit_sections.iter().position(|&x| x == sec);
        let key = |h: &mut H128, si: u32| {
            let Some(sym) = symbols.get(si as usize) else {
                h.u64(u64::MAX);
                return;
            };
            if sym.bind() != 0 || si == 0 {
                h.u64(1);
                h.bytes(&sym.name);
                h.u64(u64::from(sym.bind()));
            } else {
                h.u64(2);
                h.u64(place(sym.shndx as usize).map_or(u64::MAX, |x| x as u64));
                h.u64(sym.value);
                h.u64(u64::from(sym.kind()));
            }
        };
        let mut bytes = 0u64;
        for &sec in &unit_sections {
            let s = &obj.sections[sec];
            h.bytes(&s.name);
            h.u64(u64::from(s.kind));
            h.u64(s.flags);
            h.u64(s.addralign);
            h.u64(s.entsize);
            match &s.data {
                Data::Bytes(b) => {
                    h.bytes(b);
                    bytes += b.len() as u64;
                }
                Data::NoBits => h.u64(s.nobits_size),
                Data::Relocations(rs) => {
                    h.u64(place(s.info as usize).map_or(u64::MAX, |x| x as u64));
                    for r in rs {
                        h.u64(r.offset);
                        h.u64(u64::from(obj.reloc_type(r)));
                        key(&mut h, obj.reloc_symbol(r));
                        h.u64(r.addend as u64);
                    }
                    bytes += rs.len() as u64 * 24;
                }
                _ => {
                    h.u64(0xBAD);
                    stays = Some("it holds a section of a kind not moved");
                }
            }
        }
        for s in symbols.iter().skip(1) {
            if sym_owner(s) == Some(ui) {
                h.bytes(&s.name);
                h.u64(u64::from(s.info));
                h.u64(u64::from(s.other));
                h.u64(place(s.shndx as usize).map_or(u64::MAX, |x| x as u64));
                h.u64(s.value);
                h.u64(s.size);
            }
        }
        if let Some((eh, records)) = &p.eh {
            let data = section_bytes(&obj.sections[*eh]);
            let rels: Vec<&Relocation> = relocation_sections(&obj)
                .into_iter()
                .filter(|&(_, t)| t == *eh)
                .flat_map(|(r, _)| relocations(&obj.sections[r]).iter())
                .collect();
            let record = |h: &mut H128, rec: &eh_frame::Record| {
                let b = &data[rec.start..rec.end()];
                h.bytes(&b[..rec.id_at]);
                h.bytes(&b[rec.id_at + 4..]);
                for rel in &rels {
                    let at = rel.offset as usize;
                    if at >= rec.start && at < rec.end() {
                        h.u64((at - rec.start) as u64);
                        h.u64(u64::from(obj.reloc_type(rel)));
                        key(h, obj.reloc_symbol(rel));
                        h.u64(rel.addend as u64);
                    }
                }
            };
            for &f in &fdes {
                let rec = &records[f];
                record(&mut h, rec);
                if let Some(cie) = rec.cie.and_then(|c| records.iter().find(|r| r.start == c)) {
                    record(&mut h, cie);
                }
            }
        }
        u.stays = stays;
        u.bytes = bytes + 4 * (unit_sections.len() as u64 + 1);
        u.hash = h.finish();
    }
    p.owner = owner;
    p.units = units;
    p.obj = Some(obj);
    p
}

/// A symbol of a synthesized member: (local?, its index among those).
type Target = (bool, usize);

/// A copied section's relocations (or a group's signature), to renumber:
/// the section, its member, and each relocation with its target.
type FixUp = (usize, usize, Vec<(Relocation, usize, Target)>);

/// A synthesized member to make: its units (member, unit) and its anchor.
type Synthesis = (Vec<(usize, usize)>, Vec<u8>);

/// A rewritten member, and what rewriting it counted.
type Rewritten = Result<(Vec<u8>, Report), String>;

/// Builds the anchor's name for a synthesized member of these units.
fn anchor_name(hashes: &[u128], env: u128) -> Vec<u8> {
    let mut h = H128::new();
    h.u128(env);
    for &x in hashes {
        h.u128(x);
    }
    format!("__turbolib.anchor.{:032x}", h.finish()).into_bytes()
}

/// Writes `value` (`size` bytes, in `endian`) at `at` in `bytes`.
fn put(bytes: &mut [u8], at: usize, size: usize, value: u64, endian: Endian) {
    let Some(place) = bytes.get_mut(at..at + size) else {
        return;
    };
    match endian {
        Endian::Little => place.copy_from_slice(&value.to_le_bytes()[..size]),
        Endian::Big => place.copy_from_slice(&value.to_be_bytes()[8 - size..]),
    }
}

/// The member `parsed` with the units `moved` (its indexes) taken out.
fn rewrite(
    parsed: &Parsed<'_>,
    moved: &[usize],
    anchors: &[Vec<u8>],
    report: &mut Report,
) -> Result<Vec<u8>, String> {
    let obj0 = parsed.obj.as_ref().ok_or("unread")?;
    let mut obj = obj0.clone();
    let n = obj.sections.len();
    let endian = obj.endian();
    let machine = obj.header.machine;
    let mut removed = vec![false; n];
    let mut moved_unit = vec![false; parsed.units.len()];
    for &u in moved {
        moved_unit[u] = true;
        let unit = &parsed.units[u];
        removed[unit.group] = true;
        for &s in &unit.sections {
            removed[s] = true;
        }
    }
    let symbols: Vec<Symbol<'_>> = obj.symbols().to_vec();
    let in_removed =
        |s: &Symbol<'_>| s.in_section() && removed.get(s.shndx as usize).copied().unwrap_or(false);

    // .eh_frame without the moved units' FDEs.
    if let Some((eh, records)) = &parsed.eh {
        let gone: Vec<usize> = moved
            .iter()
            .flat_map(|&u| parsed.units[u].fdes.iter().copied())
            .collect();
        if !gone.is_empty() {
            let data = section_bytes(&obj.sections[*eh]).to_vec();
            let big = endian == Endian::Big;
            let kept: Vec<usize> = (0..records.len()).filter(|k| !gone.contains(k)).collect();
            let index_of = |start: usize| kept.iter().position(|&k| records[k].start == start);
            let pieces: Vec<(&[u8], Option<usize>, usize)> = kept
                .iter()
                .map(|&k| {
                    let r = &records[k];
                    (&data[r.start..r.end()], r.cie.and_then(index_of), r.id_at)
                })
                .collect();
            let (bytes, starts) = eh_frame::build(&pieces, big);
            // Bytes after the records (a terminator) stay at the end.
            let tail = records.last().map_or(0, eh_frame::Record::end);
            let mut bytes = bytes;
            bytes.extend_from_slice(&data[tail.min(data.len())..]);
            let shift = |off: u64| -> Option<u64> {
                let at = off as usize;
                kept.iter()
                    .zip(&starts)
                    .find(|&(&k, _)| at >= records[k].start && at < records[k].end())
                    .map(|(&k, &s)| (at - records[k].start + s) as u64)
            };
            for (r, t) in relocation_sections(&obj) {
                if t != *eh {
                    continue;
                }
                if let Data::Relocations(rs) = &mut obj.sections[r].data {
                    let mut out = Vec::with_capacity(rs.len());
                    for rel in rs.iter() {
                        if let Some(o) = shift(rel.offset) {
                            out.push(Relocation { offset: o, ..*rel });
                        }
                    }
                    *rs = out;
                }
            }
            obj.sections[*eh].data = Data::Bytes(Cow::Owned(bytes));
        }
    }
    // Debug relocations to the moved units' locals: tombstones.
    for (r, t) in relocation_sections(&obj) {
        let Some(target) = obj.sections.get(t) else {
            continue;
        };
        if !is_debug(&target.name) || target.flags & shf::ALLOC != 0 || removed[t] {
            continue;
        }
        let value = tombstone(&target.name);
        let rels = relocations(&obj.sections[r]).to_vec();
        let mut keep = Vec::with_capacity(rels.len());
        let mut bytes: Option<Vec<u8>> = None;
        for rel in rels {
            let si = obj.reloc_symbol(&rel) as usize;
            let hit = symbols
                .get(si)
                .is_some_and(|s| s.bind() == 0 && si != 0 && in_removed(s));
            if !hit {
                keep.push(rel);
                continue;
            }
            let size = debug_reloc_size(machine, obj.reloc_type(&rel))
                .ok_or("a debug relocation not tombstoned")?;
            let b = bytes.get_or_insert_with(|| section_bytes(&obj.sections[t]).to_vec());
            put(b, rel.offset as usize, size, value, endian);
            report.tombstoned += 1;
        }
        if let Some(b) = bytes {
            obj.sections[t].data = Data::Bytes(Cow::Owned(b));
            obj.sections[r].data = Data::Relocations(keep);
        }
    }
    // The symbols: moved definitions undefined, the moved locals gone.
    let mut keep_symbols = vec![true; symbols.len()];
    if let Some(table) = obj.symbols_mut() {
        for (i, s) in table.iter_mut().enumerate() {
            if i == 0 || !in_removed(s) {
                continue;
            }
            if s.bind() == 0 {
                keep_symbols[i] = false;
            } else {
                // A weak reference, of its type (STT_TLS must stay). Weak,
                // so that it loads nothing itself: the anchor loads the
                // synthesized member that defines it. (A strong one made
                // lld and mold take the name from the first library on the
                // command line with a copy of it, and load that member:
                // mold ranks a lazy definition by its file's place, and
                // lld meets the reference before the synthesized member,
                // at the archive's end, is known.)
                s.info = (2 << 4) | s.kind();
                s.value = 0;
                s.size = 0;
                s.set_special(SHN_UNDEF);
            }
        }
        for a in anchors {
            table.push(Symbol {
                name: Cow::Owned(a.clone()),
                name_offset: 0,
                value: 0,
                size: 0,
                // Strong: a weak reference loads nothing.
                info: 1 << 4,
                other: 2,
                shndx: SHN_UNDEF,
                xindex: false,
            });
            keep_symbols.push(true);
        }
    }
    let keep: Vec<bool> = removed.iter().map(|&r| !r).collect();
    obj.retain_sections(&keep);
    let kept = obj.retain_symbols(&keep_symbols);
    // The anchors first among the globals: lld and mold take an archive
    // member the moment a loaded object's undefined symbol meets a lazy
    // one, in symbol-table order, and the first library on the command line
    // with any definition of a name gives it. With the anchors first, the
    // synthesized members (which define the moved names) come in before
    // another library's copy of one of those names could be taken.
    let t = obj.symtab().ok_or("no symbol table")?;
    let count = obj.symbols().len();
    let first_global = (obj.sections[t].info as usize).min(count);
    let from = count - anchors.len();
    let order: Vec<usize> = (0..first_global)
        .chain(from..count)
        .chain(first_global..from)
        .collect();
    let mut moved_to = vec![None; count];
    for (new, &old) in order.iter().enumerate() {
        moved_to[old] = Some(new as u32);
    }
    if let Some(table) = obj.symbols_mut() {
        let old = std::mem::take(table);
        let mut slots: Vec<Option<Symbol<'_>>> = old.into_iter().map(Some).collect();
        *table = order.iter().filter_map(|&i| slots[i].take()).collect();
    }
    obj.renumber_symbols(t, &moved_to);
    let map: Vec<Option<u32>> = kept
        .iter()
        .map(|k| k.and_then(|k| moved_to.get(k as usize).copied().flatten()))
        .collect();
    // .llvm_addrsig: symbol indexes (ULEB128), renumbered.
    for s in &mut obj.sections {
        if s.kind == sht::LLVM_ADDRSIG
            && let Data::Bytes(b) = &s.data
        {
            let mut out = Vec::with_capacity(b.len());
            let mut at = 0;
            while at < b.len() {
                let (v, n) = uleb(&b[at..]).ok_or("a malformed .llvm_addrsig")?;
                at += n;
                if let Some(Some(new)) = map.get(v as usize) {
                    put_uleb(&mut out, u64::from(*new));
                }
            }
            s.data = Data::Bytes(Cow::Owned(out));
        }
    }
    obj.write().map_err(|e| e.to_string())
}

fn uleb(b: &[u8]) -> Option<(u64, usize)> {
    let mut v = 0u64;
    for (i, &x) in b.iter().enumerate().take(10) {
        v |= u64::from(x & 0x7F) << (7 * i);
        if x & 0x80 == 0 {
            return Some((v, i + 1));
        }
    }
    None
}

fn put_uleb(out: &mut Vec<u8>, mut v: u64) {
    loop {
        let b = (v & 0x7F) as u8;
        v >>= 7;
        if v == 0 {
            out.push(b);
            break;
        }
        out.push(b | 0x80);
    }
}

/// A synthesized member holding `units` (each its member and its index
/// there), with their FDEs and CIEs, in the first one's environment. It is
/// synthesized member `me`; `moved_into` gives each moved name's
/// synthesized member, and `anchors` each one's anchor.
fn synthesize<'a>(
    parsed: &[Parsed<'a>],
    units: &[(usize, usize)],
    me: usize,
    moved_into: &BTreeMap<&[u8], usize>,
    anchors: &[Vec<u8>],
) -> Result<Vec<u8>, String> {
    let anchor = &anchors[me];
    let mut deps: std::collections::BTreeSet<usize> = std::collections::BTreeSet::new();
    let (m0, _) = *units.first().ok_or("no units")?;
    let first = parsed[m0].obj.as_ref().ok_or("unread")?;
    let class = first.class();
    let endian = first.endian();
    let header: Header = first.header;
    let (symsize, relasize) = match class {
        Class::Elf32 => (16, 12),
        Class::Elf64 => (24, 24),
    };
    let mut sections: Vec<Section<'a>> = vec![Section {
        name: Cow::Borrowed(b""),
        name_offset: 0,
        kind: sht::NULL,
        flags: 0,
        addr: 0,
        link: 0,
        info: 0,
        addralign: 0,
        entsize: 0,
        nobits_size: 0,
        data: Data::NoBits,
    }];
    // Each (member, section) copied: its index here.
    let mut placed: BTreeMap<(usize, usize), usize> = BTreeMap::new();
    for &(m, u) in units {
        let obj = parsed[m].obj.as_ref().ok_or("unread")?;
        let unit = &parsed[m].units[u];
        let g = &obj.sections[unit.group];
        placed.insert((m, unit.group), sections.len());
        sections.push(g.clone());
        for &s in &unit.sections {
            placed.insert((m, s), sections.len());
            sections.push(obj.sections[s].clone());
        }
    }
    // SHF_LINK_ORDER links to the copies.
    for &(m, u) in units {
        let obj = parsed[m].obj.as_ref().ok_or("unread")?;
        for &s in &parsed[m].units[u].sections {
            let sec = &obj.sections[s];
            if sec.flags & shf::LINK_ORDER != 0 {
                let to = *placed
                    .get(&(m, sec.link as usize))
                    .ok_or("SHF_LINK_ORDER to a section not copied")?;
                sections[placed[&(m, s)]].link = to as u32;
            }
        }
    }
    // Environment sections.
    let env_names: [&[u8]; 7] = [
        b".note.GNU-stack",
        b".note.gnu.property",
        b".comment",
        b".ARM.attributes",
        b".riscv.attributes",
        b".MIPS.abiflags",
        b".gnu.attributes",
    ];
    for name in env_names {
        if let Some(s) = first.sections.iter().find(|s| &*s.name == name) {
            let mut s = s.clone();
            s.link = 0;
            sections.push(s);
        }
    }
    // .eh_frame: the CIEs the FDEs name, then the FDEs.
    let eh_template = first
        .sections
        .iter()
        .find(|s| &*s.name == b".eh_frame")
        .cloned();
    let mut eh_pieces: Vec<(usize, usize, bool)> = Vec::new(); // (member, record, is cie)
    for &(m, u) in units {
        if let Some((_, records)) = &parsed[m].eh {
            for &f in &parsed[m].units[u].fdes {
                if let Some(c) = records[f]
                    .cie
                    .and_then(|c| records.iter().position(|r| r.start == c))
                    && !eh_pieces.contains(&(m, c, true))
                {
                    eh_pieces.push((m, c, true));
                }
            }
        }
    }
    let cies = eh_pieces.len();
    for &(m, u) in units {
        for &f in &parsed[m].units[u].fdes {
            eh_pieces.push((m, f, false));
        }
    }
    let eh_index = if eh_pieces.is_empty() {
        None
    } else {
        let mut s = eh_template.ok_or("FDEs without .eh_frame")?;
        s.link = 0;
        s.info = 0;
        sections.push(s);
        Some(sections.len() - 1)
    };
    // Its relocations, as the first member's (`.rela.eh_frame` or
    // `.rel.eh_frame`).
    let eh_rel_template = parsed[m0].eh.as_ref().and_then(|(eh, _)| {
        relocation_sections(first)
            .into_iter()
            .find(|&(_, t)| t == *eh)
            .map(|(r, _)| first.sections[r].clone())
    });
    let rela_eh = match (eh_index, eh_rel_template) {
        (Some(eh), Some(mut s)) => {
            s.info = eh as u32;
            s.data = Data::Relocations(Vec::new());
            sections.push(s);
            Some(sections.len() - 1)
        }
        (Some(eh), None) => {
            sections.push(Section {
                name: Cow::Borrowed(b".rela.eh_frame"),
                name_offset: 0,
                kind: sht::RELA,
                flags: shf::INFO_LINK,
                addr: 0,
                link: 0,
                info: eh as u32,
                addralign: 8,
                entsize: relasize,
                nobits_size: 0,
                data: Data::Relocations(Vec::new()),
            });
            Some(sections.len() - 1)
        }
        _ => None,
    };
    let symtab = sections.len();
    sections.push(Section {
        name: Cow::Borrowed(b".symtab"),
        name_offset: 0,
        kind: sht::SYMTAB,
        flags: 0,
        addr: 0,
        link: (symtab + 1) as u32,
        info: 0,
        addralign: 8,
        entsize: symsize,
        nobits_size: 0,
        data: Data::Symbols(Vec::new()),
    });
    sections.push(Section {
        name: Cow::Borrowed(b".strtab"),
        name_offset: 0,
        kind: sht::STRTAB,
        flags: 0,
        addr: 0,
        link: 0,
        info: 0,
        addralign: 1,
        entsize: 0,
        nobits_size: 0,
        data: Data::Bytes(Cow::Borrowed(b"\0")),
    });
    sections.push(Section {
        name: Cow::Borrowed(b".shstrtab"),
        name_offset: 0,
        kind: sht::STRTAB,
        flags: 0,
        addr: 0,
        link: 0,
        info: 0,
        addralign: 1,
        entsize: 0,
        nobits_size: 0,
        data: Data::Bytes(Cow::Borrowed(b"\0")),
    });
    let shstrndx = sections.len() - 1;

    // Symbols: locals (sections' and the units'), then globals.
    let mut locals: Vec<Symbol<'a>> = vec![Symbol {
        name: Cow::Borrowed(b""),
        name_offset: 0,
        value: 0,
        size: 0,
        info: 0,
        other: 0,
        shndx: 0,
        xindex: false,
    }];
    let mut globals: Vec<Symbol<'a>> = Vec::new();
    // (member, old symbol) -> (local?, index in its list).
    let mut sym_map: BTreeMap<(usize, usize), (bool, usize)> = BTreeMap::new();
    let mut global_of: BTreeMap<Vec<u8>, usize> = BTreeMap::new();
    let mut section_symbol: BTreeMap<usize, usize> = BTreeMap::new();
    for &(m, u) in units {
        let obj = parsed[m].obj.as_ref().ok_or("unread")?;
        for (i, s) in obj.symbols().iter().enumerate().skip(1) {
            let Some(owner) = (if s.in_section() {
                parsed[m].owner.get(s.shndx as usize).copied().flatten()
            } else {
                None
            }) else {
                continue;
            };
            if owner != u {
                continue;
            }
            let new_sec = placed[&(m, s.shndx as usize)] as u32;
            let mut c = s.clone();
            c.set_section(new_sec);
            if s.bind() == 0 {
                if s.kind() == 3 {
                    let at = *section_symbol.entry(new_sec as usize).or_insert_with(|| {
                        locals.push(c.clone());
                        locals.len() - 1
                    });
                    sym_map.insert((m, i), (true, at));
                } else {
                    locals.push(c);
                    sym_map.insert((m, i), (true, locals.len() - 1));
                }
            } else {
                global_of.insert(s.name.to_vec(), globals.len());
                sym_map.insert((m, i), (false, globals.len()));
                globals.push(c);
            }
        }
    }
    // A section symbol for every copied section a relocation may name.
    let mut section_sym = |locals: &mut Vec<Symbol<'a>>, sec: usize| -> usize {
        *section_symbol.entry(sec).or_insert_with(|| {
            locals.push(Symbol {
                name: Cow::Borrowed(b""),
                name_offset: 0,
                value: 0,
                size: 0,
                info: 3,
                other: 0,
                shndx: sec as u32,
                xindex: sec as u32 >= turbonop_elf_object::SHN_LORESERVE,
            });
            locals.len() - 1
        })
    };
    // The symbol a relocation of member m names, here.
    let mut target = |locals: &mut Vec<Symbol<'a>>,
                      globals: &mut Vec<Symbol<'a>>,
                      m: usize,
                      si: usize|
     -> Result<(bool, usize), String> {
        if let Some(&t) = sym_map.get(&(m, si)) {
            return Ok(t);
        }
        let obj = parsed[m].obj.as_ref().ok_or("unread")?;
        let s = obj
            .symbols()
            .get(si)
            .ok_or("a relocation names no symbol")?;
        if si == 0 {
            return Ok((true, 0));
        }
        if s.bind() == 0 {
            // A section symbol of a copied section (a local symbol of a
            // unit is mapped already).
            if s.kind() == 3
                && let Some(&sec) = placed.get(&(m, s.shndx as usize))
            {
                return Ok((true, section_sym(locals, sec)));
            }
            return Err("a relocation names a local symbol not copied".into());
        }
        if let Some(&g) = global_of.get(&*s.name) {
            return Ok((false, g));
        }
        let mut u = s.clone();
        u.value = 0;
        u.size = 0;
        if let Some(&k) = moved_into.get(&*s.name)
            && k != me
        {
            // Moved into another synthesized member: a weak reference,
            // and that member's anchor (as a giver refers to it).
            u.info = (2 << 4) | s.kind();
            deps.insert(k);
        } else if s.shndx != SHN_UNDEF {
            // Defined in its member but not moved: a strong reference, so
            // that it loads that member (a weak one wouldn't).
            u.info = (1 << 4) | s.kind();
        }
        u.set_special(SHN_UNDEF);
        global_of.insert(s.name.to_vec(), globals.len());
        globals.push(u);
        Ok((false, globals.len() - 1))
    };
    // Relocations, groups' signatures, and .eh_frame.
    let mut fix_ups: Vec<FixUp> = Vec::new();
    for &(m, u) in units {
        let obj = parsed[m].obj.as_ref().ok_or("unread")?;
        let unit = &parsed[m].units[u];
        for &s in &unit.sections {
            let sec = &obj.sections[s];
            if let Data::Relocations(rs) = &sec.data {
                let mut out = Vec::with_capacity(rs.len());
                for r in rs {
                    let t = target(&mut locals, &mut globals, m, obj.reloc_symbol(r) as usize)?;
                    out.push((*r, m, t));
                }
                fix_ups.push((placed[&(m, s)], m, out));
            }
        }
        let g = &obj.sections[unit.group];
        let sig = target(&mut locals, &mut globals, m, g.info as usize)?;
        fix_ups.push((
            placed[&(m, unit.group)],
            m,
            vec![(
                Relocation {
                    offset: 0,
                    info: 0,
                    addend: 0,
                },
                m,
                sig,
            )],
        ));
    }
    let mut eh_relocs: Vec<(Relocation, (bool, usize))> = Vec::new();
    let eh_bytes;
    if let Some(eh) = eh_index {
        let big = endian == Endian::Big;
        let mut pieces: Vec<(Vec<u8>, Option<usize>, usize)> = Vec::new();
        let mut origin: Vec<(usize, usize)> = Vec::new();
        for (k, &(m, rec, is_cie)) in eh_pieces.iter().enumerate() {
            let records = &parsed[m].eh.as_ref().ok_or("no records")?.1;
            let obj = parsed[m].obj.as_ref().ok_or("unread")?;
            let (eh_sec, _) = parsed[m].eh.as_ref().ok_or("no records")?;
            let data = section_bytes(&obj.sections[*eh_sec]);
            let r = &records[rec];
            let cie = if is_cie {
                None
            } else {
                let c = r
                    .cie
                    .and_then(|c| records.iter().position(|x| x.start == c))
                    .ok_or("an FDE without its CIE")?;
                Some(
                    eh_pieces[..cies]
                        .iter()
                        .position(|&p| p == (m, c, true))
                        .ok_or("a CIE not copied")?,
                )
            };
            let _ = k;
            pieces.push((data[r.start..r.end()].to_vec(), cie, r.id_at));
            origin.push((m, rec));
        }
        let refs: Vec<(&[u8], Option<usize>, usize)> = pieces
            .iter()
            .map(|(b, c, i)| (b.as_slice(), *c, *i))
            .collect();
        let (bytes, starts) = eh_frame::build(&refs, big);
        eh_bytes = bytes;
        for (k, &(m, rec)) in origin.iter().enumerate() {
            let obj = parsed[m].obj.as_ref().ok_or("unread")?;
            let (eh_sec, records) = parsed[m].eh.as_ref().ok_or("no records")?;
            let r = &records[rec];
            for (rs, t) in relocation_sections(obj) {
                if t != *eh_sec {
                    continue;
                }
                for rel in relocations(&obj.sections[rs]) {
                    let at = rel.offset as usize;
                    if at >= r.start && at < r.end() {
                        let tgt =
                            target(&mut locals, &mut globals, m, obj.reloc_symbol(rel) as usize)?;
                        eh_relocs.push((
                            Relocation {
                                offset: (at - r.start + starts[k]) as u64,
                                ..*rel
                            },
                            tgt,
                        ));
                    }
                }
            }
        }
        sections[eh].data = Data::Bytes(Cow::Owned(eh_bytes));
    }
    // The anchor.
    globals.push(Symbol {
        name: Cow::Owned(anchor.clone()),
        name_offset: 0,
        value: 0,
        size: 0,
        info: (2 << 4),
        other: 2,
        shndx: SHN_ABS,
        xindex: false,
    });
    // The anchors of the synthesized members this one's units refer into,
    // first among the globals (as givers have theirs).
    let refs: Vec<Symbol<'a>> = deps
        .iter()
        .map(|&k| Symbol {
            name: Cow::Owned(anchors[k].clone()),
            name_offset: 0,
            value: 0,
            size: 0,
            info: 1 << 4,
            other: 2,
            shndx: SHN_UNDEF,
            xindex: false,
        })
        .collect();
    let nrefs = refs.len();
    let nlocal = locals.len();
    let index = |(local, i): (bool, usize)| -> u32 {
        if local {
            i as u32
        } else {
            (nlocal + nrefs + i) as u32
        }
    };
    let globals: Vec<Symbol<'a>> = refs.into_iter().chain(globals).collect();
    // The OS ABI: GNU with STB_GNU_UNIQUE or STT_GNU_IFUNC symbols (as GNU
    // as sets it), else what the first member had without them.
    let mut header = header;
    let gnu = locals
        .iter()
        .chain(&globals)
        .any(|s| s.bind() == 10 || s.kind() == 10);
    if gnu {
        header.ident[7] = 3;
    } else if header.ident[7] == 3 {
        header.ident[7] = 0;
    }
    let mut obj = Object::new(header, sections);
    obj.shstrndx = shstrndx as u32;
    // Relocations and signatures with the final indexes.
    for (sec, _m, rels) in fix_ups {
        let is_group = matches!(obj.sections[sec].data, Data::Group(_));
        if is_group {
            let sig = rels.first().map_or(0, |&(_, _, t)| index(t));
            obj.sections[sec].info = sig;
            obj.sections[sec].link = symtab as u32;
            if let Data::Group(Group { members, .. }) = &mut obj.sections[sec].data {
                // Members renumbered (each copied right after its group).
                let base = sec as u32;
                let n = members.len() as u32;
                *members = (1..=n).map(|k| base + k).collect();
            }
            continue;
        }
        let mut out = Vec::with_capacity(rels.len());
        for (r, _, t) in rels {
            let mut r = r;
            obj.set_reloc_symbol(&mut r, index(t));
            out.push(r);
        }
        obj.sections[sec].data = Data::Relocations(out);
        obj.sections[sec].link = symtab as u32;
    }
    // Relocation sections' targets.
    for &(m, u) in units {
        let parsed_obj = parsed[m].obj.as_ref().ok_or("unread")?;
        for &s in &parsed[m].units[u].sections {
            if parsed_obj.sections[s].is_relocations() {
                let t = parsed_obj.sections[s].info as usize;
                let new = *placed
                    .get(&(m, t))
                    .ok_or("a relocation section for a section not copied")?;
                obj.sections[placed[&(m, s)]].info = new as u32;
            }
        }
    }
    if let Some(r) = rela_eh {
        let mut out = Vec::with_capacity(eh_relocs.len());
        for (rel, t) in eh_relocs {
            let mut rel = rel;
            obj.set_reloc_symbol(&mut rel, index(t));
            out.push(rel);
        }
        obj.sections[r].data = Data::Relocations(out);
        obj.sections[r].link = symtab as u32;
    }
    let mut all = locals;
    all.extend(globals);
    obj.sections[symtab].info = nlocal as u32;
    obj.sections[symtab].data = Data::Symbols(all);
    obj.write().map_err(|e| e.to_string())
}

/// Folds the library `inputs`.
pub fn fold(inputs: &[Input<'_>], options: &Options) -> (Vec<Output>, Report) {
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
    if parsed.iter().any(|p| p.opaque) {
        *report
            .passed
            .entry("a member's definitions can't be read, so nothing folds".into())
            .or_default() += 1;
        return abandon(inputs, report);
    }
    // Every definition of each name, in member order.
    let mut definers: BTreeMap<&[u8], Vec<(usize, Option<usize>)>> = BTreeMap::new();
    for (m, p) in parsed.iter().enumerate() {
        for (name, u) in &p.defs {
            definers.entry(name.as_slice()).or_default().push((m, *u));
        }
    }
    // The copies of each unit, by hash, in member order.
    let mut copies: BTreeMap<u128, Vec<(usize, usize)>> = BTreeMap::new();
    let mut order: Vec<u128> = Vec::new();
    for (m, p) in parsed.iter().enumerate() {
        for (u, unit) in p.units.iter().enumerate() {
            let e = copies.entry(unit.hash).or_default();
            if e.is_empty() {
                order.push(unit.hash);
            }
            e.push((m, u));
        }
    }
    // Which fold.
    let mut folded: Vec<(u128, Vec<(usize, usize)>)> = Vec::new();
    for h in order {
        let list = &copies[&h];
        let mut members: Vec<usize> = list.iter().map(|&(m, _)| m).collect();
        members.dedup();
        if members.len() < 2 {
            continue;
        }
        report.duplicated += 1;
        let (m0, u0) = list[0];
        let unit = &parsed[m0].units[u0];
        let why = list
            .iter()
            .find_map(|&(m, u)| parsed[m].units[u].stays)
            .or_else(|| {
                unit.names.iter().find_map(|n| {
                    let defs = definers.get(n.as_slice())?;
                    if defs
                        .iter()
                        .any(|&(m, u)| u.is_none_or(|u| parsed[m].units[u].hash != h))
                    {
                        return Some("its names have other definitions in the library");
                    }
                    let first = defs.first()?.0;
                    (options.pull == Pull::Preserve && parsed[first].side_effects)
                        .then_some("the member the index sends its names to has side effects")
                })
            })
            .or_else(|| unit.names.is_empty().then_some("it defines no name"));
        if let Some(w) = why {
            *report.kept.entry(w).or_default() += list.len();
            continue;
        }
        folded.push((h, list.clone()));
    }
    // Synthesized members: by the members that held copies, and the
    // environment.
    let mut groups: BTreeMap<(Vec<usize>, u128), Vec<usize>> = BTreeMap::new();
    let mut group_order: Vec<(Vec<usize>, u128)> = Vec::new();
    for (k, (_, list)) in folded.iter().enumerate() {
        let mut members: Vec<usize> = list.iter().map(|&(m, _)| m).collect();
        members.dedup();
        let env = parsed[list[0].0].env;
        let key = (members, env);
        let e = groups.entry(key.clone()).or_default();
        if e.is_empty() {
            group_order.push(key);
        }
        e.push(k);
    }
    // Each synthesized member's units (the survivors) and anchor.
    let mut synth: Vec<Synthesis> = Vec::new();
    let mut moved: Vec<Vec<usize>> = vec![Vec::new(); parsed.len()];
    let mut member_anchors: Vec<Vec<Vec<u8>>> = vec![Vec::new(); parsed.len()];
    for key in &group_order {
        let ks = &groups[key];
        let survivors: Vec<(usize, usize)> = ks.iter().map(|&k| folded[k].1[0]).collect();
        let hashes: Vec<u128> = ks.iter().map(|&k| folded[k].0).collect();
        let anchor = anchor_name(&hashes, key.1);
        for &k in ks {
            for &(m, u) in &folded[k].1 {
                moved[m].push(u);
                report.removed_bytes += parsed[m].units[u].bytes;
                if parsed[m].units[u].debug {
                    report.debug_dropped += 1;
                }
            }
            report.folded += 1;
            report.copies += folded[k].1.len();
        }
        for &m in &key.0 {
            member_anchors[m].push(anchor.clone());
        }
        synth.push((survivors, anchor));
    }
    // Rewrite the members and make the synthesized ones, on every core.
    let rewritten: Vec<Option<Rewritten>> = turbonop_par::map_range(parsed.len(), 1, |m| {
        if moved[m].is_empty() {
            return None;
        }
        let mut r = Report::default();
        Some(rewrite(&parsed[m], &moved[m], &member_anchors[m], &mut r).map(|b| (b, r)))
    });
    // Each moved name's synthesized member.
    let mut moved_into: BTreeMap<&[u8], usize> = BTreeMap::new();
    for (k, (units, _)) in synth.iter().enumerate() {
        for &(m, u) in units {
            for n in &parsed[m].units[u].names {
                moved_into.insert(n.as_slice(), k);
            }
        }
    }
    let anchors: Vec<Vec<u8>> = synth.iter().map(|(_, a)| a.clone()).collect();
    let made: Vec<Result<Vec<u8>, String>> = turbonop_par::map_range(synth.len(), 1, |k| {
        synthesize(&parsed, &synth[k].0, k, &moved_into, &anchors)
    });
    let mut out = Vec::with_capacity(inputs.len() + made.len());
    for (m, r) in rewritten.into_iter().enumerate() {
        match r {
            None => out.push(Output {
                name: inputs[m].name.to_vec(),
                data: None,
                input: Some(m),
            }),
            Some(Ok((bytes, sub))) => {
                report.tombstoned += sub.tombstoned;
                out.push(Output {
                    name: inputs[m].name.to_vec(),
                    data: Some(bytes),
                    input: Some(m),
                });
            }
            Some(Err(e)) => {
                // Shouldn't happen: the member stays as it was, and so must
                // every unit it gave up (so the whole fold is abandoned).
                *report
                    .passed
                    .entry(format!("rewriting failed: {e}"))
                    .or_default() += 1;
                return abandon(inputs, report);
            }
        }
    }
    for (k, m) in made.into_iter().enumerate() {
        match m {
            Ok(bytes) => {
                report.synthesized += 1;
                report.synthesized_bytes += bytes.len() as u64;
                out.push(Output {
                    name: format!("__turbolib.{k}.o").into_bytes(),
                    data: Some(bytes),
                    input: None,
                });
            }
            Err(e) => {
                *report
                    .passed
                    .entry(format!("synthesizing failed: {e}"))
                    .or_default() += 1;
                return abandon(inputs, report);
            }
        }
    }
    (out, report)
}

/// The library as it was, when folding couldn't finish.
fn abandon(inputs: &[Input<'_>], mut report: Report) -> (Vec<Output>, Report) {
    report.folded = 0;
    report.copies = 0;
    report.synthesized = 0;
    report.removed_bytes = 0;
    report.synthesized_bytes = 0;
    let out = inputs
        .iter()
        .enumerate()
        .map(|(m, i)| Output {
            name: i.name.to_vec(),
            data: None,
            input: Some(m),
        })
        .collect();
    (out, report)
}

#[cfg(test)]
mod tests;
