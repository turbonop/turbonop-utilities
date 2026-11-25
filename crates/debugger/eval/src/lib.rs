//! A frame's variables, evaluated: each parameter's and local's place at
//! the frame's PC (from its function's debug information: a PDB's
//! def-ranges and registers, DWARF's location lists and expressions), read
//! with the frame's registers and the target's memory, and shown as its
//! type says ([`turbonop_debuginfo::value`]).
//!
//! What can't be read says why ([`Unavailable`]): the register a variable
//! is in was clobbered by a call since, or isn't known; the memory it's in
//! wasn't captured; it isn't live at this PC (optimized out); its location
//! needs the caller's values (`DW_OP_entry_value`). Nothing is guessed: a
//! value shown is the bytes the target holds (or an image stands in for:
//! see each local's [`Provenance`]).

pub mod expr;

use std::fmt;

use turbonop_core::{Arch, Endian};
use turbonop_debuginfo::value::{self, Limits, Value};
use turbonop_debuginfo::{DebugInfo, Place, TypeRef, VarRange};
use turbonop_frames::{Modules, RegState, Stack};
use turbonop_target::{AddressSpace, Provenance};

use crate::expr::{Loc, Piece};

/// Why a variable's value can't be read.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Unavailable {
    /// The frame's module has no debug information.
    NoDebugInfo,
    /// No function's variables are known at the frame's address.
    NoFunction,
    /// The variable isn't live at this address (optimized out here).
    NotLive,
    /// It's in a register a call clobbered (by the calling convention).
    RegisterClobbered(u16),
    /// It's in a register the frame doesn't know.
    RegisterUnknown(u16),
    /// A register the debug information names that isn't one here.
    NoSuchRegister(u16),
    /// It's in memory the dump didn't capture (and no image has).
    MemoryNotCaptured { address: u64, len: u64 },
    /// It's relative to the frame's CFA, which isn't known (its caller
    /// wasn't found by unwind data).
    CfaUnknown,
    /// It's relative to a frame pointer the frame doesn't know.
    FramePointerUnknown,
    /// It's relative to x86's virtual frame (`$T0`, from frame data), which
    /// isn't evaluated yet.
    VirtualFrame,
    /// Its location needs the caller's values (`DW_OP_entry_value`).
    EntryValue,
    /// It's a pointer to a variable with no address (an implicit pointer).
    ImplicitPointer,
    /// It's thread-local.
    ThreadLocal,
    /// Its location uses a DWARF operation not evaluated here.
    Operation(u8),
    /// Its location is malformed.
    Malformed,
    /// Its type's size isn't known.
    NoSize,
}

impl fmt::Display for Unavailable {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Unavailable::NoDebugInfo => f.write_str("no debug information"),
            Unavailable::NoFunction => f.write_str("no function's variables here"),
            Unavailable::NotLive => f.write_str("not live here (optimized out)"),
            Unavailable::RegisterClobbered(r) => {
                write!(f, "in register {r}, which a call since may have changed")
            }
            Unavailable::RegisterUnknown(r) => write!(f, "in register {r}, which isn't known here"),
            Unavailable::NoSuchRegister(r) => write!(f, "in register {r}, which isn't one"),
            Unavailable::MemoryNotCaptured { address, len } => {
                write!(f, "in memory not captured ({len} bytes at {address:#x})")
            }
            Unavailable::CfaUnknown => {
                f.write_str("relative to the frame's CFA, which isn't known")
            }
            Unavailable::FramePointerUnknown => {
                f.write_str("relative to the frame pointer, which isn't known")
            }
            Unavailable::VirtualFrame => f.write_str("relative to x86's virtual frame ($T0)"),
            Unavailable::EntryValue => f.write_str("needs the caller's values (an entry value)"),
            Unavailable::ImplicitPointer => f.write_str("a pointer to a value with no address"),
            Unavailable::ThreadLocal => f.write_str("thread-local"),
            Unavailable::Operation(op) => write!(f, "uses DWARF operation {op:#x}"),
            Unavailable::Malformed => f.write_str("its location is malformed"),
            Unavailable::NoSize => f.write_str("its type's size isn't known"),
        }
    }
}

/// A parameter or local, evaluated.
#[derive(Clone, Debug, PartialEq)]
pub struct Local {
    pub name: String,
    pub type_name: String,
    pub param: bool,
    /// Declared in a function inlined here.
    pub inlined: bool,
    /// Where it is, in words (`rcx`, `[rbp-0x18]`, `3 pieces`).
    pub location: String,
    /// Its value, or why it can't be read.
    pub value: Result<Value, Unavailable>,
    /// Where its bytes came from, if any came from memory (else from
    /// registers, or computed).
    pub provenance: Option<Provenance>,
}

/// The largest variable read whole.
const MAX_SIZE: u64 = 1 << 20;

/// The variables of frame `index` of `stack`.
///
/// # Errors
///
/// If the frame's module has no debug information, or no function's
/// variables are known at its address.
pub fn locals(
    stack: &Stack,
    index: usize,
    modules: &Modules<'_>,
    space: &AddressSpace<'_>,
    arch: Arch,
) -> Result<Vec<Local>, Unavailable> {
    let frame = stack.frames.get(index).ok_or(Unavailable::NoFunction)?;
    let m = frame.module.ok_or(Unavailable::NoDebugInfo)?;
    let (debug, bias) = modules.debug(m).ok_or(Unavailable::NoDebugInfo)?;
    let symbol = frame.symbol.as_ref().ok_or(Unavailable::NoFunction)?;
    let at = frame.lookup().wrapping_sub(bias);
    let start = at.wrapping_sub(symbol.offset);
    let vars = debug
        .variables(None, start)
        .ok()
        .flatten()
        .ok_or(Unavailable::NoFunction)?;
    let pdb = matches!(debug, DebugInfo::Pdb(_));
    let ctx = Context {
        stack,
        index,
        modules,
        space,
        arch,
        pdb,
        bias,
        frame_pointers: vars.frame,
        frame_base: None,
    };
    Ok(vars
        .variables
        .iter()
        .map(|v| {
            let ty = TypeRef(v.type_index);
            // A PDB variable described by def-ranges (S_LOCAL's) isn't where
            // its S_REGREL32 (a whole-function place relative to a register:
            // its home slot, which optimized code may never write) says, as
            // DbgEng reads it: only its def-ranges count. (Those include
            // S_DEFRANGE_FRAMEPOINTER_REL_FULL_SCOPE, a spill for the whole
            // scope: a Frame place.)
            let regrel = |r: &VarRange| {
                pdb && matches!(r.place, Place::Relative { .. })
                    && r.start <= vars.start
                    && r.end >= vars.end
            };
            let described = pdb && v.ranges.iter().any(|r| !regrel(r));
            let live: Vec<&VarRange> = v
                .ranges
                .iter()
                .filter(|r| !(described && regrel(r)))
                .filter(|r| {
                    r.start <= at
                        && at < r.end
                        && !r.gaps.iter().any(|&(g, n)| g <= at && at - g < n)
                })
                .collect();
            // One place, of several live (a def-range and a home slot): the
            // narrowest, then a register's (the def-range's) over memory
            // (the home slot, which may be stale). Pieces are all kept.
            let pieces = live.iter().any(|r| matches!(r.place, Place::Field { .. }));
            let live: Vec<&VarRange> = if pieces || live.len() < 2 {
                live
            } else {
                let in_register = |r: &VarRange| matches!(r.place, Place::Register(_));
                live.iter()
                    .min_by_key(|r| (r.end - r.start, !in_register(r)))
                    .map(|r| vec![*r])
                    .unwrap_or_default()
            };
            let (location, value, provenance) = match ctx.read(debug, ty, &live) {
                Ok((loc, bytes, p)) => (
                    loc,
                    Ok(value::read(debug, ty, &bytes, Limits::default())),
                    p,
                ),
                Err((loc, why)) => (loc, Err(why), None),
            };
            Local {
                name: v.name.clone(),
                type_name: debug.type_name(ty),
                param: v.param,
                inlined: v.inlined,
                location,
                value,
                provenance,
            }
        })
        .collect())
}

/// What a frame's variables are read with.
struct Context<'c, 'a, 'm> {
    stack: &'c Stack,
    index: usize,
    modules: &'c Modules<'m>,
    space: &'c AddressSpace<'a>,
    arch: Arch,
    pdb: bool,
    bias: u64,
    /// S_FRAMEPROC's frame pointers (locals', parameters').
    frame_pointers: (u8, u8),
    /// A DWARF function's frame base expression.
    frame_base: Option<Vec<u8>>,
}

type Read = Result<(String, Vec<u8>, Option<Provenance>), (String, Unavailable)>;

impl Context<'_, '_, '_> {
    fn registers(&self) -> &turbonop_frames::FrameRegisters {
        &self.stack.frames[self.index].registers
    }

    /// A register's bytes (DWARF number), its low `bits`.
    fn register_bytes(&self, n: u16) -> Result<(u128, u16), Unavailable> {
        match self.registers().get(n) {
            Some(RegState::Known { value, bits, .. }) => Ok((value, bits)),
            Some(RegState::Clobbered) => Err(Unavailable::RegisterClobbered(n)),
            None => Err(Unavailable::RegisterUnknown(n)),
        }
    }

    /// The DWARF register (and the part of it: first bit, bits) a
    /// variable's register number names.
    fn dwarf_register(&self, n: u16) -> Result<(u16, u16, u16), Unavailable> {
        if self.pdb {
            let p = turbonop_codeview::registers::to_dwarf(self.arch, n)
                .ok_or(Unavailable::NoSuchRegister(n))?;
            Ok((p.register, p.lsb, p.bits))
        } else {
            Ok((n, 0, 128))
        }
    }

    fn register_value(&self, n: u16) -> Result<u64, Unavailable> {
        let (r, lsb, bits) = self.dwarf_register(n)?;
        let (v, _) = self.register_bytes(r)?;
        let v = v >> lsb;
        let v = if bits >= 128 {
            v
        } else {
            v & ((1u128 << bits) - 1)
        };
        #[allow(clippy::cast_possible_truncation)]
        Ok(v as u64)
    }

    fn cfa(&self) -> Result<u64, Unavailable> {
        let next = self
            .stack
            .frames
            .get(self.index + 1)
            .ok_or(Unavailable::CfaUnknown)?;
        if next.confidence != turbonop_frames::Confidence::Certain {
            return Err(Unavailable::CfaUnknown);
        }
        Ok(next.sp)
    }

    fn frame_pointer(&self, param: bool) -> Result<u64, Unavailable> {
        let enc = if param {
            self.frame_pointers.1
        } else {
            self.frame_pointers.0
        };
        // (x86's "stack pointer" frame is its virtual frame.)
        if self.arch == Arch::X86 && enc == 1 {
            return Err(Unavailable::VirtualFrame);
        }
        let name = turbonop_codeview::registers::frame_pointer(self.arch, enc)
            .ok_or(Unavailable::FramePointerUnknown)?;
        let n = (0..512u16)
            .find(|&n| turbonop_dwarf::registers::name(self.arch, n).as_deref() == Some(name))
            .ok_or(Unavailable::FramePointerUnknown)?;
        let (v, _) = self
            .register_bytes(n)
            .map_err(|_| Unavailable::FramePointerUnknown)?;
        #[allow(clippy::cast_possible_truncation)]
        Ok(v as u64)
    }

    fn memory(
        &self,
        address: u64,
        len: u64,
        provenance: &mut Option<Provenance>,
    ) -> Result<Vec<u8>, Unavailable> {
        let n = usize::try_from(len).map_err(|_| Unavailable::NoSize)?;
        let mut b = vec![0u8; n];
        let p = self
            .space
            .read(address, &mut b)
            .ok_or(Unavailable::MemoryNotCaptured { address, len })?;
        *provenance = Some(provenance.map_or(p, |q| q.worst(p)));
        Ok(b)
    }

    /// A variable's bytes, by its places live here.
    fn read(&self, debug: &DebugInfo<'_>, ty: TypeRef, live: &[&VarRange]) -> Read {
        let size = debug
            .type_size(ty)
            .ok_or((String::from("?"), Unavailable::NoSize))?;
        if size > MAX_SIZE {
            return Err((String::from("?"), Unavailable::NoSize));
        }
        if live.is_empty() {
            return Err((String::from("-"), Unavailable::NotLive));
        }
        // Its pieces: (offset into it, where, how many bytes).
        let mut pieces: Vec<(u64, Loc, u64)> = Vec::new();
        let mut location = String::new();
        let fail = |loc: &str, why| Err((loc.to_string(), why));
        for r in live {
            match &r.place {
                Place::Register(n) => {
                    let (d, _, _) = match self.dwarf_register(*n) {
                        Ok(x) => x,
                        Err(e) => return fail("?", e),
                    };
                    location = self.register_name(d);
                    pieces.push((0, Loc::Register(*n), size));
                }
                Place::Relative { register, offset } => {
                    location = format!(
                        "[{}{offset:+#x}]",
                        self.register_name(
                            self.dwarf_register(*register).map_or(*register, |d| d.0)
                        )
                    );
                    match self.register_value(*register) {
                        Ok(base) => pieces.push((
                            0,
                            Loc::Memory(base.wrapping_add_signed(i64::from(*offset))),
                            size,
                        )),
                        Err(e) => return fail(&location, e),
                    }
                }
                Place::Frame { offset, param: p } => {
                    location = format!("[frame{offset:+#x}]");
                    match self.frame_pointer(*p) {
                        Ok(fp) => pieces.push((
                            0,
                            Loc::Memory(fp.wrapping_add_signed(i64::from(*offset))),
                            size,
                        )),
                        Err(e) => return fail(&location, e),
                    }
                }
                Place::Field { register, field } => {
                    location = format!("{} pieces", live.len());
                    let (_, _, bits) = match self.dwarf_register(*register) {
                        Ok(x) => x,
                        Err(e) => return fail(&location, e),
                    };
                    let width = u64::from(bits.min(64) / 8).max(1);
                    let n = width.min(size.saturating_sub(u64::from(*field)));
                    pieces.push((u64::from(*field), Loc::Register(*register), n));
                }
                Place::Cfa { offset } => {
                    location = format!("[cfa{offset:+#x}]");
                    match self.cfa() {
                        Ok(c) => pieces.push((
                            0,
                            Loc::Memory(c.wrapping_add_signed(i64::from(*offset))),
                            size,
                        )),
                        Err(e) => return fail(&location, e),
                    }
                }
                Place::Expression { expr, frame_base } => {
                    location = String::from("expression");
                    let ctx = Context {
                        frame_base: frame_base.clone(),
                        ..*self
                    };
                    let endian = Endian::Little;
                    let addr_size = self.arch.pointer_size().unwrap_or(8);
                    match expr::evaluate(expr, endian, addr_size, &ctx) {
                        Ok(ps) => {
                            let mut off = 0;
                            if ps.len() > 1 {
                                location = format!("{} pieces", ps.len());
                            }
                            for Piece { loc, size: s, .. } in ps {
                                let n = s.unwrap_or(size);
                                if let Loc::Register(r) = loc {
                                    // (DWARF's numbers: as they are.)
                                    pieces.push((off, Loc::Register(r), n));
                                } else {
                                    pieces.push((off, loc, n));
                                }
                                off += n;
                            }
                        }
                        Err(e) => return fail(&location, e),
                    }
                }
                _ => return fail("?", Unavailable::Malformed),
            }
        }
        let n = usize::try_from(size).map_err(|_| (location.clone(), Unavailable::NoSize))?;
        let mut bytes = vec![0u8; n];
        let mut provenance = None;
        for (off, loc, len) in pieces {
            let Ok(off) = usize::try_from(off) else {
                continue;
            };
            let len = usize::try_from(len).unwrap_or(0).min(n.saturating_sub(off));
            if len == 0 {
                continue;
            }
            let part: Vec<u8> = match loc {
                Loc::Memory(a) => match self.memory(a, len as u64, &mut provenance) {
                    Ok(b) => b,
                    Err(e) => return Err((location, e)),
                },
                Loc::Register(r) => {
                    let (v, avail) = match self.register_piece(r) {
                        Ok(x) => x,
                        Err(e) => return Err((location, e)),
                    };
                    if len * 8 > usize::from(avail) {
                        return Err((location, Unavailable::RegisterClobbered(r)));
                    }
                    v.to_le_bytes()[..len.min(16)].to_vec()
                }
                Loc::Value(v) => v.to_le_bytes()[..len.min(8)].to_vec(),
                Loc::Implicit(b) => b.into_iter().take(len).collect(),
                Loc::Empty => return Err((location, Unavailable::NotLive)),
            };
            bytes[off..off + part.len()].copy_from_slice(&part);
        }
        Ok((location, bytes, provenance))
    }

    /// A register's value as a variable's piece holds it (PDB: the part
    /// of the DWARF register its number names), and how many of its bits
    /// are known.
    fn register_piece(&self, n: u16) -> Result<(u128, u16), Unavailable> {
        let (r, lsb, bits) = if self.pdb {
            self.dwarf_register(n)?
        } else {
            (n, 0, 128)
        };
        let (v, known) = self.register_bytes(r)?;
        let v = v >> lsb;
        let v = if bits >= 128 {
            v
        } else {
            v & ((1u128 << bits) - 1)
        };
        Ok((v, known.saturating_sub(lsb).min(bits)))
    }

    fn register_name(&self, n: u16) -> String {
        turbonop_dwarf::registers::name(self.arch, n).unwrap_or_else(|| format!("r{n}"))
    }
}

impl expr::Frame for Context<'_, '_, '_> {
    fn register(&self, n: u16) -> Result<u64, Unavailable> {
        let (v, _) = self.register_bytes(n)?;
        #[allow(clippy::cast_possible_truncation)]
        Ok(v as u64)
    }

    fn memory(&self, address: u64, len: u8) -> Result<u64, Unavailable> {
        let mut b = [0u8; 8];
        let n = usize::from(len.min(8));
        self.space
            .read(address, &mut b[..n])
            .ok_or(Unavailable::MemoryNotCaptured {
                address,
                len: u64::from(len),
            })?;
        Ok(u64::from_le_bytes(b))
    }

    fn cfa(&self) -> Result<u64, Unavailable> {
        Context::cfa(self)
    }

    fn frame_base(&self) -> Result<u64, Unavailable> {
        let fb = self
            .frame_base
            .as_ref()
            .ok_or(Unavailable::FramePointerUnknown)?;
        // (Evaluated without a frame base of its own.)
        let inner = Context {
            frame_base: None,
            ..*self
        };
        let addr_size = self.arch.pointer_size().unwrap_or(8);
        let pieces = expr::evaluate(fb, Endian::Little, addr_size, &inner)?;
        match pieces.first().map(|p| &p.loc) {
            Some(Loc::Memory(a)) => Ok(*a),
            Some(Loc::Register(r)) => expr::Frame::register(&inner, *r),
            Some(Loc::Value(v)) => Ok(*v),
            _ => Err(Unavailable::FramePointerUnknown),
        }
    }

    fn bias(&self) -> u64 {
        self.bias
    }

    /// A register's value at the function's entry: what the caller's call
    /// site says it passed (DWARF's `DW_AT_call_value`), evaluated in the
    /// caller's frame.
    fn entry_value(&self, expr: &[u8]) -> Result<u64, Unavailable> {
        let register = match expr {
            [op] if (0x50..=0x6f).contains(op) => u16::from(op - 0x50),
            [0x90, n] if n & 0x80 == 0 => u16::from(*n),
            _ => return Err(Unavailable::EntryValue),
        };
        let caller = self
            .stack
            .frames
            .get(self.index + 1)
            .ok_or(Unavailable::EntryValue)?;
        if caller.confidence != turbonop_frames::Confidence::Certain || !caller.return_address {
            return Err(Unavailable::EntryValue);
        }
        let m = caller.module.ok_or(Unavailable::EntryValue)?;
        let (debug, bias) = self.modules.debug(m).ok_or(Unavailable::EntryValue)?;
        let symbol = caller.symbol.as_ref().ok_or(Unavailable::EntryValue)?;
        let start = caller
            .lookup()
            .wrapping_sub(bias)
            .wrapping_sub(symbol.offset);
        let values = debug.call_site_values(start, caller.pc.wrapping_sub(bias));
        let value = values
            .iter()
            .find(|v| v.register == register)
            .ok_or(Unavailable::EntryValue)?;
        // (In the caller's frame, with its frame base.)
        let in_caller = Context {
            index: self.index + 1,
            frame_base: debug.frame_base(start),
            bias,
            ..*self
        };
        let addr_size = self.arch.pointer_size().unwrap_or(8);
        let pieces = expr::evaluate(&value.value, Endian::Little, addr_size, &in_caller)?;
        // (A call value is a value: the stack's top, not an address.)
        match pieces.first().map(|p| &p.loc) {
            Some(Loc::Memory(v) | Loc::Value(v)) => Ok(*v),
            Some(Loc::Register(r)) => expr::Frame::register(&in_caller, *r),
            _ => Err(Unavailable::EntryValue),
        }
    }
}
