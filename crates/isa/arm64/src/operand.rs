//! An instruction's operands, typed: registers (with their width, and
//! for vectors their arrangement and lane), lists of them, immediates,
//! labels, conditions, shifts and extends, memory addresses (with their
//! indexing and writeback), and SME's ZA slices.
//!
//! The operands are the template's (what the formatter shows, in its
//! order; an alias's, for an alias), each classified when the tables are
//! built (tools/isa-gen's `operand_specs`) and evaluated here from the word.
//! Each says how the instruction uses it ([`Access`], from LLVM's
//! records); [`Instruction::written_registers`] has the complete write
//! set, implicit registers included.

use crate::access::{Element, Reg};
use crate::format::{Special, bitmask, fp_imm8, is_fp_imm8, number, raw, raw_value, special_value};
use crate::tables::{
    Kind, NONE, OpSpec, REG_CLASSES, Symbol, attr, file, opk, role, symbol, value,
};
use crate::{Inst, Instruction};

/// How an instruction uses an operand: for a register, whether it reads
/// or writes it; for memory, whether it loads or stores.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Access {
    /// Neither (an immediate, a prefetch's address).
    None,
    /// Read (or loaded from).
    Read,
    /// Written (or stored to).
    Write,
    /// Both.
    ReadWrite,
}

impl Access {
    fn from_bits(b: u8) -> Access {
        match b & 3 {
            1 => Access::Read,
            2 => Access::Write,
            3 => Access::ReadWrite,
            _ => Access::None,
        }
    }

    /// Whether it reads.
    pub fn reads(self) -> bool {
        matches!(self, Access::Read | Access::ReadWrite)
    }

    /// Whether it writes.
    pub fn writes(self) -> bool {
        matches!(self, Access::Write | Access::ReadWrite)
    }
}

/// A vector's arrangement (`16B`: 16 lanes of bytes), or an element size
/// alone (`D` of an SVE register, or of an indexed lane): `lanes` 0.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Arrangement {
    /// The number of lanes (0 for an SVE vector or a lone element).
    pub lanes: u8,
    /// The lanes' size.
    pub element: Element,
}

impl Arrangement {
    /// The arrangement's size in bits, if it has lanes (64 or 128).
    pub fn bits(self) -> Option<u32> {
        (self.lanes != 0).then(|| u32::from(self.lanes) * self.element.bits())
    }

    fn parse(t: &str) -> Option<Arrangement> {
        let b = t.as_bytes();
        let (&e, digits) = b.split_last()?;
        let element = Element::from_letter(e)?;
        let lanes = if digits.is_empty() {
            0
        } else {
            core::str::from_utf8(digits).ok()?.parse().ok()?
        };
        Some(Arrangement { lanes, element })
    }
}

/// SVE predication: merging (`/M`) or zeroing (`/Z`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Predication {
    /// No qualifier.
    None,
    /// `/M`: inactive elements keep their values.
    Merging,
    /// `/Z`: inactive elements are set to zero.
    Zeroing,
}

/// A register operand.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct RegOperand {
    /// The register, with its width.
    pub reg: Reg,
    /// A vector's arrangement, or an SVE register's or tile's element
    /// size.
    pub arrangement: Option<Arrangement>,
    /// A lane (`v0.s[1]`), or SME2's ZT0 offset.
    pub lane: Option<u8>,
    /// A predicate's qualifier (`p0/m`).
    pub predication: Predication,
    /// PSEL's predicate select: a register and an immediate
    /// (`p0.s[w12, 1]`).
    pub select: Option<(Reg, u8)>,
    /// Written back in place (MOPS's `x2!`).
    pub writeback: bool,
}

/// A list of registers (`{ v0.16b, v1.16b }`, `{ z0.d-z3.d }`,
/// `{ z0.s, z8.s }`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct RegList {
    /// The first register.
    pub first: Reg,
    /// How many.
    pub count: u8,
    /// How far apart (1: consecutive, modulo 32 for V and Z, 16 for P).
    pub stride: u8,
    /// Their arrangement or element size.
    pub arrangement: Option<Arrangement>,
    /// A lane, for structure loads and stores of one lane.
    pub lane: Option<u8>,
}

impl RegList {
    /// The list's registers, in order.
    pub fn iter(&self) -> impl Iterator<Item = Reg> {
        let (first, stride) = (*self, self.stride);
        (0..self.count).map(move |i| first.nth(u32::from(i) * u32::from(stride)))
    }

    fn nth(&self, k: u32) -> Reg {
        let wrap = |n: u8, m: u32| ((u32::from(n) + k) % m) as u8;
        match self.first {
            Reg::V(n) => Reg::V(wrap(n, 32)),
            Reg::Q(n) => Reg::Q(wrap(n, 32)),
            Reg::D(n) => Reg::D(wrap(n, 32)),
            Reg::Z(n) => Reg::Z(wrap(n, 32)),
            Reg::P(n) => Reg::P(wrap(n, 16)),
            Reg::Pn(n) => Reg::Pn(wrap(n, 16)),
            Reg::X(n) if u32::from(n) + k >= 31 => Reg::Xzr,
            Reg::X(n) => Reg::X((u32::from(n) + k) as u8),
            Reg::W(n) if u32::from(n) + k >= 31 => Reg::Wzr,
            Reg::W(n) => Reg::W((u32::from(n) + k) as u8),
            r => r,
        }
    }
}

/// A shift or extend: of a register operand, a memory operand's index,
/// or an immediate (`#1, lsl #12`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum ShiftKind {
    /// Logical shift left.
    Lsl,
    /// Logical shift right.
    Lsr,
    /// Arithmetic shift right.
    Asr,
    /// Rotate right.
    Ror,
    /// Masking shift left (shifting in ones: MOVI).
    Msl,
    /// Zero-extend a byte.
    Uxtb,
    /// Zero-extend a halfword.
    Uxth,
    /// Zero-extend a word.
    Uxtw,
    /// Zero-extend a doubleword (no change).
    Uxtx,
    /// Sign-extend a byte.
    Sxtb,
    /// Sign-extend a halfword.
    Sxth,
    /// Sign-extend a word.
    Sxtw,
    /// Sign-extend a doubleword (no change).
    Sxtx,
}

impl ShiftKind {
    /// Whether it is an extend (UXTB to SXTX) rather than a shift.
    pub fn is_extend(self) -> bool {
        !matches!(
            self,
            ShiftKind::Lsl | ShiftKind::Lsr | ShiftKind::Asr | ShiftKind::Ror | ShiftKind::Msl
        )
    }

    fn parse(t: &str) -> Option<ShiftKind> {
        Some(match t {
            "LSL" => ShiftKind::Lsl,
            "LSR" => ShiftKind::Lsr,
            "ASR" => ShiftKind::Asr,
            "ROR" => ShiftKind::Ror,
            "MSL" => ShiftKind::Msl,
            "UXTB" => ShiftKind::Uxtb,
            "UXTH" => ShiftKind::Uxth,
            "UXTW" => ShiftKind::Uxtw,
            "UXTX" => ShiftKind::Uxtx,
            "SXTB" => ShiftKind::Sxtb,
            "SXTH" => ShiftKind::Sxth,
            "SXTW" => ShiftKind::Sxtw,
            "SXTX" => ShiftKind::Sxtx,
            _ => return None,
        })
    }
}

/// A condition code (as the operand shows it: CSET's is the inverse of
/// the encoded one).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[allow(missing_docs, reason = "the architecture's names")]
pub enum Cond {
    Eq,
    Ne,
    Hs,
    Lo,
    Mi,
    Pl,
    Vs,
    Vc,
    Hi,
    Ls,
    Ge,
    Lt,
    Gt,
    Le,
    Al,
    Nv,
}

impl Cond {
    /// The condition by its encoding (0 EQ to 15 NV).
    pub fn from_bits(b: u8) -> Cond {
        const ALL: [Cond; 16] = [
            Cond::Eq,
            Cond::Ne,
            Cond::Hs,
            Cond::Lo,
            Cond::Mi,
            Cond::Pl,
            Cond::Vs,
            Cond::Vc,
            Cond::Hi,
            Cond::Ls,
            Cond::Ge,
            Cond::Lt,
            Cond::Gt,
            Cond::Le,
            Cond::Al,
            Cond::Nv,
        ];
        ALL[usize::from(b & 15)]
    }

    fn parse(t: &str) -> Option<Cond> {
        // ("CS|HS": either name.)
        let t = t.rsplit('|').next()?;
        const NAMES: [&str; 16] = [
            "EQ", "NE", "HS", "LO", "MI", "PL", "VS", "VC", "HI", "LS", "GE", "LT", "GT", "LE",
            "AL", "NV",
        ];
        let i = NAMES.iter().position(|n| *n == t).or(match t {
            "CS" => Some(2),
            "CC" => Some(3),
            _ => None,
        })?;
        Some(Cond::from_bits(i as u8))
    }
}

/// How a memory operand's base is updated.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum AddrMode {
    /// `[base, offset]`: not updated.
    Offset,
    /// `[base, offset]!`: the address is base + offset, written back to
    /// the base first.
    PreIndex,
    /// `[base], offset`: the address is the base, which is then advanced
    /// by the offset (or the index register).
    PostIndex,
}

/// A memory operand.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Mem {
    /// The base: X*n* or SP, or an SVE vector of addresses (`[z0.d, #8]`).
    pub base: Reg,
    /// The base vector's element size (SVE).
    pub base_arrangement: Option<Arrangement>,
    /// An index register (`[x0, x1, lsl #3]`, `[x0, z1.d]`), or a
    /// post-index register (`[x0], x1`).
    pub index: Option<Reg>,
    /// The index's shift or extend, and its amount.
    pub index_shift: Option<(ShiftKind, u8)>,
    /// The immediate offset, in bytes (or in vector lengths, with
    /// `mul_vl`).
    pub offset: i64,
    /// Whether the offset is in multiples of the vector (or predicate)
    /// length: `[x0, #1, mul vl]`.
    pub mul_vl: bool,
    /// Pre- or post-indexing (the base is written back), or neither.
    pub mode: AddrMode,
}

/// An SME ZA operand that selects part of ZA: an array vector group
/// (`za.s[w8, 0, vgx2]`) or a tile slice (`za1h.s[w12, 3]`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct ZaSlice {
    /// The tile, for tile slices.
    pub tile: Option<u8>,
    /// The element size.
    pub element: Option<Element>,
    /// For tile slices: vertical (`V`) or horizontal (`H`).
    pub vertical: Option<bool>,
    /// The slice index register (W8-W11 or W12-W15).
    pub index: Reg,
    /// The offset added to it.
    pub offset: u16,
    /// For a range of offsets (`0:1`), the last.
    pub last: Option<u16>,
    /// The vector group size (`vgx2`, `vgx4`), or 0.
    pub vgx: u8,
}

/// A system register by its encoding (MRS and MSR).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[allow(missing_docs, reason = "the architecture's names")]
pub struct SysReg {
    pub op0: u8,
    pub op1: u8,
    pub crn: u8,
    pub crm: u8,
    pub op2: u8,
}

/// What an operand is.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum OperandKind {
    /// A register.
    Reg(RegOperand),
    /// A list of registers.
    List(RegList),
    /// An integer immediate (logical immediates decoded to their value;
    /// shifts' amounts are [`OperandKind::Shift`]).
    Imm(i64),
    /// A floating-point immediate.
    FpImm(f64),
    /// A PC-relative target address.
    Label(u64),
    /// A condition.
    Cond(Cond),
    /// A shift or extend of the operand before it.
    Shift(ShiftKind, u8),
    /// SVE's `mul #<imm>`: a multiplier.
    Mul(u8),
    /// A memory address.
    Mem(Mem),
    /// A part of ZA.
    Za(ZaSlice),
    /// ZERO's tiles: a mask of the 64-bit tiles ZA0.D to ZA7.D.
    ZaTiles(u8),
    /// A system register (MRS, MSR).
    SysReg(SysReg),
    /// A named option: a prefetch operation (`PLDL1KEEP`), barrier
    /// option (`SY`), SVE pattern (`POW2`), system operation (`IVAC`),
    /// PSTATE field, hint argument (`CSYNC`), ...
    Name(&'static str),
}

/// An operand, and how the instruction uses it.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Operand {
    /// What it is.
    pub kind: OperandKind,
    /// Whether the instruction reads or writes it (registers), loads or
    /// stores it (memory); `None` for immediates and the like. A
    /// memory operand's registers are read, and with pre- or
    /// post-indexing its base is written too (as written_registers says).
    pub access: Access,
}

/// The most operands an instruction has.
pub const MAX_OPERANDS: usize = 8;

/// An instruction's operands: a fixed-size array (no allocation).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Operands {
    ops: [Operand; MAX_OPERANDS],
    len: u8,
}

impl core::ops::Deref for Operands {
    type Target = [Operand];
    fn deref(&self) -> &[Operand] {
        &self.ops[..usize::from(self.len)]
    }
}

impl<'a> IntoIterator for &'a Operands {
    type Item = &'a Operand;
    type IntoIter = core::slice::Iter<'a, Operand>;
    fn into_iter(self) -> Self::IntoIter {
        self.iter()
    }
}

/// A symbol's value: a number, a floating-point number, or a table's
/// text.
#[derive(Clone, Copy)]
enum Val {
    Num(i64),
    Fp(f64),
    Text(&'static str),
}

/// What format.rs's `render_symbol` shows for `sym`, as a value (`None`
/// where it shows nothing, or can't).
fn sym_value(inst: &Inst, sym: &Symbol) -> Option<Val> {
    let w = inst.word;
    if let Some((p, v)) = sym.when()
        && value(p, w) != v
    {
        return None;
    }
    if let Some(t) = sym.table() {
        let key = value(t.key, w);
        let Some(&(_, _, result, expr, mul, add, _)) = t.rows.iter().find(|r| key & r.0 == r.1)
        else {
            if matches!(sym.display(), "<cond>" | "<invcond>") {
                return Some(Val::Text("AL"));
            }
            return sym.has(attr::ORIMM).then(|| Val::Num(raw(sym, w) as i64));
        };
        if !expr.is_empty() {
            return Some(Val::Num(mul * value(expr, w) as i64 + add));
        }
        return match result {
            "RESERVED" | "[absent]" => None,
            // (An operation with no name: its number.)
            _ if result.starts_with("#u") || result.starts_with("#imm") => {
                Some(Val::Num(raw(sym, w) as i64))
            }
            // (A condition's "CS|HS": either name.)
            _ if matches!(sym.display(), "<cond>" | "<invcond>") => Some(Val::Text(result)),
            // An extended register's UXTW or UXTX is LSL where Rd or Rn is
            // SP (as format.rs's render_symbol says).
            _ if result.contains('|') => {
                let (lsl, other) = result.split_once('|')?;
                let sets_flags = (w >> 29) & 1 == 1;
                let sp = (w >> 5) & 0x1F == 31 || (w & 0x1F == 31 && !sets_flags);
                Some(Val::Text(if sp { lsl } else { other }))
            }
            _ => Some(Val::Text(result)),
        };
    }
    if sym.has(attr::MAY_SPECIAL)
        && let Some(s) = special_value(inst, sym)
    {
        return match s {
            Special::Num(v, _) | Special::Hash(v) | Special::Decimal(v) => Some(Val::Num(v)),
            Special::Text(t) | Special::Lower(t) => Some(t.parse().map_or(Val::Text(t), Val::Num)),
            Special::ZaMask(m) => Some(Val::Num(m as i64)),
            Special::Absent => None,
        };
    }
    if is_fp_imm8(sym) {
        return Some(Val::Fp(fp_imm8(raw_value(sym, w))));
    }
    Some(Val::Num(match sym.kind {
        Kind::Pair(_) => ((raw(sym, w) + 1) & 31) as i64,
        Kind::Label => {
            let n = number(sym, w) as u64;
            let base = if sym.has(attr::PAGE) {
                inst.address & !0xFFF
            } else {
                inst.address
            };
            base.wrapping_add(n) as i64
        }
        Kind::Bitmask => bitmask(inst)?,
        Kind::Pn if sym.has(attr::PN8) && number(sym, w) < 8 => number(sym, w) + 8,
        _ => number(sym, w),
    }))
}

/// A number written in a template or a table (`16`, `-8`, `#3`, `0x10`).
fn parse_int(t: &str) -> Option<i64> {
    let t = t.trim().trim_start_matches('#');
    let (neg, t) = match t.strip_prefix('-') {
        Some(r) => (true, r),
        None => (false, t),
    };
    let v = match t.strip_prefix("0x") {
        Some(h) => i64::from_str_radix(h, 16).ok()?,
        None => t.parse().ok()?,
    };
    Some(if neg { -v } else { v })
}

/// A part's value: its symbol's, or its literal's.
fn part_value(inst: &Inst, sym: u16, lit: &'static str) -> Option<Val> {
    if sym == NONE {
        if lit.is_empty() {
            return None;
        }
        if let Some(n) = parse_int(lit) {
            return Some(Val::Num(n));
        }
        return Some(
            lit.trim_start_matches('#')
                .parse()
                .map_or(Val::Text(lit), Val::Fp),
        );
    }
    sym_value(inst, symbol(usize::from(sym)))
}

fn part_num(inst: &Inst, sym: u16, lit: &'static str) -> Option<i64> {
    match part_value(inst, sym, lit)? {
        Val::Num(n) => Some(n),
        Val::Text(t) => parse_int(t),
        Val::Fp(_) => None,
    }
}

fn part_text(inst: &Inst, sym: u16, lit: &'static str) -> Option<&'static str> {
    match part_value(inst, sym, lit)? {
        Val::Text(t) => Some(t),
        _ => None,
    }
}

/// The register a symbol names (with `prefix`: `<V>`'s or `<R>`'s
/// letter, or a template's literal one).
fn register(inst: &Inst, sym: u16, lit: &str, prefix: Option<u8>) -> Option<Reg> {
    if sym == NONE {
        return match lit {
            "ZT0" => Some(Reg::Zt0),
            "X16" => Some(Reg::X(16)),
            _ => None,
        };
    }
    let s = symbol(usize::from(sym));
    let Val::Num(n) = sym_value(inst, s)? else {
        return None;
    };
    // A register of one of LLVM's classes is as its decoder has it (Z0-Z14
    // even, encoded as half; PN8-PN15; W12-W15; ...): the field's value
    // through the class, where the symbol is the field.
    let class = (s.class != 255).then(|| &REG_CLASSES[usize::from(s.class)]);
    let n = match class {
        Some(c) if c.count == 1 && !matches!(s.kind, Kind::Pair(_)) => c
            .firsts
            .get(raw(s, inst.word) as usize)
            .filter(|&&f| f != 255)
            .map_or(n, |&f| i64::from(f)),
        _ => n,
    };
    let n8 = (n & 0xFF) as u8;
    let gpr = |wide: bool, sp: bool| match (n8, wide, sp) {
        (31, true, true) => Reg::Sp,
        (31, false, true) => Reg::Wsp,
        (31, true, false) => Reg::Xzr,
        (31, false, false) => Reg::Wzr,
        (_, true, _) => Reg::X(n8),
        (_, false, _) => Reg::W(n8),
    };
    // A prefix says the file.
    if let Some(p) = prefix {
        return Some(match p {
            b'W' => gpr(false, false),
            b'X' => gpr(true, false),
            b'B' => Reg::B(n8),
            b'H' => Reg::H(n8),
            b'S' => Reg::S(n8),
            b'D' => Reg::D(n8),
            b'Q' => Reg::Q(n8),
            b'V' => Reg::V(n8),
            b'Z' => Reg::Z(n8),
            b'P' => Reg::P(n8),
            _ => return None,
        });
    }
    // The template's kind says the width (`sxtb x0, w1`'s source is W1,
    // whatever LLVM's class); the class, the file otherwise.
    match s.kind {
        Kind::Gpr(wide) | Kind::Pair(wide) => return Some(gpr(wide, false)),
        Kind::GprSp(wide) => return Some(gpr(wide, true)),
        Kind::Pn => return Some(Reg::Pn(n8)),
        Kind::Reg(p) if p != "za" && !p.is_empty() && p != "C" => {
            return Some(match p {
                "v" => Reg::V(n8),
                "b" => Reg::B(n8),
                "h" => Reg::H(n8),
                "s" => Reg::S(n8),
                "d" => Reg::D(n8),
                "q" => Reg::Q(n8),
                "z" => Reg::Z(n8),
                "p" => Reg::P(n8),
                _ => return None,
            });
        }
        _ => {}
    }
    if let Some(c) = class {
        return Some(match c.file {
            file::X => gpr(true, false),
            file::XSP => gpr(true, true),
            file::W => gpr(false, false),
            file::WSP => gpr(false, true),
            4 => Reg::B(n8),
            5 => Reg::H(n8),
            6 => Reg::S(n8),
            7 => Reg::D(n8),
            8 => Reg::Q(n8),
            9 | 10 => Reg::V(n8),
            file::Z => Reg::Z(n8),
            file::P => Reg::P(n8),
            file::PN => Reg::Pn(n8),
            file::ZA => match Element::from_code(c.element) {
                Some(e) => Reg::ZaTile(e, n8),
                None => Reg::Za,
            },
            _ => Reg::Zt0,
        });
    }
    Some(match s.kind {
        Kind::Gpr(wide) | Kind::Pair(wide) => gpr(wide, false),
        Kind::GprSp(wide) => gpr(wide, true),
        Kind::Pn => Reg::Pn(n8),
        Kind::Reg(p) => match p {
            "v" => Reg::V(n8),
            "b" => Reg::B(n8),
            "h" => Reg::H(n8),
            "s" => Reg::S(n8),
            "d" => Reg::D(n8),
            "q" => Reg::Q(n8),
            "z" => Reg::Z(n8),
            "p" => Reg::P(n8),
            "za" => Reg::ZaTile(Element::B, n8),
            _ => return None,
        },
        _ => return None,
    })
}

/// A register's prefix letter: `<V>`'s or `<R>`'s value, or a literal.
fn prefix_letter(inst: &Inst, sym: u16, lit: &'static str) -> Option<u8> {
    let t = if sym == NONE {
        lit
    } else {
        part_text(inst, sym, lit)?
    };
    t.bytes().next().map(|b| b.to_ascii_uppercase())
}

/// The arrangement a part gives.
fn arrangement(inst: &Inst, sym: u16, lit: &'static str) -> Option<Arrangement> {
    let t = if sym == NONE {
        lit
    } else {
        part_text(inst, sym, lit)?
    };
    Arrangement::parse(t)
}

/// A shift or extend from its name part and amount part.
fn shift(inst: &Inst, sym: u16, lit: &'static str) -> Option<(ShiftKind, u8)> {
    let t = if sym == NONE {
        lit
    } else {
        part_text(inst, sym, lit)?
    };
    // ("LSL #8": a table's result with its amount.)
    let (name, amount) = match t.split_once(' ') {
        Some((n, a)) => (n, parse_int(a)?),
        None => (t, 0),
    };
    Some((ShiftKind::parse(name)?, amount as u8))
}

impl Instruction {
    /// The instruction's operands (at `address`: a label's target is from
    /// it), in the order the formatter shows them (an alias's, for an
    /// alias), with the condition Microsoft folds into the mnemonic
    /// (`b.eq`, `cseleq`) last. Operands the word leaves out (an optional
    /// shift that has no value) are left out.
    pub fn operands(&self, address: u64) -> Operands {
        self.at(address).operands()
    }

    /// For tests: how many operands the template has (some may be left
    /// out of [`Instruction::operands`], where the word has no value for
    /// them).
    #[doc(hidden)]
    pub fn template_operand_count(&self) -> usize {
        crate::tables::semantics(usize::from(self.encoding))
            .operands
            .len()
    }
}

impl Inst {
    /// The address its label operand names (a branch's target, `adr`'s
    /// address, a literal's): that operand alone, not all of them.
    pub(crate) fn label(&self) -> Option<u64> {
        for spec in crate::tables::semantics(usize::from(self.encoding)).operands {
            if spec.kind != opk::IMM {
                continue;
            }
            let Some(&(_, s, l)) = spec.parts.iter().find(|p| p.0 == role::VALUE) else {
                continue;
            };
            if s != NONE && matches!(symbol(usize::from(s)).kind, Kind::Label) {
                return part_num(self, s, l).map(|n| n as u64);
            }
        }
        None
    }

    fn operands(&self) -> Operands {
        let blank = Operand {
            kind: OperandKind::Imm(0),
            access: Access::None,
        };
        let mut out = Operands {
            ops: [blank; MAX_OPERANDS],
            len: 0,
        };
        for spec in crate::tables::semantics(usize::from(self.encoding)).operands {
            if usize::from(out.len) == MAX_OPERANDS {
                break;
            }
            if let Some(kind) = self.operand(spec) {
                out.ops[usize::from(out.len)] = Operand {
                    kind,
                    access: Access::from_bits(spec.access),
                };
                out.len += 1;
            }
        }
        out
    }

    fn operand(&self, spec: &OpSpec) -> Option<OperandKind> {
        let parts = spec.parts;
        let find = |r: u8| parts.iter().find(|p| p.0 == r).map(|&(_, s, l)| (s, l));
        Some(match spec.kind {
            opk::REG => {
                let prefix = find(role::PREFIX).and_then(|(s, l)| prefix_letter(self, s, l));
                let (s, l) = find(role::REG)?;
                let reg = register(self, s, l, prefix)?;
                let arrangement = find(role::ARR).and_then(|(s, l)| arrangement(self, s, l));
                OperandKind::Reg(RegOperand {
                    reg: match (reg, arrangement) {
                        (Reg::ZaTile(_, n), Some(a)) => Reg::ZaTile(a.element, n),
                        (r, _) => r,
                    },
                    arrangement,
                    lane: find(role::LANE)
                        .and_then(|(s, l)| part_num(self, s, l))
                        .map(|n| n as u8),
                    predication: match find(role::PRED).and_then(|(s, l)| {
                        if s == NONE {
                            Some(l)
                        } else {
                            part_text(self, s, l)
                        }
                    }) {
                        Some("M" | "m") => Predication::Merging,
                        Some("Z" | "z") => Predication::Zeroing,
                        _ => Predication::None,
                    },
                    select: match (find(role::ZAINDEX), find(role::ZAOFF)) {
                        (Some((s, l)), Some((os, ol))) => {
                            Some((register(self, s, l, None)?, part_num(self, os, ol)? as u8))
                        }
                        _ => None,
                    },
                    writeback: find(role::WB).is_some(),
                })
            }
            opk::LIST => {
                let regs = parts
                    .iter()
                    .filter(|p| p.0 == role::REG || p.0 == role::RANGE);
                let mut first = None;
                let mut second = None;
                let mut count = 0u8;
                let mut range_end = None;
                for &(r, s, l) in regs {
                    let reg = register(self, s, l, None)?;
                    if r == role::RANGE {
                        range_end = Some(reg);
                        continue;
                    }
                    count += 1;
                    if first.is_none() {
                        first = Some(reg);
                    } else if second.is_none() {
                        second = Some(reg);
                    }
                }
                let first = first?;
                let num = |r: Reg| match r {
                    Reg::V(n) | Reg::Q(n) | Reg::D(n) | Reg::Z(n) | Reg::X(n) | Reg::W(n) => {
                        Some((n, 32u8))
                    }
                    Reg::P(n) | Reg::Pn(n) => Some((n, 16)),
                    Reg::Xzr | Reg::Wzr => Some((31, 32)),
                    _ => None,
                };
                let (f, m) = num(first)?;
                let (count, stride) = match (range_end, second) {
                    (Some(last), _) => {
                        let (l, _) = num(last)?;
                        ((l + m - f) % m + 1, 1)
                    }
                    (None, Some(s)) => (count, (num(s)?.0 + m - f) % m),
                    (None, None) => (1, 1),
                };
                OperandKind::List(RegList {
                    first,
                    count,
                    stride,
                    arrangement: find(role::ARR).and_then(|(s, l)| arrangement(self, s, l)),
                    lane: find(role::LANE)
                        .and_then(|(s, l)| part_num(self, s, l))
                        .map(|n| n as u8),
                })
            }
            opk::IMM | opk::FPIMM => {
                let (s, l) = find(role::VALUE)?;
                if s != NONE {
                    let sym = symbol(usize::from(s));
                    if matches!(sym.display(), "<cond>" | "<invcond>") {
                        return Some(OperandKind::Cond(Cond::parse(part_text(self, s, l)?)?));
                    }
                    if matches!(sym.kind, Kind::Label) {
                        return Some(OperandKind::Label(part_num(self, s, l)? as u64));
                    }
                }
                match part_value(self, s, l)? {
                    Val::Num(n) => OperandKind::Imm(n),
                    Val::Fp(f) => OperandKind::FpImm(f),
                    Val::Text(t) => {
                        if let Some(n) = parse_int(t) {
                            OperandKind::Imm(n)
                        } else if let Some(f) =
                            t.strip_prefix('#').and_then(|f| f.parse::<f64>().ok())
                        {
                            OperandKind::FpImm(f)
                        } else {
                            OperandKind::Name(t)
                        }
                    }
                }
            }
            opk::COND => {
                let (s, l) = find(role::VALUE)?;
                OperandKind::Cond(Cond::parse(part_text(self, s, l)?)?)
            }
            opk::SHIFT => {
                let (s, l) = find(role::SHIFT)?;
                let (kind, mut amount) = shift(self, s, l)?;
                if let Some((s, l)) = find(role::AMOUNT) {
                    amount = part_num(self, s, l).unwrap_or(0) as u8;
                }
                OperandKind::Shift(kind, amount)
            }
            opk::MUL => {
                let (s, l) = find(role::VALUE)?;
                OperandKind::Mul(part_num(self, s, l)? as u8)
            }
            opk::MEM => {
                let (s, l) = find(role::REG)?;
                let base = register(self, s, l, None)?;
                let mut m = Mem {
                    base,
                    base_arrangement: None,
                    index: None,
                    index_shift: None,
                    offset: 0,
                    mul_vl: false,
                    mode: AddrMode::Offset,
                };
                let mut last = role::REG;
                for &(r, s, l) in parts {
                    match r {
                        role::ARR if last == role::REG => {
                            m.base_arrangement = arrangement(self, s, l);
                        }
                        role::INDEX => m.index = register(self, s, l, None),
                        role::OFFSET => m.offset = part_num(self, s, l).unwrap_or(0),
                        role::SHIFT => m.index_shift = shift(self, s, l),
                        role::AMOUNT => {
                            if let Some((k, _)) = m.index_shift {
                                m.index_shift = Some((k, part_num(self, s, l).unwrap_or(0) as u8));
                            }
                        }
                        role::MULVL => m.mul_vl = true,
                        role::WB => m.mode = AddrMode::PreIndex,
                        role::POST => m.mode = AddrMode::PostIndex,
                        _ => {}
                    }
                    if r != role::ARR {
                        last = r;
                    }
                }
                // (A post-index register XZR is the immediate form's: no
                // index.)
                if m.index == Some(Reg::Xzr) && m.mode == AddrMode::PostIndex {
                    m.index = None;
                }
                OperandKind::Mem(m)
            }
            opk::ZA => {
                let tile = find(role::TILE).and_then(|(s, l)| {
                    if s == NONE {
                        l.parse::<u8>().ok()
                    } else {
                        part_num(self, s, l).map(|n| n as u8)
                    }
                });
                let element = find(role::ARR)
                    .and_then(|(s, l)| arrangement(self, s, l))
                    .map(|a| a.element);
                let Some((is, il)) = find(role::ZAINDEX) else {
                    // A whole tile, or ZA.
                    return Some(OperandKind::Reg(RegOperand {
                        reg: match (tile, element) {
                            (Some(t), Some(e)) => Reg::ZaTile(e, t),
                            _ => Reg::Za,
                        },
                        arrangement: element.map(|e| Arrangement {
                            lanes: 0,
                            element: e,
                        }),
                        lane: None,
                        predication: Predication::None,
                        select: None,
                        writeback: false,
                    }));
                };
                OperandKind::Za(ZaSlice {
                    tile,
                    element,
                    vertical: find(role::HV)
                        .and_then(|(s, l)| part_text(self, s, l))
                        .map(|t| t == "V"),
                    index: register(self, is, il, None)?,
                    offset: find(role::ZAOFF)
                        .and_then(|(s, l)| part_num(self, s, l))
                        .unwrap_or(0) as u16,
                    last: find(role::ZALAST)
                        .and_then(|(s, l)| part_num(self, s, l))
                        .map(|n| n as u16),
                    vgx: find(role::VGX)
                        .and_then(|(_, l)| l.strip_prefix("VGx"))
                        .and_then(|n| n.parse().ok())
                        .unwrap_or(0),
                })
            }
            opk::SYSREG => {
                let mut v = [0u8; 5];
                for (i, &(_, s, l)) in parts.iter().take(5).enumerate() {
                    v[i] = part_num(self, s, l)? as u8;
                }
                OperandKind::SysReg(SysReg {
                    op0: v[0],
                    op1: v[1],
                    crn: v[2],
                    crm: v[3],
                    op2: v[4],
                })
            }
            _ => {
                let (s, l) = find(role::VALUE)?;
                if s != NONE && symbol(usize::from(s)).display() == "<mask>" {
                    return Some(OperandKind::ZaTiles(part_num(self, s, l)? as u8));
                }
                match part_value(self, s, l)? {
                    Val::Text(t) => OperandKind::Name(t),
                    Val::Num(n) => OperandKind::Imm(n),
                    Val::Fp(f) => OperandKind::FpImm(f),
                }
            }
        })
    }
}
