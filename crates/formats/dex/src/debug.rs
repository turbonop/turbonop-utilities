//! Debug information (`debug_info_item`): a state machine's program that
//! gives each instruction its line and each register its local variable,
//! run as ART runs it.

use turbonop_core::{Error, ErrorKind, Result};

use crate::Dex;
use crate::reader::Cursor;

/// The opcodes.
pub mod op {
    /// The end of the program.
    pub const END_SEQUENCE: u8 = 0x00;
    /// Advances the address.
    pub const ADVANCE_PC: u8 = 0x01;
    /// Advances the line.
    pub const ADVANCE_LINE: u8 = 0x02;
    /// A local variable starts.
    pub const START_LOCAL: u8 = 0x03;
    /// A local variable with a generic signature starts.
    pub const START_LOCAL_EXTENDED: u8 = 0x04;
    /// A local variable ends.
    pub const END_LOCAL: u8 = 0x05;
    /// A local variable that ended starts again.
    pub const RESTART_LOCAL: u8 = 0x06;
    /// The prologue ends here.
    pub const SET_PROLOGUE_END: u8 = 0x07;
    /// The epilogue begins here.
    pub const SET_EPILOGUE_BEGIN: u8 = 0x08;
    /// The source file changes.
    pub const SET_FILE: u8 = 0x09;
    /// The first special opcode: each advances the address and the line,
    /// and emits a position.
    pub const FIRST_SPECIAL: u8 = 0x0A;
    /// The smallest line advance of a special opcode.
    pub const LINE_BASE: i32 = -4;
    /// How many line advances the special opcodes have.
    pub const LINE_RANGE: u8 = 15;
}

/// An instruction of the program.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum DebugOp {
    /// `DBG_END_SEQUENCE`.
    EndSequence,
    /// `DBG_ADVANCE_PC`.
    AdvancePc(u32),
    /// `DBG_ADVANCE_LINE`.
    AdvanceLine(i32),
    /// `DBG_START_LOCAL` and `DBG_START_LOCAL_EXTENDED`.
    StartLocal {
        /// The register.
        reg: u32,
        /// Its name (a string index), if it has one.
        name_idx: Option<u32>,
        /// Its type (a type index), if it has one.
        type_idx: Option<u32>,
        /// Its generic signature (a string index), if it has one (only
        /// the extended form can).
        signature_idx: Option<u32>,
        /// Whether it's `DBG_START_LOCAL_EXTENDED`.
        extended: bool,
    },
    /// `DBG_END_LOCAL`.
    EndLocal(u32),
    /// `DBG_RESTART_LOCAL`.
    RestartLocal(u32),
    /// `DBG_SET_PROLOGUE_END`.
    SetPrologueEnd,
    /// `DBG_SET_EPILOGUE_BEGIN`.
    SetEpilogueBegin,
    /// `DBG_SET_FILE`: the file's name (a string index), if any.
    SetFile(Option<u32>),
    /// A special opcode.
    Special(u8),
}

/// A debug information item.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DebugInfoItem {
    /// Where it is.
    pub offset: u32,
    /// The first line.
    pub line_start: u32,
    /// The parameters' names (string indices), if they have them.
    pub parameter_names: Vec<Option<u32>>,
    /// The program, through its `DBG_END_SEQUENCE`.
    pub ops: Vec<DebugOp>,
    /// Its size in bytes.
    pub size: u32,
}

/// A position: an address and its line.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Position {
    /// The instruction (in code units).
    pub address: u32,
    /// The line (as ART keeps it, unsigned: an advance below zero wraps).
    pub line: u32,
    /// The source file, if `DBG_SET_FILE` changed it (a string index).
    pub source_file_idx: Option<u32>,
    /// Whether the prologue ends here.
    pub prologue_end: bool,
    /// Whether the epilogue begins here.
    pub epilogue_begin: bool,
}

/// A local variable: its register over a range of addresses.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Local {
    /// The register.
    pub reg: u16,
    /// The first address it's live at (in code units).
    pub start: u32,
    /// The address after the last.
    pub end: u32,
    /// Whether it's `this` (and so has no name in the string table).
    pub is_this: bool,
    /// Its name (a string index), if it has one.
    pub name_idx: Option<u32>,
    /// Its type (a type index), if it has one.
    pub type_idx: Option<u32>,
    /// Its generic signature (a string index), if it has one.
    pub signature_idx: Option<u32>,
}

impl DebugInfoItem {
    /// Parses the debug information item at `off` of `data`.
    pub fn parse(data: &[u8], off: u32) -> Result<Self> {
        let mut c = Cursor::at(data, off as usize);
        let line_start = c.uleb128("line_start")?;
        let n = c.uleb128("parameters_size")?;
        let n = c.check_count(n, 1, "parameter names")?;
        let mut parameter_names = Vec::with_capacity(n);
        for _ in 0..n {
            parameter_names.push(c.uleb128p1("parameter name")?);
        }
        let mut ops = Vec::new();
        loop {
            let opcode = c.u8("debug opcode")?;
            let o = match opcode {
                op::END_SEQUENCE => DebugOp::EndSequence,
                op::ADVANCE_PC => DebugOp::AdvancePc(c.uleb128("addr_diff")?),
                op::ADVANCE_LINE => DebugOp::AdvanceLine(c.sleb128("line_diff")?),
                op::START_LOCAL | op::START_LOCAL_EXTENDED => {
                    let reg = c.uleb128("register_num")?;
                    let name_idx = c.uleb128p1("name_idx")?;
                    let type_idx = c.uleb128p1("type_idx")?;
                    let extended = opcode == op::START_LOCAL_EXTENDED;
                    let signature_idx = if extended {
                        c.uleb128p1("sig_idx")?
                    } else {
                        None
                    };
                    DebugOp::StartLocal {
                        reg,
                        name_idx,
                        type_idx,
                        signature_idx,
                        extended,
                    }
                }
                op::END_LOCAL => DebugOp::EndLocal(c.uleb128("register_num")?),
                op::RESTART_LOCAL => DebugOp::RestartLocal(c.uleb128("register_num")?),
                op::SET_PROLOGUE_END => DebugOp::SetPrologueEnd,
                op::SET_EPILOGUE_BEGIN => DebugOp::SetEpilogueBegin,
                op::SET_FILE => DebugOp::SetFile(c.uleb128p1("name_idx")?),
                special => DebugOp::Special(special),
            };
            ops.push(o);
            if o == DebugOp::EndSequence {
                break;
            }
        }
        #[allow(clippy::cast_possible_truncation)]
        let size = (c.pos() - off as usize) as u32;
        Ok(Self {
            offset: off,
            line_start,
            parameter_names,
            ops,
            size,
        })
    }

    /// The positions the program emits (one for each special opcode), as
    /// ART's `DecodeDebugPositionInfo` gives them.
    pub fn positions(&self) -> Vec<Position> {
        let mut out = Vec::new();
        let mut p = Position {
            address: 0,
            line: self.line_start,
            source_file_idx: None,
            prologue_end: false,
            epilogue_begin: false,
        };
        for &o in &self.ops {
            match o {
                DebugOp::EndSequence => break,
                DebugOp::AdvancePc(d) => p.address = p.address.wrapping_add(d),
                DebugOp::AdvanceLine(d) => p.line = p.line.wrapping_add_signed(d),
                DebugOp::SetPrologueEnd => p.prologue_end = true,
                DebugOp::SetEpilogueBegin => p.epilogue_begin = true,
                DebugOp::SetFile(f) => p.source_file_idx = f,
                DebugOp::Special(s) => {
                    let adjusted = s - op::FIRST_SPECIAL;
                    p.address = p.address.wrapping_add(u32::from(adjusted / op::LINE_RANGE));
                    p.line = p
                        .line
                        .wrapping_add_signed(op::LINE_BASE + i32::from(adjusted % op::LINE_RANGE));
                    out.push(p);
                    p.prologue_end = false;
                    p.epilogue_begin = false;
                }
                DebugOp::StartLocal { .. } | DebugOp::EndLocal(_) | DebugOp::RestartLocal(_) => {}
            }
        }
        out
    }

    /// The local variables the program describes for method `method_idx`
    /// (static or not), whose code has `registers_size` registers, the
    /// last `ins_size` its arguments, and `insns_size` code units: `this`
    /// and the parameters from the start, then each as it ends (and at
    /// the end, those still live, by register), as ART's
    /// `DecodeDebugLocalInfo` gives them. If the program is invalid for
    /// the method, the error, and the locals that came before it.
    pub fn locals(
        &self,
        dex: &Dex<'_>,
        method_idx: u32,
        is_static: bool,
        registers_size: u16,
        ins_size: u16,
        insns_size: u32,
    ) -> (Vec<Local>, Result<()>) {
        let mut out = Vec::new();
        let r = self.run_locals(dex, method_idx, is_static, registers_size, ins_size, insns_size, &mut out);
        (out, r)
    }

    #[allow(clippy::too_many_arguments)]
    fn run_locals(
        &self,
        dex: &Dex<'_>,
        method_idx: u32,
        is_static: bool,
        registers_size: u16,
        ins_size: u16,
        insns_size: u32,
        out: &mut Vec<Local>,
    ) -> Result<()> {
        let bad = |what: &'static str| Error::new(ErrorKind::Malformed, u64::from(self.offset), what);
        let method = dex.method(method_idx)?;
        let params = dex.proto_parameters(u32::from(method.proto_idx))?;
        let n = usize::from(registers_size);
        let mut live: Vec<Option<Local>> = vec![None; n];
        // Whether each register's local is live (a local stays after it
        // ends, for a restart).
        let mut is_live = vec![false; n];
        let mut arg_reg = registers_size.wrapping_sub(ins_size);
        let mut address = 0u32;
        if !is_static {
            if usize::from(arg_reg) >= n {
                return Err(bad("debug info: arg reg >= reg size"));
            }
            live[usize::from(arg_reg)] = Some(Local {
                reg: arg_reg,
                start: 0,
                end: 0,
                is_this: true,
                name_idx: None,
                type_idx: Some(u32::from(method.class_idx)),
                signature_idx: None,
            });
            is_live[usize::from(arg_reg)] = true;
            arg_reg = arg_reg.wrapping_add(1);
        }
        if params.len() != self.parameter_names.len() {
            return Err(bad("debug info: parameters_size"));
        }
        for (name, ty) in self.parameter_names.iter().zip(params.iter()) {
            if usize::from(arg_reg) >= n {
                return Err(bad("debug info: arg reg >= reg size"));
            }
            live[usize::from(arg_reg)] = Some(Local {
                reg: arg_reg,
                start: address,
                end: 0,
                is_this: false,
                name_idx: *name,
                type_idx: Some(u32::from(ty)),
                signature_idx: None,
            });
            is_live[usize::from(arg_reg)] = true;
            let wide = matches!(
                dex.type_descriptor(u32::from(ty)).ok().and_then(|d| d.first()),
                Some(b'D' | b'J')
            );
            arg_reg = arg_reg.wrapping_add(if wide { 2 } else { 1 });
        }
        for &o in &self.ops {
            match o {
                DebugOp::EndSequence => {
                    for (reg, l) in live.iter_mut().enumerate() {
                        if is_live[reg]
                            && let Some(l) = l
                        {
                            l.end = insns_size;
                            out.push(*l);
                        }
                    }
                    return Ok(());
                }
                DebugOp::AdvancePc(d) => address = address.wrapping_add(d),
                DebugOp::StartLocal {
                    reg,
                    name_idx,
                    type_idx,
                    signature_idx,
                    ..
                } => {
                    let r = (reg & 0xFFFF) as usize; // (ART keeps registers in 16 bits.)
                    if r >= n {
                        return Err(bad("debug info: reg > reg size"));
                    }
                    if is_live[r]
                        && let Some(mut l) = live[r]
                    {
                        l.end = address;
                        // (d8 declares generic parameters null in the
                        // header and starts them again at 0: ART drops
                        // the empty range.)
                        if l.end != 0 {
                            out.push(l);
                        }
                    }
                    // (ART keeps a type index in 16 bits.)
                    let type_idx = type_idx.map(|t| t & 0xFFFF).filter(|&t| t != 0xFFFF);
                    #[allow(clippy::cast_possible_truncation)]
                    let reg = reg as u16;
                    live[r] = Some(Local {
                        reg,
                        start: address,
                        end: 0,
                        is_this: false,
                        name_idx,
                        type_idx,
                        signature_idx,
                    });
                    is_live[r] = true;
                }
                DebugOp::EndLocal(reg) => {
                    let r = (reg & 0xFFFF) as usize; // (ART keeps registers in 16 bits.)
                    if r >= n {
                        return Err(bad("debug info: reg >= reg size"));
                    }
                    if !is_live[r] {
                        return Err(bad("debug info: end without start"));
                    }
                    if let Some(l) = &mut live[r] {
                        l.end = address;
                        out.push(*l);
                    }
                    is_live[r] = false;
                }
                DebugOp::RestartLocal(reg) => {
                    let r = (reg & 0xFFFF) as usize; // (ART keeps registers in 16 bits.)
                    if r >= n {
                        return Err(bad("debug info: reg >= reg size"));
                    }
                    if !is_live[r] {
                        // (A register never started restarts as ART's
                        // empty local: no name, no type, register 0.)
                        let l = live[r].get_or_insert(Local {
                            reg: 0,
                            start: 0,
                            end: 0,
                            is_this: false,
                            name_idx: None,
                            type_idx: None,
                            signature_idx: None,
                        });
                        l.start = address;
                        is_live[r] = true;
                    }
                }
                DebugOp::Special(s) => {
                    address = address.wrapping_add(u32::from((s - op::FIRST_SPECIAL) / op::LINE_RANGE));
                }
                DebugOp::AdvanceLine(_)
                | DebugOp::SetPrologueEnd
                | DebugOp::SetEpilogueBegin
                | DebugOp::SetFile(_) => {}
            }
        }
        // (The program always ends with DBG_END_SEQUENCE.)
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::reader::{encode_sleb128, encode_uleb128};

    #[test]
    fn positions_from_special_opcodes() {
        let mut d = vec![0u8; 2];
        encode_uleb128(10, &mut d); // line_start
        encode_uleb128(1, &mut d);
        encode_uleb128(0, &mut d); // parameter 0: no name
        d.push(op::SET_PROLOGUE_END);
        d.push(op::FIRST_SPECIAL + 4); // +0 address, +0 line
        d.push(op::ADVANCE_PC);
        encode_uleb128(3, &mut d);
        d.push(op::ADVANCE_LINE);
        encode_sleb128(-2, &mut d);
        d.push(op::SET_FILE);
        encode_uleb128(6, &mut d); // string 5
        d.push(op::FIRST_SPECIAL + 15 * 2 + 9); // +2 address, +5 line
        d.push(op::END_SEQUENCE);
        let item = DebugInfoItem::parse(&d, 2).unwrap();
        assert_eq!(item.size as usize, d.len() - 2);
        assert_eq!(item.parameter_names, [None]);
        let p = item.positions();
        assert_eq!(
            p.iter().map(|p| (p.address, p.line, p.source_file_idx, p.prologue_end)).collect::<Vec<_>>(),
            [(0, 10, None, true), (5, 13, Some(5), false)]
        );
        for cut in 2..d.len() {
            assert!(DebugInfoItem::parse(&d[..cut], 2).is_err(), "{cut}");
        }
    }
}
