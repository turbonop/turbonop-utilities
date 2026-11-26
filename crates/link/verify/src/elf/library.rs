//! `turbolib verify` for GNU and BSD archives of ELF objects: a folded
//! library against the original, from the two files alone.
//!
//! Each COMDAT group (`GRP_COMDAT`) of a member is a unit, reduced to a
//! 128-bit hash of a canonical form: the group's flags; each member
//! section (the group's list, then relocation sections for them) by name,
//! type, flags, alignment, entry size, and bytes; each relocation as
//! (offset, type, target, addend), a target outside the unit (or global)
//! by its name, one inside it by its section's place in the unit and its
//! value; the symbols it defines (name, binding and type, visibility,
//! place, value, size); and its FDEs in `.eh_frame` (those whose
//! `pc_begin` names one of its sections) with their CIEs, their bytes
//! without the CIE pointer and their relocations as above. This is
//! written apart from the engine (`turbonop_fold::elf`). A member's
//! environment (class, byte order, versions, machine, flags, and its
//! `.note.gnu.property`, `.note.GNU-stack`, `.comment`, and attribute
//! sections) is hashed apart, and units hold identically only in the same
//! environment.
//!
//! It checks:
//!
//! - **members**: every original member present, by name, in order;
//!   every added one synthesized (it defines a `__turbolib.anchor.` name:
//!   hidden, weak, absolute); unchanged members byte for byte.
//! - **each rewritten member against the original**: its environment;
//!   its sections outside groups (names, types, flags, in order) the same,
//!   with the same bytes and relocations, except `.eh_frame` (its records
//!   the original's but the lost units' FDEs, as a multiset of canonical
//!   records) and the `.debug_*` sections (the same bytes but where a
//!   relocation to a lost unit's local symbol was, which must be gone and
//!   its place hold the tombstone: 1 in `.debug_loc` and `.debug_ranges`,
//!   all ones in `.debug_names`, else 0); the units it keeps the
//!   original's; every unit it lost held, with the same hash and
//!   environment, by a synthesized member it anchors (a strong undefined
//!   reference to that member's anchor); every definition the original
//!   made outside lost units still made, of the same binding and type;
//!   every one in a lost unit now a weak undefined reference of the same
//!   type; its undefined names the original's, the lost ones, and anchors.
//! - **synthesized members**: each unit matches one a giver lost, in its
//!   environment; no unit in two synthesized members; no sections but
//!   their units', their environment's, `.eh_frame`, the symbol and string
//!   tables, and DWARF (whose contents `compare-images` checks, in the
//!   linked programs); every name they refer to defined or referred to by
//!   the original library.
//! - **the index** of each archive: the names each member gives an index
//!   (`turbonop_archive::unix::symbols`, as GNU `ar` computes them), each
//!   for its member.
//! - **`--pull=preserve`**: every name of the original's index still sent
//!   (to its first definer) to the same member, or to a synthesized member
//!   when the original's first definer has no side effects (`.init_array`,
//!   `.fini_array`, `.preinit_array`, `.ctors`, `.dtors` outside groups,
//!   `.gnu.warning`, sections named like C identifiers, `SHF_GNU_RETAIN`).
//!   `--pull=relaxed` lists those sent elsewhere as expected.

use std::collections::{BTreeMap, BTreeSet};

use turbonop_archive::Archive;
use turbonop_archive::unix::Archiver;
use turbonop_archive::unix::symbols::member_symbols;
use turbonop_elf_object::{Data, Endian, Object, Relocation, SHN_ABS, SHN_UNDEF, Symbol, shf, sht};

use super::tables::ANCHOR;
use crate::hash::H128;
use crate::library::{Options, Pull};
use crate::report::{Report, show};

const GRP_COMDAT: u32 = 1;

/// Whether `data` is a GNU or BSD archive whose first object is ELF.
pub fn is_elf_archive(data: &[u8]) -> bool {
    let Ok(a) = Archive::parse(data) else {
        return false;
    };
    a.files()
        .find(|m| m.data.len() >= 4)
        .is_some_and(|m| m.data.starts_with(b"\x7fELF"))
}

/// One unit of a member.
#[derive(Clone, Debug)]
struct Unit {
    group: usize,
    sections: Vec<usize>,
    hash: u128,
    names: Vec<Vec<u8>>,
    fdes: Vec<usize>,
}

/// One record of `.eh_frame`.
#[derive(Clone, Debug)]
struct Record {
    start: usize,
    end: usize,
    cie: Option<usize>,
    /// Its unit, for an FDE whose `pc_begin` names one.
    unit: Option<usize>,
    /// Its bytes (but the CIE pointer) and relocations, canonical.
    hash: u128,
}

/// One member, read.
struct Member<'a> {
    name: Vec<u8>,
    data: &'a [u8],
    header_offset: u64,
    obj: Option<Object<'a>>,
    /// Why it isn't read as an ELF relocatable object.
    unread: Option<String>,
    env: u128,
    side_effects: bool,
    units: Vec<Unit>,
    owner: Vec<Option<usize>>,
    records: Vec<Record>,
    /// Definitions: name, (binding, type), unit.
    defs: Vec<Definition>,
    /// Undefined names: name, (binding, type).
    undefs: BTreeMap<Vec<u8>, (u8, u8)>,
    /// The anchor it defines (a synthesized member).
    anchor: Option<Vec<u8>>,
    /// Each section's relocation sections.
    rels_for: BTreeMap<usize, Vec<usize>>,
    /// The symbol table's string table.
    strtab: Option<usize>,
}

/// A definition: its name, (binding, type), and unit.
type Definition = (Vec<u8>, (u8, u8), Option<usize>);

fn is_debug(name: &[u8]) -> bool {
    name.starts_with(b".debug_")
}

fn is_c_identifier(name: &[u8]) -> bool {
    !name.is_empty()
        && !name[0].is_ascii_digit()
        && name.iter().all(|&b| b.is_ascii_alphanumeric() || b == b'_')
}

const ENV_SECTIONS: [&[u8]; 7] = [
    b".note.gnu.property",
    b".note.GNU-stack",
    b".comment",
    b".ARM.attributes",
    b".riscv.attributes",
    b".MIPS.abiflags",
    b".gnu.attributes",
];

fn bytes<'o>(data: &'o Data<'_>) -> &'o [u8] {
    match data {
        Data::Bytes(b) => b,
        _ => &[],
    }
}

fn relocations<'o>(data: &'o Data<'_>) -> &'o [Relocation] {
    match data {
        Data::Relocations(r) => r,
        _ => &[],
    }
}

fn read_u32(b: &[u8], at: usize, big: bool) -> Option<u32> {
    let x: [u8; 4] = b.get(at..at + 4)?.try_into().ok()?;
    Some(if big {
        u32::from_be_bytes(x)
    } else {
        u32::from_le_bytes(x)
    })
}

/// A relocation's target, canonical: by name for globals and undefined
/// symbols, by place and value inside the unit `unit`, else by its
/// section's name and its name and value.
fn target_key(
    m: &Member<'_>,
    obj: &Object<'_>,
    symbols: &[Symbol<'_>],
    unit: Option<usize>,
    si: u32,
    h: &mut H128,
) {
    let Some(s) = symbols.get(si as usize) else {
        h.u64(u64::MAX);
        return;
    };
    if si == 0 {
        h.u64(0);
        return;
    }
    if s.bind() != 0 || s.shndx == SHN_UNDEF {
        h.u64(1);
        h.bytes(&s.name);
        return;
    }
    let owner = if s.in_section() {
        m.owner.get(s.shndx as usize).copied().flatten()
    } else {
        None
    };
    match (owner, unit) {
        (Some(o), Some(u)) if o == u => {
            let place = m.units[u]
                .sections
                .iter()
                .position(|&x| x == s.shndx as usize);
            h.u64(2);
            h.u64(place.map_or(u64::MAX, |p| p as u64));
            h.u64(s.value);
            h.u64(u64::from(s.kind()));
        }
        _ => {
            h.u64(3);
            let name = obj
                .sections
                .get(s.shndx as usize)
                .map_or(&b""[..], |x| &x.name);
            h.bytes(name);
            h.bytes(&s.name);
            h.u64(s.value);
        }
    }
}

fn environment(obj: &Object<'_>) -> u128 {
    let mut h = H128::new();
    h.bytes(&obj.header.ident[4..7]);
    h.u64(u64::from(obj.header.ident[8]));
    h.u64(u64::from(obj.header.machine));
    h.u64(u64::from(obj.header.flags));
    for name in ENV_SECTIONS {
        match obj.sections.iter().find(|s| &*s.name == name) {
            Some(s) => {
                h.u64(1);
                h.u64(s.flags);
                h.u64(u64::from(s.kind));
                h.bytes(bytes(&s.data));
            }
            None => h.u64(0),
        }
    }
    h.finish()
}

fn has_side_effects(name: &[u8], kind: u32, flags: u64) -> bool {
    let registration = matches!(kind, sht::INIT_ARRAY | sht::FINI_ARRAY | sht::PREINIT_ARRAY)
        || [
            &b".init_array"[..],
            b".fini_array",
            b".preinit_array",
            b".ctors",
            b".dtors",
            b".gnu.warning",
        ]
        .iter()
        .any(|p| name.starts_with(p));
    registration || is_c_identifier(name) || flags & shf::GNU_RETAIN != 0
}

fn read_member<'a>(name: &[u8], data: &'a [u8], header_offset: u64) -> Member<'a> {
    let mut m = Member {
        name: name.to_vec(),
        data,
        header_offset,
        obj: None,
        unread: None,
        env: 0,
        side_effects: false,
        units: Vec::new(),
        owner: Vec::new(),
        records: Vec::new(),
        defs: Vec::new(),
        undefs: BTreeMap::new(),
        anchor: None,
        rels_for: BTreeMap::new(),
        strtab: None,
    };
    if !data.starts_with(b"\x7fELF") {
        m.unread = Some("not ELF".into());
        return m;
    }
    let obj = match Object::parse(data) {
        Ok(o) => o,
        Err(e) => {
            m.unread = Some(format!("not read: {e}"));
            return m;
        }
    };
    if obj.header.kind != 1 {
        m.unread = Some("not a relocatable object".into());
        return m;
    }
    let n = obj.sections.len();
    let symtab = obj.symtab();
    m.strtab = symtab.map(|t| obj.sections[t].link as usize);
    for (i, s) in obj.sections.iter().enumerate() {
        if s.is_relocations() {
            m.rels_for.entry(s.info as usize).or_default().push(i);
        }
    }
    m.env = environment(&obj);
    let mut owner: Vec<Option<usize>> = vec![None; n];
    for (i, s) in obj.sections.iter().enumerate() {
        if let Data::Group(g) = &s.data
            && g.flags & GRP_COMDAT != 0
            && Some(s.link as usize) == symtab
        {
            let u = m.units.len();
            let mut sections = Vec::new();
            for &x in &g.members {
                let x = x as usize;
                if x < n && owner[x].is_none() {
                    owner[x] = Some(u);
                    sections.push(x);
                }
            }
            owner[i] = Some(u);
            m.units.push(Unit {
                group: i,
                sections,
                hash: 0,
                names: Vec::new(),
                fdes: Vec::new(),
            });
        }
    }
    for (i, s) in obj.sections.iter().enumerate() {
        if s.is_relocations()
            && owner[i].is_none()
            && let Some(&Some(u)) = owner.get(s.info as usize)
        {
            owner[i] = Some(u);
            m.units[u].sections.push(i);
        }
    }
    for (i, s) in obj.sections.iter().enumerate() {
        if owner[i].is_none() && has_side_effects(&s.name, s.kind, s.flags) {
            m.side_effects = true;
        }
    }
    m.owner = owner;
    // Symbols.
    for s in obj.symbols().iter().skip(1) {
        let class = (s.bind(), s.kind());
        if s.bind() == 0 {
            continue;
        }
        if s.shndx == SHN_UNDEF {
            m.undefs.insert(s.name.to_vec(), class);
            continue;
        }
        if s.name.starts_with(ANCHOR) && s.shndx == SHN_ABS && s.bind() == 2 && s.visibility() == 2
        {
            m.anchor = Some(s.name.to_vec());
        }
        let u = if s.in_section() {
            m.owner.get(s.shndx as usize).copied().flatten()
        } else {
            None
        };
        if let Some(u) = u {
            m.units[u].names.push(s.name.to_vec());
        }
        m.defs.push((s.name.to_vec(), class, u));
    }
    read_eh_frame(&mut m, &obj);
    // Each unit's symbols, in symbol-table order.
    let mut unit_symbols: Vec<Vec<usize>> = vec![Vec::new(); m.units.len()];
    for (k, s) in obj.symbols().iter().enumerate().skip(1) {
        if s.in_section()
            && let Some(u) = m.owner.get(s.shndx as usize).copied().flatten()
        {
            unit_symbols[u].push(k);
        }
    }
    // Units' hashes.
    let symbols = obj.symbols();
    for u in 0..m.units.len() {
        let mut h = H128::new();
        let unit = m.units[u].clone();
        if let Data::Group(g) = &obj.sections[unit.group].data {
            h.u64(u64::from(g.flags));
        }
        for &sec in &unit.sections {
            let s = &obj.sections[sec];
            h.bytes(&s.name);
            h.u64(u64::from(s.kind));
            h.u64(s.flags);
            h.u64(s.addralign);
            h.u64(s.entsize);
            match &s.data {
                Data::Bytes(b) => {
                    h.u64(1);
                    h.bytes(b);
                }
                Data::NoBits => {
                    h.u64(2);
                    h.u64(s.nobits_size);
                }
                Data::Relocations(rs) => {
                    h.u64(3);
                    h.u64(
                        unit.sections
                            .iter()
                            .position(|&x| x == s.info as usize)
                            .map_or(u64::MAX, |p| p as u64),
                    );
                    for r in rs {
                        h.u64(r.offset);
                        h.u64(u64::from(obj.reloc_type(r)));
                        target_key(&m, &obj, symbols, Some(u), obj.reloc_symbol(r), &mut h);
                        h.u64(r.addend as u64);
                    }
                }
                _ => h.u64(0xBAD),
            }
        }
        for &k in &unit_symbols[u] {
            let s = &symbols[k];
            h.bytes(&s.name);
            h.u64(u64::from(s.info));
            h.u64(u64::from(s.other));
            h.u64(
                unit.sections
                    .iter()
                    .position(|&x| x == s.shndx as usize)
                    .map_or(u64::MAX, |p| p as u64),
            );
            h.u64(s.value);
            h.u64(s.size);
        }
        for &f in &unit.fdes {
            h.u128(m.records[f].hash);
            match m.records[f].cie {
                Some(c) => h.u128(m.records[c].hash),
                None => h.u64(0),
            }
        }
        m.units[u].hash = h.finish();
    }
    m.obj = Some(obj);
    m
}

/// `.eh_frame`'s records, each with its unit and canonical hash.
fn read_eh_frame(m: &mut Member<'_>, obj: &Object<'_>) {
    let Some(eh) = obj.sections.iter().position(|s| &*s.name == b".eh_frame") else {
        return;
    };
    let big = obj.endian() == Endian::Big;
    let data = bytes(&obj.sections[eh].data);
    let mut rels: Vec<Relocation> = obj
        .sections
        .iter()
        .filter(|s| s.is_relocations() && s.info as usize == eh)
        .flat_map(|s| relocations(&s.data).iter().copied())
        .collect();
    rels.sort_by_key(|r| r.offset);
    let mut at = 0usize;
    let mut starts: BTreeMap<usize, usize> = BTreeMap::new();
    while let Some(len) = read_u32(data, at, big) {
        if len == 0 || len == u32::MAX {
            break;
        }
        let end = at + 4 + len as usize;
        let Some(id) = read_u32(data, at + 4, big) else {
            break;
        };
        if end > data.len() {
            break;
        }
        let cie = (id != 0).then(|| (at + 4).wrapping_sub(id as usize));
        starts.insert(at, m.records.len());
        m.records.push(Record {
            start: at,
            end,
            cie,
            unit: None,
            hash: 0,
        });
        at = end;
    }
    let symbols = obj.symbols();
    for k in 0..m.records.len() {
        let (start, end) = (m.records[k].start, m.records[k].end);
        let lo = rels.partition_point(|r| (r.offset as usize) < start);
        let hi = rels.partition_point(|r| (r.offset as usize) < end);
        let mut unit = None;
        if m.records[k].cie.is_some()
            && let Some(r) = rels[lo..hi].iter().find(|r| r.offset as usize == start + 8)
        {
            let s = symbols.get(obj.reloc_symbol(r) as usize);
            unit = s
                .filter(|s| s.bind() == 0 && s.in_section())
                .and_then(|s| m.owner.get(s.shndx as usize).copied().flatten());
        }
        let mut h = H128::new();
        h.bytes(&data[start..(start + 4).min(end)]);
        h.bytes(&data[(start + 8).min(end)..end]);
        for r in &rels[lo..hi] {
            h.u64(r.offset - start as u64);
            h.u64(u64::from(obj.reloc_type(r)));
            target_key(m, obj, symbols, unit, obj.reloc_symbol(r), &mut h);
            h.u64(r.addend as u64);
        }
        m.records[k].unit = unit;
        m.records[k].hash = h.finish();
        if let Some(u) = unit {
            m.units[u].fdes.push(k);
        }
    }
    // CIE pointers as indexes.
    for k in 0..m.records.len() {
        if let Some(c) = m.records[k].cie {
            m.records[k].cie = starts.get(&c).copied();
        }
    }
}

/// A library: its members, read, and its index.
struct Library<'a> {
    members: Vec<Member<'a>>,
    /// (name, member index), in the index's order.
    index: Vec<(Vec<u8>, usize)>,
    unindexed: usize,
}

fn read_library<'a>(data: &'a [u8], what: &str) -> Result<Library<'a>, String> {
    let a = Archive::parse(data).map_err(|e| format!("{what}: {e}"))?;
    if a.is_thin() {
        return Err(format!("{what}: thin archives aren't verified"));
    }
    let files: Vec<_> = a.files().collect();
    let members =
        turbonop_par::map_balanced(&files, 1, |f| read_member(&f.name, f.data, f.header_offset));
    let by_offset: BTreeMap<u64, usize> = members
        .iter()
        .enumerate()
        .map(|(i, m)| (m.header_offset, i))
        .collect();
    let mut index = Vec::new();
    let mut unindexed = 0;
    for s in a.symbols() {
        match by_offset.get(&s.member) {
            Some(&i) => index.push((s.name.to_vec(), i)),
            None => unindexed += 1,
        }
    }
    Ok(Library {
        members,
        index,
        unindexed,
    })
}

fn tombstone(section: &[u8]) -> u64 {
    match section {
        b".debug_loc" | b".debug_ranges" => 1,
        b".debug_names" => u64::MAX,
        _ => 0,
    }
}

/// The size of a debug section's absolute relocation.
fn debug_reloc_size(machine: u16, kind: u32) -> Option<usize> {
    match (machine, kind) {
        (62, 1 | 17) | (183, 257) => Some(8),
        (62, 10 | 11 | 21) | (183, 258) | (3, 1 | 32) => Some(4),
        (183, 259) => Some(2),
        _ => None,
    }
}

fn read_value(b: &[u8], at: usize, size: usize, big: bool) -> Option<u64> {
    let x = b.get(at..at + size)?;
    let mut v = 0u64;
    if big {
        for &c in x {
            v = (v << 8) | u64::from(c);
        }
    } else {
        for &c in x.iter().rev() {
            v = (v << 8) | u64::from(c);
        }
    }
    Some(v)
}

fn mask(size: usize) -> u64 {
    if size >= 8 {
        u64::MAX
    } else {
        (1u64 << (8 * size)) - 1
    }
}

/// Canonical relocations of a section outside units: (offset, hash of
/// type, target, addend), in offset order.
fn reloc_list(m: &Member<'_>, obj: &Object<'_>, target: usize) -> Vec<(u64, u128)> {
    let symbols = obj.symbols();
    let mut out: Vec<(u64, u128)> = m
        .rels_for
        .get(&target)
        .into_iter()
        .flatten()
        .flat_map(|&s| relocations(&obj.sections[s].data).iter())
        .map(|r| {
            let mut h = H128::new();
            h.u64(u64::from(obj.reloc_type(r)));
            target_key(m, obj, symbols, None, obj.reloc_symbol(r), &mut h);
            h.u64(r.addend as u64);
            (r.offset, h.finish())
        })
        .collect();
    out.sort_unstable();
    out
}

/// Whether section `i` is compared as one of the member's own (outside
/// units, not a table the rewrite renumbers).
fn own_section(m: &Member<'_>, obj: &Object<'_>, i: usize) -> bool {
    let s = &obj.sections[i];
    i != 0
        && m.owner[i].is_none()
        && !s.is_relocations()
        && !matches!(
            s.kind,
            sht::SYMTAB | sht::SYMTAB_SHNDX | sht::GROUP | sht::LLVM_ADDRSIG
        )
        && !(s.kind == sht::STRTAB && i as u32 == obj.shstrndx)
        && m.strtab != Some(i)
}

/// What comparing one rewritten member found.
struct Compared {
    report: Report,
    /// The units it lost: (hash, environment).
    lost: Vec<(u128, u128)>,
    changed: bool,
}

/// Compares rewritten member `n` with original `o`.
fn compare_member(o: &Member<'_>, n: &Member<'_>, rep: &mut Report) -> Vec<(u128, u128)> {
    let name = show(&o.name);
    let (Some(oo), Some(no)) = (&o.obj, &n.obj) else {
        rep.fail("rewritten members that don't read", || name.clone());
        return Vec::new();
    };
    if o.env != n.env {
        rep.fail("rewritten members of another environment", || name.clone());
    }
    // Units: kept and lost.
    let mut have: BTreeMap<u128, usize> = BTreeMap::new();
    for u in &n.units {
        *have.entry(u.hash).or_default() += 1;
    }
    let mut lost: Vec<usize> = Vec::new();
    for (k, u) in o.units.iter().enumerate() {
        match have.get_mut(&u.hash) {
            Some(c) if *c > 0 => *c -= 1,
            _ => lost.push(k),
        }
    }
    if have.values().any(|&c| c > 0) {
        rep.fail(
            "rewritten members with units the original didn't have",
            || name.clone(),
        );
    }
    rep.checked("units kept", (o.units.len() - lost.len()) as u64);
    let lost_set: BTreeSet<usize> = lost.iter().copied().collect();
    let lost_names: BTreeSet<&[u8]> = lost
        .iter()
        .flat_map(|&u| o.units[u].names.iter().map(Vec::as_slice))
        .collect();
    // Sections outside units.
    let os: Vec<usize> = (0..oo.sections.len())
        .filter(|&i| own_section(o, oo, i))
        .collect();
    let ns: Vec<usize> = (0..no.sections.len())
        .filter(|&i| own_section(n, no, i))
        .collect();
    let describe = |obj: &Object<'_>, i: usize| {
        let s = &obj.sections[i];
        (s.name.to_vec(), s.kind, s.flags, s.addralign, s.entsize)
    };
    let da: Vec<_> = os.iter().map(|&i| describe(oo, i)).collect();
    let db: Vec<_> = ns.iter().map(|&i| describe(no, i)).collect();
    rep.checked("sections outside units", os.len() as u64);
    if da != db {
        rep.fail(
            "rewritten members whose sections outside units differ",
            || name.clone(),
        );
        return Vec::new();
    }
    for (&i, &j) in os.iter().zip(&ns) {
        let (si, sj) = (&oo.sections[i], &no.sections[j]);
        let sname = show(&si.name);
        if &*si.name == b".eh_frame" {
            // The original's records but the lost units' FDEs.
            let mut want: BTreeMap<u128, i64> = BTreeMap::new();
            for r in &o.records {
                if r.unit.is_some_and(|u| lost_set.contains(&u)) {
                    continue;
                }
                *want.entry(r.hash).or_default() += 1;
            }
            for r in &n.records {
                *want.entry(r.hash).or_default() -= 1;
            }
            rep.checked(".eh_frame records", o.records.len() as u64);
            if want.values().any(|&c| c != 0) {
                rep.fail(
                    ".eh_frame sections whose records aren't the original's but the lost units' FDEs",
                    || name.clone(),
                );
            }
            continue;
        }
        if is_debug(&si.name) && si.flags & shf::ALLOC == 0 {
            compare_debug(o, oo, i, n, no, j, &lost_set, rep, &name);
            continue;
        }
        let same = match (&si.data, &sj.data) {
            (Data::NoBits, Data::NoBits) => si.nobits_size == sj.nobits_size,
            (a, b) => bytes(a) == bytes(b),
        };
        if !same {
            rep.fail("sections outside units whose bytes changed", || {
                format!("{name}: {sname}")
            });
        }
        if reloc_list(o, oo, i) != reloc_list(n, no, j) {
            rep.fail("sections outside units whose relocations changed", || {
                format!("{name}: {sname}")
            });
        }
    }
    // Definitions.
    let ndefs: BTreeMap<&[u8], (u8, u8)> =
        n.defs.iter().map(|(x, c, _)| (x.as_slice(), *c)).collect();
    for (d, class, u) in &o.defs {
        rep.checked("definitions", 1);
        if u.is_some_and(|u| lost_set.contains(&u)) {
            match n.undefs.get(d.as_slice()) {
                Some(&(bind, kind)) if bind == 2 && kind == class.1 => {}
                Some(_) => rep.fail(
                    "moved definitions that aren't weak references of their type",
                    || format!("{name}: {}", show(d)),
                ),
                None => rep.fail("moved definitions neither defined nor referred to", || {
                    format!("{name}: {}", show(d))
                }),
            }
        } else if ndefs.get(d.as_slice()) != Some(class) {
            rep.fail("definitions lost or changed", || {
                format!("{name}: {}", show(d))
            });
        }
    }
    for d in n.undefs.keys() {
        if !o.undefs.contains_key(d) && !lost_names.contains(d.as_slice()) && !d.starts_with(ANCHOR)
        {
            rep.fail(
                "rewritten members referring to names the original didn't",
                || format!("{name}: {}", show(d)),
            );
        }
    }
    lost.iter().map(|&u| (o.units[u].hash, o.env)).collect()
}

#[allow(clippy::too_many_arguments)]
fn compare_debug(
    o: &Member<'_>,
    oo: &Object<'_>,
    i: usize,
    n: &Member<'_>,
    no: &Object<'_>,
    j: usize,
    lost: &BTreeSet<usize>,
    rep: &mut Report,
    name: &str,
) {
    let big = oo.endian() == Endian::Big;
    let (si, sj) = (&oo.sections[i], &no.sections[j]);
    let (ba, bb) = (bytes(&si.data), bytes(&sj.data));
    let sname = show(&si.name);
    if ba.len() != bb.len() {
        rep.fail("debug sections of another size", || {
            format!("{name}: {sname}")
        });
        return;
    }
    let value = tombstone(&si.name);
    let symbols = oo.symbols();
    let mut masked = vec![false; ba.len()];
    let mut gone: BTreeSet<u64> = BTreeSet::new();
    for r in o
        .rels_for
        .get(&i)
        .into_iter()
        .flatten()
        .flat_map(|&s| relocations(&oo.sections[s].data).iter())
    {
        let k = oo.reloc_symbol(r) as usize;
        let to_lost = symbols.get(k).is_some_and(|s| {
            k != 0
                && s.bind() == 0
                && s.in_section()
                && o.owner
                    .get(s.shndx as usize)
                    .copied()
                    .flatten()
                    .is_some_and(|u| lost.contains(&u))
        });
        if !to_lost {
            continue;
        }
        let Some(size) = debug_reloc_size(oo.header.machine, oo.reloc_type(r)) else {
            rep.fail(
                "debug relocations to lost units that can't be tombstoned",
                || format!("{name}: {sname}"),
            );
            continue;
        };
        let at = r.offset as usize;
        rep.checked("debug relocations tombstoned", 1);
        if read_value(bb, at, size, big) != Some(value & mask(size)) {
            rep.fail(
                "debug places of lost units' addresses that don't hold the tombstone",
                || format!("{name}: {sname}+{at:#x}"),
            );
        }
        if let Some(m) = masked.get_mut(at..at + size) {
            m.fill(true);
        }
        gone.insert(r.offset);
    }
    if ba
        .iter()
        .zip(bb)
        .zip(&masked)
        .any(|((x, y), &m)| !m && x != y)
    {
        rep.fail(
            "debug sections whose bytes changed elsewhere than at tombstones",
            || format!("{name}: {sname}"),
        );
    }
    let want: Vec<(u64, u128)> = reloc_list(o, oo, i)
        .into_iter()
        .filter(|(off, _)| !gone.contains(off))
        .collect();
    if want != reloc_list(n, no, j) {
        rep.fail(
            "debug sections whose relocations changed elsewhere than at tombstones",
            || format!("{name}: {sname}"),
        );
    }
}

/// Checks a synthesized member's sections: its units', its environment's,
/// `.eh_frame`, the tables, and DWARF.
fn synthesized_sections(m: &Member<'_>, rep: &mut Report) {
    let Some(obj) = &m.obj else { return };
    let mut debug = 0u64;
    for (i, s) in obj.sections.iter().enumerate() {
        if i == 0 || m.owner[i].is_some() {
            continue;
        }
        let target = obj.sections.get(s.info as usize);
        if is_debug(&s.name) || (s.is_relocations() && target.is_some_and(|t| is_debug(&t.name))) {
            debug += 1;
            continue;
        }
        let ok = ENV_SECTIONS.contains(&&*s.name)
            || &*s.name == b".eh_frame"
            || (s.is_relocations() && target.is_some_and(|t| &*t.name == b".eh_frame"))
            || matches!(s.kind, sht::SYMTAB | sht::STRTAB | sht::SYMTAB_SHNDX);
        if !ok {
            rep.fail("synthesized members' sections outside their units", || {
                format!("{}: {}", show(&m.name), show(&s.name))
            });
        }
    }
    if debug > 0 {
        rep.unchecked(
            "synthesized members with DWARF (compare-images checks it, in links)",
            || show(&m.name),
        );
    }
}

/// Verifies a folded ELF library against the original (see the module's
/// documentation), on every core.
pub fn verify(a: &[u8], b: &[u8], options: &Options) -> Result<Report, String> {
    let (la, lb) = turbonop_par::join(
        || read_library(a, "the original library"),
        || read_library(b, "the rewritten library"),
    );
    let (la, lb) = (la?, lb?);
    let mut rep = Report::new();
    // Members: the originals in order, the rest synthesized.
    let mut place: Vec<Option<usize>> = vec![None; la.members.len()];
    let mut synth: Vec<usize> = Vec::new();
    let mut next = 0;
    for (j, m) in lb.members.iter().enumerate() {
        if next < la.members.len() && la.members[next].name == m.name {
            place[next] = Some(j);
            next += 1;
        } else if m.anchor.is_some() {
            synth.push(j);
        } else {
            rep.fail("added members that aren't synthesized", || show(&m.name));
        }
    }
    rep.checked("original members", la.members.len() as u64);
    rep.checked("synthesized members", synth.len() as u64);
    for (k, p) in place.iter().enumerate() {
        if p.is_none() {
            rep.fail("original members missing (or out of order)", || {
                show(&la.members[k].name)
            });
        }
    }
    for (k, m) in la.members.iter().enumerate() {
        if let Some(why) = &m.unread {
            if place[k].is_some_and(|j| lb.members[j].data != m.data) {
                rep.fail("members not read as ELF objects that changed", || {
                    show(&m.name)
                });
            } else {
                rep.unchecked(&format!("members passed through ({why})"), || show(&m.name));
            }
        }
    }
    // Rewritten members.
    let pairs: Vec<(usize, usize)> = place
        .iter()
        .enumerate()
        .filter_map(|(k, p)| p.map(|j| (k, j)))
        .collect();
    let results: Vec<Compared> = turbonop_par::map_balanced(&pairs, 4, |&(k, j)| {
        let (o, n) = (&la.members[k], &lb.members[j]);
        let mut report = Report::new();
        if o.data == n.data || o.unread.is_some() {
            return Compared {
                report,
                lost: Vec::new(),
                changed: false,
            };
        }
        let lost = compare_member(o, n, &mut report);
        Compared {
            report,
            lost,
            changed: true,
        }
    });
    let anchors: BTreeMap<&[u8], usize> = synth
        .iter()
        .filter_map(|&j| lb.members[j].anchor.as_deref().map(|a| (a, j)))
        .collect();
    // Each synthesized member's units, by (hash, environment).
    let mut held: BTreeMap<(u128, u128), Vec<usize>> = BTreeMap::new();
    for &j in &synth {
        let m = &lb.members[j];
        for u in &m.units {
            held.entry((u.hash, m.env)).or_default().push(j);
        }
        synthesized_sections(m, &mut rep);
    }
    for v in held.values() {
        if v.len() > 1 {
            rep.fail("units in two synthesized members", || {
                show(&lb.members[v[0]].name)
            });
        }
    }
    let mut given: BTreeSet<(u128, u128)> = BTreeSet::new();
    let mut rewritten = 0u64;
    for (c, &(k, j)) in results.into_iter().zip(&pairs) {
        rep.merge(c.report);
        if !c.changed {
            continue;
        }
        rewritten += 1;
        let n = &lb.members[j];
        let mine: BTreeSet<usize> = n
            .undefs
            .iter()
            .filter(|(x, cl)| x.starts_with(ANCHOR) && cl.0 == 1)
            .filter_map(|(x, _)| anchors.get(x.as_slice()).copied())
            .collect();
        if n.undefs
            .keys()
            .any(|x| x.starts_with(ANCHOR) && !anchors.contains_key(x.as_slice()))
        {
            rep.fail("anchors no synthesized member defines", || show(&n.name));
        }
        for key in c.lost {
            rep.checked("units moved", 1);
            given.insert(key);
            let ok = held
                .get(&key)
                .is_some_and(|v| v.iter().any(|j| mine.contains(j)));
            if !ok {
                rep.fail("moved units no synthesized member it anchors holds", || {
                    show(&la.members[k].name)
                });
            }
        }
    }
    rep.checked("rewritten members", rewritten);
    for (key, v) in &held {
        if !given.contains(key) {
            rep.fail("synthesized members' units that no member gave up", || {
                show(&lb.members[v[0]].name)
            });
        }
    }
    // Names synthesized members refer to.
    let known: BTreeSet<&[u8]> = la
        .members
        .iter()
        .flat_map(|m| {
            m.defs
                .iter()
                .map(|d| d.0.as_slice())
                .chain(m.undefs.keys().map(Vec::as_slice))
        })
        .collect();
    for &j in &synth {
        for x in lb.members[j].undefs.keys() {
            if !x.starts_with(ANCHOR) && !known.contains(x.as_slice()) {
                rep.fail(
                    "synthesized members referring to names the original doesn't know",
                    || format!("{}: {}", show(&lb.members[j].name), show(x)),
                );
            }
        }
    }
    index(&la, "the original's", &mut rep);
    index(&lb, "the rewritten library's", &mut rep);
    pull(&la, &lb, &place, options, &mut rep);
    Ok(rep)
}

/// Checks an archive's index against its members.
fn index(l: &Library<'_>, what: &str, rep: &mut Report) {
    if l.unindexed > 0 {
        rep.fail(&format!("{what} index entries naming no member"), || {
            l.unindexed.to_string()
        });
    }
    let mut want: Vec<(Vec<u8>, usize)> = Vec::new();
    for (i, m) in l.members.iter().enumerate() {
        match member_symbols(m.data, Archiver::GnuAr) {
            Ok(s) => want.extend(s.symbols.iter().map(|x| (x.to_vec(), i))),
            Err(e) => rep.unchecked(
                &format!("{what} members whose index entries aren't modeled"),
                || format!("{}: {e}", show(&m.name)),
            ),
        }
    }
    let mut have = l.index.clone();
    rep.checked(&format!("{what} index entries"), have.len() as u64);
    want.sort();
    have.sort();
    if want != have {
        let cat = format!("{what} index entries that differ from its members' definitions");
        for x in want.iter().filter(|x| have.binary_search(x).is_err()) {
            rep.fail(&cat, || {
                format!("missing: {} of {}", show(&x.0), show(&l.members[x.1].name))
            });
        }
        for x in have.iter().filter(|x| want.binary_search(x).is_err()) {
            rep.fail(&cat, || {
                format!("extra: {} of {}", show(&x.0), show(&l.members[x.1].name))
            });
        }
    }
}

/// `--pull`: where the index sends each of the original's names.
fn pull(
    la: &Library<'_>,
    lb: &Library<'_>,
    place: &[Option<usize>],
    options: &Options,
    rep: &mut Report,
) {
    let first = |l: &Library<'_>| -> BTreeMap<Vec<u8>, usize> {
        let mut m = BTreeMap::new();
        for (x, i) in &l.index {
            m.entry(x.clone()).or_insert(*i);
        }
        m
    };
    let (fa, fb) = (first(la), first(lb));
    rep.checked("names of the original's index", fa.len() as u64);
    for (x, &i) in &fa {
        let Some(&j) = fb.get(x) else {
            rep.fail("names the rewritten index lost", || show(x));
            continue;
        };
        if place[i] == Some(j) {
            continue;
        }
        let to_synth = lb.members[j].anchor.is_some();
        if to_synth && !la.members[i].side_effects {
            continue;
        }
        match options.pull {
            Pull::Preserve if to_synth => rep.fail(
                "names sent from a definer with side effects to a synthesized member",
                || format!("{}: {}", show(x), show(&la.members[i].name)),
            ),
            Pull::Preserve => rep.fail("names the index sends to another original member", || {
                show(x)
            }),
            Pull::Relaxed => rep.expect("names the index sends elsewhere (--pull=relaxed)", || {
                show(x)
            }),
        }
    }
}

#[cfg(test)]
mod tests;
