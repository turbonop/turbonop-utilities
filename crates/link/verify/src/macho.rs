//! Mach-O: `turbolib verify` for Darwin (and BSD) archives of Mach-O
//! objects ([`verify`]), written apart from the folding engine
//! (`turbonop_fold::macho`).
//!
//! Each member is read into atoms (each section of an object marked
//! `MH_SUBSECTIONS_VIA_SYMBOLS` split at its symbols that aren't
//! `N_ALT_ENTRY`), and each atom hashed by a canonical form that doesn't
//! depend on where the atom is: its section's names, flags, and
//! alignment; its size and bytes, with the places of local relocations
//! zeroed; its relocations by offset in the atom, with targets by name
//! (an external symbol), or by the target atom's key and the offset in it
//! (a local symbol, or the address a local relocation encodes); its
//! symbols (names, types, descriptions, offsets); and its
//! `__compact_unwind` entries (length, encoding, personality by name, LSDA
//! by atom). An atom's key is its first symbol's name, or its section and
//! its place among the section's unnamed atoms.
//!
//! Checked: every original member present, in order, and every added one
//! synthesized (it defines an anchor, a private external absolute
//! `~turbolib.anchor.` symbol); in each rewritten member, the header and
//! load commands but the symbol table's and segments' sizes unchanged,
//! every atom of the original present with the same canonical form, or
//! lost, and then a weak-definition unit held identically by a
//! synthesized member the member anchors (refers to its anchor); every
//! external definition kept, or (in a lost unit) a weak undefined
//! reference; no new undefined names but the lost units' names and
//! anchors; the debug sections the original's bytes but at the places of
//! relocations into lost atoms, which are gone and zero; in synthesized
//! members, only units, none held by two; each archive's index against its
//! members (`llvm-ar`'s rules); and, under `--pull=preserve`, each name of
//! the original's index sent to the same member, or to a synthesized one
//! from a first definer without side effects.
//!
//! What it can't prove: it relies on 128-bit hashes; it doesn't follow
//! references (what a definer without side effects loads); and it doesn't
//! read the DWARF of synthesized members (they have none yet).

use std::borrow::Cow;
use std::collections::{BTreeMap, BTreeSet};

use turbonop_archive::Archive;
use turbonop_archive::unix::Archiver;
use turbonop_archive::unix::symbols::member_symbols;
use turbonop_macho_object::consts::{
    LC_LINKER_OPTION, N_ABS, N_EXT, N_PEXT, N_TYPE, N_UNDF, S_INIT_FUNC_OFFSETS,
    S_MOD_INIT_FUNC_POINTERS, S_MOD_TERM_FUNC_POINTERS, S_THREAD_LOCAL_INIT_FUNCTION_POINTERS,
};
use turbonop_macho_object::{Command, Object, Section, reloc};

use crate::hash::H128;

/// A canonical form's hash, and (with `TURBOLIB_VERIFY_DUMP` set) its
/// parts as text, to show where two atoms differ.
struct Canon {
    h: H128,
    log: Option<Vec<String>>,
}

impl Canon {
    fn new() -> Self {
        Self {
            h: H128::new(),
            log: std::env::var_os("TURBOLIB_VERIFY_DUMP").map(|_| Vec::new()),
        }
    }
    fn u64(&mut self, v: u64) {
        self.h.u64(v);
        if let Some(l) = &mut self.log {
            l.push(format!("{v:#x}"));
        }
    }
    fn u128(&mut self, v: u128) {
        self.h.u128(v);
        if let Some(l) = &mut self.log {
            l.push(format!("{v:#x}"));
        }
    }
    fn bytes(&mut self, b: &[u8]) {
        self.h.bytes(b);
        if let Some(l) = &mut self.log {
            l.push(show(b));
        }
    }
    fn finish(&self) -> u128 {
        self.h.finish()
    }
}
use crate::library::{Options, Pull};
use crate::report::{Report, show};

const N_WEAK_REF: u16 = 0x40;
const N_WEAK_DEF: u16 = 0x80;
const N_ALT_ENTRY: u16 = 0x200;
const ANCHOR: &[u8] = b"~turbolib.anchor.";

/// Whether `data` is an archive whose first object is Mach-O.
pub fn is_macho_archive(data: &[u8]) -> bool {
    let Ok(a) = Archive::parse(data) else {
        return false;
    };
    a.files()
        .find(|m| m.data.len() >= 4)
        .is_some_and(|m| turbonop_macho::is_macho(m.data))
}

/// An atom, read.
#[derive(Clone, Debug)]
struct Atom {
    key: Vec<u8>,
    hash: u128,
    /// Whether it is a weak-definition unit (its external symbols all weak
    /// definitions, at least one).
    unit: bool,
    names: Vec<Vec<u8>>,
    /// Its canonical form's parts (with `TURBOLIB_VERIFY_DUMP`).
    desc: Vec<String>,
}

/// A debug section: its name, its bytes with relocations' places zeroed,
/// and its relocations (place, canonical form, the key of the atom reached).
type Debug = (Vec<u8>, Vec<u8>, Vec<(u32, u128, Option<Vec<u8>>)>);

/// An atom a rewritten member lost: its key and hash.
type Lost = (Vec<u8>, u128);

/// A member, read.
struct Member<'a> {
    name: Vec<u8>,
    data: &'a [u8],
    header_offset: u64,
    obj: Option<Object<'a>>,
    atoms: Vec<Atom>,
    /// Debug sections: name, bytes with relocations' places zeroed, and
    /// the relocations (place, canonical form, the key of the atom reached).
    debug: Vec<Debug>,
    anchor: Option<Vec<u8>>,
    side_effects: bool,
}

struct Library<'a> {
    members: Vec<Member<'a>>,
    index: Vec<(Vec<u8>, usize)>,
}

/// The atoms of `o`: for each section, (offset range, symbol indexes).
fn split(o: &Object<'_>) -> Vec<Vec<(core::ops::Range<u64>, Vec<usize>)>> {
    let mut out = Vec::new();
    for (si, s) in o.sections().enumerate() {
        let mut syms: Vec<(u64, usize)> = o
            .symbols
            .iter()
            .enumerate()
            .filter(|(_, y)| {
                y.in_section()
                    && usize::from(y.n_sect) == si + 1
                    && !(y.n_type & N_EXT == 0
                        && y.name.starts_with(b"ltmp")
                        && y.name[4..].iter().all(u8::is_ascii_digit))
            })
            .map(|(i, y)| (y.n_value.wrapping_sub(s.addr), i))
            .collect();
        syms.sort_unstable();
        let mut starts: BTreeSet<u64> = syms
            .iter()
            .filter(|&&(_, i)| o.symbols[i].n_desc & N_ALT_ENTRY == 0)
            .map(|&(a, _)| a)
            .collect();
        starts.insert(0);
        let starts: Vec<u64> = starts
            .into_iter()
            .filter(|&a| a < s.size().max(1))
            .collect();
        let mut atoms: Vec<(core::ops::Range<u64>, Vec<usize>)> = starts
            .iter()
            .enumerate()
            .map(|(k, &a)| {
                (
                    a..starts.get(k + 1).copied().unwrap_or(s.size()),
                    Vec::new(),
                )
            })
            .collect();
        for (a, i) in syms {
            let k = atoms
                .partition_point(|(r, _)| r.start <= a)
                .saturating_sub(1);
            if let Some(x) = atoms.get_mut(k) {
                x.1.push(i);
            }
        }
        out.push(atoms);
    }
    out
}

fn side_effects(o: &Object<'_>) -> bool {
    o.commands.iter().any(|c| c.cmd() == LC_LINKER_OPTION)
        || o.sections().any(|s| {
            matches!(
                s.kind(),
                S_MOD_INIT_FUNC_POINTERS
                    | S_MOD_TERM_FUNC_POINTERS
                    | S_INIT_FUNC_OFFSETS
                    | S_THREAD_LOCAL_INIT_FUNCTION_POINTERS
            ) || (s.name().starts_with(b"__objc_") && s.name() != b"__objc_imageinfo")
        })
}

fn read_member<'a>(name: &[u8], data: &'a [u8], header_offset: u64) -> Member<'a> {
    let mut m = Member {
        name: name.to_vec(),
        data,
        header_offset,
        obj: None,
        atoms: Vec::new(),
        debug: Vec::new(),
        anchor: None,
        side_effects: false,
    };
    let Ok(o) = Object::parse(data) else {
        return m;
    };
    m.side_effects = side_effects(&o);
    m.anchor = o
        .symbols
        .iter()
        .find(|y| y.name.starts_with(ANCHOR) && y.n_type & N_TYPE == N_ABS && y.n_type & N_EXT != 0)
        .map(|y| y.name.to_vec());
    let cputype = o.header.cputype;
    let endian = o.endian();
    let atoms = split(&o);
    let sections: Vec<&Section<'_>> = o.sections().collect();
    // Each atom's key.
    let mut keys: Vec<Vec<Vec<u8>>> = Vec::new();
    for (si, list) in atoms.iter().enumerate() {
        let mut unnamed = 0;
        keys.push(
            list.iter()
                .map(|(_, syms)| match syms.first() {
                    Some(&i) => o.symbols[i].name.to_vec(),
                    None => {
                        unnamed += 1;
                        [
                            sections[si].segment_name(),
                            b",",
                            sections[si].name(),
                            b"#",
                            unnamed.to_string().as_bytes(),
                        ]
                        .concat()
                    }
                })
                .collect(),
        );
    }
    // (An address at a section's end, as a range list's end has, reaches no
    // atom: the last atom may be cut while the address stays the end.)
    let atom_at = |si: usize, off: u64| -> Option<usize> {
        let l = atoms.get(si)?;
        let k = l.partition_point(|(r, _)| r.start <= off).checked_sub(1)?;
        (off < l[k].0.end).then_some(k)
    };
    // An address's atom: in section `hint` (a symbol's, or the section a
    // local relocation names) if known, else the section holding it (one
    // that ends there, for a label at a section's end).
    let locate = |addr: u64, hint: Option<usize>| -> Option<(usize, usize, u64)> {
        let si = match hint {
            Some(h) if h < sections.len() => h,
            _ => sections
                .iter()
                .position(|s| addr >= s.addr && addr - s.addr < s.size())
                .or_else(|| {
                    sections
                        .iter()
                        .position(|s| addr >= s.addr && addr - s.addr == s.size())
                })?,
        };
        if addr < sections[si].addr {
            return None;
        }
        let off = addr - sections[si].addr;
        let k = atom_at(si, off)?;
        Some((si, k, off - atoms[si][k].0.start))
    };
    // A relocation's target, canonical.
    let target = |s: &Section<'_>, ri: usize, h: &mut Canon| {
        let r = s.relocations[ri];
        if r.value.is_some() {
            h.u64(9);
            h.u64(u64::from(r.value.unwrap_or(0)));
            return;
        }
        if r.external {
            match o.symbols.get(r.symbolnum as usize) {
                Some(y) if y.n_type & N_EXT != 0 || !y.in_section() => h.bytes(&y.name),
                Some(y) => match locate(y.n_value, usize::from(y.n_sect).checked_sub(1)) {
                    Some((si, k, off)) => {
                        h.bytes(&keys[si][k]);
                        h.u64(off);
                    }
                    None => h.u64(8),
                },
                None => h.u64(7),
            }
        } else if reloc::names_section(cputype, &r) {
            match reloc::local_address(cputype, endian, s, ri)
                .and_then(|a| locate(a, (r.symbolnum as usize).checked_sub(1)))
            {
                Some((si, k, off)) => {
                    h.bytes(&keys[si][k]);
                    h.u64(off);
                }
                None => {
                    h.u64(6);
                    h.u64(u64::from(r.symbolnum));
                }
            }
        } else {
            h.u64(u64::from(r.symbolnum));
        }
    };
    let unwind = sections
        .iter()
        .position(|s| s.segment_name() == b"__LD" && s.name() == b"__compact_unwind");
    let eh = sections
        .iter()
        .position(|s| s.segment_name() == b"__TEXT" && s.name() == b"__eh_frame");
    let lsda = sections
        .iter()
        .position(|s| s.segment_name() == b"__TEXT" && s.name() == b"__gcc_except_tab");
    // Each function's length by its unwind entries: (atom) -> (offset, length).
    let mut lengths: BTreeMap<(usize, usize), Vec<(u64, u64)>> = BTreeMap::new();
    if let Some(ui) = unwind {
        let s = sections[ui];
        let b = s.bytes().unwrap_or(&[]);
        for e in (0..b.len() / 32).map(|i| i * 32) {
            let start = u64::from_le_bytes(b[e..e + 8].try_into().unwrap_or([0; 8]));
            let len = u64::from(u32::from_le_bytes(
                b[e + 8..e + 12].try_into().unwrap_or([0; 4]),
            ));
            if let Some((si, k, off)) = locate(start, None) {
                lengths.entry((si, k)).or_default().push((off, len));
            }
        }
    }
    // Each atom's own canonical form: its section, bytes, relocations, and
    // symbols (unwind information is added below, by the atom it is for).
    // A function's padding (after its end by its one unwind entry, up to
    // the next atom: the next function's alignment), with nothing in it,
    // isn't part of it.
    let mut own: BTreeMap<(usize, usize), (Canon, bool, Vec<Vec<u8>>)> = BTreeMap::new();
    for (si, list) in atoms.iter().enumerate() {
        let s = sections[si];
        if s.segment_name() == b"__DWARF" || Some(si) == unwind || Some(si) == eh {
            continue;
        }
        // Relocations by place, once.
        let mut by_place: Vec<usize> = (0..s.relocations.len()).collect();
        by_place.sort_by_key(|&i| (s.relocations[i].address, i));
        for (k, (range, syms)) in list.iter().enumerate() {
            let mut h = Canon::new();
            h.bytes(s.segment_name());
            h.bytes(s.name());
            h.u64(u64::from(s.flags));
            h.u64(u64::from(s.align));
            let lo =
                by_place.partition_point(|&i| u64::from(s.relocations[i].address) < range.start);
            let hi = by_place.partition_point(|&i| u64::from(s.relocations[i].address) < range.end);
            let rels = &by_place[lo..hi];
            let code = s.flags & 0x8000_0400 != 0;
            let end = match lengths.get(&(si, k)).map(Vec::as_slice) {
                Some(&[(0, len)])
                    if code
                        && len > 0
                        && range.start + len < range.end
                        && rels
                            .iter()
                            .all(|&i| u64::from(s.relocations[i].address) < range.start + len)
                        && syms.iter().all(|&i| {
                            o.symbols[i].n_value.wrapping_sub(s.addr) < range.start + len
                        }) =>
                {
                    range.start + len
                }
                _ => range.end,
            };
            h.u64(end - range.start);
            if let Some(b) = s.bytes() {
                let mut bytes = b[range.start as usize..end as usize].to_vec();
                for &i in rels {
                    let r = s.relocations[i];
                    if !r.external {
                        let at = (u64::from(r.address) - range.start) as usize;
                        let n = 1usize << r.length.min(3);
                        if let Some(x) = bytes.get_mut(at..at + n) {
                            x.fill(0);
                        }
                    }
                }
                h.bytes(&bytes);
            }
            for &i in rels {
                let r = s.relocations[i];
                h.u64(u64::from(r.address) - range.start);
                h.u64(u64::from(r.kind) | u64::from(r.length) << 8 | u64::from(r.pcrel) << 16);
                target(s, i, &mut h);
            }
            let mut ext = 0;
            let mut weak = 0;
            let mut names = Vec::new();
            for &i in syms {
                let y = &o.symbols[i];
                h.bytes(&y.name);
                h.u64(u64::from(y.n_type) | u64::from(y.n_desc) << 8);
                h.u64(y.n_value.wrapping_sub(s.addr) - range.start);
                if y.n_type & N_EXT != 0 {
                    ext += 1;
                    names.push(y.name.to_vec());
                    if y.n_desc & N_WEAK_DEF != 0 {
                        weak += 1;
                    }
                }
            }
            own.insert((si, k), (h, ext > 0 && weak == ext, names));
        }
    }
    // LSDAs (atoms of `__gcc_except_tab`) are part of the atoms whose unwind
    // information names them: by their own canonical form there.
    let own_hash: BTreeMap<(usize, usize), u128> =
        own.iter().map(|(&k, (h, _, _))| (k, h.finish())).collect();
    let mut attached: BTreeSet<(usize, usize)> = BTreeSet::new();
    // A pointer of unwind information to `addr`, canonical: an LSDA by its
    // form and the offset in it, anything else by its atom's key.
    let pointer =
        |addr: u64, hint: Option<usize>, h: &mut Canon, attached: &mut BTreeSet<_>| match locate(
            addr, hint,
        ) {
            Some((si, k, off)) if Some(si) == lsda => {
                h.u64(0x15DA);
                h.u128(own_hash.get(&(si, k)).copied().unwrap_or(0));
                h.u64(off);
                attached.insert((si, k));
            }
            Some((si, k, off)) => {
                h.bytes(&keys[si][k]);
                h.u64(off);
            }
            None => {
                h.u64(5);
                h.u64(addr);
            }
        };
    // Unwind entries and FDEs by the atom of their function.
    let mut entries: BTreeMap<(usize, usize), Vec<u128>> = BTreeMap::new();
    if let Some(ui) = unwind {
        let s = sections[ui];
        let b = s.bytes().unwrap_or(&[]);
        let rel_at: BTreeMap<u64, usize> = s
            .relocations
            .iter()
            .enumerate()
            .map(|(i, r)| (u64::from(r.address), i))
            .collect();
        let rel_at = |at: u64| rel_at.get(&at).copied();
        for e in (0..b.len() as u64 / 32).map(|i| i * 32) {
            let func = rel_at(e).and_then(|ri| {
                let r = s.relocations[ri];
                if r.external {
                    o.symbols
                        .get(r.symbolnum as usize)
                        .and_then(|y| locate(y.n_value, usize::from(y.n_sect).checked_sub(1)))
                } else {
                    reloc::local_address(cputype, endian, s, ri)
                        .and_then(|a| locate(a, (r.symbolnum as usize).checked_sub(1)))
                }
            });
            let Some((si, k, off)) = func else { continue };
            let mut h = Canon::new();
            h.u64(off);
            h.bytes(&b[e as usize + 8..e as usize + 16]);
            match rel_at(e + 16) {
                Some(ri) => target(s, ri, &mut h),
                None => h.bytes(&b[e as usize + 16..e as usize + 24]),
            }
            // The LSDA.
            match rel_at(e + 24) {
                Some(ri) => {
                    let r = s.relocations[ri];
                    let a = if r.external {
                        o.symbols.get(r.symbolnum as usize).and_then(|y| {
                            (y.n_type & N_EXT == 0 && y.in_section()).then(|| {
                                (
                                    y.n_value.wrapping_add(u64::from_le_bytes(
                                        b[e as usize + 24..e as usize + 32]
                                            .try_into()
                                            .unwrap_or([0; 8]),
                                    )),
                                    usize::from(y.n_sect).checked_sub(1),
                                )
                            })
                        })
                    } else {
                        reloc::local_address(cputype, endian, s, ri)
                            .map(|a| (a, (r.symbolnum as usize).checked_sub(1)))
                    };
                    match a {
                        Some((a, hint)) => pointer(a, hint, &mut h, &mut attached),
                        None => target(s, ri, &mut h),
                    }
                }
                None => h.bytes(&b[e as usize + 24..e as usize + 32]),
            }
            entries.entry((si, k)).or_default().push(h.finish());
        }
    }
    // FDEs: each by its record's bytes (its CIE pointer and pointers taken
    // out), its pointers' targets, and its CIE's bytes and relocations; FDEs
    // whose function is in no atom make an atom of their own.
    let mut orphans = Canon::new();
    let mut orphan_count = 0;
    if let Some(ei) = eh {
        let s = sections[ei];
        let bytes = s.bytes().unwrap_or(&[]);
        let covered: BTreeMap<u32, usize> = s
            .relocations
            .iter()
            .enumerate()
            .map(|(i, r)| (r.address, i))
            .collect();
        match turbonop_macho_object::ehframe::pointers(bytes, o.is_64(), endian) {
            None => {
                orphans.u64(0xEEEE);
                orphans.bytes(bytes);
                orphan_count += 1;
            }
            Some(ptrs) => {
                // Records: (start, end) -> pointers; CIEs by start.
                let mut records: BTreeMap<(usize, usize), Vec<_>> = BTreeMap::new();
                for p in &ptrs {
                    records.entry(p.record).or_default().push(*p);
                }
                let cie_of: BTreeMap<usize, usize> =
                    turbonop_macho_object::ehframe::cie_links(bytes, endian)
                        .unwrap_or_default()
                        .into_iter()
                        .map(|(field, cie)| (field - 4, cie))
                        .collect();
                // A record's span by its start (CIEs too).
                let mut spans: BTreeMap<usize, usize> = BTreeMap::new();
                let mut at = 0usize;
                while at + 4 <= bytes.len() {
                    let len =
                        u32::from_le_bytes(bytes[at..at + 4].try_into().unwrap_or([0; 4])) as usize;
                    if len == 0 || len == 0xFFFF_FFFF || at + 4 + len > bytes.len() {
                        break;
                    }
                    spans.insert(at, at + 4 + len);
                    at += 4 + len;
                }
                let canon_record = |start: usize,
                                    end: usize,
                                    skip: &[std::ops::Range<usize>],
                                    h: &mut Canon| {
                    let mut b = bytes[start..end].to_vec();
                    for r in skip {
                        b[r.start - start..r.end - start].fill(0);
                    }
                    for (&a, &ri) in covered.range(start as u32..end as u32) {
                        let r = s.relocations[ri];
                        let n = 1usize << r.length.min(3);
                        if !r.external
                            && let Some(x) = b.get_mut(a as usize - start..a as usize - start + n)
                        {
                            x.fill(0);
                        }
                        h.u64(u64::from(a) - start as u64);
                        h.u64(
                            u64::from(r.kind) | u64::from(r.length) << 8 | u64::from(r.pcrel) << 16,
                        );
                        target(s, ri, h);
                    }
                    h.bytes(&b);
                };
                for (&start, &end) in &spans {
                    let Some(&cie) = cie_of.get(&start) else {
                        continue; // A CIE: checked with each FDE that uses it.
                    };
                    let mut h = Canon::new();
                    let mut skip = vec![start + 4..start + 8];
                    let mut func = None;
                    for p in records.get(&(start, end)).map_or(&[][..], Vec::as_slice) {
                        let n = usize::from(p.size);
                        if covered.contains_key(&(p.offset as u32)) {
                            continue;
                        }
                        let Some(raw) = bytes.get(p.offset..p.offset + n) else {
                            continue;
                        };
                        let mut v = [0u8; 8];
                        v[..n.min(8)].copy_from_slice(&raw[..n.min(8)]);
                        let v = u64::from_le_bytes(v);
                        let bits = 8 * n as u32;
                        let signed = if bits >= 64 {
                            v as i64
                        } else {
                            ((v << (64 - bits)) as i64) >> (64 - bits)
                        };
                        let t = if p.encoding & 0x70
                            == turbonop_macho_object::ehframe::DW_EH_PE_PCREL
                        {
                            s.addr
                                .wrapping_add(p.offset as u64)
                                .wrapping_add(signed as u64)
                        } else {
                            v
                        };
                        h.u64((p.offset - start) as u64);
                        if p.kind == turbonop_macho_object::ehframe::PointerKind::PcBegin {
                            func = locate(t, None);
                            if let Some((_, _, off)) = func {
                                h.u64(off);
                            }
                        } else {
                            pointer(t, None, &mut h, &mut attached);
                        }
                        skip.push(p.offset..p.offset + n);
                    }
                    canon_record(start, end, &skip, &mut h);
                    // Its CIE.
                    match spans.get(&cie) {
                        Some(&cend) => {
                            let skip: Vec<_> = records
                                .get(&(cie, cend))
                                .map_or(&[][..], Vec::as_slice)
                                .iter()
                                .filter(|p| !covered.contains_key(&(p.offset as u32)))
                                .map(|p| p.offset..p.offset + usize::from(p.size))
                                .collect();
                            canon_record(cie, cend, &skip, &mut h);
                        }
                        None => h.u64(0xC1E),
                    }
                    match func {
                        Some((si, k, _)) => entries.entry((si, k)).or_default().push(h.finish()),
                        None => {
                            orphans.u128(h.finish());
                            orphan_count += 1;
                        }
                    }
                }
            }
        }
    }
    for ((si, k), (mut h, unit, names)) in own {
        if attached.contains(&(si, k)) {
            continue;
        }
        for e in entries.get(&(si, k)).into_iter().flatten() {
            h.u128(*e);
        }
        m.atoms.push(Atom {
            key: keys[si][k].clone(),
            hash: h.finish(),
            unit,
            names,
            desc: h.log.take().unwrap_or_default(),
        });
    }
    if orphan_count > 0 {
        m.atoms.push(Atom {
            key: b"__TEXT,__eh_frame".to_vec(),
            hash: orphans.finish(),
            unit: false,
            names: Vec::new(),
            desc: orphans.log.take().unwrap_or_default(),
        });
    }
    for s in sections.iter().filter(|s| s.segment_name() == b"__DWARF") {
        // The bytes with every relocation's place zeroed, and each
        // relocation canonical, with the key of the atom it reaches (if it
        // reaches one through a local symbol or a local relocation).
        let mut bytes = s.bytes().unwrap_or(&[]).to_vec();
        let mut relocs = Vec::with_capacity(s.relocations.len());
        for (ri, r) in s.relocations.iter().enumerate() {
            let n = 1usize << r.length.min(3);
            if let Some(x) = bytes.get_mut(r.address as usize..r.address as usize + n) {
                x.fill(0);
            }
            let reached = if r.external {
                o.symbols
                    .get(r.symbolnum as usize)
                    .filter(|y| y.n_type & N_EXT == 0 && y.in_section())
                    .map(|y| (y.n_value, usize::from(y.n_sect).checked_sub(1)))
            } else if reloc::names_section(cputype, r) {
                reloc::local_address(cputype, endian, s, ri)
                    .map(|a| (a, (r.symbolnum as usize).checked_sub(1)))
            } else {
                None
            };
            let key = reached
                .and_then(|(a, hint)| locate(a, hint))
                .map(|(si, k, _)| keys[si][k].clone());
            let mut h = Canon::new();
            h.u64(u64::from(r.kind) | u64::from(r.length) << 8 | u64::from(r.pcrel) << 16);
            target(s, ri, &mut h);
            relocs.push((r.address, h.finish(), key));
        }
        relocs.sort();
        m.debug
            .push(([s.segment_name(), b",", s.name()].concat(), bytes, relocs));
    }
    m.obj = Some(o);
    m
}

fn read_library<'a>(data: &'a [u8], what: &str) -> Result<Library<'a>, String> {
    let a = Archive::parse(data).map_err(|e| format!("{what}: {e}"))?;
    let files: Vec<_> = a.files().collect();
    let members =
        turbonop_par::map_balanced(&files, 1, |f| read_member(&f.name, f.data, f.header_offset));
    let by_offset: BTreeMap<u64, usize> = members
        .iter()
        .enumerate()
        .map(|(i, m)| (m.header_offset, i))
        .collect();
    let index = a
        .symbols()
        .iter()
        .filter_map(|s| by_offset.get(&s.member).map(|&i| (s.name.to_vec(), i)))
        .collect();
    Ok(Library { members, index })
}

/// Compares a rewritten member with its original: the keys of the atoms it
/// lost (units, with their hashes), and what it anchors.
fn compare_member(o: &Member<'_>, n: &Member<'_>, rep: &mut Report) -> (Vec<Lost>, Vec<Vec<u8>>) {
    let (Some(oo), Some(no)) = (&o.obj, &n.obj) else {
        rep.fail("rewritten members that don't read", || show(&n.name));
        return (Vec::new(), Vec::new());
    };
    if oo.header != no.header {
        rep.fail("rewritten members' headers changed", || show(&n.name));
    }
    let others = |obj: &Object<'_>| -> Vec<Command<'static>> {
        obj.commands
            .iter()
            .filter(|c| matches!(c, Command::Other { .. }))
            .map(|c| match c {
                Command::Other { cmd, body } => Command::Other {
                    cmd: *cmd,
                    body: Cow::Owned(body.to_vec()),
                },
                _ => unreachable!(),
            })
            .collect()
    };
    if others(oo) != others(no) {
        rep.fail("rewritten members' load commands changed", || show(&n.name));
    }
    let new: BTreeMap<&[u8], u128> = n.atoms.iter().map(|a| (a.key.as_slice(), a.hash)).collect();
    let desc = |key: &[u8]| n.atoms.iter().find(|a| a.key == key).map(|a| &a.desc);
    let mut lost = Vec::new();
    let mut lost_names: BTreeSet<Vec<u8>> = BTreeSet::new();
    for a in &o.atoms {
        match new.get(a.key.as_slice()) {
            Some(&h) if h == a.hash => {}
            Some(_) => rep.fail("atoms changed in rewritten members", || {
                let d = desc(&a.key).map(|d| {
                    let k = a.desc.iter().zip(d).position(|(x, y)| x != y).unwrap_or(0);
                    let lo = k.saturating_sub(3);
                    format!(
                        " (part {k}: {:?} vs {:?})",
                        a.desc.get(lo..(k + 3).min(a.desc.len())),
                        d.get(lo..(k + 3).min(d.len()))
                    )
                });
                format!(
                    "{}: {}{}",
                    show(&n.name),
                    show(&a.key),
                    d.unwrap_or_default()
                )
            }),
            None if a.unit => {
                lost.push((a.key.clone(), a.hash));
                lost_names.extend(a.names.iter().cloned());
            }
            None => rep.fail("atoms lost that aren't units", || {
                format!("{}: {}", show(&n.name), show(&a.key))
            }),
        }
    }
    rep.checked("atoms kept", (o.atoms.len() - lost.len()) as u64);
    // Definitions and references.
    let defined = |obj: &Object<'_>| -> BTreeSet<Vec<u8>> {
        obj.symbols
            .iter()
            .filter(|y| y.n_type & N_EXT != 0 && y.n_type & N_TYPE != N_UNDF)
            .map(|y| y.name.to_vec())
            .collect()
    };
    let undefined = |obj: &Object<'_>| -> BTreeMap<Vec<u8>, u16> {
        obj.symbols
            .iter()
            .filter(|y| y.n_type & N_EXT != 0 && y.n_type & N_TYPE == N_UNDF && y.n_value == 0)
            .map(|y| (y.name.to_vec(), y.n_desc))
            .collect()
    };
    let (od, nd) = (defined(oo), defined(no));
    let (ou, nu) = (undefined(oo), undefined(no));
    let mut anchors = Vec::new();
    for d in &od {
        if nd.contains(d) {
            continue;
        }
        if lost_names.contains(d) && nu.get(d).is_some_and(|&desc| desc & N_WEAK_REF != 0) {
            continue;
        }
        rep.fail("definitions lost that aren't weak references", || {
            format!("{}: {}", show(&n.name), show(d))
        });
    }
    rep.checked("definitions", od.len() as u64);
    for u in nu.keys() {
        if ou.contains_key(u) || lost_names.contains(u) {
            continue;
        }
        if u.starts_with(ANCHOR) {
            anchors.push(u.clone());
            continue;
        }
        rep.fail("new undefined names", || {
            format!("{}: {}", show(&n.name), show(u))
        });
    }
    if !lost.is_empty() && anchors.is_empty() {
        rep.fail("members that lost units and anchor nothing", || {
            show(&n.name)
        });
    }
    // Debug sections.
    let lost_keys: BTreeSet<&[u8]> = lost.iter().map(|(k, _)| k.as_slice()).collect();
    for (name, bytes, relocs) in &o.debug {
        let Some((_, nb, nr)) = n.debug.iter().find(|d| &d.0 == name) else {
            rep.fail("debug sections lost", || {
                format!("{}: {}", show(&n.name), show(name))
            });
            continue;
        };
        let expect: Vec<&(u32, u128, Option<Vec<u8>>)> = relocs
            .iter()
            .filter(|r| !r.2.as_deref().is_some_and(|k| lost_keys.contains(k)))
            .collect();
        let removed = relocs.len() - expect.len();
        let same_relocs = expect.len() == nr.len()
            && expect.iter().zip(nr).all(|(a, b)| a.0 == b.0 && a.1 == b.1);
        if bytes != nb || !same_relocs {
            rep.fail(
                "debug sections changed but at relocations into lost units (removed, zeroed)",
                || format!("{}: {}", show(&n.name), show(name)),
            );
        }
        rep.checked("debug relocations into lost units removed", removed as u64);
    }
    (lost, anchors)
}

/// Verifies a folded Mach-O library against the original, on every core.
pub fn verify(a: &[u8], b: &[u8], options: &Options) -> Result<Report, String> {
    let (la, lb) = turbonop_par::join(
        || read_library(a, "the original library"),
        || read_library(b, "the rewritten library"),
    );
    let (la, lb) = (la?, lb?);
    let mut rep = Report::new();
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
    let pairs: Vec<(usize, usize)> = place
        .iter()
        .enumerate()
        .filter_map(|(k, p)| p.map(|j| (k, j)))
        .collect();
    let results = turbonop_par::map_balanced(&pairs, 4, |&(k, j)| {
        let (o, n) = (&la.members[k], &lb.members[j]);
        let mut r = Report::new();
        if o.data == n.data {
            return (r, Vec::new(), Vec::new());
        }
        if o.obj.is_none() {
            r.fail("members not read as Mach-O objects that changed", || {
                show(&o.name)
            });
            return (r, Vec::new(), Vec::new());
        }
        let (lost, anchors) = compare_member(o, n, &mut r);
        (r, lost, anchors)
    });
    // Synthesized members' units, by anchor.
    let mut held: BTreeMap<u128, usize> = BTreeMap::new();
    let by_anchor: BTreeMap<&[u8], usize> = synth
        .iter()
        .filter_map(|&j| lb.members[j].anchor.as_deref().map(|a| (a, j)))
        .collect();
    for &j in &synth {
        for at in &lb.members[j].atoms {
            if at.key == b"l_turbolib.pad" {
                continue;
            }
            if !at.unit {
                rep.fail("synthesized members' atoms that aren't units", || {
                    format!("{}: {}", show(&lb.members[j].name), show(&at.key))
                });
            }
            if held.insert(at.hash, j).is_some_and(|other| other != j) {
                rep.fail("units held by two synthesized members", || show(&at.key));
            }
        }
    }
    let mut moved = 0u64;
    for ((k, j), (r, lost, anchors)) in pairs.iter().zip(results) {
        rep.merge(r);
        for (key, hash) in lost {
            moved += 1;
            let ok = held.get(&hash).is_some_and(|&s| {
                anchors
                    .iter()
                    .any(|a| by_anchor.get(a.as_slice()) == Some(&s))
            });
            if !ok {
                rep.fail("lost units no anchored synthesized member holds", || {
                    let mut s = format!("{}: {}", show(&la.members[*k].name), show(&key));
                    if std::env::var_os("TURBOLIB_VERIFY_DUMP").is_some() {
                        // The lost atom's form, and that of each synthesized
                        // atom of its key.
                        let mine = la.members[*k].atoms.iter().find(|a| a.key == key);
                        s += &format!("\n      lost: {:?}", mine.map(|a| &a.desc));
                        for &j in &synth {
                            for at in lb.members[j].atoms.iter().filter(|a| a.key == key) {
                                s += &format!(
                                    "\n      in {}: {:?}",
                                    show(&lb.members[j].name),
                                    at.desc
                                );
                            }
                        }
                    }
                    s
                });
            }
        }
        let _ = j;
    }
    rep.checked("moved units", moved);
    // The indexes, as llvm-ar computes them.
    for (lib, what) in [(&la, "original"), (&lb, "rewritten")] {
        let mut computed = Vec::new();
        for (i, m) in lib.members.iter().enumerate() {
            if let Ok(s) = member_symbols(m.data, Archiver::LlvmAr) {
                computed.extend(s.symbols.iter().map(|n| (n.to_vec(), i)));
            }
        }
        if computed != lib.index {
            rep.fail("indexes not computed from their members", || {
                what.to_string()
            });
        }
    }
    // --pull=preserve.
    if options.pull == Pull::Preserve {
        let new_index: BTreeMap<&[u8], usize> = {
            let mut m = BTreeMap::new();
            for (n, i) in &lb.index {
                m.entry(n.as_slice()).or_insert(*i);
            }
            m
        };
        let mut first: BTreeMap<&[u8], usize> = BTreeMap::new();
        for (n, i) in &la.index {
            first.entry(n.as_slice()).or_insert(*i);
        }
        let mut sent = 0;
        for (name, &i) in &first {
            let ok = match new_index.get(name) {
                Some(&j) if place[i] == Some(j) => true,
                Some(&j) if lb.members[j].anchor.is_some() => !la.members[i].side_effects,
                _ => false,
            };
            if ok {
                sent += 1;
            } else {
                rep.fail("names the index sends elsewhere (--pull=preserve)", || {
                    show(name)
                });
            }
        }
        rep.checked("names of the index sent where --pull=preserve allows", sent);
    }
    let _ = N_PEXT;
    Ok(rep)
}
