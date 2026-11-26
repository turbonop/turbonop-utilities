//! Moving folded units' DWARF into the synthesized member that holds them.
//!
//! Every unit of a synthesized member comes from one member (the first
//! that held a copy), whose compile unit describes them. The synthesized
//! member gets a compile unit of its own, built from that one:
//!
//! - **DIEs.** The roots are the DIEs whose addresses (`DW_AT_low_pc`, a
//!   range list, `DW_OP_addr`, through `.debug_addr` or not) are in the
//!   moved sections, each taken up to the subprogram that holds it (or to
//!   the last DIE below the unit or a namespace). Each root's subtree is
//!   kept, then everything a kept DIE refers to (types, specifications,
//!   abstract origins, the DIEs location expressions name), whole, to a
//!   fixpoint; a namespace or unit referred to, and every kept DIE's
//!   ancestors, are kept as skeletons (the DIE and only its kept children).
//!   They are written in the original order, `DW_AT_sibling` left out.
//! - **The unit's DIE** is the original's attributes but its addresses,
//!   line table, bases, and macros: `DW_AT_low_pc` 0, `DW_AT_ranges` the
//!   moved code sections, `DW_AT_stmt_list` the new line table.
//! - **The line table** is the original's header, then the sequences
//!   whose `DW_LNE_set_address` names a moved section (assemblers write
//!   one per section).
//! - **Range and location lists** are rewritten into the new unit's
//!   `.debug_rnglists` and `.debug_loclists` (`.debug_ranges` and
//!   `.debug_loc` before DWARF 5) as start-and-length entries; GCC's
//!   location views (`DW_AT_GNU_locviews`) go with them.
//! - **Forms.** Addresses are written `DW_FORM_addr` (an index into
//!   `.debug_addr` is resolved, so no `.debug_addr` is written; in
//!   expressions `DW_OP_addrx` becomes `DW_OP_addr` and `DW_OP_constx` a
//!   `DW_OP_const<n>u`), strings `DW_FORM_strp` or `DW_FORM_line_strp`
//!   into the member's own string sections, references `DW_FORM_ref4` (in
//!   expressions, unit-relative ULEB128 references are written padded to 4
//!   bytes). The rest is copied as it was.
//! - **Relocations** go to the copied sections' symbols, to the new debug
//!   sections, or, for a global name, to it (a weak reference where the
//!   member doesn't define it, so that it loads nothing). An address in
//!   something outside the moved sections (a `static` variable a moved
//!   function's type or a skeleton names) is written 0, as a linker
//!   tombstones discarded code, and counted.
//!
//! The givers' DIEs stay, their addresses tombstoned (see the parent
//! module). What isn't handled (64-bit DWARF, REL machines, compressed
//! sections, split units, a unit referring outside itself) gives the
//! synthesized member no debug information, as before, with the reason.

use std::collections::{BTreeMap, BTreeSet, HashMap};

use turbonop_dwarf::consts::*;
use turbonop_dwarf::ops::{self, Operand};
use turbonop_dwarf::{Abbrev, Abbrevs, Attr, Reader, Sections, Unit, Value};
use turbonop_elf_object::{Data, Endian, Object, Symbol, shf};

/// A debug section's role.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub(crate) enum Kind {
    Info,
    Abbrev,
    Str,
    LineStr,
    Line,
    Rnglists,
    Loclists,
    Ranges,
    Loc,
    Addr,
    StrOffsets,
    Aranges,
}

const KINDS: usize = 12;

impl Kind {
    fn of(name: &[u8]) -> Option<Kind> {
        Some(match name {
            b".debug_info" => Kind::Info,
            b".debug_abbrev" => Kind::Abbrev,
            b".debug_str" => Kind::Str,
            b".debug_line_str" => Kind::LineStr,
            b".debug_line" => Kind::Line,
            b".debug_rnglists" => Kind::Rnglists,
            b".debug_loclists" => Kind::Loclists,
            b".debug_ranges" => Kind::Ranges,
            b".debug_loc" => Kind::Loc,
            b".debug_addr" => Kind::Addr,
            b".debug_str_offsets" => Kind::StrOffsets,
            b".debug_aranges" => Kind::Aranges,
            _ => return None,
        })
    }

    /// Whether its relocations to other debug sections are applied in the
    /// copy that's read.
    fn relocated(self) -> bool {
        !matches!(self, Kind::Abbrev | Kind::Str | Kind::LineStr)
    }
}

/// An address as a relocatable object has it: a symbol and an addend, or
/// a constant.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Addr {
    Sym { sym: u32, addend: i64, rtype: u32 },
    Const(u64),
}

impl Addr {
    fn add(self, other: Addr) -> Option<Addr> {
        match (self, other) {
            (Addr::Const(a), Addr::Const(b)) => Some(Addr::Const(a.wrapping_add(b))),
            (Addr::Sym { sym, addend, rtype }, Addr::Const(c))
            | (Addr::Const(c), Addr::Sym { sym, addend, rtype }) => Some(Addr::Sym {
                sym,
                addend: addend.wrapping_add(c as i64),
                rtype,
            }),
            _ => None,
        }
    }

    fn plus(self, c: u64) -> Addr {
        match self {
            Addr::Const(a) => Addr::Const(a.wrapping_add(c)),
            Addr::Sym { sym, addend, rtype } => Addr::Sym {
                sym,
                addend: addend.wrapping_add(c as i64),
                rtype,
            },
        }
    }

    /// `end - self`, if both are of one symbol (or constants).
    fn distance(self, end: Addr) -> Option<u64> {
        match (self, end) {
            (Addr::Const(a), Addr::Const(b)) => Some(b.wrapping_sub(a)),
            (
                Addr::Sym {
                    sym: s, addend: a, ..
                },
                Addr::Sym {
                    sym: t, addend: b, ..
                },
            ) if s == t => Some(b.wrapping_sub(a) as u64),
            _ => None,
        }
    }
}

/// A relocation of a debug section.
#[derive(Clone, Copy, Debug)]
struct Rel {
    offset: u64,
    rtype: u32,
    sym: u32,
    addend: i64,
}

/// A debug section of the source member: its bytes (with relocations to
/// debug sections applied) and its relocations, by offset.
struct Sec {
    bytes: Vec<u8>,
    rels: Vec<Rel>,
}

/// One DIE of a source unit.
#[derive(Clone, Copy, Debug)]
struct Die {
    offset: u64,
    /// Its parent's index (`u32::MAX` for the unit's DIE).
    parent: u32,
    tag: u16,
    /// Where its subtree ends (its next sibling, or its parent's end).
    end: u64,
}

/// A line-table sequence of a source unit.
#[derive(Clone, Copy, Debug)]
struct Seq {
    start: u64,
    end: u64,
    /// The section its first `DW_LNE_set_address` names, if one of the
    /// member's.
    section: Option<usize>,
}

/// A source compile unit.
struct Cu {
    offset: u64,
    version: u16,
    address_size: u8,
    abbrevs: Abbrevs,
    dies: Vec<Die>,
    /// Its base address (`DW_AT_low_pc`), for lists relative to it.
    low_pc: Addr,
    addr_base: u64,
    str_offsets_base: u64,
    rnglists_base: u64,
    loclists_base: u64,
    /// Its line table: where it starts, its program starts, and it ends.
    line: Option<(u64, u64, u64)>,
    seqs: Vec<Seq>,
}

/// One member's DWARF, read once for every synthesized member it gives
/// units to.
pub(crate) struct Source<'p> {
    symbols: &'p [Symbol<'p>],
    sections: Vec<(u64, u64)>,
    endian: Endian,
    abs32: u32,
    abs64: u32,
    secs: Vec<Option<Sec>>,
    /// Each symbol's debug section, if it is one's (a section symbol).
    sym_kind: Vec<Option<Kind>>,
    cus: Vec<Cu>,
    /// The root DIEs of each section: (unit, DIE).
    roots: BTreeMap<usize, Vec<(usize, usize)>>,
}

/// A DIE's attribute as read: where its value lies in `.debug_info`.
#[derive(Clone, Copy, Debug)]
struct A<'d> {
    name: u16,
    form: u16,
    implicit: i64,
    pos: u64,
    end: u64,
    value: Value<'d>,
}

type R<T> = Result<T, &'static str>;

fn reloc_size(machine: u16, rtype: u32) -> Option<usize> {
    super::debug_reloc_size(machine, rtype)
}

fn read_uint(b: &[u8], at: usize, n: usize, endian: Endian) -> Option<u64> {
    Reader::new(
        b,
        at,
        match endian {
            Endian::Little => turbonop_core::Endian::Little,
            Endian::Big => turbonop_core::Endian::Big,
        },
    )
    .uint(n)
}

fn core_endian(e: Endian) -> turbonop_core::Endian {
    match e {
        Endian::Little => turbonop_core::Endian::Little,
        Endian::Big => turbonop_core::Endian::Big,
    }
}

impl<'p> Source<'p> {
    /// Reads `obj`'s DWARF. `owner` gives each section's unit, so that only
    /// sections of units get roots.
    pub(crate) fn new(obj: &'p Object<'p>, owner: &[Option<usize>]) -> R<Self> {
        let machine = obj.header.machine;
        let (abs32, abs64) = match machine {
            62 => (10, 1),
            183 => (258, 257),
            _ => return Err("a machine whose debug relocations aren't rewritten"),
        };
        if obj.class() != turbonop_elf_object::Class::Elf64 {
            return Err("a 32-bit object");
        }
        let endian = obj.endian();
        let symbols = obj.symbols();
        // The debug sections outside groups.
        let mut index: [Option<usize>; KINDS] = [None; KINDS];
        let mut kind_of_section: Vec<Option<Kind>> = vec![None; obj.sections.len()];
        for (i, s) in obj.sections.iter().enumerate() {
            let Some(k) = Kind::of(&s.name) else {
                continue;
            };
            if s.flags & shf::GROUP != 0 || owner.get(i).copied().flatten().is_some() {
                continue;
            }
            if s.flags & shf::COMPRESSED != 0 {
                return Err("compressed debug sections");
            }
            if index[k as usize].is_some() {
                return Err("two debug sections of one kind");
            }
            index[k as usize] = Some(i);
            kind_of_section[i] = Some(k);
        }
        let sym_kind: Vec<Option<Kind>> = symbols
            .iter()
            .map(|s| {
                if s.bind() == 0 && s.in_section() {
                    kind_of_section.get(s.shndx as usize).copied().flatten()
                } else {
                    None
                }
            })
            .collect();
        let mut rels_of: BTreeMap<usize, Vec<Rel>> = BTreeMap::new();
        for s in &obj.sections {
            if !s.is_relocations() {
                continue;
            }
            let t = s.info as usize;
            if kind_of_section.get(t).copied().flatten().is_none() {
                continue;
            }
            if s.kind != turbonop_elf_object::sht::RELA {
                return Err("REL debug relocations");
            }
            let Data::Relocations(rs) = &s.data else {
                continue;
            };
            let e = rels_of.entry(t).or_default();
            for r in rs {
                e.push(Rel {
                    offset: r.offset,
                    rtype: obj.reloc_type(r),
                    sym: obj.reloc_symbol(r),
                    addend: r.addend,
                });
            }
        }
        let mut secs: Vec<Option<Sec>> = (0..KINDS).map(|_| None).collect();
        for (k, at) in index.iter().enumerate() {
            let Some(at) = *at else { continue };
            let s = &obj.sections[at];
            let Data::Bytes(b) = &s.data else {
                return Err("a debug section without contents");
            };
            let mut bytes = b.to_vec();
            let mut rels = rels_of.remove(&at).unwrap_or_default();
            rels.sort_by_key(|r| r.offset);
            let kind = index_kind(k);
            for r in &rels {
                let size = reloc_size(machine, r.rtype).ok_or("a debug relocation not known")?;
                if !kind.relocated() {
                    return Err("relocations in a string or abbreviation section");
                }
                let Some(_) = sym_kind.get(r.sym as usize).copied().flatten() else {
                    continue;
                };
                let value = symbols[r.sym as usize].value.wrapping_add(r.addend as u64);
                super::put(&mut bytes, r.offset as usize, size, value, endian);
            }
            secs[k] = Some(Sec { bytes, rels });
        }
        let mut src = Source {
            symbols,
            sections: obj
                .sections
                .iter()
                .map(|s| {
                    let size = match &s.data {
                        Data::Bytes(b) => b.len() as u64,
                        _ => s.nobits_size,
                    };
                    (s.flags, size)
                })
                .collect(),
            endian,
            abs32,
            abs64,
            secs,
            sym_kind,
            cus: Vec::new(),
            roots: BTreeMap::new(),
        };
        if src.secs[Kind::Info as usize].is_none() {
            return Ok(src);
        }
        src.read_units(owner)?;
        Ok(src)
    }

    fn sec(&self, k: Kind) -> &[u8] {
        self.secs[k as usize].as_ref().map_or(&[], |s| &s.bytes)
    }

    fn rels(&self, k: Kind) -> &[Rel] {
        self.secs[k as usize].as_ref().map_or(&[], |s| &s.rels)
    }

    fn dwarf(&self) -> Sections<'_> {
        let mut s = Sections::new(core_endian(self.endian));
        s.info = self.sec(Kind::Info);
        s.abbrev = self.sec(Kind::Abbrev);
        s.str = self.sec(Kind::Str);
        s.line_str = self.sec(Kind::LineStr);
        s.str_offsets = self.sec(Kind::StrOffsets);
        s.addr = self.sec(Kind::Addr);
        s.ranges = self.sec(Kind::Ranges);
        s.rnglists = self.sec(Kind::Rnglists);
        s.loc = self.sec(Kind::Loc);
        s.loclists = self.sec(Kind::Loclists);
        s.line = self.sec(Kind::Line);
        s
    }

    /// The relocation at `offset` of `k`, if any.
    fn rel_at(&self, k: Kind, offset: u64) -> Option<&Rel> {
        let rels = self.rels(k);
        let i = rels.partition_point(|r| r.offset < offset);
        rels.get(i).filter(|r| r.offset == offset)
    }

    /// The relocations in `[start, end)` of `k`.
    fn rels_in(&self, k: Kind, start: u64, end: u64) -> &[Rel] {
        let rels = self.rels(k);
        let lo = rels.partition_point(|r| r.offset < start);
        let hi = rels.partition_point(|r| r.offset < end);
        &rels[lo..hi]
    }

    /// The address of `n` bytes at `offset` of `k`.
    fn addr_at(&self, k: Kind, offset: u64, n: usize) -> R<Addr> {
        if let Some(r) = self.rel_at(k, offset)
            && self
                .sym_kind
                .get(r.sym as usize)
                .copied()
                .flatten()
                .is_none()
        {
            return Ok(Addr::Sym {
                sym: r.sym,
                addend: r.addend,
                rtype: r.rtype,
            });
        }
        read_uint(self.sec(k), offset as usize, n, self.endian)
            .map(Addr::Const)
            .ok_or("an address past its section")
    }

    /// The address at `index` of the unit's `.debug_addr` entries.
    fn indexed(&self, cu: &Cu, index: u64) -> R<Addr> {
        let n = u64::from(cu.address_size);
        let at = cu
            .addr_base
            .checked_add(index.checked_mul(n).ok_or("a bad address index")?)
            .ok_or("a bad address index")?;
        self.addr_at(Kind::Addr, at, usize::from(cu.address_size))
    }

    /// The member section an address is in, if it names one of the
    /// member's sections by a local symbol or a definition.
    fn section_of(&self, a: Addr) -> Option<usize> {
        let Addr::Sym { sym, .. } = a else {
            return None;
        };
        let s = self.symbols.get(sym as usize)?;
        s.in_section().then_some(s.shndx as usize)
    }

    fn unit<'s>(&'s self, cu: &Cu) -> R<Unit<'s>> {
        let mut u = Unit::at(self.dwarf(), cu.offset).ok_or("a unit that doesn't read")?;
        u.addr_base = cu.addr_base;
        u.str_offsets_base = cu.str_offsets_base;
        u.rnglists_base = cu.rnglists_base;
        u.loclists_base = cu.loclists_base;
        Ok(u)
    }

    fn read_units(&mut self, owner: &[Option<usize>]) -> R<()> {
        let dwarf = self.dwarf();
        let units: Vec<Unit<'_>> =
            Unit::iter(dwarf, turbonop_dwarf::UnitSection::Info, 0).collect();
        let mut cus = Vec::new();
        for u in &units {
            if u.dwarf64 {
                return Err("64-bit DWARF");
            }
            if u.unit_type != DW_UT_COMPILE && u.unit_type != DW_UT_PARTIAL {
                if u.unit_type == DW_UT_SKELETON || u.unit_type == DW_UT_SPLIT_COMPILE {
                    return Err("split DWARF");
                }
                continue;
            }
            let abbrevs = Abbrevs::parse(dwarf.abbrev, u.abbrev_offset, dwarf.endian)
                .ok_or("an abbreviation table that doesn't read")?;
            let mut cu = Cu {
                offset: u.offset,
                version: u.version,
                address_size: u.address_size,
                abbrevs,
                dies: Vec::new(),
                low_pc: Addr::Const(0),
                addr_base: u.addr_base,
                str_offsets_base: u.str_offsets_base,
                rnglists_base: u.rnglists_base,
                loclists_base: u.loclists_base,
                line: None,
                seqs: Vec::new(),
            };
            // The unit's DIE: its bases.
            let (_, top) = read_die(u, &cu.abbrevs, u.dies)?;
            let attrs: Vec<Attr<'_>> = top
                .iter()
                .map(|a| Attr {
                    name: a.name,
                    form: a.form,
                    value: a.value,
                })
                .collect();
            let mut unit = *u;
            unit.set_bases(&attrs);
            cu.addr_base = unit.addr_base;
            cu.str_offsets_base = unit.str_offsets_base;
            cu.rnglists_base = unit.rnglists_base;
            cu.loclists_base = unit.loclists_base;
            if top
                .iter()
                .any(|a| a.name == DW_AT_GNU_DWO_ID || a.name == DW_AT_DWO_NAME)
            {
                return Err("split DWARF");
            }
            cu.dies = walk(&unit, &cu.abbrevs)?;
            cus.push((
                cu,
                unit,
                top.iter().find(|a| a.name == DW_AT_LOW_PC).copied(),
                top.iter().find(|a| a.name == DW_AT_STMT_LIST).copied(),
            ));
        }
        let mut out = Vec::new();
        for (mut cu, unit, low, stmt) in cus {
            if let Some(a) = low {
                cu.low_pc = self.attr_addr(&cu, &a)?;
            }
            if let Some(a) = stmt {
                let off = unit.uint(&a.value).ok_or("a bad DW_AT_stmt_list")?;
                let (line, seqs) = self.sequences(off, cu.address_size)?;
                cu.line = Some(line);
                cu.seqs = seqs;
            }
            out.push(cu);
        }
        // Roots: DIEs with addresses in sections of units.
        let mut roots: BTreeMap<usize, BTreeSet<(usize, usize)>> = BTreeMap::new();
        for (ci, cu) in out.iter().enumerate() {
            let unit = self.unit(cu)?;
            for (di, d) in cu.dies.iter().enumerate() {
                if di == 0 {
                    continue;
                }
                let (_, attrs) = read_die(&unit, &cu.abbrevs, d.offset)?;
                if attrs.iter().any(|a| a.name == DW_AT_DECLARATION) {
                    continue;
                }
                let mut here = BTreeSet::new();
                for a in &attrs {
                    for addr in self.addresses_of(cu, &unit, a)? {
                        if let Some(s) = self.section_of(addr)
                            && owner.get(s).copied().flatten().is_some()
                        {
                            here.insert(s);
                        }
                    }
                }
                if here.is_empty() {
                    continue;
                }
                let root = root_of(&cu.dies, di);
                for s in here {
                    roots.entry(s).or_default().insert((ci, root));
                }
            }
        }
        self.roots = roots
            .into_iter()
            .map(|(s, v)| (s, v.into_iter().collect()))
            .collect();
        self.cus = out;
        Ok(())
    }

    /// The address an address-class attribute holds.
    fn attr_addr(&self, cu: &Cu, a: &A<'_>) -> R<Addr> {
        match a.value {
            Value::Addr(_) => self.addr_at(Kind::Info, a.pos, usize::from(cu.address_size)),
            Value::AddrIndex(i) => self.indexed(cu, i),
            Value::Uint(v) => Ok(Addr::Const(v)),
            Value::Data(_) | Value::Sint(_) => Ok(Addr::Const(0)),
            _ => Err("an address of a form not known"),
        }
    }

    /// The addresses an attribute holds or names (through a range list or
    /// an expression's `DW_OP_addr`), for finding roots.
    fn addresses_of(&self, cu: &Cu, unit: &Unit<'_>, a: &A<'_>) -> R<Vec<Addr>> {
        let mut out = Vec::new();
        match a.form {
            DW_FORM_ADDR
            | DW_FORM_ADDRX
            | DW_FORM_ADDRX1
            | DW_FORM_ADDRX2
            | DW_FORM_ADDRX3
            | DW_FORM_ADDRX4
            | DW_FORM_GNU_ADDR_INDEX => out.push(self.attr_addr(cu, a)?),
            DW_FORM_EXPRLOC => {
                if let Value::Block(b) = a.value {
                    let at = self.offset_in(Kind::Info, b);
                    if let Ok(list) =
                        ops::decode(b, core_endian(self.endian), cu.address_size, 4, cu.version)
                    {
                        for op in list {
                            for o in &op.operands {
                                match *o {
                                    Operand::Address { at: p, .. } => out.push(self.addr_at(
                                        Kind::Info,
                                        at + p as u64,
                                        usize::from(cu.address_size),
                                    )?),
                                    Operand::AddressIndex(i)
                                        if op.code == DW_OP_ADDRX
                                            || op.code == DW_OP_GNU_ADDR_INDEX =>
                                    {
                                        out.push(self.indexed(cu, i)?);
                                    }
                                    _ => {}
                                }
                            }
                        }
                    }
                }
            }
            DW_FORM_SEC_OFFSET | DW_FORM_RNGLISTX
                if a.name == DW_AT_RANGES || a.name == DW_AT_START_SCOPE =>
            {
                let off = self.list_offset(cu, unit, a, true)?;
                for (s, _) in self.range_list(cu, off)? {
                    out.push(s);
                }
            }
            _ => {}
        }
        Ok(out)
    }

    /// Where `b` (a slice of section `k`'s bytes) starts in it.
    fn offset_in(&self, k: Kind, b: &[u8]) -> u64 {
        let base = self.sec(k).as_ptr() as usize;
        (b.as_ptr() as usize).wrapping_sub(base) as u64
    }

    /// The offset of the list a range- or location-list attribute names.
    fn list_offset(&self, cu: &Cu, unit: &Unit<'_>, a: &A<'_>, ranges: bool) -> R<u64> {
        match (a.form, a.value) {
            (DW_FORM_RNGLISTX, Value::ListIndex(i)) => {
                unit.rnglist_offset(i).ok_or("a bad range list index")
            }
            (DW_FORM_LOCLISTX, Value::ListIndex(i)) => {
                unit.loclist_offset(i).ok_or("a bad location list index")
            }
            (_, Value::Uint(v)) => {
                // DWARF 5 section offsets; GNU's split DWARF 4 isn't read.
                let _ = (cu, ranges);
                Ok(v)
            }
            _ => Err("a list of a form not known"),
        }
    }

    /// The entries of the range list at `off`: (start, length).
    fn range_list(&self, cu: &Cu, off: u64) -> R<Vec<(Addr, u64)>> {
        let n = usize::from(cu.address_size);
        let mut out = Vec::new();
        let bad = "a range list that doesn't read";
        if cu.version >= 5 {
            let k = Kind::Rnglists;
            let data = self.sec(k);
            let mut r = Reader::new(data, off as usize, core_endian(self.endian));
            let mut base = cu.low_pc;
            loop {
                let code = r.u8().ok_or(bad)?;
                match code {
                    DW_RLE_END_OF_LIST => break,
                    DW_RLE_BASE_ADDRESSX => base = self.indexed(cu, r.uleb().ok_or(bad)?)?,
                    DW_RLE_STARTX_ENDX => {
                        let s = self.indexed(cu, r.uleb().ok_or(bad)?)?;
                        let e = self.indexed(cu, r.uleb().ok_or(bad)?)?;
                        out.push((s, s.distance(e).ok_or(bad)?));
                    }
                    DW_RLE_STARTX_LENGTH => {
                        let s = self.indexed(cu, r.uleb().ok_or(bad)?)?;
                        out.push((s, r.uleb().ok_or(bad)?));
                    }
                    DW_RLE_OFFSET_PAIR => {
                        let a = r.uleb().ok_or(bad)?;
                        let b = r.uleb().ok_or(bad)?;
                        out.push((base.plus(a), b.wrapping_sub(a)));
                    }
                    DW_RLE_BASE_ADDRESS => {
                        base = self.addr_at(k, r.pos as u64, n)?;
                        r.uint(n).ok_or(bad)?;
                    }
                    DW_RLE_START_END => {
                        let s = self.addr_at(k, r.pos as u64, n)?;
                        r.uint(n).ok_or(bad)?;
                        let e = self.addr_at(k, r.pos as u64, n)?;
                        r.uint(n).ok_or(bad)?;
                        out.push((s, s.distance(e).ok_or(bad)?));
                    }
                    DW_RLE_START_LENGTH => {
                        let s = self.addr_at(k, r.pos as u64, n)?;
                        r.uint(n).ok_or(bad)?;
                        out.push((s, r.uleb().ok_or(bad)?));
                    }
                    _ => return Err(bad),
                }
            }
        } else {
            let k = Kind::Ranges;
            let data = self.sec(k);
            let mut r = Reader::new(data, off as usize, core_endian(self.endian));
            let max = if n == 8 {
                u64::MAX
            } else {
                u64::from(u32::MAX)
            };
            let mut base = cu.low_pc;
            loop {
                let pa = r.pos as u64;
                let a = r.uint(n).ok_or(bad)?;
                let pb = r.pos as u64;
                let b = r.uint(n).ok_or(bad)?;
                let (sa, sb) = (self.addr_at(k, pa, n)?, self.addr_at(k, pb, n)?);
                if a == 0 && b == 0 && sa == Addr::Const(0) && sb == Addr::Const(0) {
                    break;
                }
                if a == max && sa == Addr::Const(max) {
                    base = sb;
                    continue;
                }
                let s = base.add(sa).ok_or(bad)?;
                let e = base.add(sb).ok_or(bad)?;
                out.push((s, s.distance(e).ok_or(bad)?));
            }
        }
        Ok(out)
    }

    /// The line table at `off`: its extent and its sequences.
    #[allow(clippy::type_complexity)]
    fn sequences(&self, off: u64, address_size: u8) -> R<((u64, u64, u64), Vec<Seq>)> {
        let bad = "a line table that doesn't read";
        let data = self.sec(Kind::Line);
        let mut r = Reader::new(data, off as usize, core_endian(self.endian));
        let (_, end, dwarf64) = r.initial_length().ok_or(bad)?;
        if dwarf64 {
            return Err("64-bit DWARF");
        }
        r.data = &data[..end];
        let version = r.uint(2).ok_or(bad)?;
        if !(2..=5).contains(&version) {
            return Err(bad);
        }
        let mut asz = address_size;
        if version >= 5 {
            asz = r.u8().ok_or(bad)?;
            r.u8().ok_or(bad)?;
        }
        let header_length = r.uint(4).ok_or(bad)?;
        let program = r.pos + header_length as usize;
        r.u8().ok_or(bad)?; // minimum instruction length
        if version >= 4 {
            r.u8().ok_or(bad)?;
        }
        r.u8().ok_or(bad)?; // default_is_stmt
        r.u8().ok_or(bad)?; // line_base
        r.u8().ok_or(bad)?; // line_range
        let opcode_base = r.u8().ok_or(bad)?;
        let mut lengths = Vec::new();
        for _ in 1..opcode_base {
            lengths.push(r.u8().ok_or(bad)?);
        }
        if program > end {
            return Err(bad);
        }
        r.pos = program;
        let mut seqs = Vec::new();
        let mut start = program;
        let mut section = None;
        let mut first = true;
        while r.pos < end {
            let op = r.u8().ok_or(bad)?;
            if op >= opcode_base {
                continue;
            }
            match op {
                0 => {
                    let len = r.uleb().ok_or(bad)?;
                    let at = r.pos;
                    let sub = r.u8().ok_or(bad)?;
                    if sub == 2 && first {
                        let a = self.addr_at(Kind::Line, r.pos as u64, usize::from(asz))?;
                        section = self.section_of(a);
                        first = false;
                    }
                    r.pos = at.checked_add(len as usize).ok_or(bad)?;
                    if r.pos > end {
                        return Err(bad);
                    }
                    if sub == 1 {
                        seqs.push(Seq {
                            start: start as u64,
                            end: r.pos as u64,
                            section,
                        });
                        start = r.pos;
                        section = None;
                        first = true;
                    }
                }
                9 => {
                    r.uint(2).ok_or(bad)?;
                }
                _ => {
                    let n = lengths.get(usize::from(op) - 1).copied().ok_or(bad)?;
                    for _ in 0..n {
                        r.uleb().ok_or(bad)?;
                    }
                }
            }
        }
        Ok(((off, program as u64, end as u64), seqs))
    }
}

fn index_kind(k: usize) -> Kind {
    [
        Kind::Info,
        Kind::Abbrev,
        Kind::Str,
        Kind::LineStr,
        Kind::Line,
        Kind::Rnglists,
        Kind::Loclists,
        Kind::Ranges,
        Kind::Loc,
        Kind::Addr,
        Kind::StrOffsets,
        Kind::Aranges,
    ][k]
}

/// Reads the DIE at `offset`: its abbreviation and attributes, each with
/// where it lies.
fn read_die<'d, 'a>(
    unit: &Unit<'d>,
    abbrevs: &'a Abbrevs,
    offset: u64,
) -> R<(&'a Abbrev, Vec<A<'d>>)> {
    let bad = "a DIE that doesn't read";
    let data = unit.sections().info;
    let mut r = Reader::new(data, offset as usize, unit.sections().endian);
    let code = r.uleb().ok_or(bad)?;
    let a = abbrevs.get(code).ok_or(bad)?;
    let mut out = Vec::with_capacity(a.attrs.len());
    for spec in &a.attrs {
        let pos = r.pos as u64;
        let value = unit
            .read_value(&mut r, spec.form, spec.implicit)
            .ok_or(bad)?;
        out.push(A {
            name: spec.name,
            form: spec.form,
            implicit: spec.implicit,
            pos,
            end: r.pos as u64,
            value,
        });
    }
    Ok((a, out))
}

/// Every DIE of `unit`, in order, with its parent and extent.
fn walk(unit: &Unit<'_>, abbrevs: &Abbrevs) -> R<Vec<Die>> {
    let bad = "a DIE that doesn't read";
    let data = unit.sections().info;
    let mut r = Reader::new(data, unit.dies as usize, unit.sections().endian);
    let mut dies: Vec<Die> = Vec::new();
    let mut stack: Vec<u32> = Vec::new();
    let end = unit.end as usize;
    while r.pos < end {
        let at = r.pos as u64;
        let code = r.uleb().ok_or(bad)?;
        if code == 0 {
            if let Some(p) = stack.pop() {
                dies[p as usize].end = r.pos as u64;
            }
            if stack.is_empty() {
                // (Padding after the unit's DIE's children.)
                if dies.is_empty() {
                    return Err(bad);
                }
            }
            continue;
        }
        let a = abbrevs.get(code).ok_or(bad)?;
        for spec in &a.attrs {
            unit.read_value(&mut r, spec.form, spec.implicit)
                .ok_or(bad)?;
        }
        let parent = stack.last().copied().unwrap_or(u32::MAX);
        if parent == u32::MAX && !dies.is_empty() {
            return Err("DIEs after the unit's DIE");
        }
        let i = dies.len() as u32;
        dies.push(Die {
            offset: at,
            parent,
            tag: a.tag,
            end: r.pos as u64,
        });
        if a.children {
            stack.push(i);
        }
    }
    // Unclosed DIEs end with the unit.
    for p in stack {
        dies[p as usize].end = end as u64;
    }
    Ok(dies)
}

fn is_scope(tag: u16) -> bool {
    matches!(
        tag,
        DW_TAG_COMPILE_UNIT | DW_TAG_PARTIAL_UNIT | DW_TAG_NAMESPACE | DW_TAG_MODULE
    )
}

/// The root of DIE `i`: the subprogram holding it, else its last ancestor
/// below a unit or namespace.
fn root_of(dies: &[Die], i: usize) -> usize {
    let mut at = i;
    loop {
        if dies[at].tag == DW_TAG_SUBPROGRAM {
            return at;
        }
        let p = dies[at].parent;
        if p == u32::MAX || is_scope(dies[p as usize].tag) {
            return at;
        }
        at = p as usize;
    }
}

/// The DIE at `offset` of a unit's DIEs.
fn die_at(dies: &[Die], offset: u64) -> Option<usize> {
    let i = dies.partition_point(|d| d.offset < offset);
    dies.get(i).filter(|d| d.offset == offset).map(|_| i)
}

/// Where a relocation of the output goes.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Target {
    /// The source member's section (moved into this member).
    Section(usize),
    /// A debug section of the output (its place in [`Out::sections`]).
    Debug(usize),
    /// The source member's symbol (a global, by name).
    Global(u32),
}

#[derive(Clone, Debug)]
pub(crate) struct OutRel {
    pub offset: u64,
    pub rtype: u32,
    pub target: Target,
    pub addend: i64,
}

/// A debug section of the output.
#[derive(Clone, Debug)]
pub(crate) struct OutSection {
    pub name: &'static [u8],
    pub strings: bool,
    pub bytes: Vec<u8>,
    pub rels: Vec<OutRel>,
}

/// The output's debug sections, and what building them counted.
#[derive(Clone, Debug, Default)]
pub(crate) struct Out {
    pub sections: Vec<OutSection>,
    /// Root DIEs copied, and addresses tombstoned in the copies.
    pub roots: usize,
    pub tombstoned: usize,
}

// The output sections' places.
const O_INFO: usize = 0;
const O_ABBREV: usize = 1;
const O_LINE: usize = 2;
const O_STR: usize = 3;
const O_LINE_STR: usize = 4;
const O_RNG: usize = 5;
const O_LOC: usize = 6;

/// A reference to fill in once DIEs have their offsets.
#[derive(Clone, Copy, Debug)]
enum Fix {
    /// A 4-byte offset from the unit's start.
    Ref4,
    /// A 4-byte ULEB128 (padded) offset from the unit's start.
    Uleb4,
    /// A 2-byte offset from the unit's start.
    Ref2,
    /// A 4-byte `.debug_info` offset, relocated.
    Info,
}

/// Bytes being built, with their relocations and references.
#[derive(Clone, Debug, Default)]
struct Piece {
    bytes: Vec<u8>,
    rels: Vec<OutRel>,
    /// (position, the source DIE, how).
    fixes: Vec<(usize, usize, Fix)>,
}

impl Piece {
    fn put(&mut self, v: u64, n: usize, endian: Endian) {
        let b = match endian {
            Endian::Little => v.to_le_bytes(),
            Endian::Big => v.to_be_bytes(),
        };
        match endian {
            Endian::Little => self.bytes.extend_from_slice(&b[..n]),
            Endian::Big => self.bytes.extend_from_slice(&b[8 - n..]),
        }
    }

    fn uleb(&mut self, v: u64) {
        super::put_uleb(&mut self.bytes, v);
    }

    fn append(&mut self, other: Piece) {
        let base = self.bytes.len();
        self.bytes.extend_from_slice(&other.bytes);
        self.rels.extend(other.rels.into_iter().map(|mut r| {
            r.offset += base as u64;
            r
        }));
        self.fixes
            .extend(other.fixes.into_iter().map(|(p, d, f)| (p + base, d, f)));
    }

    fn rel(&mut self, rtype: u32, target: Target, addend: i64) {
        self.rels.push(OutRel {
            offset: self.bytes.len() as u64,
            rtype,
            target,
            addend,
        });
    }
}

/// A string section being built.
#[derive(Default)]
struct Strings {
    bytes: Vec<u8>,
    at: HashMap<Vec<u8>, u64>,
}

impl Strings {
    fn intern(&mut self, s: &[u8]) -> u64 {
        if let Some(&o) = self.at.get(s) {
            return o;
        }
        let o = self.bytes.len() as u64;
        self.bytes.extend_from_slice(s);
        self.bytes.push(0);
        self.at.insert(s.to_vec(), o);
        o
    }
}

/// An abbreviation of the output.
#[derive(Clone, PartialEq, Eq, Hash)]
struct Shape {
    tag: u16,
    children: bool,
    attrs: Vec<(u16, u16, i64)>,
}

struct Builder<'s, 'p> {
    src: &'s Source<'p>,
    moved: &'s BTreeSet<usize>,
    out: Out,
    info: Piece,
    line: Piece,
    rng: Piece,
    loc: Piece,
    aranges: Piece,
    str: Strings,
    line_str: Strings,
    abbrevs: Vec<Shape>,
    codes: HashMap<Shape, u64>,
}

/// The DIEs of one unit to keep: 2 whole, 1 as a skeleton.
struct Keep {
    mark: Vec<u8>,
    work: Vec<usize>,
}

impl Keep {
    /// Keeps DIE `i` and its ancestors: as skeletons those that are scopes
    /// (units, namespaces, subprograms, blocks), whole the rest (a class
    /// whose nested type or member is kept is kept with all its members,
    /// so that its definition is the original's).
    fn skeleton(&mut self, dies: &[Die], i: usize) {
        let mut at = i;
        loop {
            if self.mark[at] != 0 {
                return;
            }
            if !is_scope(dies[at].tag)
                && !matches!(
                    dies[at].tag,
                    DW_TAG_SUBPROGRAM | DW_TAG_LEXICAL_BLOCK | DW_TAG_INLINED_SUBROUTINE
                )
            {
                self.whole(dies, at);
                return;
            }
            self.mark[at] = 1;
            self.work.push(at);
            let p = dies[at].parent;
            if p == u32::MAX {
                return;
            }
            at = p as usize;
        }
    }

    fn whole(&mut self, dies: &[Die], i: usize) {
        if self.mark[i] == 2 {
            return;
        }
        let end = dies[i].end;
        let mut j = i;
        while j < dies.len() && (j == i || dies[j].offset < end) {
            if self.mark[j] != 2 {
                if self.mark[j] == 0 {
                    self.work.push(j);
                }
                self.mark[j] = 2;
            }
            j += 1;
        }
        let p = dies[i].parent;
        if p != u32::MAX {
            self.skeleton(dies, p as usize);
        }
    }

    fn refer(&mut self, dies: &[Die], i: usize) {
        if is_scope(dies[i].tag) {
            self.skeleton(dies, i);
        } else {
            self.whole(dies, i);
        }
    }
}

fn is_location(name: u16) -> bool {
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
            | DW_AT_DATA_LOCATION
            | DW_AT_ALLOCATED
            | DW_AT_ASSOCIATED
            | DW_AT_UPPER_BOUND
            | DW_AT_LOWER_BOUND
            | DW_AT_COUNT
            | DW_AT_BYTE_SIZE
            | DW_AT_BIT_SIZE
    )
}

/// One entry of a location list, as read.
enum LocEntry<'d> {
    Bounded {
        start: Addr,
        len: u64,
        expr: &'d [u8],
        at: u64,
    },
    Default {
        expr: &'d [u8],
        at: u64,
    },
    /// GCC's inline view pair (`DW_LLE_GNU_view_pair`).
    View(u64, u64),
}

impl<'s, 'p> Builder<'s, 'p> {
    fn endian(&self) -> Endian {
        self.src.endian
    }

    /// Where an address of the source goes in the output: `None` to
    /// tombstone it.
    fn map_addr(&self, a: Addr) -> Option<(u32, Target, i64)> {
        let Addr::Sym { sym, addend, rtype } = a else {
            return None;
        };
        let s = self.src.symbols.get(sym as usize)?;
        if s.bind() != 0 {
            return Some((rtype, Target::Global(sym), addend));
        }
        if s.in_section() && self.moved.contains(&(s.shndx as usize)) {
            let value = if s.kind() == 3 { 0 } else { s.value as i64 };
            return Some((rtype, Target::Section(s.shndx as usize), addend + value));
        }
        None
    }

    /// Writes an address of `n` bytes.
    fn address(&mut self, p: &mut Piece, a: Addr, n: usize) {
        match (a, self.map_addr(a)) {
            (_, Some((rtype, t, addend))) => {
                p.rel(rtype, t, addend);
                let e = self.endian();
                p.put(0, n, e);
            }
            (Addr::Const(c), None) => {
                let e = self.endian();
                p.put(c, n, e);
            }
            (Addr::Sym { .. }, None) => {
                self.out.tombstoned += 1;
                let e = self.endian();
                p.put(0, n, e);
            }
        }
    }

    /// Copies `[start, end)` of section `k` (relocated bytes), its
    /// relocations mapped.
    fn copy(&mut self, k: Kind, start: u64, end: u64, p: &mut Piece) -> R<()> {
        let src = self.src;
        let data = src.sec(k);
        let bytes = data
            .get(start as usize..end as usize)
            .ok_or("a copy past its section")?;
        let base = p.bytes.len();
        p.bytes.extend_from_slice(bytes);
        for r in src.rels_in(k, start, end) {
            let n = if r.rtype == src.abs32 {
                4
            } else if r.rtype == src.abs64 {
                8
            } else {
                reloc_size_any(r.rtype)
            };
            let at = base + (r.offset - start) as usize;
            if at + n > p.bytes.len() {
                return Err("a relocation past its copy");
            }
            p.bytes[at..at + n].fill(0);
            let target_kind = src.sym_kind.get(r.sym as usize).copied().flatten();
            match target_kind {
                Some(Kind::Str | Kind::LineStr) => {
                    let sk = target_kind.unwrap_or(Kind::Str);
                    let off = src.symbols[r.sym as usize]
                        .value
                        .wrapping_add(r.addend as u64);
                    let s = Reader::new(src.sec(sk), off as usize, core_endian(src.endian))
                        .cstr()
                        .ok_or("a string past its section")?;
                    let (table, o) = if sk == Kind::Str {
                        (&mut self.str, O_STR)
                    } else {
                        (&mut self.line_str, O_LINE_STR)
                    };
                    let new = table.intern(s);
                    p.rels.push(OutRel {
                        offset: at as u64,
                        rtype: r.rtype,
                        target: Target::Debug(o),
                        addend: new as i64,
                    });
                }
                Some(_) => return Err("a copied relocation to another debug section"),
                None => {
                    let a = Addr::Sym {
                        sym: r.sym,
                        addend: r.addend,
                        rtype: r.rtype,
                    };
                    match self.map_addr(a) {
                        Some((rtype, t, addend)) => p.rels.push(OutRel {
                            offset: at as u64,
                            rtype,
                            target: t,
                            addend,
                        }),
                        None => self.out.tombstoned += 1,
                    }
                }
            }
        }
        Ok(())
    }

    /// Rewrites an expression of unit `cu` that lies at `at` of section
    /// `k` (see the module's documentation).
    fn expression(&mut self, cu: &Cu, k: Kind, expr: &[u8], at: u64) -> R<Piece> {
        let list = ops::decode(
            expr,
            core_endian(self.endian()),
            cu.address_size,
            4,
            cu.version,
        )
        .map_err(|_| "an expression that doesn't decode")?;
        let n = usize::from(cu.address_size);
        let mut p = Piece::default();
        let mut starts: Vec<(usize, usize)> = Vec::with_capacity(list.len() + 1);
        let mut branches: Vec<(usize, usize, i16)> = Vec::new();
        for op in &list {
            starts.push((op.start, p.bytes.len()));
            match op.code {
                DW_OP_ADDR => {
                    p.bytes.push(DW_OP_ADDR);
                    let a = self.src.addr_at(k, at + op.start as u64 + 1, n)?;
                    self.address(&mut p, a, n);
                }
                DW_OP_ADDRX | DW_OP_GNU_ADDR_INDEX | DW_OP_CONSTX | DW_OP_GNU_CONST_INDEX => {
                    let Some(Operand::AddressIndex(i)) = op.operands.first().copied() else {
                        return Err("a malformed address index");
                    };
                    let a = self.src.indexed(cu, i)?;
                    let is_addr = matches!(op.code, DW_OP_ADDRX | DW_OP_GNU_ADDR_INDEX);
                    p.bytes.push(if is_addr {
                        DW_OP_ADDR
                    } else if n == 8 {
                        DW_OP_CONST8U
                    } else {
                        DW_OP_CONST4U
                    });
                    self.address(&mut p, a, n);
                }
                DW_OP_CALL_REF
                | DW_OP_GNU_VARIABLE_VALUE
                | DW_OP_IMPLICIT_POINTER
                | DW_OP_GNU_IMPLICIT_POINTER => {
                    let Some(Operand::InfoOffset { at: f, value }) = op.operands.first().copied()
                    else {
                        return Err("a malformed DIE reference");
                    };
                    if cu.version <= 2 {
                        return Err("DWARF 2's address-sized DIE references");
                    }
                    let d = die_at(&cu.dies, value).ok_or("an expression naming another unit")?;
                    p.bytes.push(op.code);
                    p.fixes.push((p.bytes.len(), d, Fix::Info));
                    p.bytes.extend_from_slice(&[0; 4]);
                    p.bytes.extend_from_slice(&expr[f + 4..op.end]);
                }
                DW_OP_ENTRY_VALUE | DW_OP_GNU_ENTRY_VALUE => {
                    let Some(Operand::Expression(inner)) = op.operands.first().copied() else {
                        return Err("a malformed entry value");
                    };
                    let inner_at = self.src.offset_in(k, inner);
                    let q = self.expression(cu, k, inner, inner_at)?;
                    p.bytes.push(op.code);
                    p.uleb(q.bytes.len() as u64);
                    p.append(q);
                }
                DW_OP_BRA | DW_OP_SKIP => {
                    let Some(Operand::Branch(off)) = op.operands.first().copied() else {
                        return Err("a malformed branch");
                    };
                    p.bytes.push(op.code);
                    branches.push((p.bytes.len(), op.end, off));
                    p.bytes.extend_from_slice(&[0; 2]);
                }
                _ => {
                    let refs: Vec<(u64, Option<u8>)> = op
                        .operands
                        .iter()
                        .filter_map(|o| match *o {
                            Operand::UnitDie { value, size } => Some((value, size)),
                            _ => None,
                        })
                        .collect();
                    if refs.is_empty() {
                        let mut q = Piece::default();
                        self.copy(k, at + op.start as u64, at + op.end as u64, &mut q)?;
                        p.append(q);
                        continue;
                    }
                    // Re-encode, the unit-relative reference fixed later.
                    p.bytes.push(op.code);
                    for o in &op.operands {
                        match *o {
                            Operand::UnitDie { value, size } => {
                                if value == 0 && size.is_none() {
                                    p.uleb(0);
                                    continue;
                                }
                                let d = die_at(&cu.dies, cu.offset + value)
                                    .ok_or("an expression naming a DIE not there")?;
                                let fix = match size {
                                    None => Fix::Uleb4,
                                    Some(2) => Fix::Ref2,
                                    Some(_) => Fix::Ref4,
                                };
                                p.fixes.push((p.bytes.len(), d, fix));
                                let w = if matches!(fix, Fix::Ref2) { 2 } else { 4 };
                                p.bytes.extend(std::iter::repeat_n(0u8, w));
                            }
                            Operand::Unsigned(v) => match op.code {
                                DW_OP_REGVAL_TYPE | DW_OP_GNU_REGVAL_TYPE => p.uleb(v),
                                _ => p.bytes.push(v as u8),
                            },
                            Operand::Bytes(b) => p.bytes.extend_from_slice(b),
                            _ => return Err("an operand not known"),
                        }
                    }
                }
            }
        }
        starts.push((expr.len(), p.bytes.len()));
        for (pos, old_end, off) in branches {
            let target = (old_end as i64 + i64::from(off)) as usize;
            let new_target = starts
                .iter()
                .find(|&&(o, _)| o == target)
                .map(|&(_, n)| n)
                .ok_or("a branch into an operation")?;
            let new_end = pos + 2;
            let d = i16::try_from(new_target as i64 - new_end as i64)
                .map_err(|_| "a branch too far")?;
            let b = match self.endian() {
                Endian::Little => d.to_le_bytes(),
                Endian::Big => d.to_be_bytes(),
            };
            p.bytes[pos..pos + 2].copy_from_slice(&b);
        }
        Ok(p)
    }

    /// The DIEs an expression refers to.
    fn expression_refs(&self, cu: &Cu, expr: &[u8], out: &mut Vec<usize>) -> R<()> {
        let list = ops::decode(
            expr,
            core_endian(self.endian()),
            cu.address_size,
            4,
            cu.version,
        )
        .map_err(|_| "an expression that doesn't decode")?;
        for op in list {
            for o in &op.operands {
                match *o {
                    Operand::InfoOffset { value, .. } => {
                        out.push(
                            die_at(&cu.dies, value).ok_or("an expression naming another unit")?,
                        );
                    }
                    Operand::UnitDie { value, .. } if value != 0 => {
                        out.push(
                            die_at(&cu.dies, cu.offset + value)
                                .ok_or("an expression naming a DIE not there")?,
                        );
                    }
                    Operand::Expression(inner) => self.expression_refs(cu, inner, out)?,
                    _ => {}
                }
            }
        }
        Ok(())
    }

    /// The entries of the location list at `off`.
    fn location_list(&self, cu: &Cu, off: u64) -> R<Vec<LocEntry<'s>>> {
        let src = self.src;
        let n = usize::from(cu.address_size);
        let bad = "a location list that doesn't read";
        let mut out = Vec::new();
        let mut base = cu.low_pc;
        if cu.version >= 5 {
            let k = Kind::Loclists;
            let data = src.sec(k);
            let mut r = Reader::new(data, off as usize, core_endian(src.endian));
            let expr = |r: &mut Reader<'s>| -> R<(&'s [u8], u64)> {
                let len = r.uleb().ok_or(bad)?;
                let at = r.pos as u64;
                Ok((r.bytes(len).ok_or(bad)?, at))
            };
            loop {
                match r.u8().ok_or(bad)? {
                    DW_LLE_END_OF_LIST => break,
                    DW_LLE_BASE_ADDRESSX => base = src.indexed(cu, r.uleb().ok_or(bad)?)?,
                    DW_LLE_STARTX_ENDX => {
                        let s = src.indexed(cu, r.uleb().ok_or(bad)?)?;
                        let e = src.indexed(cu, r.uleb().ok_or(bad)?)?;
                        let (x, at) = expr(&mut r)?;
                        out.push(LocEntry::Bounded {
                            start: s,
                            len: s.distance(e).ok_or(bad)?,
                            expr: x,
                            at,
                        });
                    }
                    DW_LLE_STARTX_LENGTH => {
                        let s = src.indexed(cu, r.uleb().ok_or(bad)?)?;
                        let len = r.uleb().ok_or(bad)?;
                        let (x, at) = expr(&mut r)?;
                        out.push(LocEntry::Bounded {
                            start: s,
                            len,
                            expr: x,
                            at,
                        });
                    }
                    DW_LLE_OFFSET_PAIR => {
                        let a = r.uleb().ok_or(bad)?;
                        let b = r.uleb().ok_or(bad)?;
                        let (x, at) = expr(&mut r)?;
                        out.push(LocEntry::Bounded {
                            start: base.plus(a),
                            len: b.wrapping_sub(a),
                            expr: x,
                            at,
                        });
                    }
                    DW_LLE_DEFAULT_LOCATION => {
                        let (x, at) = expr(&mut r)?;
                        out.push(LocEntry::Default { expr: x, at });
                    }
                    DW_LLE_BASE_ADDRESS => {
                        base = src.addr_at(k, r.pos as u64, n)?;
                        r.uint(n).ok_or(bad)?;
                    }
                    DW_LLE_START_END => {
                        let s = src.addr_at(k, r.pos as u64, n)?;
                        r.uint(n).ok_or(bad)?;
                        let e = src.addr_at(k, r.pos as u64, n)?;
                        r.uint(n).ok_or(bad)?;
                        let (x, at) = expr(&mut r)?;
                        out.push(LocEntry::Bounded {
                            start: s,
                            len: s.distance(e).ok_or(bad)?,
                            expr: x,
                            at,
                        });
                    }
                    DW_LLE_START_LENGTH => {
                        let s = src.addr_at(k, r.pos as u64, n)?;
                        r.uint(n).ok_or(bad)?;
                        let len = r.uleb().ok_or(bad)?;
                        let (x, at) = expr(&mut r)?;
                        out.push(LocEntry::Bounded {
                            start: s,
                            len,
                            expr: x,
                            at,
                        });
                    }
                    DW_LLE_GNU_VIEW_PAIR => {
                        let a = r.uleb().ok_or(bad)?;
                        let b = r.uleb().ok_or(bad)?;
                        out.push(LocEntry::View(a, b));
                    }
                    _ => return Err(bad),
                }
            }
        } else {
            let k = Kind::Loc;
            let data = src.sec(k);
            let mut r = Reader::new(data, off as usize, core_endian(src.endian));
            let max = if n == 8 {
                u64::MAX
            } else {
                u64::from(u32::MAX)
            };
            loop {
                let pa = r.pos as u64;
                let a = r.uint(n).ok_or(bad)?;
                let pb = r.pos as u64;
                let b = r.uint(n).ok_or(bad)?;
                let (sa, sb) = (src.addr_at(k, pa, n)?, src.addr_at(k, pb, n)?);
                if a == 0 && b == 0 && sa == Addr::Const(0) && sb == Addr::Const(0) {
                    break;
                }
                if a == max && sa == Addr::Const(max) {
                    base = sb;
                    continue;
                }
                let len = r.uint(2).ok_or(bad)?;
                let at = r.pos as u64;
                let x = r.bytes(len).ok_or(bad)?;
                let s = base.add(sa).ok_or(bad)?;
                let e = base.add(sb).ok_or(bad)?;
                out.push(LocEntry::Bounded {
                    start: s,
                    len: s.distance(e).unwrap_or(0),
                    expr: x,
                    at,
                });
            }
        }
        Ok(out)
    }

    /// Writes a range list into the output's; its offset there.
    fn write_ranges(&mut self, cu: &Cu, entries: &[(Addr, u64)]) -> u64 {
        let n = usize::from(cu.address_size);
        let e = self.endian();
        let mut p = Piece::default();
        let at = self.rng.bytes.len() as u64;
        for &(s, len) in entries {
            let Some((rtype, t, addend)) = self.map_addr(s) else {
                if let Addr::Sym { .. } = s {
                    self.out.tombstoned += 1;
                    continue;
                }
                // A constant range: as it was.
                if cu.version >= 5 {
                    p.bytes.push(DW_RLE_START_LENGTH);
                    if let Addr::Const(c) = s {
                        p.put(c, n, e);
                    }
                    p.uleb(len);
                } else if let Addr::Const(c) = s {
                    p.put(c, n, e);
                    p.put(c.wrapping_add(len), n, e);
                }
                continue;
            };
            if cu.version >= 5 {
                p.bytes.push(DW_RLE_START_LENGTH);
                p.rel(rtype, t, addend);
                p.put(0, n, e);
                p.uleb(len);
            } else {
                p.rel(rtype, t.clone(), addend);
                p.put(0, n, e);
                p.rel(rtype, t, addend.wrapping_add(len as i64));
                p.put(0, n, e);
            }
        }
        if cu.version >= 5 {
            p.bytes.push(DW_RLE_END_OF_LIST);
        } else {
            p.put(0, n, e);
            p.put(0, n, e);
        }
        self.rng.append(p);
        at
    }

    /// Writes a location list (and the view pairs `views` gives, before it);
    /// the offsets of the list and of the views.
    fn write_locations(
        &mut self,
        cu: &Cu,
        entries: &[LocEntry<'s>],
        views: Option<u64>,
    ) -> R<(u64, Option<u64>)> {
        let n = usize::from(cu.address_size);
        let e = self.endian();
        let (lk, bounded) = (
            if cu.version >= 5 {
                Kind::Loclists
            } else {
                Kind::Loc
            },
            entries
                .iter()
                .filter(|x| matches!(x, LocEntry::Bounded { .. }))
                .count(),
        );
        let mut p = Piece::default();
        let views_at = match views {
            Some(v) => {
                let at = self.loc.bytes.len() as u64;
                let data = self.src.sec(lk);
                let mut r = Reader::new(data, v as usize, core_endian(self.src.endian));
                for _ in 0..bounded {
                    let a = r.uleb().ok_or("view pairs that don't read")?;
                    let b = r.uleb().ok_or("view pairs that don't read")?;
                    p.uleb(a);
                    p.uleb(b);
                }
                Some(at)
            }
            None => None,
        };
        let list_at = self.loc.bytes.len() as u64 + p.bytes.len() as u64;
        for x in entries {
            match *x {
                LocEntry::View(a, b) => {
                    p.bytes.push(DW_LLE_GNU_VIEW_PAIR);
                    p.uleb(a);
                    p.uleb(b);
                }
                LocEntry::Default { expr, at } => {
                    let q = self.expression(cu, lk, expr, at)?;
                    p.bytes.push(DW_LLE_DEFAULT_LOCATION);
                    p.uleb(q.bytes.len() as u64);
                    p.append(q);
                }
                LocEntry::Bounded {
                    start,
                    len,
                    expr,
                    at,
                } => {
                    let q = self.expression(cu, lk, expr, at)?;
                    let mapped = self.map_addr(start);
                    if mapped.is_none() && matches!(start, Addr::Sym { .. }) {
                        self.out.tombstoned += 1;
                    }
                    if cu.version >= 5 {
                        p.bytes.push(DW_LLE_START_LENGTH);
                        match (mapped, start) {
                            (Some((rtype, t, addend)), _) => {
                                p.rel(rtype, t, addend);
                                p.put(0, n, e);
                                p.uleb(len);
                            }
                            (None, Addr::Const(c)) => {
                                p.put(c, n, e);
                                p.uleb(len);
                            }
                            (None, _) => {
                                p.put(0, n, e);
                                p.uleb(0);
                            }
                        }
                        p.uleb(q.bytes.len() as u64);
                    } else {
                        match (mapped, start) {
                            (Some((rtype, t, addend)), _) => {
                                p.rel(rtype, t.clone(), addend);
                                p.put(0, n, e);
                                p.rel(rtype, t, addend.wrapping_add(len as i64));
                                p.put(0, n, e);
                            }
                            (None, Addr::Const(c)) => {
                                p.put(c, n, e);
                                p.put(c.wrapping_add(len), n, e);
                            }
                            (None, _) => {
                                // lld's tombstone for .debug_loc: an empty
                                // range that doesn't end the list.
                                p.put(1, n, e);
                                p.put(1, n, e);
                            }
                        }
                        p.put(q.bytes.len() as u64, 2, e);
                    }
                    p.append(q);
                }
            }
        }
        if cu.version >= 5 {
            p.bytes.push(DW_LLE_END_OF_LIST);
        } else {
            p.put(0, n, e);
            p.put(0, n, e);
        }
        self.loc.append(p);
        Ok((list_at, views_at))
    }

    fn abbrev(&mut self, shape: Shape) -> u64 {
        if let Some(&c) = self.codes.get(&shape) {
            return c;
        }
        let c = self.abbrevs.len() as u64 + 1;
        self.abbrevs.push(shape.clone());
        self.codes.insert(shape, c);
        c
    }

    /// Writes unit `cu` of the source, with the roots `roots`, into the
    /// output.
    fn unit(&mut self, cu: &Cu, roots: &[usize]) -> R<()> {
        let src = self.src;
        let unit = src.unit(cu)?;
        let dies = &cu.dies;
        // What to keep.
        let mut keep = Keep {
            mark: vec![0; dies.len()],
            work: Vec::new(),
        };
        for &r in roots {
            keep.whole(dies, r);
        }
        keep.skeleton(dies, 0);
        let mut refs = Vec::new();
        while let Some(i) = keep.work.pop() {
            let (_, attrs) = read_die(&unit, &cu.abbrevs, dies[i].offset)?;
            refs.clear();
            for a in &attrs {
                if a.name == DW_AT_SIBLING {
                    continue;
                }
                match a.form {
                    DW_FORM_REF1 | DW_FORM_REF2 | DW_FORM_REF4 | DW_FORM_REF8
                    | DW_FORM_REF_UDATA | DW_FORM_REF_ADDR => {
                        let at = unit
                            .reference(&Attr {
                                name: a.name,
                                form: a.form,
                                value: a.value,
                            })
                            .ok_or("a bad reference")?;
                        refs.push(die_at(dies, at).ok_or("a reference outside its unit")?);
                    }
                    DW_FORM_EXPRLOC => {
                        if let Value::Block(b) = a.value {
                            self.expression_refs(cu, b, &mut refs)?;
                        }
                    }
                    DW_FORM_SEC_OFFSET | DW_FORM_LOCLISTX if is_location(a.name) && i != 0 => {
                        let off = src.list_offset(cu, &unit, a, false)?;
                        for x in self.location_list(cu, off)? {
                            if let LocEntry::Bounded { expr, .. } | LocEntry::Default { expr, .. } =
                                x
                            {
                                self.expression_refs(cu, expr, &mut refs)?;
                            }
                        }
                    }
                    _ => {}
                }
            }
            for &t in &refs {
                keep.refer(dies, t);
            }
        }
        // Each kept DIE's kept children.
        let mut children: Vec<Vec<usize>> = vec![Vec::new(); dies.len()];
        for (i, d) in dies.iter().enumerate() {
            if keep.mark[i] != 0 && d.parent != u32::MAX {
                children[d.parent as usize].push(i);
            }
        }
        // The unit.
        let e = self.endian();
        let start = self.info.bytes.len();
        let mut p = Piece::default();
        p.put(0, 4, e); // the length, later
        p.put(u64::from(cu.version), 2, e);
        if cu.version >= 5 {
            p.bytes.push(DW_UT_COMPILE);
            p.bytes.push(cu.address_size);
            p.rel(src.abs32, Target::Debug(O_ABBREV), 0);
            p.put(0, 4, e);
        } else {
            p.rel(src.abs32, Target::Debug(O_ABBREV), 0);
            p.put(0, 4, e);
            p.bytes.push(cu.address_size);
        }
        let mut offsets: Vec<u64> = vec![u64::MAX; dies.len()];
        offsets[0] = p.bytes.len() as u64;
        let q = self.top_die(cu, &unit, !children[0].is_empty())?;
        p.append(q);
        // Pre-order: (DIE, next child to write).
        let mut stack: Vec<(usize, usize)> = vec![(0, 0)];
        while let Some(top) = stack.last_mut() {
            let (d, next) = *top;
            if next < children[d].len() {
                let c = children[d][next];
                top.1 += 1;
                offsets[c] = p.bytes.len() as u64;
                let q = self.die(cu, &unit, c, !children[c].is_empty())?;
                p.append(q);
                if !children[c].is_empty() {
                    stack.push((c, 0));
                }
            } else {
                if !children[d].is_empty() {
                    p.bytes.push(0);
                }
                stack.pop();
            }
        }
        let len = p.bytes.len() - 4;
        let lb = match e {
            Endian::Little => (len as u32).to_le_bytes(),
            Endian::Big => (len as u32).to_be_bytes(),
        };
        p.bytes[..4].copy_from_slice(&lb);
        // References.
        let fixes = std::mem::take(&mut p.fixes);
        for (pos, d, fix) in fixes {
            let o = offsets[d];
            if o == u64::MAX {
                return Err("a reference to a DIE not kept");
            }
            write_fix(&mut p, pos, o, start as u64, fix, e, src.abs32)?;
        }
        let fixes = std::mem::take(&mut self.loc.fixes);
        for (pos, d, fix) in fixes {
            let o = offsets[d];
            if o == u64::MAX {
                return Err("a reference to a DIE not kept");
            }
            write_fix(&mut self.loc, pos, o, start as u64, fix, e, src.abs32)?;
        }
        self.info.append(p);
        // The aranges.
        if src.secs[Kind::Aranges as usize].is_some() {
            let mut a = Piece::default();
            let n = usize::from(cu.address_size);
            a.put(0, 4, e);
            a.put(2, 2, e);
            a.rel(src.abs32, Target::Debug(O_INFO), start as i64);
            a.put(0, 4, e);
            a.bytes.push(cu.address_size);
            a.bytes.push(0);
            while a.bytes.len() % (2 * n) != 0 {
                a.bytes.push(0);
            }
            for &s in &self.code_sections() {
                a.rel(src.abs64, Target::Section(s), 0);
                a.put(0, n, e);
                a.put(src.sections[s].1, n, e);
            }
            a.put(0, n, e);
            a.put(0, n, e);
            let len = a.bytes.len() - 4;
            let lb = match e {
                Endian::Little => (len as u32).to_le_bytes(),
                Endian::Big => (len as u32).to_be_bytes(),
            };
            a.bytes[..4].copy_from_slice(&lb);
            self.aranges.append(a);
        }
        self.out.roots += roots.len();
        Ok(())
    }

    /// The moved sections that hold code, in order.
    fn code_sections(&self) -> Vec<usize> {
        self.moved
            .iter()
            .copied()
            .filter(|&s| {
                let (flags, size) = self.src.sections[s];
                flags & shf::EXECINSTR != 0 && flags & shf::ALLOC != 0 && size > 0
            })
            .collect()
    }

    /// The unit's DIE (see the module's documentation).
    fn top_die(&mut self, cu: &Cu, unit: &Unit<'s>, children: bool) -> R<Piece> {
        let (abbrev, attrs) = read_die(unit, &cu.abbrevs, cu.dies[0].offset)?;
        let e = self.endian();
        let n = usize::from(cu.address_size);
        let mut p = Piece::default();
        let mut shape = Shape {
            tag: abbrev.tag,
            children,
            attrs: Vec::new(),
        };
        let mut body = Piece::default();
        let mut stmt = false;
        for a in &attrs {
            match a.name {
                DW_AT_LOW_PC
                | DW_AT_HIGH_PC
                | DW_AT_RANGES
                | DW_AT_STR_OFFSETS_BASE
                | DW_AT_ADDR_BASE
                | DW_AT_GNU_ADDR_BASE
                | DW_AT_RNGLISTS_BASE
                | DW_AT_GNU_RANGES_BASE
                | DW_AT_LOCLISTS_BASE
                | DW_AT_MACROS
                | DW_AT_GNU_MACROS
                | DW_AT_MACRO_INFO
                | DW_AT_SIBLING
                | DW_AT_GNU_PUBNAMES
                | DW_AT_GNU_PUBTYPES => {}
                DW_AT_STMT_LIST => stmt = true,
                _ => self.attribute(cu, unit, 0, a, &attrs, &mut shape, &mut body)?,
            }
        }
        // The new unit's code.
        shape.attrs.push((DW_AT_LOW_PC, DW_FORM_ADDR, 0));
        body.put(0, n, e);
        let code = self.code_sections();
        let at = self.write_section_ranges(cu, &code);
        shape.attrs.push((DW_AT_RANGES, DW_FORM_SEC_OFFSET, 0));
        body.rel(self.src.abs32, Target::Debug(O_RNG), at as i64);
        body.put(0, 4, e);
        if stmt && let Some(line) = cu.line {
            let at = self.line_table(cu, line)?;
            shape.attrs.push((DW_AT_STMT_LIST, DW_FORM_SEC_OFFSET, 0));
            body.rel(self.src.abs32, Target::Debug(O_LINE), at as i64);
            body.put(0, 4, e);
        }
        let code = self.abbrev(shape);
        p.uleb(code);
        p.append(body);
        Ok(p)
    }

    /// Writes the unit's ranges (the moved sections, whole): its offset.
    fn write_section_ranges(&mut self, cu: &Cu, sections: &[usize]) -> u64 {
        let n = usize::from(cu.address_size);
        let e = self.endian();
        let at = self.rng.bytes.len() as u64;
        let mut p = Piece::default();
        let rtype = self.src.abs64;
        for &s in sections {
            let len = self.src.sections[s].1;
            let t = Target::Section(s);
            if cu.version >= 5 {
                p.bytes.push(DW_RLE_START_LENGTH);
                p.rel(rtype, t, 0);
                p.put(0, n, e);
                p.uleb(len);
            } else {
                p.rel(rtype, t.clone(), 0);
                p.put(0, n, e);
                p.rel(rtype, t, len as i64);
                p.put(0, n, e);
            }
        }
        if cu.version >= 5 {
            p.bytes.push(DW_RLE_END_OF_LIST);
        } else {
            p.put(0, n, e);
            p.put(0, n, e);
        }
        self.rng.append(p);
        at
    }

    /// Writes the unit's line table: the header, then the moved sections'
    /// sequences. Its offset.
    fn line_table(&mut self, cu: &Cu, (start, program, _end): (u64, u64, u64)) -> R<u64> {
        let at = self.line.bytes.len() as u64;
        let mut p = Piece::default();
        self.copy(Kind::Line, start, program, &mut p)?;
        for s in &cu.seqs {
            if s.section.is_some_and(|x| self.moved.contains(&x)) {
                self.copy(Kind::Line, s.start, s.end, &mut p)?;
            }
        }
        let len = p.bytes.len() - 4;
        let e = self.endian();
        let lb = match e {
            Endian::Little => (len as u32).to_le_bytes(),
            Endian::Big => (len as u32).to_be_bytes(),
        };
        p.bytes[..4].copy_from_slice(&lb);
        self.line.append(p);
        Ok(at)
    }

    /// A DIE other than the unit's.
    fn die(&mut self, cu: &Cu, unit: &Unit<'s>, i: usize, children: bool) -> R<Piece> {
        let (abbrev, attrs) = read_die(unit, &cu.abbrevs, cu.dies[i].offset)?;
        let mut shape = Shape {
            tag: abbrev.tag,
            children,
            attrs: Vec::new(),
        };
        let mut body = Piece::default();
        for a in &attrs {
            self.attribute(cu, unit, i, a, &attrs, &mut shape, &mut body)?;
        }
        let mut p = Piece::default();
        let code = self.abbrev(shape);
        p.uleb(code);
        p.append(body);
        Ok(p)
    }

    /// Writes one attribute of DIE `i` (whose attributes are `all`).
    #[allow(clippy::too_many_arguments)]
    fn attribute(
        &mut self,
        cu: &Cu,
        unit: &Unit<'s>,
        i: usize,
        a: &A<'s>,
        all: &[A<'s>],
        shape: &mut Shape,
        p: &mut Piece,
    ) -> R<()> {
        let e = self.endian();
        let n = usize::from(cu.address_size);
        let src = self.src;
        if a.name == DW_AT_SIBLING {
            return Ok(());
        }
        match a.form {
            DW_FORM_DATA1
            | DW_FORM_DATA2
            | DW_FORM_DATA4
            | DW_FORM_DATA8
            | DW_FORM_DATA16
            | DW_FORM_FLAG
            | DW_FORM_FLAG_PRESENT
            | DW_FORM_SDATA
            | DW_FORM_UDATA
            | DW_FORM_STRING
            | DW_FORM_REF_SIG8
            | DW_FORM_IMPLICIT_CONST
            | DW_FORM_BLOCK1
            | DW_FORM_BLOCK2
            | DW_FORM_BLOCK4
            | DW_FORM_BLOCK => {
                shape.attrs.push((a.name, a.form, a.implicit));
                self.copy(Kind::Info, a.pos, a.end, p)?;
            }
            DW_FORM_ADDR
            | DW_FORM_ADDRX
            | DW_FORM_ADDRX1
            | DW_FORM_ADDRX2
            | DW_FORM_ADDRX3
            | DW_FORM_ADDRX4
            | DW_FORM_GNU_ADDR_INDEX => {
                let addr = src.attr_addr(cu, a)?;
                shape.attrs.push((a.name, DW_FORM_ADDR, 0));
                self.address(p, addr, n);
            }
            DW_FORM_STRP
            | DW_FORM_STRX
            | DW_FORM_STRX1
            | DW_FORM_STRX2
            | DW_FORM_STRX3
            | DW_FORM_STRX4
            | DW_FORM_GNU_STR_INDEX
            | DW_FORM_LINE_STRP => {
                let s = unit.string(&a.value).ok_or("a string that doesn't read")?;
                let line = a.form == DW_FORM_LINE_STRP;
                let (o, sec, form) = if line {
                    (self.line_str.intern(s.bytes), O_LINE_STR, DW_FORM_LINE_STRP)
                } else {
                    (self.str.intern(s.bytes), O_STR, DW_FORM_STRP)
                };
                shape.attrs.push((a.name, form, 0));
                p.rel(src.abs32, Target::Debug(sec), o as i64);
                p.put(0, 4, e);
            }
            DW_FORM_REF1 | DW_FORM_REF2 | DW_FORM_REF4 | DW_FORM_REF8 | DW_FORM_REF_UDATA
            | DW_FORM_REF_ADDR => {
                let at = unit
                    .reference(&Attr {
                        name: a.name,
                        form: a.form,
                        value: a.value,
                    })
                    .ok_or("a bad reference")?;
                let d = die_at(&cu.dies, at).ok_or("a reference outside its unit")?;
                shape.attrs.push((a.name, DW_FORM_REF4, 0));
                p.fixes.push((p.bytes.len(), d, Fix::Ref4));
                p.put(0, 4, e);
            }
            DW_FORM_EXPRLOC => {
                let Value::Block(b) = a.value else {
                    return Err("a malformed expression");
                };
                let at = src.offset_in(Kind::Info, b);
                let q = self.expression(cu, Kind::Info, b, at)?;
                shape.attrs.push((a.name, DW_FORM_EXPRLOC, 0));
                p.uleb(q.bytes.len() as u64);
                p.append(q);
            }
            DW_FORM_SEC_OFFSET | DW_FORM_LOCLISTX | DW_FORM_RNGLISTX => {
                if a.name == DW_AT_RANGES || a.name == DW_AT_START_SCOPE {
                    let off = src.list_offset(cu, unit, a, true)?;
                    let entries = src.range_list(cu, off)?;
                    let at = self.write_ranges(cu, &entries);
                    shape.attrs.push((a.name, DW_FORM_SEC_OFFSET, 0));
                    p.rel(src.abs32, Target::Debug(O_RNG), at as i64);
                    p.put(0, 4, e);
                } else if is_location(a.name) {
                    let off = src.list_offset(cu, unit, a, false)?;
                    let entries = self.location_list(cu, off)?;
                    let views = all
                        .iter()
                        .find(|v| v.name == DW_AT_GNU_LOCVIEWS && a.name == DW_AT_LOCATION)
                        .map(|v| match v.value {
                            Value::Uint(x) => Ok(x),
                            _ => Err("a malformed DW_AT_GNU_locviews"),
                        })
                        .transpose()?;
                    let (at, views_at) = self.write_locations(cu, &entries, views)?;
                    shape.attrs.push((a.name, DW_FORM_SEC_OFFSET, 0));
                    p.rel(src.abs32, Target::Debug(O_LOC), at as i64);
                    p.put(0, 4, e);
                    if let Some(v) = views_at {
                        shape
                            .attrs
                            .push((DW_AT_GNU_LOCVIEWS, DW_FORM_SEC_OFFSET, 0));
                        p.rel(src.abs32, Target::Debug(O_LOC), v as i64);
                        p.put(0, 4, e);
                    }
                } else if a.name == DW_AT_GNU_LOCVIEWS {
                    // Written with its DW_AT_location; alone, it names
                    // nothing that is copied.
                    if !all.iter().any(|x| {
                        x.name == DW_AT_LOCATION
                            && matches!(x.form, DW_FORM_SEC_OFFSET | DW_FORM_LOCLISTX)
                    }) {
                        return Err("DW_AT_GNU_locviews without a location list");
                    }
                } else if i == 0 {
                    // (The unit's own are handled by the caller.)
                } else {
                    return Err("a section offset of an attribute not known");
                }
            }
            _ => return Err("an attribute of a form not known"),
        }
        Ok(())
    }
}

fn reloc_size_any(rtype: u32) -> usize {
    // R_X86_64_DTPOFF32, _32S; R_AARCH64_ABS32: 4; the rest are 8.
    match rtype {
        10 | 11 | 21 | 258 => 4,
        259 => 2,
        _ => 8,
    }
}

fn write_fix(
    p: &mut Piece,
    pos: usize,
    o: u64,
    unit_start: u64,
    fix: Fix,
    e: Endian,
    abs32: u32,
) -> R<()> {
    let put = |p: &mut Piece, v: u64, n: usize| {
        let b = match e {
            Endian::Little => v.to_le_bytes(),
            Endian::Big => v.to_be_bytes(),
        };
        let s = match e {
            Endian::Little => &b[..n],
            Endian::Big => &b[8 - n..],
        };
        p.bytes[pos..pos + n].copy_from_slice(s);
    };
    match fix {
        Fix::Ref4 => put(p, o, 4),
        Fix::Ref2 => {
            if o > 0xFFFF {
                return Err("a 2-byte reference too far");
            }
            put(p, o, 2);
        }
        Fix::Uleb4 => {
            if o >= 1 << 28 {
                return Err("a unit too large");
            }
            for k in 0..4 {
                let mut b = ((o >> (7 * k)) & 0x7F) as u8;
                if k < 3 {
                    b |= 0x80;
                }
                p.bytes[pos + k] = b;
            }
        }
        Fix::Info => {
            p.rels.push(OutRel {
                offset: pos as u64,
                rtype: abs32,
                target: Target::Debug(O_INFO),
                addend: (unit_start + o) as i64,
            });
        }
    }
    Ok(())
}

/// Builds the debug sections of a synthesized member holding `moved` (the
/// source member's sections). `None` when they describe nothing.
pub(crate) fn build(src: &Source<'_>, moved: &BTreeSet<usize>) -> R<Option<Out>> {
    // The roots, by unit.
    let mut by_unit: BTreeMap<usize, BTreeSet<usize>> = BTreeMap::new();
    for s in moved {
        for &(cu, d) in src.roots.get(s).map_or(&[][..], Vec::as_slice) {
            by_unit.entry(cu).or_default().insert(d);
        }
    }
    let has_lines = src.cus.iter().any(|cu| {
        cu.seqs
            .iter()
            .any(|q| q.section.is_some_and(|x| moved.contains(&x)))
    });
    if by_unit.is_empty() && !has_lines {
        return Ok(None);
    }
    if by_unit.len() > 1 {
        return Err("units from more than one compile unit");
    }
    let cu_index = match by_unit.keys().next() {
        Some(&c) => c,
        None => src
            .cus
            .iter()
            .position(|cu| {
                cu.seqs
                    .iter()
                    .any(|q| q.section.is_some_and(|x| moved.contains(&x)))
            })
            .unwrap_or(0),
    };
    let cu = src.cus.get(cu_index).ok_or("no unit")?;
    let roots: Vec<usize> = by_unit
        .get(&cu_index)
        .map(|s| s.iter().copied().collect())
        .unwrap_or_default();
    let mut b = Builder {
        src,
        moved,
        out: Out::default(),
        info: Piece::default(),
        line: Piece::default(),
        rng: Piece::default(),
        loc: Piece::default(),
        aranges: Piece::default(),
        str: Strings::default(),
        line_str: Strings::default(),
        abbrevs: Vec::new(),
        codes: HashMap::new(),
    };
    if cu.version >= 5 {
        // The list sections' headers.
        for piece in [&mut b.rng, &mut b.loc] {
            let e = src.endian;
            piece.put(0, 4, e);
            piece.put(5, 2, e);
            piece.bytes.push(cu.address_size);
            piece.bytes.push(0);
            piece.put(0, 4, e);
        }
    }
    b.unit(cu, &roots)?;
    let e = src.endian;
    if cu.version >= 5 {
        for piece in [&mut b.rng, &mut b.loc] {
            if piece.bytes.len() == 12 {
                // No lists: no section.
                piece.bytes.clear();
                continue;
            }
            let len = (piece.bytes.len() - 4) as u32;
            let lb = match e {
                Endian::Little => len.to_le_bytes(),
                Endian::Big => len.to_be_bytes(),
            };
            piece.bytes[..4].copy_from_slice(&lb);
        }
    }
    // The abbreviations.
    let mut ab = Piece::default();
    for (k, s) in b.abbrevs.iter().enumerate() {
        ab.uleb(k as u64 + 1);
        ab.uleb(u64::from(s.tag));
        ab.bytes.push(u8::from(s.children));
        for &(name, form, implicit) in &s.attrs {
            ab.uleb(u64::from(name));
            ab.uleb(u64::from(form));
            if form == DW_FORM_IMPLICIT_CONST {
                let mut v = implicit;
                loop {
                    let byte = (v & 0x7F) as u8;
                    v >>= 7;
                    let done = (v == 0 && byte & 0x40 == 0) || (v == -1 && byte & 0x40 != 0);
                    ab.bytes.push(if done { byte } else { byte | 0x80 });
                    if done {
                        break;
                    }
                }
            }
        }
        ab.bytes.extend_from_slice(&[0, 0]);
    }
    ab.bytes.push(0);
    let v5 = cu.version >= 5;
    let mut out = std::mem::take(&mut b.out);
    let pieces: [(&'static [u8], bool, Piece); 8] = [
        (b".debug_info", false, std::mem::take(&mut b.info)),
        (b".debug_abbrev", false, ab),
        (b".debug_line", false, std::mem::take(&mut b.line)),
        (
            b".debug_str",
            true,
            Piece {
                bytes: std::mem::take(&mut b.str.bytes),
                ..Piece::default()
            },
        ),
        (
            b".debug_line_str",
            true,
            Piece {
                bytes: std::mem::take(&mut b.line_str.bytes),
                ..Piece::default()
            },
        ),
        (
            if v5 {
                b".debug_rnglists"
            } else {
                b".debug_ranges"
            },
            false,
            std::mem::take(&mut b.rng),
        ),
        (
            if v5 {
                b".debug_loclists"
            } else {
                b".debug_loc"
            },
            false,
            std::mem::take(&mut b.loc),
        ),
        (b".debug_aranges", false, std::mem::take(&mut b.aranges)),
    ];
    for (name, strings, piece) in pieces {
        if !piece.fixes.is_empty() {
            return Err("references left unfilled");
        }
        out.sections.push(OutSection {
            name,
            strings,
            bytes: piece.bytes,
            rels: piece.rels,
        });
    }
    Ok(Some(out))
}

#[cfg(test)]
mod tests;
