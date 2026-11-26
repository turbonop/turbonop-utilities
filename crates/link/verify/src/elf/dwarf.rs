//! The DWARF of two linked ELF programs, compared by content.
//!
//! Folding moves a function's code, and (once turbolib moves debug
//! information) its DWARF, into another unit; a link of the original keeps
//! one copy's DWARF and tombstones the rest. So DWARF is compared by what
//! it describes, not where it is:
//!
//! - **Subprograms**: every live subprogram DIE (one whose `DW_AT_low_pc`,
//!   or first range, is in an executable section: linkers set discarded
//!   copies' addresses to 0 or all ones), keyed by the label at its entry
//!   address. Its subtree is compared DIE by DIE: each DIE's tag and
//!   attributes, sorted by name, by value: forms ignored (an address by
//!   what it names, through `.debug_addr` or not; strings by value;
//!   constants by number; references by the identity of what they name,
//!   see below), `DW_AT_sibling` left out, range and location lists as
//!   their entries (each range by what its start names and its length;
//!   expressions with `DW_OP_addr` and `DW_OP_addrx` operands as names and
//!   typed operations' and calls' references as identities),
//!   `DW_AT_GNU_locviews` as its view pairs, `DW_AT_high_pc` as a length,
//!   and files by path. Then the unit's producer, language, name, and
//!   compilation directory; and the unit's line rows over the
//!   subprogram's ranges (each row's address by name, file path, line,
//!   column, flags, and discriminator).
//! - **Variables at fixed addresses** (a location of one `DW_OP_addr`),
//!   by the label of the address, as sets of their DIEs' contents.
//! - **Named type definitions** (structures, classes, unions,
//!   enumerations, typedefs, base types; not declarations), as a set of
//!   (qualified name, contents) pairs.
//!
//! A DIE's identity (what a reference compares as) is its tag, qualified
//! name (the names of the namespaces, types, and subprograms around it),
//! name, linkage name, and the identity of what its `DW_AT_specification`
//! or `DW_AT_abstract_origin` names; for a DIE without a name, also its
//! size, declaration file and line, the identity of its type, and its
//! children's tags and names. (So a reference to a type's declaration and
//! one to its definition compare equal.) It doesn't follow a named type's members (those are
//! compared as type definitions); a reference cycle through anonymous
//! DIEs is cut where it closes.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt::Write as _;

use turbonop_dwarf::consts::*;
use turbonop_dwarf::{Abbrevs, Attr, LineProgram, Reader, Sections, Unit, UnitSection, Value};

use super::program::{CONTENT, Program};
use crate::hash::H128;
use crate::report::{Report, show};

const DW_AT_GNU_LOCVIEWS: u16 = 0x2137;

/// What canonical forms are written to: a hash, or text for examples.
trait Sink {
    fn num(&mut self, v: u64);
    fn text(&mut self, b: &[u8]);
    fn sep(&mut self, s: &str);
}

impl Sink for H128 {
    fn num(&mut self, v: u64) {
        self.u64(v);
    }
    fn text(&mut self, b: &[u8]) {
        self.u64(b.len() as u64);
        self.bytes(b);
    }
    fn sep(&mut self, s: &str) {
        self.bytes(s.as_bytes());
    }
}

impl Sink for String {
    fn num(&mut self, v: u64) {
        let _ = write!(self, "{v:#x}");
    }
    fn text(&mut self, b: &[u8]) {
        let _ = write!(self, "\"{}\"", show(b));
    }
    fn sep(&mut self, s: &str) {
        self.push_str(s);
    }
}

/// The program's DWARF sections.
fn sections<'a>(p: &Program<'a>, rep: &mut Report) -> Sections<'a> {
    let mut s = Sections::new(p.file.endian());
    for sec in &p.sections {
        let Some(name) = sec.name.strip_prefix(b".debug_") else {
            continue;
        };
        if sec.flags & turbonop_elf::consts::SHF_COMPRESSED != 0 {
            rep.unchecked("compressed debug sections", || show(sec.name));
            continue;
        }
        s.set(name, p.file.section_data(sec));
    }
    s
}

/// One DIE, read.
struct Die<'d> {
    offset: u64,
    tag: u16,
    depth: i64,
    parent: Option<usize>,
    attrs: Vec<Attr<'d>>,
    /// Where its subtree ends (the index after its last descendant).
    end: usize,
}

/// One unit, read whole.
struct Cu<'p, 'd> {
    p: &'p Program<'d>,
    unit: Unit<'d>,
    dies: Vec<Die<'d>>,
    line: Option<LineProgram<'d>>,
    comp_dir: &'d [u8],
    ids: Vec<u128>,
    /// GNU's split DWARF 4: the skeleton's `DW_AT_GNU_ranges_base`, which
    /// the split unit's range list offsets are from.
    ranges_base: u64,
}

const IN_PROGRESS: u128 = u128::MAX;

impl<'p, 'd> Cu<'p, 'd> {
    fn read(p: &'p Program<'d>, s: Sections<'d>, offset: u64) -> Option<Self> {
        Self::read_split(p, s, offset, None)
    }

    /// Reads a unit; a split one (of a `.dwo`, whose sections `s` holds
    /// with the program's `.debug_addr` and `.debug_line`) with what its
    /// skeleton gives it.
    fn read_split(
        p: &'p Program<'d>,
        s: Sections<'d>,
        offset: u64,
        skeleton: Option<Skeleton<'d>>,
    ) -> Option<Self> {
        let mut unit = Unit::at(s, offset)?;
        let abbrevs = Abbrevs::parse(s.abbrev, unit.abbrev_offset, s.endian)?;
        let mut r = unit.dies();
        let mut dies: Vec<Die<'d>> = Vec::new();
        let mut stack: Vec<usize> = Vec::new();
        let mut attrs = Vec::new();
        while let Some(a) = r.next(&abbrevs) {
            let depth = r.depth - i64::from(a.children);
            let offset = r.offset;
            r.attrs(&unit, a, &mut attrs)?;
            let d = usize::try_from(depth.max(0)).ok()?;
            stack.truncate(d);
            let i = dies.len();
            dies.push(Die {
                offset,
                tag: a.tag,
                depth,
                parent: stack.last().copied(),
                attrs: attrs.clone(),
                end: 0,
            });
            if a.children {
                stack.push(i);
            }
            if dies.len() == 1 {
                unit.set_bases(&dies[0].attrs);
            }
        }
        // Subtree ends.
        let n = dies.len();
        let mut open: Vec<usize> = Vec::new();
        for i in 0..n {
            while let Some(&o) = open.last() {
                if dies[o].depth >= dies[i].depth {
                    dies[o].end = i;
                    open.pop();
                } else {
                    break;
                }
            }
            open.push(i);
        }
        for o in open {
            dies[o].end = n;
        }
        let top = dies.first()?;
        let get = |name| top.attrs.iter().find(|a| a.name == name);
        let mut comp_dir = get(DW_AT_COMP_DIR)
            .and_then(|a| unit.string(&a.value))
            .map_or(&b""[..], |s| s.bytes);
        let mut stmt = get(DW_AT_STMT_LIST).and_then(|a| unit.uint(&a.value));
        let mut ranges_base = 0;
        if let Some(sk) = skeleton {
            // Its addresses are the skeleton's, its lists' offsets after
            // the `.dwo` sections' headers (GNU's DWARF 4: its location
            // lists of the split form, its range lists in the program's
            // `.debug_ranges` from the skeleton's base), its lines the
            // skeleton's.
            unit.addr_base = sk.addr_base;
            unit.low_pc = sk.low_pc;
            if let Some(base) = sk.gnu {
                unit.split = true;
                ranges_base = base;
            } else {
                if unit.loclists_base == 0 {
                    unit.loclists_base = 12;
                }
                if unit.rnglists_base == 0 {
                    unit.rnglists_base = 12;
                }
            }
            if comp_dir.is_empty() {
                comp_dir = sk.comp_dir;
            }
            stmt = sk.stmt;
        }
        let line = stmt.and_then(|o| turbonop_dwarf::line_program(&s, o, unit.address_size));
        Some(Cu {
            p,
            unit,
            ids: vec![0; dies.len()],
            dies,
            line,
            comp_dir,
            ranges_base,
        })
    }

    fn index_of(&self, offset: u64) -> Option<usize> {
        let k = self.dies.partition_point(|d| d.offset < offset);
        (self.dies.get(k)?.offset == offset).then_some(k)
    }

    fn attr(&self, i: usize, name: u16) -> Option<&Attr<'d>> {
        self.dies[i].attrs.iter().find(|a| a.name == name)
    }

    fn string(&self, i: usize, name: u16) -> Option<&'d [u8]> {
        let a = self.attr(i, name)?;
        self.unit.string(&a.value).map(|s| s.bytes)
    }

    fn file(&self, index: u64) -> String {
        self.line
            .as_ref()
            .and_then(|l| l.path(index, self.comp_dir))
            .unwrap_or_else(|| format!("file {index}"))
    }

    /// Whether `addr` is a live address: in an executable section, or any
    /// allocated one for data.
    fn live(&self, addr: u64, code: bool) -> bool {
        if !self.unit.is_valid_address(addr) {
            return false;
        }
        self.p
            .section_at(addr)
            .is_some_and(|s| !code || s.flags & turbonop_elf::consts::SHF_EXECINSTR != 0)
    }

    /// `addr` as one past the end of what holds the byte before it, if
    /// that byte is in a section and `addr` is in none.
    fn end_of_section(&self, addr: u64) -> Option<String> {
        if !self.unit.is_valid_address(addr) || self.p.section_at(addr).is_some() {
            return None;
        }
        let before = addr.checked_sub(1)?;
        self.p.section_at(before)?;
        match self.p.name(before)? {
            (_, CONTENT) => None,
            (n, o) => Some(format!("{}+{:#x}", show(n), u64::from(o) + 1)),
        }
    }

    fn address(&self, out: &mut dyn Sink, addr: u64) {
        if self.unit.is_valid_address(addr) && self.p.section_at(addr).is_some() {
            out.sep("@");
            out.text(self.p.render(addr).as_bytes());
        } else if let Some(end) = self.end_of_section(addr) {
            // One past a section's last byte (a tail call's
            // `DW_AT_call_return_pc` in the last function of `.text`): named
            // from the byte before it, as it is wherever something follows.
            out.sep("@");
            out.text(end.as_bytes());
        } else {
            out.sep("dead");
        }
    }

    /// The DIE a reference attribute names, in this unit.
    fn target(&self, a: &Attr<'d>) -> Option<usize> {
        let off = match a.form {
            DW_FORM_REF_ADDR => match a.value {
                Value::Uint(v) => v,
                _ => return None,
            },
            _ => self.unit.reference(a)?,
        };
        self.index_of(off)
    }

    fn is_ref(form: u16) -> bool {
        matches!(
            form,
            DW_FORM_REF1
                | DW_FORM_REF2
                | DW_FORM_REF4
                | DW_FORM_REF8
                | DW_FORM_REF_UDATA
                | DW_FORM_REF_ADDR
        )
    }

    /// A qualified name: the names around DIE `i`, outermost first.
    fn qualified(&self, i: usize, out: &mut dyn Sink) {
        let mut chain = Vec::new();
        let mut at = self.dies[i].parent;
        while let Some(j) = at {
            if self.dies[j].tag == DW_TAG_COMPILE_UNIT || self.dies[j].tag == DW_TAG_PARTIAL_UNIT {
                break;
            }
            chain.push(j);
            at = self.dies[j].parent;
        }
        for &j in chain.iter().rev() {
            out.num(u64::from(self.dies[j].tag));
            match self.scope_name(j) {
                Some(n) => out.text(n),
                None => out.sep("(anonymous)"),
            }
            out.sep("::");
        }
    }

    /// The name a scope goes by: its linkage name or name, or (a concrete
    /// subprogram's) those of what its `DW_AT_specification` or
    /// `DW_AT_abstract_origin` names.
    fn scope_name(&self, i: usize) -> Option<&'d [u8]> {
        let mut at = i;
        for _ in 0..4 {
            for n in [DW_AT_LINKAGE_NAME, DW_AT_MIPS_LINKAGE_NAME, DW_AT_NAME] {
                if let Some(s) = self.string(at, n) {
                    return Some(s);
                }
            }
            let next = [DW_AT_SPECIFICATION, DW_AT_ABSTRACT_ORIGIN]
                .iter()
                .find_map(|&n| self.attr(at, n).and_then(|a| self.target(a)))?;
            at = next;
        }
        None
    }

    /// DIE `i`'s identity (see the module's documentation).
    fn id(&mut self, i: usize) -> u128 {
        match self.ids[i] {
            0 => {}
            IN_PROGRESS => return 1,
            v => return v,
        }
        self.ids[i] = IN_PROGRESS;
        let mut h = H128::new();
        self.identity(i, &mut h);
        let v = h.finish().max(2);
        self.ids[i] = if v == IN_PROGRESS { 3 } else { v };
        self.ids[i]
    }

    fn identity(&mut self, i: usize, out: &mut dyn Sink) {
        out.num(u64::from(self.dies[i].tag));
        self.qualified(i, out);
        let name = self.string(i, DW_AT_NAME);
        for n in [DW_AT_NAME, DW_AT_LINKAGE_NAME, DW_AT_MIPS_LINKAGE_NAME] {
            if let Some(s) = self.string(i, n) {
                out.sep(" n");
                out.text(s);
            }
        }
        // (A named DIE goes by its name: a unit may have a type's
        // declaration where another has its definition. Definitions are
        // compared apart.)
        if name.is_none() {
            if let Some(v) = self
                .attr(i, DW_AT_BYTE_SIZE)
                .and_then(|a| self.unit.uint(&a.value))
            {
                out.sep(" size ");
                out.num(v);
            }
            if let Some(v) = self
                .attr(i, DW_AT_DECL_FILE)
                .and_then(|a| self.unit.uint(&a.value))
            {
                out.sep(" file ");
                out.text(self.file(v).as_bytes());
            }
            if let Some(v) = self
                .attr(i, DW_AT_DECL_LINE)
                .and_then(|a| self.unit.uint(&a.value))
            {
                out.sep(" line ");
                out.num(v);
            }
        }
        let mut follow = vec![DW_AT_SPECIFICATION, DW_AT_ABSTRACT_ORIGIN];
        if name.is_none() {
            follow.push(DW_AT_TYPE);
        }
        for n in follow {
            if let Some(a) = self.attr(i, n).copied() {
                out.sep(" ref ");
                out.num(u64::from(n));
                self.reference(&a, out);
            }
        }
        if name.is_none() {
            let (start, end) = (i + 1, self.dies[i].end);
            let depth = self.dies[i].depth;
            for j in start..end {
                if self.dies[j].depth == depth + 1 {
                    out.sep(" child ");
                    out.num(u64::from(self.dies[j].tag));
                    if let Some(n) = self.string(j, DW_AT_NAME) {
                        out.text(n);
                    }
                }
            }
        }
    }

    fn reference(&mut self, a: &Attr<'d>, out: &mut dyn Sink) {
        match a.form {
            DW_FORM_REF_SIG8 => {
                out.sep("signature ");
                out.num(match a.value {
                    Value::Uint(v) => v,
                    _ => 0,
                });
            }
            _ => match self.target(a) {
                Some(j) => {
                    let v = self.id(j);
                    out.num(v as u64);
                    out.num((v >> 64) as u64);
                }
                None => out.sep("(in another unit)"),
            },
        }
    }

    /// A reference by a unit-relative offset (expressions' typed
    /// operations and calls).
    fn unit_ref(&mut self, offset: u64, out: &mut dyn Sink) {
        match self.index_of(self.unit.offset.wrapping_add(offset)) {
            Some(j) => {
                let v = self.id(j);
                out.num(v as u64);
                out.num((v >> 64) as u64);
            }
            None => out.sep("(no DIE)"),
        }
    }

    fn section_ref(&mut self, offset: u64, out: &mut dyn Sink) {
        match self.index_of(offset) {
            Some(j) => {
                let v = self.id(j);
                out.num(v as u64);
                out.num((v >> 64) as u64);
            }
            None => out.sep("(in another unit)"),
        }
    }

    /// An expression, with addresses as names and references as
    /// identities.
    fn expression(&mut self, expr: &'d [u8], out: &mut dyn Sink) {
        let e = self.unit.sections().endian;
        let asize = usize::from(self.unit.address_size);
        let osize = if self.unit.dwarf64 { 8 } else { 4 };
        let mut r = Reader::new(expr, 0, e);
        while let Some(op) = r.u8() {
            out.sep(" op");
            // An indexed address is the address; an indexed constant, the
            // constant of the address's size (forms aren't compared).
            out.num(u64::from(match op {
                0xa1 | 0xfb => 0x03,
                0xa2 | 0xfc if asize == 8 => 0x0e,
                0xa2 | 0xfc => 0x0c,
                _ => op,
            }));
            let ok = (|| -> Option<()> {
                match op {
                    0x03 => {
                        let a = r.uint(asize)?;
                        self.address(out, a);
                    }
                    0xa1 | 0xfb => {
                        let i = r.uleb()?;
                        match self.unit.indexed_address(i) {
                            Some(a) => self.address(out, a),
                            None => out.sep("(no address)"),
                        }
                    }
                    0xa2 | 0xfc => {
                        let i = r.uleb()?;
                        out.num(self.unit.indexed_address(i).unwrap_or(u64::MAX));
                    }
                    0x08 | 0x09 | 0x15 | 0x94 | 0x95 => out.num(r.uint(1)?),
                    0x0a | 0x0b | 0x28 | 0x2f => out.num(r.uint(2)?),
                    0x0c | 0x0d => out.num(r.uint(4)?),
                    0x0e | 0x0f => out.num(r.uint(8)?),
                    0x10 | 0x23 | 0x90 | 0x93 => out.num(r.uleb()?),
                    0x11 | 0x91 | 0x70..=0x8f => out.num(r.sleb()? as u64),
                    0x92 => {
                        out.num(r.uleb()?);
                        out.num(r.sleb()? as u64);
                    }
                    0x9d => {
                        out.num(r.uleb()?);
                        out.num(r.uleb()?);
                    }
                    0x9e => {
                        let n = r.uleb()?;
                        out.text(r.bytes(n)?);
                    }
                    0x98 => {
                        let o = r.uint(2)?;
                        self.unit_ref(o, out);
                    }
                    0x99 | 0xfa => {
                        let o = r.uint(4)?;
                        self.unit_ref(o, out);
                    }
                    0x9a | 0xfd => {
                        let o = r.uint(osize)?;
                        self.section_ref(o, out);
                    }
                    0xa0 | 0xf2 => {
                        let o = r.uint(osize)?;
                        self.section_ref(o, out);
                        out.num(r.sleb()? as u64);
                    }
                    0xa3 | 0xf3 => {
                        let n = r.uleb()?;
                        let inner = r.bytes(n)?;
                        out.sep("(");
                        self.expression(inner, out);
                        out.sep(")");
                    }
                    0xa4 | 0xf4 => {
                        let o = r.uleb()?;
                        self.unit_ref(o, out);
                        let n = r.u8()?;
                        out.text(r.bytes(u64::from(n))?);
                    }
                    0xa5 | 0xf5 => {
                        out.num(r.uleb()?);
                        let o = r.uleb()?;
                        self.unit_ref(o, out);
                    }
                    0xa6 | 0xa7 | 0xf6 => {
                        out.num(r.uint(1)?);
                        let o = r.uleb()?;
                        self.unit_ref(o, out);
                    }
                    0xa8 | 0xa9 | 0xf7 | 0xf9 => {
                        let o = r.uleb()?;
                        self.unit_ref(o, out);
                    }
                    0x06
                    | 0x12..=0x14
                    | 0x16..=0x27
                    | 0x29..=0x2e
                    | 0x30..=0x6f
                    | 0x96
                    | 0x97
                    | 0x9b
                    | 0x9c
                    | 0x9f
                    | 0xe0
                    | 0xf0 => {}
                    _ => return None,
                }
                Some(())
            })();
            if ok.is_none() {
                out.sep(" raw ");
                out.text(&expr[r.pos.saturating_sub(1).min(expr.len())..]);
                return;
            }
        }
    }

    /// A location list's offset for an attribute, if its value is one.
    fn list_offset(&self, a: &Attr<'d>) -> Option<u64> {
        match (a.form, a.value) {
            (DW_FORM_LOCLISTX, Value::ListIndex(i)) => self.unit.loclist_offset(i),
            (DW_FORM_RNGLISTX, Value::ListIndex(i)) => self.unit.rnglist_offset(i),
            (DW_FORM_SEC_OFFSET, Value::Uint(v)) => Some(v),
            (DW_FORM_DATA4 | DW_FORM_DATA8, _) if self.unit.version < 4 => self.unit.uint(&a.value),
            _ => None,
        }
    }

    fn ranges(&self, a: &Attr<'d>) -> Option<Vec<(u64, u64)>> {
        let off = self.list_offset(a)?.checked_add(self.ranges_base)?;
        if self.unit.version >= 5 {
            self.unit.rnglist(off).map(|(v, _)| v)
        } else {
            self.unit.range_list(off, self.unit.low_pc).map(|(v, _)| v)
        }
    }

    /// DIE `i`'s entry address and ranges, if it has code.
    fn extent(&self, i: usize) -> Option<(u64, Vec<(u64, u64)>)> {
        if let Some(lo) = self
            .attr(i, DW_AT_LOW_PC)
            .and_then(|a| self.unit.uint(&a.value))
        {
            let len = match self.attr(i, DW_AT_HIGH_PC) {
                Some(a) if matches!(a.value, Value::Addr(_) | Value::AddrIndex(_)) => {
                    self.unit.uint(&a.value).map_or(0, |h| h.wrapping_sub(lo))
                }
                Some(a) => self.unit.uint(&a.value).unwrap_or(0),
                None => 0,
            };
            return Some((lo, vec![(lo, len)]));
        }
        let a = self.attr(i, DW_AT_RANGES)?;
        let r = self.ranges(a)?;
        let entry = self
            .attr(i, DW_AT_ENTRY_PC)
            .and_then(|a| self.unit.uint(&a.value))
            .or_else(|| r.first().map(|x| x.0))?;
        Some((entry, r))
    }

    fn location_like(name: u16) -> bool {
        matches!(
            name,
            0x02 | 0x19
                | 0x2a
                | 0x2f
                | 0x37
                | 0x38
                | 0x40
                | 0x46
                | 0x48
                | 0x4a
                | 0x4d
                | 0x4e
                | 0x4f
                | 0x50
                | 0x7e
                | 0x83
                | 0x85
                | 0x86
                | 0x2111
                | 0x2112
                | 0x2113
        )
    }

    fn location_list(&mut self, off: u64, out: &mut dyn Sink) -> usize {
        let Some(list) = self.unit.location_list(off) else {
            out.sep("(bad location list)");
            return 0;
        };
        for &(s, e, expr) in &list {
            out.sep(" [");
            if s == 0 && e == u64::MAX {
                out.sep("default");
            } else if s == e {
                // An empty range covers nothing: where it is (perhaps a
                // section's end, which no address names) isn't compared.
                out.sep("empty");
            } else {
                self.address(out, s);
                out.sep(" len ");
                out.num(e.wrapping_sub(s));
            }
            self.expression(expr, out);
            out.sep("]");
        }
        list.len()
    }

    /// The view pairs of `DW_AT_GNU_locviews` at `off`, `n` of them.
    fn views(&self, off: u64, n: usize, out: &mut dyn Sink) {
        let s = self.unit.sections();
        let data = if self.unit.version >= 5 {
            s.loclists
        } else {
            s.loc
        };
        let mut r = Reader::new(data, usize::try_from(off).unwrap_or(usize::MAX), s.endian);
        for _ in 0..n {
            match (r.uleb(), r.uleb()) {
                (Some(a), Some(b)) => {
                    out.num(a);
                    out.num(b);
                }
                _ => {
                    out.sep("(bad views)");
                    return;
                }
            }
        }
    }

    /// DIE `i`'s contents: its tag and attributes.
    fn canon(&mut self, i: usize, out: &mut dyn Sink) {
        out.num(u64::from(self.dies[i].tag));
        let mut attrs = self.dies[i].attrs.clone();
        attrs.sort_by_key(|a| a.name);
        let low = self
            .attr(i, DW_AT_LOW_PC)
            .and_then(|a| self.unit.uint(&a.value));
        let mut entries = 0usize;
        let mut views = None;
        for a in &attrs {
            match a.name {
                DW_AT_SIBLING
                | DW_AT_STMT_LIST
                | DW_AT_STR_OFFSETS_BASE
                | DW_AT_ADDR_BASE
                | DW_AT_RNGLISTS_BASE
                | DW_AT_LOCLISTS_BASE
                | DW_AT_GNU_ADDR_BASE
                | 0x2134
                | DW_AT_MACROS
                | DW_AT_MACRO_INFO
                | 0x2119 => continue,
                _ => {}
            }
            out.sep(" a");
            out.num(u64::from(a.name));
            out.sep("=");
            match a.name {
                DW_AT_GNU_LOCVIEWS => {
                    views = self.list_offset(a).or(match a.value {
                        Value::Uint(v) => Some(v),
                        _ => None,
                    });
                    out.sep("views");
                    continue;
                }
                DW_AT_DECL_FILE | DW_AT_CALL_FILE => {
                    let f = self.unit.uint(&a.value).unwrap_or(u64::MAX);
                    out.text(self.file(f).as_bytes());
                    continue;
                }
                DW_AT_HIGH_PC if matches!(a.value, Value::Addr(_) | Value::AddrIndex(_)) => {
                    let h = self.unit.uint(&a.value).unwrap_or(0);
                    out.sep("len ");
                    out.num(h.wrapping_sub(low.unwrap_or(0)));
                    continue;
                }
                DW_AT_RANGES => {
                    match self.ranges(a) {
                        Some(r) => {
                            for (s, n) in r {
                                out.sep(" [");
                                self.address(out, s);
                                out.sep(" len ");
                                out.num(n);
                                out.sep("]");
                            }
                        }
                        None => out.sep("(bad range list)"),
                    }
                    continue;
                }
                n if Self::location_like(n) => {
                    if let Value::Block(b) = a.value {
                        self.expression(b, out);
                        continue;
                    }
                    if let Some(off) = self.list_offset(a) {
                        entries = self.location_list(off, out);
                        continue;
                    }
                }
                _ => {}
            }
            if Self::is_ref(a.form) || a.form == DW_FORM_REF_SIG8 {
                self.reference(a, out);
                continue;
            }
            match a.value {
                Value::Addr(_) | Value::AddrIndex(_) => match self.unit.uint(&a.value) {
                    Some(x) => self.address(out, x),
                    None => out.sep("(no address)"),
                },
                Value::Str(_) | Value::StrOffset(..) | Value::StrIndex(_) => {
                    match self.unit.string(&a.value) {
                        Some(s) => out.text(s.bytes),
                        None => out.sep("(no string)"),
                    }
                }
                Value::Uint(v) => out.num(v),
                Value::Sint(v) => out.num(v as u64),
                Value::Data(b) => match self.unit.uint(&a.value) {
                    Some(v) if b.len() <= 8 => out.num(v),
                    _ => out.text(b),
                },
                Value::Block(b) => out.text(b),
                Value::ListIndex(v) => {
                    out.sep("list ");
                    out.num(v);
                }
            }
        }
        if let Some(off) = views {
            out.sep(" views");
            self.views(off, entries, out);
        }
    }

    fn canon_hash(&mut self, i: usize) -> u128 {
        let mut h = H128::new();
        self.canon(i, &mut h);
        h.finish()
    }

    fn canon_text(&mut self, i: usize) -> String {
        let mut s = String::new();
        self.canon(i, &mut s);
        s
    }

    fn unit_attrs(&self, out: &mut dyn Sink) {
        for n in [DW_AT_PRODUCER, DW_AT_NAME, DW_AT_COMP_DIR] {
            out.sep(" ");
            match self.string(0, n) {
                Some(s) => out.text(s),
                None => out.sep("none"),
            }
        }
        out.sep(" language ");
        out.num(
            self.attr(0, DW_AT_LANGUAGE)
                .and_then(|a| self.unit.uint(&a.value))
                .unwrap_or(0),
        );
        out.sep(" version ");
        out.num(u64::from(self.unit.version));
    }

    /// The line rows over `ranges` (`rows` with each one's sequence's
    /// number, by address).
    fn lines(
        &self,
        rows: &[(turbonop_dwarf::Row, u64)],
        ranges: &[(u64, u64)],
        out: &mut dyn Sink,
    ) {
        for &(s, n) in ranges {
            out.sep(" range ");
            self.address(out, s);
            let lo = rows.partition_point(|r| r.0.address < s);
            // The sequences with rows in the range: an end of sequence at
            // either end is another sequence's unless it is one of them.
            let mut seqs: Vec<u64> = rows[lo..]
                .iter()
                .take_while(|r| r.0.address < s + n)
                .filter(|r| !r.0.end_sequence)
                .map(|r| r.1)
                .collect();
            seqs.sort_unstable();
            seqs.dedup();
            for (r, seq) in &rows[lo..] {
                if r.address > s + n {
                    break;
                }
                if r.address == s + n && !r.end_sequence {
                    continue;
                }
                if r.end_sequence && seqs.binary_search(seq).is_err() {
                    continue;
                }
                out.sep(" row +");
                out.num(r.address - s);
                out.text(self.file(r.file).as_bytes());
                out.num(r.line);
                out.num(r.column);
                out.num(
                    u64::from(r.is_stmt)
                        | u64::from(r.end_sequence) << 1
                        | u64::from(r.prologue_end) << 2
                        | u64::from(r.epilogue_begin) << 3,
                );
                out.num(r.discriminator);
            }
        }
    }

    /// The live rows, by address.
    fn live_rows(&self) -> Vec<(turbonop_dwarf::Row, u64)> {
        let mut rows = Vec::new();
        if let Some(l) = &self.line {
            let mut seq = 0u64;
            for r in &l.rows {
                if !r.discarded {
                    rows.push((*r, seq));
                }
                if r.end_sequence {
                    seq += 1;
                }
            }
        }
        rows.sort_by_key(|r| r.0.address);
        rows
    }
}

/// A live subprogram, as compared.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
struct Sub {
    unit_attrs: u128,
    /// Each DIE of its subtree: (depth from it, tag, contents).
    dies: Vec<(i64, u16, u128)>,
    lines: u128,
    /// Where it is (unit offset, DIE index), for examples.
    at: (u64, usize),
}

impl Sub {
    fn same(&self, o: &Sub) -> bool {
        self.unit_attrs == o.unit_attrs && self.dies == o.dies && self.lines == o.lines
    }
}

/// What one unit holds.
#[derive(Default)]
struct UnitOut {
    subs: Vec<(String, Sub)>,
    dead: u64,
    variables: Vec<(String, u128)>,
    dead_variables: u64,
    /// (qualified name's hash, contents, (unit, DIE)).
    types: Vec<(u128, u128, (u64, usize))>,
    unread: u64,
}

fn is_type(tag: u16) -> bool {
    matches!(
        tag,
        DW_TAG_STRUCTURE_TYPE
            | DW_TAG_CLASS_TYPE
            | DW_TAG_UNION_TYPE
            | DW_TAG_ENUMERATION_TYPE
            | DW_TAG_TYPEDEF
            | DW_TAG_BASE_TYPE
    )
}

/// The contents of DIE `i`'s subtree, with each nested subprogram that
/// has code of its own (compared apart) as its identity alone.
fn subtree(cu: &mut Cu<'_, '_>, code: &[bool], i: usize, out: &mut dyn Sink) {
    let end = cu.dies[i].end.max(i + 1);
    let depth = cu.dies[i].depth;
    let mut j = i;
    while j < end {
        out.num((cu.dies[j].depth - depth) as u64);
        if j > i && code[j] {
            out.sep("nested");
            let id = cu.id(j);
            out.num(id as u64);
            out.num((id >> 64) as u64);
            j = cu.dies[j].end.max(j + 1);
            continue;
        }
        cu.canon(j, out);
        j += 1;
    }
}

/// What a split unit takes from its skeleton (in the program).
#[derive(Clone, Copy, Debug)]
struct Skeleton<'d> {
    /// GNU's split DWARF 4 (`DW_AT_GNU_dwo_id`): `DW_AT_GNU_ranges_base`.
    gnu: Option<u64>,
    addr_base: u64,
    low_pc: u64,
    stmt: Option<u64>,
    comp_dir: &'d [u8],
}

/// The skeleton unit at `offset`, if it is one: what its split unit
/// takes, its ID, and where its `.dwo` file is (`DW_AT_dwo_name` in
/// `DW_AT_comp_dir`).
fn skeleton<'a>(
    p: &Program<'a>,
    s: Sections<'a>,
    offset: u64,
) -> Option<(Skeleton<'a>, u64, std::path::PathBuf)> {
    let u = Unit::at(s, offset)?;
    let gnu_id = if u.version < 5 {
        gnu_dwo_id(s, &u)
    } else {
        None
    };
    if u.unit_type != DW_UT_SKELETON && gnu_id.is_none() {
        return None;
    }
    let cu = Cu::read(p, s, offset)?;
    let name = cu
        .string(0, DW_AT_DWO_NAME)
        .or_else(|| cu.string(0, DW_AT_GNU_DWO_NAME))?;
    let gnu = gnu_id.map(|_| {
        cu.attr(0, DW_AT_GNU_RANGES_BASE)
            .and_then(|a| cu.unit.uint(&a.value))
            .unwrap_or(0)
    });
    let stmt = cu
        .attr(0, DW_AT_STMT_LIST)
        .and_then(|a| cu.unit.uint(&a.value));
    let name = std::path::PathBuf::from(String::from_utf8_lossy(name).into_owned());
    let path = if name.is_absolute() {
        name
    } else {
        std::path::Path::new(&*String::from_utf8_lossy(cu.comp_dir)).join(name)
    };
    Some((
        Skeleton {
            gnu,
            addr_base: cu.unit.addr_base,
            low_pc: cu.unit.low_pc,
            stmt,
            comp_dir: cu.comp_dir,
        },
        cu.unit.dwo_id.or(gnu_id)?,
        path,
    ))
}

/// A DWARF 4 unit's `DW_AT_GNU_dwo_id` (GNU's split DWARF: a skeleton's,
/// or its split unit's in the `.dwo`), read from its first DIE alone.
fn gnu_dwo_id(s: Sections<'_>, u: &Unit<'_>) -> Option<u64> {
    let abbrevs = Abbrevs::parse(s.abbrev, u.abbrev_offset, s.endian)?;
    let mut r = u.dies();
    let a = r.next(&abbrevs)?;
    let mut attrs = Vec::new();
    r.attrs(u, a, &mut attrs)?;
    let id = attrs.iter().find(|x| x.name == DW_AT_GNU_DWO_ID)?;
    u.uint(&id.value)
}

/// The split unit of the skeleton at `offset`, from its `.dwo` file
/// (`dwo`), read with the program's addresses and lines.
fn read_split_unit(p: &Program<'_>, s: Sections<'_>, offset: u64, dwo: &[u8]) -> UnitOut {
    let unread = UnitOut {
        unread: 1,
        ..UnitOut::default()
    };
    let Some((sk, id, _)) = skeleton(p, s, offset) else {
        return unread;
    };
    let Ok(file) = turbonop_elf::ElfFile::parse(dwo) else {
        return unread;
    };
    let mut d = Sections::new(s.endian);
    d.addr = s.addr;
    d.line = s.line;
    d.line_str = s.line_str;
    if sk.gnu.is_some() {
        // GNU's DWARF 4: range lists in the program.
        d.ranges = s.ranges;
    }
    for sec in file.sections() {
        let Some(name) = sec
            .name
            .strip_suffix(b".dwo")
            .and_then(|n| n.strip_prefix(b".debug_"))
        else {
            continue;
        };
        if !matches!(name, b"line" | b"addr" | b"line_str") {
            d.set(name, file.section_data(&sec));
        }
    }
    let Some(u) = Unit::iter(d, UnitSection::Info, 0).find(|u| match sk.gnu {
        Some(_) => u.version < 5 && gnu_dwo_id(d, u) == Some(id),
        None => u.unit_type == DW_UT_SPLIT_COMPILE && u.dwo_id == Some(id),
    }) else {
        return unread;
    };
    read_unit_with(p, d, u.offset, Some(sk))
}
fn read_unit(p: &Program<'_>, s: Sections<'_>, offset: u64) -> UnitOut {
    read_unit_with(p, s, offset, None)
}

fn read_unit_with(
    p: &Program<'_>,
    s: Sections<'_>,
    offset: u64,
    skeleton: Option<Skeleton<'_>>,
) -> UnitOut {
    let mut out = UnitOut::default();
    let Some(mut cu) = Cu::read_split(p, s, offset, skeleton) else {
        out.unread = 1;
        return out;
    };
    let mut ua = H128::new();
    cu.unit_attrs(&mut ua);
    let unit_attrs = ua.finish();
    let rows = cu.live_rows();
    let n = cu.dies.len();
    // Subprograms with code of their own: each compared apart, also when
    // nested in another (a lambda's, a local class's), whose subtree then
    // holds its identity alone (its code may be another unit's).
    let code: Vec<bool> = cu
        .dies
        .iter()
        .map(|d| {
            d.tag == DW_TAG_SUBPROGRAM
                && d.attrs
                    .iter()
                    .any(|a| a.name == DW_AT_LOW_PC || a.name == DW_AT_RANGES)
        })
        .collect();
    let mut i = 1;
    while i < n {
        let tag = cu.dies[i].tag;
        let end = cu.dies[i].end.max(i + 1);
        if code[i]
            && let Some((entry, ranges)) = cu.extent(i)
        {
            if cu.live(entry, true) {
                let depth = cu.dies[i].depth;
                let mut dies = Vec::new();
                let mut j = i;
                while j < end {
                    let d = cu.dies[j].depth - depth;
                    if j > i && code[j] {
                        let id = cu.id(j);
                        dies.push((d, cu.dies[j].tag, id ^ 0x6E65_7374));
                        j = cu.dies[j].end.max(j + 1);
                        continue;
                    }
                    dies.push((d, cu.dies[j].tag, cu.canon_hash(j)));
                    j += 1;
                }
                let mut lh = H128::new();
                cu.lines(&rows, &ranges, &mut lh);
                out.subs.push((
                    p.render(entry),
                    Sub {
                        unit_attrs,
                        dies,
                        lines: lh.finish(),
                        at: (offset, i),
                    },
                ));
            } else {
                out.dead += 1;
            }
        }
        if tag == DW_TAG_VARIABLE
            && let Some(addr) = cu
                .attr(i, DW_AT_LOCATION)
                .copied()
                .and_then(|a| cu.unit.location_address(&a.value))
        {
            if cu.live(addr, false) {
                let mut h = H128::new();
                subtree(&mut cu, &code, i, &mut h);
                out.variables.push((p.render(addr), h.finish()));
            } else {
                out.dead_variables += 1;
            }
        }
        if is_type(tag)
            && !cu.dies[i].attrs.iter().any(|a| a.name == DW_AT_DECLARATION)
            && cu.string(i, DW_AT_NAME).is_some()
        {
            let mut k = H128::new();
            k.num(u64::from(tag));
            cu.qualified(i, &mut k);
            k.text(cu.string(i, DW_AT_NAME).unwrap_or(b""));
            let mut h = H128::new();
            subtree(&mut cu, &code, i, &mut h);
            out.types.push((k.finish(), h.finish(), (offset, i)));
        }
        i += 1;
    }
    out
}

/// Everything one program's DWARF holds, gathered.
#[derive(Default)]
struct All {
    subs: BTreeMap<String, Vec<Sub>>,
    dead: u64,
    variables: BTreeMap<String, BTreeSet<u128>>,
    dead_variables: u64,
    types: BTreeMap<(u128, u128), (u64, usize)>,
    units: u64,
    unread: u64,
}

fn gather<'a>(p: &Program<'a>, s: Sections<'a>) -> All {
    let offsets: Vec<u64> = Unit::iter(s, UnitSection::Info, 0)
        .filter(|u| {
            u.unit_type == DW_UT_COMPILE
                || u.unit_type == DW_UT_PARTIAL
                || u.unit_type == DW_UT_SKELETON
        })
        .map(|u| u.offset)
        .collect();
    // Split units' `.dwo` files, read once (where they are as the
    // skeletons say; a missing one leaves its unit unread).
    let dwos: Vec<Option<Vec<u8>>> = offsets
        .iter()
        .map(|&o| skeleton(p, s, o).and_then(|(_, _, path)| std::fs::read(path).ok()))
        .collect();
    let at: Vec<usize> = (0..offsets.len()).collect();
    let outs = turbonop_par::map_balanced(&at, 4, |&k| match &dwos[k] {
        Some(d) => read_split_unit(p, s, offsets[k], d),
        None => read_unit(p, s, offsets[k]),
    });
    let mut all = All {
        units: offsets.len() as u64,
        ..All::default()
    };
    for o in outs {
        for (k, sub) in o.subs {
            all.subs.entry(k).or_default().push(sub);
        }
        all.dead += o.dead;
        for (k, h) in o.variables {
            all.variables.entry(k).or_default().insert(h);
        }
        all.dead_variables += o.dead_variables;
        for (k, h, at) in o.types {
            all.types.entry((k, h)).or_insert(at);
        }
        all.unread += o.unread;
    }
    // By place (the contents' hashes are keyed per process, so ordering
    // by them would give each run its own examples).
    for v in all.subs.values_mut() {
        v.sort_by_key(|s| s.at);
    }
    all
}

/// A type definition's qualified name, for examples.
fn type_name(p: &Program<'_>, s: Sections<'_>, at: (u64, usize)) -> String {
    let Some(cu) = Cu::read(p, s, at.0) else {
        return "?".into();
    };
    let mut t = String::new();
    cu.qualified(at.1, &mut t);
    t.push_str(&show(cu.string(at.1, DW_AT_NAME).unwrap_or(b"")));
    t
}

/// Where two subprograms first differ, as text.
fn explain(
    pa: &Program<'_>,
    sa: Sections<'_>,
    a: &Sub,
    pb: &Program<'_>,
    sb: Sections<'_>,
    b: &Sub,
) -> String {
    let unit = |p: &Program<'_>, s: Sections<'_>, at: (u64, usize)| -> String {
        Cu::read(p, s, at.0)
            .and_then(|cu| cu.string(0, DW_AT_NAME).map(show))
            .unwrap_or_default()
    };
    let units = if a.unit_attrs == b.unit_attrs {
        String::new()
    } else {
        format!("(units {} and {}) ", unit(pa, sa, a.at), unit(pb, sb, b.at))
    };
    if a.dies == b.dies && a.lines == b.lines {
        let text = |p: &Program<'_>, s: Sections<'_>, at: (u64, usize)| {
            let mut t = String::new();
            if let Some(cu) = Cu::read(p, s, at.0) {
                cu.unit_attrs(&mut t);
            }
            t
        };
        return format!("unit:{} and{}", text(pa, sa, a.at), text(pb, sb, b.at));
    }
    format!("{units}{}", explain_contents(pa, sa, a, pb, sb, b))
}

fn explain_contents(
    pa: &Program<'_>,
    sa: Sections<'_>,
    a: &Sub,
    pb: &Program<'_>,
    sb: Sections<'_>,
    b: &Sub,
) -> String {
    if a.dies != b.dies {
        let k = a
            .dies
            .iter()
            .zip(&b.dies)
            .position(|(x, y)| x != y)
            .unwrap_or(a.dies.len().min(b.dies.len()));
        let text = |p: &Program<'_>, s: Sections<'_>, at: (u64, usize)| -> String {
            match Cu::read(p, s, at.0) {
                Some(mut cu) if at.1 + k < cu.dies.len() => cu.canon_text(at.1 + k),
                _ => "nothing".into(),
            }
        };
        return format!(
            "DIE {k} of {} and {}: {} and {}",
            a.dies.len(),
            b.dies.len(),
            text(pa, sa, a.at),
            text(pb, sb, b.at)
        );
    }
    let text = |p: &Program<'_>, s: Sections<'_>, sub: &Sub| -> Vec<String> {
        let Some(cu) = Cu::read(p, s, sub.at.0) else {
            return Vec::new();
        };
        let mut t = String::new();
        if let Some((_, ranges)) = cu.extent(sub.at.1) {
            cu.lines(&cu.live_rows(), &ranges, &mut t);
        }
        t.split(" r").map(str::to_string).collect()
    };
    let (ta, tb) = (text(pa, sa, a), text(pb, sb, b));
    let k = ta
        .iter()
        .zip(&tb)
        .position(|(x, y)| x != y)
        .unwrap_or(ta.len().min(tb.len()));
    format!(
        "line rows: item {k} of {} and {}: {} and {}",
        ta.len(),
        tb.len(),
        ta.get(k).map_or("nothing", String::as_str),
        tb.get(k).map_or("nothing", String::as_str)
    )
}

/// Compares the two programs' DWARF (see the module's documentation).
pub(crate) fn compare(a: &Program<'_>, b: &Program<'_>) -> Report {
    let mut rep = Report::new();
    let sa = sections(a, &mut rep);
    let sb = sections(b, &mut rep);
    if sa.info.is_empty() && sb.info.is_empty() {
        rep.unchecked("DWARF (neither program has any)", String::new);
        return rep;
    }
    let (x, y) = turbonop_par::join(|| gather(a, sa), || gather(b, sb));
    rep.checked("DWARF units (first program)", x.units);
    rep.checked("DWARF units (second program)", y.units);
    if x.unread + y.unread > 0 {
        rep.fail("DWARF units that don't read", || {
            format!("{} and {}", x.unread, y.unread)
        });
    }
    rep.checked(
        "live subprograms",
        x.subs.values().map(|v| v.len() as u64).sum(),
    );
    rep.expect("dead subprograms (tombstoned), first program", || {
        x.dead.to_string()
    });
    rep.expect("dead subprograms (tombstoned), second program", || {
        y.dead.to_string()
    });
    for (k, va) in &x.subs {
        let Some(vb) = y.subs.get(k) else {
            rep.fail("subprograms only in the first program", || {
                show(k.as_bytes())
            });
            continue;
        };
        // As sets: GNU ld and gold point a discarded copy's DWARF at the
        // kept copy (where lld and mold tombstone it), so a subprogram may
        // be described several times, once per unit that had a copy.
        let distinct = |v: &[Sub]| -> Vec<Sub> {
            let mut d: Vec<Sub> = Vec::new();
            for x in v {
                if !d.iter().any(|y| y.same(x)) {
                    d.push(x.clone());
                }
            }
            d
        };
        let (va, vb) = (distinct(va), distinct(vb));
        let in_a = vb.iter().all(|x| va.iter().any(|y| y.same(x)));
        let in_b = va.iter().all(|x| vb.iter().any(|y| y.same(x)));
        if in_a && in_b {
            continue;
        }
        if in_a {
            rep.expect(
                "subprograms described by more units' copies in the first program (GNU ld and gold give discarded copies the kept one's addresses)",
                || format!("{k}: {} and {}", va.len(), vb.len()),
            );
            continue;
        }
        // The second's descriptions that the first doesn't have, each
        // classified against the first's closest one; the worst decides.
        let mut worst: Option<(u8, &Sub, &Sub)> = None;
        for x in vb.iter().filter(|x| !va.iter().any(|y| y.same(x))) {
            let Some((rank, y)) = va
                .iter()
                .map(|y| {
                    let rank = match (y.dies == x.dies, y.lines == x.lines) {
                        (true, true) => 0,
                        (true, false) => 1,
                        _ => 2,
                    };
                    (rank, y)
                })
                .min_by_key(|&(rank, y)| (rank, y.at))
            else {
                continue;
            };
            if worst.is_none_or(|(w, ..)| rank > w) {
                worst = Some((rank, y, x));
            }
        }
        if let Some((rank, s, other)) = worst {
            match rank {
                // The same DIEs and rows, from another unit: a link keeps the
                // first loaded copy's, turbolib the first member's.
                0 => rep.expect(
                    "subprograms described by another unit's copy (the same DIEs and line rows)",
                    || format!("{k}: {}", explain(a, sa, s, b, sb, other)),
                ),
                1 => rep.fail("subprograms whose line rows differ", || {
                    format!("{k}: {}", explain(a, sa, s, b, sb, other))
                }),
                _ => rep.fail("subprograms whose DIEs differ", || {
                    format!("{k}: {}", explain(a, sa, s, b, sb, other))
                }),
            }
        }
    }
    for k in y.subs.keys() {
        if !x.subs.contains_key(k) {
            rep.fail("subprograms only in the second program", || {
                show(k.as_bytes())
            });
        }
    }
    rep.checked("variables at fixed addresses", x.variables.len() as u64);
    rep.expect("dead variables (tombstoned), first program", || {
        x.dead_variables.to_string()
    });
    rep.expect("dead variables (tombstoned), second program", || {
        y.dead_variables.to_string()
    });
    for (k, va) in &x.variables {
        match y.variables.get(k) {
            None => rep.fail("variables only in the first program", || k.clone()),
            Some(vb) if vb != va => rep.fail("variables whose DIEs differ", || k.clone()),
            _ => {}
        }
    }
    for k in y.variables.keys() {
        if !x.variables.contains_key(k) {
            rep.fail("variables only in the second program", || k.clone());
        }
    }
    rep.checked("type definitions (distinct)", x.types.len() as u64);
    let names = |t: &BTreeMap<(u128, u128), (u64, usize)>| -> BTreeSet<u128> {
        t.keys().map(|k| k.0).collect()
    };
    let (na, nb) = (names(&x.types), names(&y.types));
    for (k, &at) in &x.types {
        if !y.types.contains_key(k) {
            let cat = if nb.contains(&k.0) {
                "type definitions with contents only in the first program"
            } else {
                "type definitions only in the first program"
            };
            rep.fail(cat, || type_name(a, sa, at));
        }
    }
    for (k, &at) in &y.types {
        if !x.types.contains_key(k) {
            let cat = if na.contains(&k.0) {
                "type definitions with contents only in the second program"
            } else {
                "type definitions only in the second program"
            };
            rep.fail(cat, || type_name(b, sb, at));
        }
    }
    rep
}
