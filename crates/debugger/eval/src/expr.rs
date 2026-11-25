//! DWARF location expressions (DWARF 5, section 2.5 and 2.6), evaluated
//! against a frame: the stack machine's operations, and the location
//! descriptions they make (memory, a register, an implicit value, a
//! computed value, and pieces of those).
//!
//! What a frame doesn't know (a register it lost, memory a dump didn't
//! capture, the caller's values `DW_OP_entry_value` asks for) makes the
//! piece unavailable, with the reason; nothing is guessed.

use turbonop_core::Endian;

use crate::Unavailable;

/// Where a piece of a variable is.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Loc {
    /// In memory, at this address.
    Memory(u64),
    /// In this register (by DWARF number).
    Register(u16),
    /// Its value, computed (`DW_OP_stack_value`).
    Value(u64),
    /// Its bytes, given (`DW_OP_implicit_value`).
    Implicit(Vec<u8>),
    /// Optimized out (an empty piece).
    Empty,
}

/// A piece: where, and how many bytes (None: the whole variable).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Piece {
    pub loc: Loc,
    pub size: Option<u64>,
    /// A `DW_OP_bit_piece`'s offset into the location, in bits.
    pub bit_offset: u64,
}

/// What an expression is evaluated against.
pub trait Frame {
    /// A register's value (its low 64 bits).
    fn register(&self, n: u16) -> Result<u64, Unavailable>;
    /// `len` (at most 8) bytes of memory, as a little-endian number.
    fn memory(&self, address: u64, len: u8) -> Result<u64, Unavailable>;
    /// The frame's canonical frame address.
    fn cfa(&self) -> Result<u64, Unavailable>;
    /// The frame base (`DW_AT_frame_base`), evaluated.
    fn frame_base(&self) -> Result<u64, Unavailable>;
    /// What a link-time address (`DW_OP_addr`) is less its load address.
    fn bias(&self) -> u64;
    /// The value `expr` (a `DW_OP_entry_value`'s: a register, usually)
    /// had when the function was entered: the caller's to give.
    fn entry_value(&self, expr: &[u8]) -> Result<u64, Unavailable> {
        let _ = expr;
        Err(Unavailable::EntryValue)
    }
}

/// The most operations one evaluation runs (a `DW_OP_skip` loop ends).
const MAX_STEPS: usize = 10_000;
/// The deepest the stack grows.
const MAX_STACK: usize = 1024;

struct Ops<'e> {
    e: &'e [u8],
    at: usize,
    endian: Endian,
}

impl Ops<'_> {
    fn u8(&mut self) -> Result<u8, Unavailable> {
        let b = *self.e.get(self.at).ok_or(Unavailable::Malformed)?;
        self.at += 1;
        Ok(b)
    }

    fn fixed(&mut self, n: usize) -> Result<u64, Unavailable> {
        let b = self
            .e
            .get(self.at..self.at + n)
            .ok_or(Unavailable::Malformed)?;
        self.at += n;
        let mut v = 0u64;
        match self.endian {
            Endian::Little => {
                for (i, &x) in b.iter().enumerate() {
                    v |= u64::from(x) << (8 * i);
                }
            }
            Endian::Big => {
                for &x in b {
                    v = v << 8 | u64::from(x);
                }
            }
        }
        Ok(v)
    }

    fn signed(&mut self, n: usize) -> Result<i64, Unavailable> {
        let v = self.fixed(n)?;
        let shift = 64 - 8 * u32::try_from(n).unwrap_or(8);
        Ok((v << shift).cast_signed() >> shift)
    }

    fn uleb(&mut self) -> Result<u64, Unavailable> {
        let mut v = 0u64;
        let mut shift = 0;
        loop {
            let b = self.u8()?;
            if shift < 64 {
                v |= u64::from(b & 0x7F) << shift;
            }
            shift += 7;
            if b & 0x80 == 0 {
                return Ok(v);
            }
        }
    }

    fn sleb(&mut self) -> Result<i64, Unavailable> {
        let mut v = 0i64;
        let mut shift = 0;
        loop {
            let b = self.u8()?;
            if shift < 64 {
                v |= i64::from(b & 0x7F) << shift;
            }
            shift += 7;
            if b & 0x80 == 0 {
                if shift < 64 && b & 0x40 != 0 {
                    v |= -1 << shift;
                }
                return Ok(v);
            }
        }
    }

    fn done(&self) -> bool {
        self.at >= self.e.len()
    }
}

/// Evaluates a location expression: its pieces (one, without a size,
/// for a variable in one place).
pub fn evaluate(
    expr: &[u8],
    endian: Endian,
    address_size: u8,
    frame: &dyn Frame,
) -> Result<Vec<Piece>, Unavailable> {
    let mut ops = Ops {
        e: expr,
        at: 0,
        endian,
    };
    let mut stack: Vec<u64> = Vec::new();
    let mut pieces = Vec::new();
    // The location so far: a register, a computed value or implicit bytes
    // (else the stack's top is a memory address).
    let mut special: Option<Loc> = None;
    let mut steps = 0;
    let addr_size = usize::from(address_size);
    let mask = if address_size >= 8 {
        u64::MAX
    } else {
        (1u64 << (8 * u32::from(address_size))) - 1
    };
    let pop = |s: &mut Vec<u64>| s.pop().ok_or(Unavailable::Malformed);
    while !ops.done() {
        steps += 1;
        if steps > MAX_STEPS || stack.len() > MAX_STACK {
            return Err(Unavailable::Malformed);
        }
        let op = ops.u8()?;
        match op {
            0x03 => stack.push(ops.fixed(addr_size)?.wrapping_add(frame.bias())),
            0x06 => {
                let a = pop(&mut stack)?;
                stack.push(frame.memory(a, address_size)?);
            }
            0x08 => stack.push(ops.fixed(1)?),
            0x09 => stack.push(ops.signed(1)?.cast_unsigned()),
            0x0a => stack.push(ops.fixed(2)?),
            0x0b => stack.push(ops.signed(2)?.cast_unsigned()),
            0x0c => stack.push(ops.fixed(4)?),
            0x0d => stack.push(ops.signed(4)?.cast_unsigned()),
            0x0e => stack.push(ops.fixed(8)?),
            0x0f => stack.push(ops.signed(8)?.cast_unsigned()),
            0x10 => stack.push(ops.uleb()?),
            0x11 => stack.push(ops.sleb()?.cast_unsigned()),
            0x12 => {
                let v = *stack.last().ok_or(Unavailable::Malformed)?;
                stack.push(v);
            }
            0x13 => {
                pop(&mut stack)?;
            }
            0x14 => {
                let v = *stack.iter().rev().nth(1).ok_or(Unavailable::Malformed)?;
                stack.push(v);
            }
            0x15 => {
                let i = usize::from(ops.u8()?);
                let v = *stack.iter().rev().nth(i).ok_or(Unavailable::Malformed)?;
                stack.push(v);
            }
            0x16 => {
                let n = stack.len();
                if n < 2 {
                    return Err(Unavailable::Malformed);
                }
                stack.swap(n - 1, n - 2);
            }
            0x17 => {
                let n = stack.len();
                if n < 3 {
                    return Err(Unavailable::Malformed);
                }
                // (top, second, third) -> (second, third, top)
                let top = stack[n - 1];
                stack[n - 1] = stack[n - 2];
                stack[n - 2] = stack[n - 3];
                stack[n - 3] = top;
            }
            0x19 => {
                let v = pop(&mut stack)?.cast_signed();
                stack.push(v.unsigned_abs());
            }
            0x1a..=0x1e | 0x21 | 0x22 | 0x24..=0x27 | 0x29..=0x2e => {
                let b = pop(&mut stack)?;
                let a = pop(&mut stack)?;
                let (sa, sb) = (a.cast_signed(), b.cast_signed());
                let shift = |n: u64| u32::try_from(n).unwrap_or(64);
                stack.push(match op {
                    0x1a => a & b,
                    0x1b if b == 0 => return Err(Unavailable::Malformed),
                    0x1b => sa.wrapping_div(sb).cast_unsigned(),
                    0x1c => a.wrapping_sub(b),
                    0x1d if b == 0 => return Err(Unavailable::Malformed),
                    0x1d => a % b,
                    0x1e => a.wrapping_mul(b),
                    0x21 => a | b,
                    0x22 => a.wrapping_add(b),
                    0x24 => a.checked_shl(shift(b)).unwrap_or(0),
                    0x25 => a.checked_shr(shift(b)).unwrap_or(0),
                    0x26 => sa.checked_shr(shift(b)).unwrap_or(sa >> 63).cast_unsigned(),
                    0x27 => a ^ b,
                    0x29 => u64::from(sa == sb),
                    0x2a => u64::from(sa >= sb),
                    0x2b => u64::from(sa > sb),
                    0x2c => u64::from(sa <= sb),
                    0x2d => u64::from(sa < sb),
                    _ => u64::from(sa != sb),
                });
            }
            0x1f => {
                let v = pop(&mut stack)?;
                stack.push(v.wrapping_neg());
            }
            0x20 => {
                let v = pop(&mut stack)?;
                stack.push(!v);
            }
            0x23 => {
                let a = pop(&mut stack)?;
                stack.push(a.wrapping_add(ops.uleb()?));
            }
            0x28 | 0x2f => {
                let off = ops.signed(2)?;
                let jump = op == 0x2f || pop(&mut stack)? != 0;
                if jump {
                    let to = i64::try_from(ops.at).map_err(|_| Unavailable::Malformed)? + off;
                    ops.at = usize::try_from(to).map_err(|_| Unavailable::Malformed)?;
                    if ops.at > expr.len() {
                        return Err(Unavailable::Malformed);
                    }
                }
            }
            0x30..=0x4f => stack.push(u64::from(op - 0x30)),
            0x50..=0x6f => special = Some(Loc::Register(u16::from(op - 0x50))),
            0x70..=0x8f => {
                let off = ops.sleb()?;
                let r = frame.register(u16::from(op - 0x70))?;
                stack.push(r.wrapping_add_signed(off));
            }
            0x90 => {
                let n = u16::try_from(ops.uleb()?).map_err(|_| Unavailable::Malformed)?;
                special = Some(Loc::Register(n));
            }
            0x91 => {
                let off = ops.sleb()?;
                stack.push(frame.frame_base()?.wrapping_add_signed(off));
            }
            0x92 => {
                let n = u16::try_from(ops.uleb()?).map_err(|_| Unavailable::Malformed)?;
                let off = ops.sleb()?;
                stack.push(frame.register(n)?.wrapping_add_signed(off));
            }
            0x93 | 0x9d => {
                let size = ops.uleb()?;
                let bit_offset = if op == 0x9d { ops.uleb()? } else { 0 };
                let size = if op == 0x9d { size.div_ceil(8) } else { size };
                let loc = match special.take() {
                    Some(l) => l,
                    None => match stack.pop() {
                        Some(a) => Loc::Memory(a & mask),
                        None => Loc::Empty,
                    },
                };
                pieces.push(Piece {
                    loc,
                    size: Some(size),
                    bit_offset,
                });
            }
            0x94 => {
                let n = ops.u8()?;
                let a = pop(&mut stack)?;
                stack.push(frame.memory(a, n.min(8))?);
            }
            0x96 => {}
            0x9c => stack.push(frame.cfa()?),
            0x9e => {
                let len = usize::try_from(ops.uleb()?).map_err(|_| Unavailable::Malformed)?;
                let b = expr
                    .get(ops.at..ops.at.saturating_add(len))
                    .ok_or(Unavailable::Malformed)?
                    .to_vec();
                ops.at += len;
                special = Some(Loc::Implicit(b));
            }
            0x9f => {
                let v = pop(&mut stack)?;
                special = Some(Loc::Value(v));
            }
            // DW_OP_entry_value, DW_OP_GNU_entry_value: the caller's.
            0xa3 | 0xf3 => {
                let len = usize::try_from(ops.uleb()?).map_err(|_| Unavailable::Malformed)?;
                let sub = expr
                    .get(ops.at..ops.at.saturating_add(len))
                    .ok_or(Unavailable::Malformed)?;
                ops.at += len;
                stack.push(frame.entry_value(sub)?);
            }
            // Implicit pointers: a pointer to a variable that has no address.
            0xa0 | 0xf2 => return Err(Unavailable::ImplicitPointer),
            // Thread-local storage.
            0x9b | 0xe0 => return Err(Unavailable::ThreadLocal),
            // Typed (DWARF 5's) and others this doesn't evaluate.
            op => return Err(Unavailable::Operation(op)),
        }
    }
    if !pieces.is_empty() {
        return Ok(pieces);
    }
    let loc = match special {
        Some(l) => l,
        None => Loc::Memory(stack.pop().ok_or(Unavailable::Malformed)? & mask),
    };
    Ok(vec![Piece {
        loc,
        size: None,
        bit_offset: 0,
    }])
}
