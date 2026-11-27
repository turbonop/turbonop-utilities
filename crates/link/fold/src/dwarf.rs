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
//!   abstract origins, the DIEs location expressions name), to a fixpoint:
//!   a named class, structure, or union in named scopes as a declaration
//!   (its name, `DW_AT_declaration`, and only its kept members, such as the
//!   declaration a moved member function's `DW_AT_specification` names:
//!   what clang's `-fno-standalone-debug` writes), since its definition is
//!   in the givers' units, which every link that loads the synthesized
//!   member loads too, and debuggers find it by name; anything else whole
//!   (base and derived types, enumerations, anonymous and local classes,
//!   abstract instances). Kept whole, LLVM's classes' members pull in most
//!   of a unit's types: 3.35 GB of DWARF for `libLLVMCodeGen.a`'s 1,343
//!   synthesized members, against 4.1 MB this way. A namespace or unit
//!   referred to, and every kept DIE's ancestors, are kept as skeletons
//!   (the DIE and only its kept children; a class ancestor as a
//!   declaration). They are written in the original order,
//!   `DW_AT_sibling` left out.
//! - **The unit's DIE** is the original's attributes but its addresses,
//!   line table, bases, and macros: `DW_AT_low_pc` 0, `DW_AT_ranges` the
//!   moved code sections, `DW_AT_stmt_list` the new line table.
//! - **The line table** is the original's header with only the files (and
//!   their directories) the kept DIEs (`DW_AT_decl_file`,
//!   `DW_AT_call_file`) and the moved sequences name, renumbered, and the
//!   unit's primary file; then the sequences whose `DW_LNE_set_address`
//!   names a moved section (assemblers write one per section), their
//!   `DW_LNS_set_file`s renumbered.
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

use std::borrow::Cow;
use std::collections::{BTreeMap, BTreeSet, HashMap};

use crate::elf::Dwo;

use turbonop_dwarf::consts::*;
use turbonop_dwarf::ops::{self, Operand};
use turbonop_dwarf::{Abbrev, Abbrevs, Attr, Reader, Sections, Unit, Value};
use turbonop_elf_object::{Data, Endian, Object, shf};

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

pub(crate) mod line;

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

    /// A Mach-O section's kind (its name, at most 16 bytes).
    fn of_macho(name: &[u8]) -> Option<Kind> {
        Some(match name {
            b"__debug_info" => Kind::Info,
            b"__debug_abbrev" => Kind::Abbrev,
            b"__debug_str" => Kind::Str,
            b"__debug_line_str" => Kind::LineStr,
            b"__debug_line" => Kind::Line,
            b"__debug_rnglists" => Kind::Rnglists,
            b"__debug_loclists" => Kind::Loclists,
            b"__debug_ranges" => Kind::Ranges,
            b"__debug_loc" => Kind::Loc,
            b"__debug_addr" => Kind::Addr,
            b"__debug_str_offs" => Kind::StrOffsets,
            b"__debug_aranges" => Kind::Aranges,
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
/// debug sections applied: the member's own, unless one applies) and its
/// relocations, by offset.
struct Sec<'p> {
    bytes: Cow<'p, [u8]>,
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
    /// Whether it has a `DW_AT_name`.
    named: bool,
    /// Whether it may have addresses that make it a root (see
    /// [`DieShape::roots`]).
    roots: bool,
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
    /// GNU's split DWARF 4: its range lists' base in the member's
    /// `.debug_ranges` (`DW_AT_GNU_ranges_base`); its location lists are in
    /// the `.dwo`'s `.debug_loc.dwo`, with GNU's entry kinds.
    gnu_split: Option<u64>,
    /// Each DIE's [`declarable`].
    declarable: Vec<bool>,
    /// Its line table: where it starts, its program starts, and it ends.
    line: Option<(u64, u64, u64)>,
    header: LineHeader,
    seqs: Vec<Seq>,
    /// Each section's sequences (indexes into `seqs`, in order): a
    /// synthesized member's are found without reading every sequence.
    by_section: BTreeMap<usize, Vec<usize>>,
    /// Mach-O: the line table's rows by real section (each row's address
    /// an offset in it), and what its program depends on: a moved atom's
    /// rows make a sequence of their own.
    rows: Vec<(usize, Vec<line::Row>)>,
    params: Option<line::Params>,
}

impl Cu {
    /// The sequences of the sections `moved`, in order.
    fn moved_seqs(&self, moved: &BTreeSet<usize>) -> Vec<&Seq> {
        let mut ks: Vec<usize> = moved
            .iter()
            .filter_map(|x| self.by_section.get(x))
            .flatten()
            .copied()
            .collect();
        ks.sort_unstable();
        ks.dedup();
        ks.into_iter().map(|k| &self.seqs[k]).collect()
    }

    /// Whether a sequence is of one of the sections `moved`.
    fn has_moved_seqs(&self, moved: &BTreeSet<usize>) -> bool {
        moved.iter().any(|x| self.by_section.contains_key(x))
    }
}

/// One member's DWARF, read once for every synthesized member it gives
/// units to.
/// A symbol, as the DWARF's relocations name it.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Sym {
    /// A global (a name of the member's, or an undefined one).
    pub global: bool,
    /// The section it is in, if any (for Mach-O, a real section, which
    /// [`Source::atoms`] divides).
    pub section: Option<usize>,
    pub value: u64,
    /// A section's symbol: its value isn't added.
    pub section_symbol: bool,
}

/// A section of the source member, as the DWARF's addresses see it (for
/// Mach-O, an atom).
#[derive(Clone, Copy, Debug)]
pub(crate) struct Sect {
    pub code: bool,
    pub size: u64,
}

/// A Mach-O debug section, for [`Source::macho`]: its name, bytes, and
/// addresses (offset, size, symbol, addend).
pub(crate) type MachoSection<'a> = (&'a [u8], Vec<u8>, Vec<(u64, u8, u32, i64)>);

pub(crate) struct Source<'p> {
    symbols: Vec<Sym>,
    sections: Vec<Sect>,
    /// Mach-O: each real section's atoms (start, end, and the atom's index
    /// among [`Source::sections`]), in order: what an address of a real
    /// section is in. `None` for ELF, whose sections are the units'.
    atoms: Option<Vec<Vec<(u64, u64, usize)>>>,
    /// Mach-O: each atom's real section, start, and end.
    extents: Vec<Option<(usize, u64, u64)>>,
    /// Mach-O: classes the moved code names (from its own DIEs: its
    /// variables', parameters', and specifications' types) and the classes
    /// it is a member of are kept whole, not as declarations: dsymutil
    /// keeps only what the linked code's DIEs reach, so a definition only
    /// the moved code named would be in no unit of the dSYM. (What those
    /// definitions name are declarations again: keeping every class whole
    /// would make the corpus's synthesized members' DWARF 6.07 GB, not
    /// 14.9 MB.)
    complete: bool,
    _p: core::marker::PhantomData<&'p ()>,
    endian: Endian,
    abs32: u32,
    abs64: u32,
    secs: Vec<Option<Sec<'p>>>,
    /// Each symbol's debug section, if it is one's (a section symbol).
    sym_kind: Vec<Option<Kind>>,
    /// A split unit's: the member's own `.debug_str` (which its relocations
    /// name), and its skeleton.
    obj_str: Option<Vec<u8>>,
    skeleton: Option<Skeleton>,
    cus: Vec<Cu>,
    /// The root DIEs of each section: (unit, DIE).
    roots: BTreeMap<usize, Vec<(usize, usize)>>,
}

/// A split unit's skeleton, in the member: what its split unit (in the
/// `.dwo`) takes from it.
#[derive(Clone, Debug)]
struct Skeleton {
    dwo_id: u64,
    /// GNU's split DWARF 4 (`DW_AT_GNU_dwo_id`): `DW_AT_GNU_ranges_base`.
    gnu: Option<u64>,
    addr_base: u64,
    low_pc: Addr,
    /// Its line table (`DW_AT_stmt_list`).
    stmt: Option<u64>,
    /// Its name, directory, and producer, which the split unit may not
    /// repeat.
    strings: Vec<(u16, Vec<u8>)>,
}

/// A DIE's attribute as read: where its value lies in `.debug_info`.
#[derive(Clone, Copy, Debug)]
pub(crate) struct A<'d> {
    pub(crate) name: u16,
    pub(crate) form: u16,
    pub(crate) implicit: i64,
    pub(crate) pos: u64,
    pub(crate) end: u64,
    pub(crate) value: Value<'d>,
}

type R<T> = Result<T, &'static str>;

fn reloc_size(machine: u16, rtype: u32) -> Option<usize> {
    crate::elf::debug_reloc_size(machine, rtype)
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
    pub(crate) fn new(obj: &'p Object<'p>, owner: &[Option<usize>], dwo: Dwo<'_>) -> R<Self> {
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
            // (RELA, or CREL with addends: REL and CREL without them hold
            // implicit addends in the bytes.)
            let explicit = s.kind == turbonop_elf_object::sht::RELA
                || matches!(&s.data, Data::Relocations(r) if r.crel_addends() == Some(true));
            if !explicit {
                return Err("REL debug relocations");
            }
            let Data::Relocations(rs) = &s.data else {
                continue;
            };
            let e = rels_of.entry(t).or_default();
            e.reserve(rs.len());
            for r in rs {
                e.push(Rel {
                    offset: r.offset,
                    rtype: obj.reloc_type(r),
                    sym: obj.reloc_symbol(r),
                    addend: r.addend,
                });
            }
        }
        let mut secs: Vec<Option<Sec<'p>>> = (0..KINDS).map(|_| None).collect();
        for (k, at) in index.iter().enumerate() {
            let Some(at) = *at else { continue };
            let s = &obj.sections[at];
            let Data::Bytes(b) = &s.data else {
                return Err("a debug section without contents");
            };
            let mut rels = rels_of.remove(&at).unwrap_or_default();
            rels.sort_by_key(|r| r.offset);
            let kind = index_kind(k);
            // The relocations to debug sections, applied to a copy (a
            // section without them, such as `.debug_str`, is read in place).
            let mut applied: Vec<(usize, usize, u64)> = Vec::with_capacity(rels.len());
            for r in &rels {
                let size = reloc_size(machine, r.rtype).ok_or("a debug relocation not known")?;
                if !kind.relocated() {
                    return Err("relocations in a string or abbreviation section");
                }
                let Some(_) = sym_kind.get(r.sym as usize).copied().flatten() else {
                    continue;
                };
                let value = symbols[r.sym as usize].value.wrapping_add(r.addend as u64);
                applied.push((r.offset as usize, size, value));
            }
            let bytes: Cow<'p, [u8]> = if applied.is_empty() {
                Cow::Borrowed(&b[..])
            } else {
                let mut bytes = b.to_vec();
                for (at, size, value) in applied {
                    crate::elf::put(&mut bytes, at, size, value, endian);
                }
                Cow::Owned(bytes)
            };
            secs[k] = Some(Sec { bytes, rels });
        }
        let mut src = Source {
            symbols: symbols
                .iter()
                .map(|s| Sym {
                    global: s.bind() != 0,
                    section: s.in_section().then_some(s.shndx as usize),
                    value: s.value,
                    section_symbol: s.kind() == 3,
                })
                .collect(),
            sections: obj
                .sections
                .iter()
                .map(|s| {
                    let size = match &s.data {
                        Data::Bytes(b) => b.len() as u64,
                        _ => s.nobits_size,
                    };
                    Sect {
                        code: s.flags & shf::EXECINSTR != 0 && s.flags & shf::ALLOC != 0,
                        size,
                    }
                })
                .collect(),
            atoms: None,
            extents: Vec::new(),
            complete: false,
            _p: core::marker::PhantomData,
            endian,
            abs32,
            abs64,
            secs,
            sym_kind,
            obj_str: None,
            skeleton: None,
            cus: Vec::new(),
            roots: BTreeMap::new(),
        };
        if src.secs[Kind::Info as usize].is_none() {
            return Ok(src);
        }
        src.split(dwo)?;
        src.read_units(owner)?;
        Ok(src)
    }

    fn sec(&self, k: Kind) -> &[u8] {
        self.secs[k as usize].as_ref().map_or(&[], |s| &s.bytes)
    }

    /// The section a relocation to a string section of kind `k` reads:
    /// the member's own (a split unit's strings are the `.dwo`'s).
    fn reloc_str(&self, k: Kind) -> &[u8] {
        match (&self.obj_str, k) {
            (Some(s), Kind::Str) => s,
            _ => self.sec(k),
        }
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
        self.locate(a).map(|(s, _)| s)
    }

    /// The section (for Mach-O, the atom) an address is in, and its offset
    /// in it, if it names one of the member's by a local symbol or a
    /// definition.
    fn locate(&self, a: Addr) -> Option<(usize, i64)> {
        let Addr::Sym { sym, addend, .. } = a else {
            return None;
        };
        let s = self.symbols.get(sym as usize)?;
        if s.global && s.section.is_none() {
            return None;
        }
        let sec = s.section?;
        let off = addend.wrapping_add(if s.section_symbol { 0 } else { s.value as i64 });
        match &self.atoms {
            None => Some((sec, off)),
            Some(atoms) => {
                let list = atoms.get(sec)?;
                let o = u64::try_from(off).ok()?;
                let k = list.partition_point(|x| x.0 <= o).checked_sub(1)?;
                let (start, end, id) = list[k];
                (o < end).then_some((id, (o - start) as i64))
            }
        }
    }

    fn unit<'s>(&'s self, cu: &Cu) -> R<Unit<'s>> {
        let mut u = Unit::at(self.dwarf(), cu.offset).ok_or("a unit that doesn't read")?;
        u.addr_base = cu.addr_base;
        u.str_offsets_base = cu.str_offsets_base;
        u.rnglists_base = cu.rnglists_base;
        u.loclists_base = cu.loclists_base;
        Ok(u)
    }

    /// A split unit (DWARF 5's `-gsplit-dwarf`): reads its skeleton, finds
    /// its `.dwo` file with `dwo`, and puts the `.dwo`'s sections in place
    /// of the member's, but for `.debug_addr` and `.debug_line`, which the
    /// skeleton's unit keeps in the member.
    fn split(&mut self, dwo: Dwo<'_>) -> R<()> {
        let dwarf = self.dwarf();
        // DWARF 5's skeleton unit, or GNU's DWARF 4 one: a compile unit
        // with `DW_AT_GNU_dwo_id`.
        let gnu_id = |u: &Unit<'_>| -> Option<u64> {
            if u.version >= 5 {
                return None;
            }
            let a = Abbrevs::parse(dwarf.abbrev, u.abbrev_offset, dwarf.endian)?;
            let (_, top) = read_die(u, &a, u.dies).ok()?;
            let id = top.iter().find(|x| x.name == DW_AT_GNU_DWO_ID)?;
            u.uint(&id.value)
        };
        let Some(u) = Unit::iter(dwarf, turbonop_dwarf::UnitSection::Info, 0)
            .find(|u| u.unit_type == DW_UT_SKELETON || gnu_id(u).is_some())
        else {
            return Ok(());
        };
        let gnu_dwo_id = gnu_id(&u);
        let abbrevs = Abbrevs::parse(dwarf.abbrev, u.abbrev_offset, dwarf.endian)
            .ok_or("an abbreviation table that doesn't read")?;
        let (_, top) = read_die(&u, &abbrevs, u.dies)?;
        let attrs: Vec<Attr<'_>> = top
            .iter()
            .map(|a| Attr {
                name: a.name,
                form: a.form,
                value: a.value,
            })
            .collect();
        let mut unit = u;
        unit.set_bases(&attrs);
        let string = |name: u16| {
            top.iter()
                .find(|a| a.name == name)
                .and_then(|a| unit.string(&a.value))
                .map(|s| s.bytes.to_vec())
        };
        let dwo_name = string(DW_AT_DWO_NAME)
            .or_else(|| string(DW_AT_GNU_DWO_NAME))
            .ok_or("a skeleton without DW_AT_dwo_name")?;
        let comp_dir = string(DW_AT_COMP_DIR).unwrap_or_default();
        let mut strings = Vec::new();
        for n in [DW_AT_NAME, DW_AT_COMP_DIR, DW_AT_PRODUCER] {
            if let Some(s) = string(n) {
                strings.push((n, s));
            }
        }
        let probe = Cu {
            offset: u.offset,
            version: u.version,
            address_size: u.address_size,
            abbrevs: abbrevs.clone(),
            dies: Vec::new(),
            low_pc: Addr::Const(0),
            addr_base: unit.addr_base,
            str_offsets_base: unit.str_offsets_base,
            rnglists_base: unit.rnglists_base,
            loclists_base: unit.loclists_base,
            gnu_split: None,
            declarable: Vec::new(),
            line: None,
            header: LineHeader::default(),
            seqs: Vec::new(),
            by_section: BTreeMap::new(),
            rows: Vec::new(),
            params: None,
        };
        let low_pc = match top.iter().find(|a| a.name == DW_AT_LOW_PC) {
            Some(a) => self.attr_addr(&probe, a)?,
            None => Addr::Const(0),
        };
        let stmt = top
            .iter()
            .find(|a| a.name == DW_AT_STMT_LIST)
            .and_then(|a| unit.uint(&a.value));
        let gnu = gnu_dwo_id.map(|_| {
            top.iter()
                .find(|a| a.name == DW_AT_GNU_RANGES_BASE)
                .and_then(|a| unit.uint(&a.value))
                .unwrap_or(0)
        });
        let skeleton = Skeleton {
            dwo_id: u.dwo_id.or(gnu_dwo_id).ok_or("a skeleton without its ID")?,
            gnu,
            addr_base: unit.addr_base,
            low_pc,
            stmt,
            strings,
        };
        let bytes = dwo(&comp_dir, &dwo_name).ok_or("its .dwo file isn't found")?;
        let file = Object::parse(&bytes).map_err(|_| "its .dwo file doesn't read")?;
        let mut found = [false; KINDS];
        let mut taken: Vec<(Kind, Vec<u8>)> = Vec::new();
        for s in &file.sections {
            let Some(base) = s.name.strip_suffix(b".dwo") else {
                continue;
            };
            let Some(k) = Kind::of(base) else { continue };
            // (DWARF 4's range lists stay in the member's `.debug_ranges`;
            // `.debug_line.dwo` is type units'.)
            let wanted = match k {
                Kind::Info | Kind::Abbrev | Kind::Str | Kind::StrOffsets => true,
                Kind::Loclists | Kind::Rnglists => gnu.is_none(),
                Kind::Loc => gnu.is_some(),
                _ => false,
            };
            if !wanted {
                continue;
            }
            if s.flags & shf::COMPRESSED != 0 {
                return Err("compressed debug sections");
            }
            if found[k as usize] {
                return Err("two debug sections of one kind");
            }
            found[k as usize] = true;
            if let Data::Bytes(b) = &s.data {
                taken.push((k, b.to_vec()));
            }
        }
        if !found[Kind::Info as usize] {
            return Err("a .dwo file without .debug_info.dwo");
        }
        self.obj_str = Some(self.sec(Kind::Str).to_vec());
        let replaced: &[Kind] = if gnu.is_some() {
            &[
                Kind::Info,
                Kind::Abbrev,
                Kind::Str,
                Kind::StrOffsets,
                Kind::Loc,
            ]
        } else {
            &[
                Kind::Info,
                Kind::Abbrev,
                Kind::Str,
                Kind::StrOffsets,
                Kind::Loclists,
                Kind::Rnglists,
            ]
        };
        for &k in replaced {
            self.secs[k as usize] = None;
        }
        for (k, b) in taken {
            self.secs[k as usize] = Some(Sec {
                bytes: Cow::Owned(b),
                rels: Vec::new(),
            });
        }
        self.skeleton = Some(skeleton);
        Ok(())
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
            let skeleton = self.skeleton.as_ref();
            match skeleton {
                // A split unit: the one the skeleton names.
                Some(sk) if sk.gnu.is_some() => {
                    // GNU's: a DWARF 4 compile unit with the skeleton's ID.
                    let id = Abbrevs::parse(dwarf.abbrev, u.abbrev_offset, dwarf.endian)
                        .and_then(|a| read_die(u, &a, u.dies).ok().map(|(_, t)| t))
                        .and_then(|t| {
                            let a = t.iter().find(|a| a.name == DW_AT_GNU_DWO_ID)?;
                            u.uint(&a.value)
                        });
                    if u.version >= 5 || u.unit_type != DW_UT_COMPILE || id != Some(sk.dwo_id) {
                        continue;
                    }
                }
                Some(sk) => {
                    if u.unit_type != DW_UT_SPLIT_COMPILE || u.dwo_id != Some(sk.dwo_id) {
                        continue;
                    }
                }
                None => {
                    if u.unit_type != DW_UT_COMPILE && u.unit_type != DW_UT_PARTIAL {
                        if u.unit_type == DW_UT_SKELETON || u.unit_type == DW_UT_SPLIT_COMPILE {
                            return Err("split DWARF");
                        }
                        continue;
                    }
                }
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
                gnu_split: None,
                declarable: Vec::new(),
                line: None,
                header: LineHeader::default(),
                seqs: Vec::new(),
                by_section: BTreeMap::new(),
                rows: Vec::new(),
                params: None,
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
            if let Some(sk) = skeleton {
                // A split unit's addresses are the skeleton's, its lists'
                // offsets after the `.dwo` sections' headers.
                cu.addr_base = sk.addr_base;
                unit.addr_base = sk.addr_base;
                cu.gnu_split = sk.gnu;
                if sk.gnu.is_some() {
                    // (DWARF 4: no list bases.)
                } else if cu.loclists_base == 0 {
                    cu.loclists_base = 12;
                    unit.loclists_base = 12;
                }
                if cu.rnglists_base == 0 {
                    cu.rnglists_base = 12;
                    unit.rnglists_base = 12;
                }
            } else if top
                .iter()
                .any(|a| a.name == DW_AT_GNU_DWO_ID || a.name == DW_AT_DWO_NAME)
            {
                return Err("split DWARF");
            }
            cu.dies = walk(&unit, &cu.abbrevs)?;
            cu.declarable = declarable(&cu.dies);
            cus.push((
                cu,
                unit,
                top.iter().find(|a| a.name == DW_AT_LOW_PC).copied(),
                top.iter().find(|a| a.name == DW_AT_STMT_LIST).copied(),
            ));
        }
        let mut out = Vec::new();
        for (mut cu, unit, low, stmt) in cus {
            let mut line = stmt.and_then(|a| unit.uint(&a.value));
            if let Some(a) = low {
                cu.low_pc = self.attr_addr(&cu, &a)?;
            }
            if let Some(sk) = &self.skeleton {
                cu.low_pc = sk.low_pc;
                line = sk.stmt;
            }
            if let Some(off) = line {
                let (line, header, seqs) = self.sequences(off, cu.address_size)?;
                cu.line = Some(line);
                cu.header = header;
                cu.seqs = seqs;
                if self.atoms.is_some() {
                    // (Sequences are a section's, not an atom's.)
                    for s in &mut cu.seqs {
                        s.section = None;
                    }
                    self.rows(&mut cu, off)?;
                }
            }
            for (k, q) in cu.seqs.iter().enumerate() {
                if let Some(x) = q.section {
                    cu.by_section.entry(x).or_default().push(k);
                }
            }
            out.push(cu);
        }
        // Roots: DIEs with addresses in sections of units. A unit's DIEs
        // are read in chunks on every core (a large member's unit has
        // millions); the first error, in DIE order, is the one reported.
        const CHUNK: usize = 16 * 1024;
        let me = &*self;
        let mut roots: BTreeMap<usize, BTreeSet<(usize, usize)>> = BTreeMap::new();
        for (ci, cu) in out.iter().enumerate() {
            let n = cu.dies.len();
            let found: Vec<R<Vec<(usize, usize)>>> =
                turbonop_par::map_range(n.div_ceil(CHUNK), 1, |c| {
                    let unit = me.unit(cu)?;
                    let mut v = Vec::new();
                    let mut attrs = Vec::new();
                    for di in (c * CHUNK).max(1)..n.min((c + 1) * CHUNK) {
                        let d = &cu.dies[di];
                        // (A DIE that can't be one isn't read again.)
                        if !d.roots {
                            continue;
                        }
                        read_die_into(&unit, &cu.abbrevs, d.offset, &mut attrs)?;
                        let mut here = BTreeSet::new();
                        for a in &attrs {
                            for addr in me.addresses_of(cu, &unit, a)? {
                                if let Some(s) = me.section_of(addr)
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
                        v.extend(here.into_iter().map(|s| (s, root)));
                    }
                    Ok(v)
                });
            for f in found {
                for (s, root) in f? {
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
            (_, Value::Uint(v)) => match cu.gnu_split {
                // GNU's split DWARF 4: a range list's offset is from the
                // skeleton's `DW_AT_GNU_ranges_base`.
                Some(base) if ranges => v.checked_add(base).ok_or("a bad range list offset"),
                _ => Ok(v),
            },
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

    /// The line table at `off`: its extent, its header, and its sequences.
    #[allow(clippy::type_complexity)]
    fn sequences(&self, off: u64, address_size: u8) -> R<((u64, u64, u64), LineHeader, Vec<Seq>)> {
        let bad = "a line table that doesn't read";
        let data = self.sec(Kind::Line);
        let mut r = Reader::new(data, off as usize, core_endian(self.endian));
        let (_, end, dwarf64) = r.initial_length().ok_or(bad)?;
        if dwarf64 {
            return Err("64-bit DWARF");
        }
        r.data = &data[..end];
        let version = u16::try_from(r.uint(2).ok_or(bad)?).map_err(|_| bad)?;
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
        let fixed_start = r.pos as u64;
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
        let mut header = LineHeader {
            version,
            fixed: (fixed_start, r.pos as u64),
            lengths: lengths.clone(),
            ..LineHeader::default()
        };
        if version >= 5 {
            for table in 0..2 {
                let fstart = r.pos as u64;
                let n = r.u8().ok_or(bad)?;
                let mut formats = Vec::new();
                for _ in 0..n {
                    let kind = r.uleb().ok_or(bad)?;
                    let form = u16::try_from(r.uleb().ok_or(bad)?).map_err(|_| bad)?;
                    formats.push((kind, form));
                }
                let fend = r.pos as u64;
                let count = r.uleb().ok_or(bad)?;
                for _ in 0..count {
                    let s = r.pos as u64;
                    let mut dir = 0;
                    let mut field = None;
                    for &(kind, form) in &formats {
                        let at = r.pos as u64;
                        let v = match form {
                            DW_FORM_STRING => {
                                r.cstr().ok_or(bad)?;
                                0
                            }
                            DW_FORM_LINE_STRP | DW_FORM_STRP | DW_FORM_DATA4 => {
                                r.uint(4).ok_or(bad)?
                            }
                            DW_FORM_UDATA => r.uleb().ok_or(bad)?,
                            DW_FORM_DATA1 => r.uint(1).ok_or(bad)?,
                            DW_FORM_DATA2 => r.uint(2).ok_or(bad)?,
                            DW_FORM_DATA8 => r.uint(8).ok_or(bad)?,
                            DW_FORM_DATA16 => {
                                r.bytes(16).ok_or(bad)?;
                                0
                            }
                            DW_FORM_BLOCK => {
                                let n = r.uleb().ok_or(bad)?;
                                r.bytes(n).ok_or(bad)?;
                                0
                            }
                            _ => return Err("a line table entry of a form not known"),
                        };
                        if kind == 2 {
                            dir = v;
                            field = Some((at, r.pos as u64, form));
                        }
                    }
                    let e = r.pos as u64;
                    if table == 0 {
                        header.dirs.push((s, e));
                    } else {
                        header.files.push(FileEnt {
                            start: s,
                            end: e,
                            dir,
                            field,
                        });
                    }
                }
                if table == 0 {
                    header.dir_formats = (fstart, fend);
                } else {
                    header.file_formats = (fstart, fend);
                }
            }
        } else {
            loop {
                let s = r.pos as u64;
                let name = r.cstr().ok_or(bad)?;
                if name.is_empty() {
                    break;
                }
                header.dirs.push((s, r.pos as u64));
            }
            loop {
                let s = r.pos as u64;
                let name = r.cstr().ok_or(bad)?;
                if name.is_empty() {
                    break;
                }
                let at = r.pos as u64;
                let dir = r.uleb().ok_or(bad)?;
                let field = Some((at, r.pos as u64, DW_FORM_UDATA));
                r.uleb().ok_or(bad)?;
                r.uleb().ok_or(bad)?;
                header.files.push(FileEnt {
                    start: s,
                    end: r.pos as u64,
                    dir,
                    field,
                });
            }
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
        Ok(((off, program as u64, end as u64), header, seqs))
    }

    /// Mach-O: the rows of `cu`'s line table (at `off`), by real section,
    /// each row's address an offset in its section (what the sequence's
    /// `DW_LNE_set_address` relocation names).
    fn rows(&self, cu: &mut Cu, off: u64) -> R<()> {
        let data = self.sec(Kind::Line);
        let e = core_endian(self.endian);
        let t = line::table(data, off as usize, cu.address_size, e)?;
        let read = |at: usize, n: u8| read_uint(data, at, usize::from(n), self.endian);
        let seqs = line::decode(data, &t, e, &read)?;
        let mut by: BTreeMap<usize, Vec<line::Row>> = BTreeMap::new();
        for s in seqs {
            let Some(at) = s.set_address else { continue };
            let Some(r) = self.rel_at(Kind::Line, at as u64) else {
                continue; // (An address no relocation names: no code's.)
            };
            let Some(sec) = self.symbols.get(r.sym as usize).and_then(|y| y.section) else {
                continue;
            };
            // The section's address: the operand's value less its offset.
            let raw = read_uint(data, at, usize::from(t.params.address_size), self.endian)
                .ok_or("a line table that doesn't read")?;
            let base = raw.wrapping_sub(r.addend as u64);
            let rows = by.entry(sec).or_default();
            for mut row in s.rows {
                row.address = row.address.wrapping_sub(base);
                rows.push(row);
            }
        }
        cu.rows = by.into_iter().collect();
        cu.params = Some(t.params);
        Ok(())
    }

    /// Mach-O: a member's DWARF, from its debug sections (`__debug_info`
    /// and the rest: each name, bytes, and relocations, every one an
    /// address: its offset, size, symbol, and addend), its symbols, its
    /// atoms (each real section's, as [`Source::atoms`]), and what each
    /// atom is ([`Sect`]); `owner` gives each atom's unit.
    pub(crate) fn macho(
        debug: Vec<MachoSection<'_>>,
        symbols: Vec<Sym>,
        atoms: Vec<Vec<(u64, u64, usize)>>,
        sections: Vec<Sect>,
        owner: &[Option<usize>],
    ) -> R<Self> {
        let (abs32, abs64) = (10, 1);
        let mut secs: Vec<Option<Sec>> = (0..KINDS).map(|_| None).collect();
        for (name, bytes, rels) in debug {
            let Some(k) = Kind::of_macho(name) else {
                continue;
            };
            if secs[k as usize].is_some() {
                return Err("two debug sections of one kind");
            }
            let mut rels: Vec<Rel> = rels
                .into_iter()
                .map(|(offset, size, sym, addend)| Rel {
                    offset,
                    rtype: if size == 4 { abs32 } else { abs64 },
                    sym,
                    addend,
                })
                .collect();
            rels.sort_by_key(|r| r.offset);
            if !k.relocated() && !rels.is_empty() {
                return Err("relocations in a string or abbreviation section");
            }
            secs[k as usize] = Some(Sec {
                bytes: Cow::Owned(bytes),
                rels,
            });
        }
        let n = symbols.len();
        let mut extents = vec![None; sections.len()];
        for (s, list) in atoms.iter().enumerate() {
            for &(a, b, id) in list {
                if let Some(x) = extents.get_mut(id) {
                    *x = Some((s, a, b));
                }
            }
        }
        let mut src = Source {
            symbols,
            sections,
            atoms: Some(atoms),
            extents,
            complete: true,
            _p: core::marker::PhantomData,
            endian: Endian::Little,
            abs32,
            abs64,
            secs,
            sym_kind: vec![None; n],
            obj_str: None,
            skeleton: None,
            cus: Vec::new(),
            roots: BTreeMap::new(),
        };
        if src.secs[Kind::Info as usize].is_none() {
            return Ok(src);
        }
        src.read_units(owner)?;
        Ok(src)
    }

    /// Mach-O: atom `id`'s real section, start, and end.
    fn atom_extent(&self, id: usize) -> Option<(usize, u64, u64)> {
        self.extents.get(id).copied().flatten()
    }

    /// Mach-O: the rows of `cu` in atom `id`, each address an offset in it,
    /// and the atom's size.
    fn atom_rows<'c>(&self, cu: &'c Cu, id: usize) -> Option<(&'c [line::Row], u64)> {
        let (s, a, b) = self.atom_extent(id)?;
        let rows = &cu.rows.iter().find(|(x, _)| *x == s)?.1;
        let lo = rows.partition_point(|r| r.address < a);
        let hi = rows.partition_point(|r| r.address < b);
        // (Within a sequence rows go up; a section's sequences don't
        // overlap.)
        let part = &rows[lo..hi];
        if part.iter().any(|r| r.end_sequence) || part.is_empty() {
            return None;
        }
        Some((part, b - a))
    }

    /// The file indexes sequence `s` sets (`DW_LNS_set_file`).
    fn files_set(&self, h: &LineHeader, s: &Seq, out: &mut BTreeSet<u64>) -> R<()> {
        let bad = "a line table that doesn't read";
        let data = self.sec(Kind::Line);
        let mut r = Reader::new(data, s.start as usize, core_endian(self.endian));
        let opcode_base = h.lengths.len() as u8 + 1;
        while (r.pos as u64) < s.end {
            let op = r.u8().ok_or(bad)?;
            if op >= opcode_base {
                continue;
            }
            match op {
                0 => {
                    let len = r.uleb().ok_or(bad)?;
                    r.pos = r.pos.checked_add(len as usize).ok_or(bad)?;
                }
                9 => {
                    r.uint(2).ok_or(bad)?;
                }
                4 => {
                    out.insert(r.uleb().ok_or(bad)?);
                }
                _ => {
                    for _ in 0..h.lengths[usize::from(op) - 1] {
                        r.uleb().ok_or(bad)?;
                    }
                }
            }
        }
        Ok(())
    }
}

/// A line table's header, as read: where its parts lie.
#[derive(Clone, Debug, Default)]
struct LineHeader {
    version: u16,
    /// From `minimum_instruction_length` through `standard_opcode_lengths`.
    fixed: (u64, u64),
    lengths: Vec<u8>,
    /// DWARF 5: each table's entry formats (their count and pairs).
    dir_formats: (u64, u64),
    file_formats: (u64, u64),
    /// Each directory's and file's entry.
    dirs: Vec<(u64, u64)>,
    files: Vec<FileEnt>,
}

/// A file entry of a line table's header.
#[derive(Clone, Copy, Debug)]
struct FileEnt {
    start: u64,
    end: u64,
    dir: u64,
    /// Where its directory index lies, and its form.
    field: Option<(u64, u64, u16)>,
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
pub(crate) fn read_die<'d, 'a>(
    unit: &Unit<'d>,
    abbrevs: &'a Abbrevs,
    offset: u64,
) -> R<(&'a Abbrev, Vec<A<'d>>)> {
    let mut out = Vec::new();
    let a = read_die_into(unit, abbrevs, offset, &mut out)?;
    Ok((a, out))
}

/// [`read_die`] into `out` (cleared first): for reading many DIEs without
/// a vector each.
fn read_die_into<'d, 'a>(
    unit: &Unit<'d>,
    abbrevs: &'a Abbrevs,
    offset: u64,
    out: &mut Vec<A<'d>>,
) -> R<&'a Abbrev> {
    let bad = "a DIE that doesn't read";
    out.clear();
    let data = unit.sections().info;
    let mut r = Reader::new(data, offset as usize, unit.sections().endian);
    let code = r.uleb().ok_or(bad)?;
    let a = abbrevs.get(code).ok_or(bad)?;
    out.reserve(a.attrs.len());
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
    Ok(a)
}

/// What walking a unit's DIEs needs of an abbreviation, and whether a DIE
/// of it can be a root.
#[derive(Clone, Copy, Debug)]
struct DieShape {
    tag: u16,
    children: bool,
    named: bool,
    /// The size of its attributes, if every form's is fixed: skipped
    /// without reading them.
    fixed: Option<u64>,
    /// Otherwise how to pass over them (`Shapes::skips[start..end]`), if
    /// every form is one [`Skip`] knows.
    skips: Option<(u32, u32)>,
    /// Whether it has attributes [`Source::addresses_of`] reads addresses
    /// from, and no `DW_AT_declaration` (the search for roots passes over
    /// the rest without reading them).
    roots: bool,
}

/// How to pass over an attribute's value (or a run of fixed-size ones),
/// reading what `Unit::read_value` reads for its form.
#[derive(Clone, Copy, Debug)]
enum Skip {
    Fixed(u64),
    Uleb,
    Sleb,
    Cstr,
    /// A block of the length a 1-, 2-, or 4-byte field (or a ULEB128, 0)
    /// gives.
    Block(u8),
}

/// A unit's abbreviations' [`DieShape`]s, by code.
struct Shapes {
    list: Vec<(u64, DieShape)>,
    skips: Vec<Skip>,
    /// Codes to places, unless every code is its place plus 1.
    sparse: Option<HashMap<u64, usize>>,
}

impl Shapes {
    fn new(unit: &Unit<'_>, abbrevs: &Abbrevs) -> Self {
        let w = if unit.dwarf64 { 8 } else { 4 };
        let a = u64::from(unit.address_size);
        let size = |form: u16| -> Option<u64> {
            Some(match form {
                DW_FORM_ADDR => a,
                DW_FORM_DATA1 | DW_FORM_REF1 | DW_FORM_FLAG | DW_FORM_STRX1 | DW_FORM_ADDRX1 => 1,
                DW_FORM_DATA2 | DW_FORM_REF2 | DW_FORM_STRX2 | DW_FORM_ADDRX2 => 2,
                DW_FORM_STRX3 | DW_FORM_ADDRX3 => 3,
                DW_FORM_DATA4 | DW_FORM_REF4 | DW_FORM_REF_SUP4 | DW_FORM_STRX4
                | DW_FORM_ADDRX4 => 4,
                DW_FORM_DATA8 | DW_FORM_REF8 | DW_FORM_REF_SIG8 | DW_FORM_REF_SUP8 => 8,
                DW_FORM_DATA16 => 16,
                DW_FORM_FLAG_PRESENT | DW_FORM_IMPLICIT_CONST => 0,
                DW_FORM_STRP | DW_FORM_LINE_STRP | DW_FORM_STRP_SUP | DW_FORM_GNU_STRP_ALT
                | DW_FORM_SEC_OFFSET | DW_FORM_GNU_REF_ALT => w,
                DW_FORM_REF_ADDR if unit.version <= 2 => a,
                DW_FORM_REF_ADDR => w,
                _ => return None,
            })
        };
        let skip = |form: u16| -> Option<Skip> {
            if let Some(n) = size(form) {
                return Some(Skip::Fixed(n));
            }
            Some(match form {
                DW_FORM_BLOCK1 => Skip::Block(1),
                DW_FORM_BLOCK2 => Skip::Block(2),
                DW_FORM_BLOCK4 => Skip::Block(4),
                DW_FORM_BLOCK | DW_FORM_EXPRLOC => Skip::Block(0),
                DW_FORM_STRING => Skip::Cstr,
                DW_FORM_SDATA => Skip::Sleb,
                DW_FORM_UDATA
                | DW_FORM_REF_UDATA
                | DW_FORM_STRX
                | DW_FORM_GNU_STR_INDEX
                | DW_FORM_ADDRX
                | DW_FORM_GNU_ADDR_INDEX
                | DW_FORM_LOCLISTX
                | DW_FORM_RNGLISTX => Skip::Uleb,
                _ => return None,
            })
        };
        let mut skips: Vec<Skip> = Vec::new();
        let list: Vec<(u64, DieShape)> = abbrevs
            .iter()
            .map(|ab| {
                let fixed = ab
                    .attrs
                    .iter()
                    .try_fold(0u64, |n, s| n.checked_add(size(s.form)?));
                // (Runs of fixed sizes merged.)
                let start = skips.len();
                let mut known = fixed.is_none();
                for a in &ab.attrs {
                    if !known {
                        break;
                    }
                    let here = skips.len() > start;
                    match (skip(a.form), skips.last_mut()) {
                        (Some(Skip::Fixed(n)), Some(Skip::Fixed(m))) if here => *m += n,
                        (Some(k), _) => skips.push(k),
                        (None, _) => known = false,
                    }
                }
                if !known {
                    skips.truncate(start);
                }
                let span = (known && skips.len() < u32::MAX as usize)
                    .then_some((start as u32, skips.len() as u32));
                let addresses = ab.attrs.iter().any(|s| match s.form {
                    DW_FORM_ADDR
                    | DW_FORM_ADDRX
                    | DW_FORM_ADDRX1
                    | DW_FORM_ADDRX2
                    | DW_FORM_ADDRX3
                    | DW_FORM_ADDRX4
                    | DW_FORM_GNU_ADDR_INDEX
                    | DW_FORM_EXPRLOC => true,
                    DW_FORM_SEC_OFFSET | DW_FORM_RNGLISTX => {
                        s.name == DW_AT_RANGES || s.name == DW_AT_START_SCOPE
                    }
                    _ => false,
                });
                let declaration = ab.attrs.iter().any(|s| s.name == DW_AT_DECLARATION);
                (
                    ab.code,
                    DieShape {
                        tag: ab.tag,
                        children: ab.children,
                        named: ab.attrs.iter().any(|s| s.name == DW_AT_NAME),
                        fixed,
                        skips: span,
                        roots: addresses && !declaration,
                    },
                )
            })
            .collect();
        let dense = list
            .iter()
            .enumerate()
            .all(|(i, &(code, _))| code == i as u64 + 1);
        let sparse = (!dense).then(|| {
            list.iter()
                .enumerate()
                .map(|(i, &(code, _))| (code, i))
                .collect()
        });
        Shapes {
            list,
            skips,
            sparse,
        }
    }

    /// Passes `r` over the attributes `skips[start..end]` describe: `None`
    /// where reading them as values would fail.
    fn skip(&self, r: &mut Reader<'_>, (start, end): (u32, u32)) -> Option<()> {
        for k in &self.skips[start as usize..end as usize] {
            match *k {
                Skip::Fixed(n) => {
                    r.bytes(n)?;
                }
                Skip::Uleb => {
                    r.uleb()?;
                }
                Skip::Sleb => {
                    r.sleb()?;
                }
                Skip::Cstr => {
                    r.cstr()?;
                }
                Skip::Block(w) => {
                    let n = if w == 0 {
                        r.uleb()?
                    } else {
                        r.uint(usize::from(w))?
                    };
                    r.bytes(n)?;
                }
            }
        }
        Some(())
    }

    fn get(&self, code: u64) -> Option<&DieShape> {
        let i = match &self.sparse {
            Some(m) => *m.get(&code)?,
            None => usize::try_from(code).ok()?.checked_sub(1)?,
        };
        self.list.get(i).map(|(_, s)| s)
    }
}

/// Every DIE of `unit`, in order, with its parent and extent.
fn walk(unit: &Unit<'_>, abbrevs: &Abbrevs) -> R<Vec<Die>> {
    let bad = "a DIE that doesn't read";
    let shapes = Shapes::new(unit, abbrevs);
    let data = unit.sections().info;
    let mut r = Reader::new(data, unit.dies as usize, unit.sections().endian);
    let end = unit.end as usize;
    // (Room for a DIE per eight bytes, so that a unit of millions isn't
    // copied as it grows; pages not written aren't taken.)
    let mut dies: Vec<Die> = Vec::with_capacity(end.saturating_sub(r.pos) / 8 + 16);
    let mut stack: Vec<u32> = Vec::new();
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
        let shape = *shapes.get(code).ok_or(bad)?;
        match (shape.fixed, shape.skips) {
            (Some(n), _) => {
                r.bytes(n).ok_or(bad)?;
            }
            (None, Some(span)) => shapes.skip(&mut r, span).ok_or(bad)?,
            (None, None) => {
                let a = abbrevs.get(code).ok_or(bad)?;
                for spec in &a.attrs {
                    unit.read_value(&mut r, spec.form, spec.implicit)
                        .ok_or(bad)?;
                }
            }
        }
        let parent = stack.last().copied().unwrap_or(u32::MAX);
        if parent == u32::MAX && !dies.is_empty() {
            return Err("DIEs after the unit's DIE");
        }
        let i = dies.len() as u32;
        dies.push(Die {
            offset: at,
            parent,
            tag: shape.tag,
            end: r.pos as u64,
            named: shape.named,
            roots: shape.roots,
        });
        if shape.children {
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
        crate::elf::put_uleb(&mut self.bytes, v);
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
    /// Each file index of the source unit's line table: its index in the
    /// output's (`u64::MAX`: not kept).
    files: Vec<u64>,
}

/// The DIEs of one unit to keep: 2 whole, 1 as a skeleton (the DIE and its
/// kept children), 3 as a declaration (its name, and its kept children).
struct Keep<'c> {
    /// The marks of the DIEs kept (a map: a synthesized member keeps a few
    /// DIEs of a unit that may have millions, and a member's unit gives
    /// DIEs to many synthesized members).
    mark: HashMap<usize, u8>,
    work: Vec<usize>,
    /// Whether each DIE is a named class, structure, or union a debugger
    /// can find the definition of by its name (in named scopes only): the
    /// unit's, worked out once.
    declarable: &'c [bool],
    /// Mach-O: classes the moved code names, and those it is a member of,
    /// whole (see [`Source::complete`]).
    complete: bool,
    /// The type DIEs (pointers, references, qualifiers, typedefs, arrays,
    /// function types) kept for the moved code: what they name, the code
    /// names too.
    via_code: std::collections::HashSet<usize>,
}

const SKELETON: u8 = 1;
const WHOLE: u8 = 2;
const DECLARATION: u8 = 3;

/// Whether each DIE is a named class, structure, or union a debugger can
/// find the definition of by its name (in named scopes only).
fn declarable(dies: &[Die]) -> Vec<bool> {
    // Whether names inside each DIE can be looked up from anywhere.
    let mut open = vec![false; dies.len()];
    let mut declarable = vec![false; dies.len()];
    for (i, d) in dies.iter().enumerate() {
        let parent_open = d.parent == u32::MAX || open[d.parent as usize];
        let aggregate = matches!(
            d.tag,
            DW_TAG_CLASS_TYPE | DW_TAG_STRUCTURE_TYPE | DW_TAG_UNION_TYPE
        );
        declarable[i] = aggregate && d.named && d.parent != u32::MAX && parent_open;
        open[i] = match d.tag {
            DW_TAG_COMPILE_UNIT | DW_TAG_PARTIAL_UNIT => true,
            DW_TAG_NAMESPACE => d.named && parent_open,
            _ => declarable[i],
        };
    }
    declarable
}

impl<'c> Keep<'c> {
    fn new(declarable: &'c [bool]) -> Self {
        Keep {
            mark: HashMap::new(),
            work: Vec::new(),
            declarable,
            complete: false,
            via_code: std::collections::HashSet::new(),
        }
    }

    fn mark(&self, i: usize) -> u8 {
        self.mark.get(&i).copied().unwrap_or(0)
    }

    /// Keeps the ancestors of a kept DIE (from `i`, its parent): scopes
    /// (units, namespaces, subprograms, blocks) as skeletons, classes a
    /// debugger finds by name as declarations, the rest whole (an anonymous
    /// or local class is kept with all its members, so that its definition
    /// is the original's).
    fn skeleton(&mut self, dies: &[Die], i: usize) {
        let mut at = i;
        loop {
            if self.mark(at) != 0 {
                return;
            }
            if self.declarable[at] && self.complete {
                self.whole(dies, at);
                return;
            } else if self.declarable[at] {
                self.mark.insert(at, DECLARATION);
            } else if is_scope(dies[at].tag)
                || matches!(
                    dies[at].tag,
                    DW_TAG_SUBPROGRAM | DW_TAG_LEXICAL_BLOCK | DW_TAG_INLINED_SUBROUTINE
                )
            {
                self.mark.insert(at, SKELETON);
                self.work.push(at);
            } else {
                self.whole(dies, at);
                return;
            }
            let p = dies[at].parent;
            if p == u32::MAX {
                return;
            }
            at = p as usize;
        }
    }

    fn whole(&mut self, dies: &[Die], i: usize) {
        if self.mark(i) == WHOLE {
            return;
        }
        let end = dies[i].end;
        let mut j = i;
        while j < dies.len() && (j == i || dies[j].offset < end) {
            let m = self.mark(j);
            if m != WHOLE {
                if m != SKELETON {
                    self.work.push(j);
                }
                self.mark.insert(j, WHOLE);
            }
            j += 1;
        }
        let p = dies[i].parent;
        if p != u32::MAX {
            self.skeleton(dies, p as usize);
        }
    }

    /// Keeps what a kept DIE refers to: a scope as a skeleton, a class a
    /// debugger finds by name as a declaration (its definition is in the
    /// givers' units, which a link that loads this member loads), the rest
    /// whole.
    fn refer(&mut self, dies: &[Die], i: usize, from_code: bool) {
        if is_scope(dies[i].tag) {
            self.skeleton(dies, i);
        } else if self.declarable[i] && !(self.complete && from_code) {
            if self.mark(i) == 0 {
                self.mark.insert(i, DECLARATION);
                let p = dies[i].parent;
                if p != u32::MAX {
                    self.skeleton(dies, p as usize);
                }
            }
        } else {
            if self.complete
                && from_code
                && matches!(
                    dies[i].tag,
                    DW_TAG_POINTER_TYPE
                        | DW_TAG_REFERENCE_TYPE
                        | DW_TAG_RVALUE_REFERENCE_TYPE
                        | DW_TAG_CONST_TYPE
                        | DW_TAG_VOLATILE_TYPE
                        | DW_TAG_RESTRICT_TYPE
                        | DW_TAG_TYPEDEF
                        | DW_TAG_ARRAY_TYPE
                        | DW_TAG_SUBROUTINE_TYPE
                        | DW_TAG_PTR_TO_MEMBER_TYPE
                )
                && self.mark(i) != WHOLE
            {
                self.via_code.insert(i);
            }
            self.whole(dies, i);
        }
    }
}

/// Whether an attribute holds a line-table file index.
fn is_file(name: u16) -> bool {
    matches!(name, DW_AT_DECL_FILE | DW_AT_CALL_FILE)
}

/// A file-index attribute's value.
fn file_value(unit: &Unit<'_>, a: &A<'_>) -> R<u64> {
    match a.value {
        Value::Sint(v) => u64::try_from(v).map_err(|_| "a negative file index"),
        v => unit.uint(&v).ok_or("a file index of a form not known"),
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
        if s.global && (self.src.atoms.is_none() || s.section.is_none()) {
            return Some((rtype, Target::Global(sym), addend));
        }
        match self.src.locate(a) {
            Some((sec, off)) if self.moved.contains(&sec) => {
                Some((rtype, Target::Section(sec), off))
            }
            _ => None,
        }
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
                    let s = Reader::new(src.reloc_str(sk), off as usize, core_endian(src.endian))
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
        } else if cu.gnu_split.is_some() {
            // `.debug_loc.dwo`: GNU's entry kinds (DWARF 5's first four,
            // but with a 4-byte length and 2-byte expression lengths).
            let data = src.sec(Kind::Loc);
            let mut r = Reader::new(data, off as usize, core_endian(src.endian));
            let expr = |r: &mut Reader<'s>| -> R<(&'s [u8], u64)> {
                let len = r.uint(2).ok_or(bad)?;
                let at = r.pos as u64;
                Ok((r.bytes(len).ok_or(bad)?, at))
            };
            loop {
                match r.u8().ok_or(bad)? {
                    DW_LLE_END_OF_LIST => break,
                    DW_LLE_BASE_ADDRESSX => {
                        // (No entry kind is relative to it.)
                        src.indexed(cu, r.uleb().ok_or(bad)?)?;
                    }
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
                        let len = r.uint(4).ok_or(bad)?;
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
        let mut keep = Keep::new(&cu.declarable);
        keep.complete = src.complete;
        for &r in roots {
            keep.whole(dies, r);
        }
        // The moved code's DIEs: the roots' subtrees (offsets).
        let mut code: Vec<(u64, u64)> = roots
            .iter()
            .map(|&r| (dies[r].offset, dies[r].end))
            .collect();
        code.sort_unstable();
        let in_code = |off: u64| {
            let k = code.partition_point(|c| c.0 <= off);
            k.checked_sub(1).is_some_and(|k| off < code[k].1)
        };
        keep.skeleton(dies, 0);
        let mut refs = Vec::new();
        let mut attrs = Vec::new();
        while let Some(i) = keep.work.pop() {
            read_die_into(&unit, &cu.abbrevs, dies[i].offset, &mut attrs)?;
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
            let from_code = in_code(dies[i].offset) || keep.via_code.contains(&i);
            for &t in &refs {
                keep.refer(dies, t, from_code);
            }
        }
        // The kept DIEs, in order.
        let mut kept: Vec<usize> = keep.mark.keys().copied().collect();
        kept.sort_unstable();
        // The line table's files the kept DIEs and the moved sequences
        // name (the unit's primary file too), renumbered in order.
        let mut used: BTreeSet<u64> = BTreeSet::new();
        if cu.line.is_some() {
            let first = u64::from(cu.header.version < 5);
            used.insert(first);
            used.insert(1);
            for s in cu.moved_seqs(self.moved) {
                src.files_set(&cu.header, s, &mut used)?;
            }
            for &m in self.moved {
                if let Some((rows, _)) = src.atom_rows(cu, m) {
                    used.extend(rows.iter().map(|r| r.file));
                }
            }
            for &i in &kept {
                if !matches!(keep.mark(i), SKELETON | WHOLE) {
                    continue;
                }
                read_die_into(&unit, &cu.abbrevs, dies[i].offset, &mut attrs)?;
                for a in &attrs {
                    if is_file(a.name) {
                        used.insert(file_value(&unit, a)?);
                    }
                }
            }
            let n = cu.header.files.len() as u64;
            used.retain(|&x| x >= first && x < n + first);
            self.files = vec![u64::MAX; (n + first) as usize];
            for (k, &old) in used.iter().enumerate() {
                self.files[old as usize] = k as u64 + first;
            }
        }
        // Each kept DIE's kept children.
        let mut children: HashMap<usize, Vec<usize>> = HashMap::new();
        for &i in &kept {
            let d = &dies[i];
            if d.parent != u32::MAX {
                children.entry(d.parent as usize).or_default().push(i);
            }
        }
        let none: Vec<usize> = Vec::new();
        let children_of = |d: usize| children.get(&d).unwrap_or(&none);
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
        let mut offsets: HashMap<usize, u64> = HashMap::new();
        offsets.insert(0, p.bytes.len() as u64);
        let q = self.top_die(cu, &unit, !children_of(0).is_empty())?;
        p.append(q);
        // Pre-order: (DIE, next child to write).
        let mut stack: Vec<(usize, usize)> = vec![(0, 0)];
        while let Some(top) = stack.last_mut() {
            let (d, next) = *top;
            if next < children_of(d).len() {
                let c = children_of(d)[next];
                top.1 += 1;
                offsets.insert(c, p.bytes.len() as u64);
                let decl = keep.mark(c) == DECLARATION;
                let q = self.die(cu, &unit, c, !children_of(c).is_empty(), decl)?;
                p.append(q);
                if !children_of(c).is_empty() {
                    stack.push((c, 0));
                }
            } else {
                if !children_of(d).is_empty() {
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
            let o = offsets.get(&d).copied().unwrap_or(u64::MAX);
            if o == u64::MAX {
                return Err("a reference to a DIE not kept");
            }
            write_fix(&mut p, pos, o, start as u64, fix, e, src.abs32)?;
        }
        let fixes = std::mem::take(&mut self.loc.fixes);
        for (pos, d, fix) in fixes {
            let o = offsets.get(&d).copied().unwrap_or(u64::MAX);
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
                a.put(src.sections[s].size, n, e);
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
                let x = self.src.sections[s];
                x.code && x.size > 0
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
                | DW_AT_GNU_PUBTYPES
                | DW_AT_DWO_NAME
                | DW_AT_GNU_DWO_NAME
                | DW_AT_GNU_DWO_ID => {}
                DW_AT_STMT_LIST => stmt = true,
                _ => self.attribute(cu, unit, 0, a, &attrs, &mut shape, &mut body)?,
            }
        }
        // A split unit's: what its skeleton says that it doesn't.
        if let Some(sk) = &self.src.skeleton {
            stmt |= sk.stmt.is_some();
            for (name, s) in &sk.strings {
                if !attrs.iter().any(|a| a.name == *name) {
                    shape.attrs.push((*name, DW_FORM_STRING, 0));
                    body.bytes.extend_from_slice(s);
                    body.bytes.push(0);
                }
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
            let len = self.src.sections[s].size;
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

    /// Writes the unit's line table: the original's header with only the
    /// files (and their directories) in `self.files`, renumbered, then the
    /// moved sections' sequences, their `DW_LNS_set_file`s renumbered. Its
    /// offset.
    fn line_table(&mut self, cu: &Cu, (start, _program, _end): (u64, u64, u64)) -> R<u64> {
        let h = &cu.header;
        let e = self.endian();
        let v5 = h.version >= 5;
        let at = self.line.bytes.len() as u64;
        let mut p = Piece::default();
        p.put(0, 4, e);
        // The version (and DWARF 5's address and selector sizes).
        self.copy(
            Kind::Line,
            start + 4,
            start + if v5 { 8 } else { 6 },
            &mut p,
        )?;
        let length_at = p.bytes.len();
        p.put(0, 4, e);
        self.copy(Kind::Line, h.fixed.0, h.fixed.1, &mut p)?;
        // The directories the kept files name.
        let first = u64::from(!v5);
        let mut dirs: BTreeSet<u64> = BTreeSet::new();
        if v5 {
            dirs.insert(0);
        }
        for (k, f) in h.files.iter().enumerate() {
            if self
                .files
                .get(k + first as usize)
                .is_some_and(|&n| n != u64::MAX)
            {
                dirs.insert(f.dir);
            }
        }
        let dir_first = u64::from(!v5);
        dirs.retain(|&d| d >= dir_first && d < h.dirs.len() as u64 + dir_first);
        let mut dir_map: BTreeMap<u64, u64> = BTreeMap::new();
        for (k, &d) in dirs.iter().enumerate() {
            dir_map.insert(d, k as u64 + dir_first);
        }
        if v5 {
            self.copy(Kind::Line, h.dir_formats.0, h.dir_formats.1, &mut p)?;
            p.uleb(dirs.len() as u64);
        }
        for &d in &dirs {
            let (s, t) = h.dirs[(d - dir_first) as usize];
            self.copy(Kind::Line, s, t, &mut p)?;
        }
        if v5 {
            self.copy(Kind::Line, h.file_formats.0, h.file_formats.1, &mut p)?;
            let kept = self.files.iter().filter(|&&n| n != u64::MAX).count();
            p.uleb(kept as u64);
        } else {
            p.bytes.push(0);
        }
        for (k, f) in h.files.clone().iter().enumerate() {
            if self
                .files
                .get(k + first as usize)
                .is_none_or(|&n| n == u64::MAX)
            {
                continue;
            }
            match f.field {
                Some((fs, fe, form)) => {
                    self.copy(Kind::Line, f.start, fs, &mut p)?;
                    let d = dir_map.get(&f.dir).copied().unwrap_or(0);
                    match form {
                        DW_FORM_DATA1 if d <= 0xFF => p.bytes.push(d as u8),
                        DW_FORM_DATA2 if d <= 0xFFFF => p.put(d, 2, e),
                        DW_FORM_UDATA => p.uleb(d),
                        _ => return Err("a directory index too large for its form"),
                    }
                    self.copy(Kind::Line, fe, f.end, &mut p)?;
                }
                None => self.copy(Kind::Line, f.start, f.end, &mut p)?,
            }
        }
        if !v5 {
            p.bytes.push(0);
        }
        let header_length = (p.bytes.len() - length_at - 4) as u64;
        let b = match e {
            Endian::Little => (header_length as u32).to_le_bytes(),
            Endian::Big => (header_length as u32).to_be_bytes(),
        };
        p.bytes[length_at..length_at + 4].copy_from_slice(&b);
        for s in cu.moved_seqs(self.moved) {
            self.sequence(h, s, &mut p)?;
        }
        // Mach-O: each moved atom's rows, a sequence of their own.
        if let Some(params) = &cu.params {
            for &m in self.moved {
                let Some((rows, size)) = self.src.atom_rows(cu, m) else {
                    continue;
                };
                let (a, _, _) = self.src.atom_extent(m).unwrap_or((0, 0, 0));
                let start = self.src.atom_extent(m).map_or(0, |x| x.1);
                let _ = a;
                let mut out: Vec<line::Row> = Vec::with_capacity(rows.len() + 1);
                for r in rows {
                    let file = self
                        .files
                        .get(r.file as usize)
                        .copied()
                        .filter(|&n| n != u64::MAX)
                        .ok_or("a file index not kept")?;
                    out.push(line::Row {
                        address: r.address - start,
                        file,
                        ..*r
                    });
                }
                let last = *out.last().ok_or("no rows")?;
                out.push(line::Row {
                    address: size,
                    end_sequence: true,
                    basic_block: false,
                    prologue_end: false,
                    epilogue_begin: false,
                    discriminator: 0,
                    ..last
                });
                let first = out[0].address;
                let (bytes, at) = line::encode(params, &[&out], core_endian(e))?;
                let base = p.bytes.len();
                p.bytes.extend_from_slice(&bytes);
                let n = usize::from(params.address_size);
                for k in at {
                    p.bytes[base + k..base + k + n].fill(0);
                    p.rels.push(OutRel {
                        offset: (base + k) as u64,
                        rtype: if n == 4 {
                            self.src.abs32
                        } else {
                            self.src.abs64
                        },
                        target: Target::Section(m),
                        addend: first as i64,
                    });
                }
            }
        }
        let len = p.bytes.len() - 4;
        let lb = match e {
            Endian::Little => (len as u32).to_le_bytes(),
            Endian::Big => (len as u32).to_be_bytes(),
        };
        p.bytes[..4].copy_from_slice(&lb);
        self.line.append(p);
        Ok(at)
    }

    /// Copies a sequence, its `DW_LNS_set_file`s renumbered.
    fn sequence(&mut self, h: &LineHeader, s: &Seq, p: &mut Piece) -> R<()> {
        let bad = "a line table that doesn't read";
        let src = self.src;
        let data = src.sec(Kind::Line);
        let mut r = Reader::new(data, s.start as usize, core_endian(src.endian));
        let opcode_base = h.lengths.len() as u8 + 1;
        let mut from = s.start;
        while (r.pos as u64) < s.end {
            let at = r.pos as u64;
            let op = r.u8().ok_or(bad)?;
            if op >= opcode_base {
                continue;
            }
            match op {
                0 => {
                    let len = r.uleb().ok_or(bad)?;
                    r.pos = r.pos.checked_add(len as usize).ok_or(bad)?;
                }
                9 => {
                    r.uint(2).ok_or(bad)?;
                }
                4 => {
                    let old = r.uleb().ok_or(bad)?;
                    let new = self
                        .files
                        .get(old as usize)
                        .copied()
                        .filter(|&n| n != u64::MAX)
                        .ok_or("a file index not kept")?;
                    self.copy(Kind::Line, from, at, p)?;
                    p.bytes.push(4);
                    p.uleb(new);
                    from = r.pos as u64;
                }
                _ => {
                    for _ in 0..h.lengths[usize::from(op) - 1] {
                        r.uleb().ok_or(bad)?;
                    }
                }
            }
        }
        self.copy(Kind::Line, from, s.end, p)
    }

    /// A DIE other than the unit's; as a declaration (its name only), if
    /// `decl`.
    fn die(&mut self, cu: &Cu, unit: &Unit<'s>, i: usize, children: bool, decl: bool) -> R<Piece> {
        let (abbrev, attrs) = read_die(unit, &cu.abbrevs, cu.dies[i].offset)?;
        let mut shape = Shape {
            tag: abbrev.tag,
            children,
            attrs: Vec::new(),
        };
        let mut body = Piece::default();
        for a in &attrs {
            if decl && a.name != DW_AT_NAME {
                continue;
            }
            self.attribute(cu, unit, i, a, &attrs, &mut shape, &mut body)?;
        }
        if decl {
            shape
                .attrs
                .push((DW_AT_DECLARATION, DW_FORM_FLAG_PRESENT, 0));
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
        if is_file(a.name) && !self.files.is_empty() {
            // Renumbered with the line table's files.
            let old = file_value(unit, a)?;
            let new = self
                .files
                .get(old as usize)
                .copied()
                .filter(|&x| x != u64::MAX)
                .ok_or("a file index not kept")?;
            shape.attrs.push((a.name, DW_FORM_UDATA, 0));
            p.uleb(new);
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
        cu.has_moved_seqs(moved) || moved.iter().any(|&m| src.atom_rows(cu, m).is_some())
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
            .position(|cu| cu.has_moved_seqs(moved))
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
        files: Vec::new(),
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
