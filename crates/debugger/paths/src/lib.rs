//! How did we get here: which way each branch of a frame's function went on
//! the way to the frame's PC, from what the dump knows, each decision with
//! its proof and never more certain than it is.
//!
//! The function's code (from the dump, or its image) is decoded into basic
//! blocks. Only blocks that the entry reaches and that reach the PC can
//! have run on the way there. A branch is then decided:
//!
//! - **By control flow:** if only one of its successors can reach the PC,
//!   it went that way.
//! - **By values that survive:** otherwise, if the instruction that set the
//!   flags it tests (`cmp`, `test`, arithmetic) read registers that nothing
//!   on any way from it to the PC can have written (by every instruction's
//!   write set, and the calling convention's at calls), their values at the
//!   PC (known in the frame) are their values there, and the condition is
//!   evaluated with them. Memory operands count only if nothing on the way
//!   can have written memory.
//!
//! Every decision is certain (it follows from the code and the dump's
//! values); a branch neither decides is unknown. A branch in a loop is
//! decided for the last time it ran. The paths left, with the decisions
//! made, say how sure the whole is: one path is certain; several are one of
//! so many; a loop on the way leaves their number open.
//!
//! x86-64 only, so far.

pub mod x64;

use std::collections::{BTreeSet, VecDeque};

use turbonop_abi::Abi;
use turbonop_code::{Block, Edge, Flow, Insn};
use turbonop_core::Arch;
use turbonop_frames::{FrameRegisters, RegState};
use turbonop_target::AddressSpace;
use turbonop_x86::{Instruction, Mode, Operand};

use crate::x64::{FlagSetter, Writes};

/// Which way a branch went.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Way {
    Taken,
    NotTaken,
    /// Neither is shown.
    Unknown,
}

/// A conditional branch, decided (or not), and why.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Branch {
    pub address: u64,
    pub way: Way,
    /// The proof, in words (or why it couldn't be decided).
    pub why: String,
    /// Whether it's in a loop on the way to the PC (decided for the last
    /// time it ran).
    pub in_loop: bool,
}

/// What's known of a block.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Ran {
    /// On every way to the PC.
    Yes,
    /// On no way to the PC.
    No,
    /// On some.
    Maybe,
}

/// A basic block, and whether it ran.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct BlockRan {
    pub start: u64,
    pub end: u64,
    pub ran: Ran,
}

/// How many ways to the PC are left.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Paths {
    /// One: it was this way (certain).
    One,
    /// One of so many.
    OneOf(u64),
    /// One of a number a loop leaves open.
    Unbounded,
}

/// The analysis of a frame's function.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Report {
    pub function: u64,
    pub pc: u64,
    pub branches: Vec<Branch>,
    pub blocks: Vec<BlockRan>,
    pub paths: Paths,
}

/// Why a function can't be analyzed.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Unanalyzable {
    /// Not x86-64 (yet).
    Architecture,
    /// Its code isn't available (neither in the dump nor an image).
    NoCode,
    /// The PC isn't in its code as decoded.
    PcNotInFunction,
}

/// The most blocks whose ways are counted one by one.
const MAX_COUNT: u64 = 1 << 20;

/// Analyzes the function at `start` (its code at most `len` bytes) for the
/// way to `pc` (an instruction's address: for a caller's frame, its call's),
/// with the frame's registers.
///
/// # Errors
///
/// If the architecture isn't x86-64, the code isn't available, or the PC
/// isn't in it.
pub fn analyze(
    space: &AddressSpace<'_>,
    arch: Arch,
    abi: Abi,
    start: u64,
    len: u64,
    pc: u64,
    registers: &FrameRegisters,
) -> Result<Report, Unanalyzable> {
    if arch != Arch::X64 {
        return Err(Unanalyzable::Architecture);
    }
    // The code: as much of it as is available from the start.
    let runs = space.runs(start, len);
    let avail = runs.first().filter(|r| r.start == start).map_or(0, |r| {
        // (Contiguous runs from the start, whatever their sources.)
        let mut end = r.start + r.len;
        for x in &runs[1..] {
            if x.start != end {
                break;
            }
            end += x.len;
        }
        end - start
    });
    if avail == 0 {
        return Err(Unanalyzable::NoCode);
    }
    let mut code = vec![0u8; usize::try_from(avail).map_err(|_| Unanalyzable::NoCode)?];
    space.read(start, &mut code).ok_or(Unanalyzable::NoCode)?;
    let (insns, _) = turbonop_code::sweep(arch, &code, start);
    let decoded: Vec<Option<Instruction>> = insns
        .iter()
        .map(|i| {
            let off = usize::try_from(i.address - start).ok()?;
            turbonop_x86::decode(&code[off..], Mode::Bits64, i.address).ok()
        })
        .collect();
    let blocks = turbonop_code::blocks(&insns);
    let block_of = |addr: u64| {
        blocks.iter().position(|b| {
            let first = insns[b.first].address;
            let last = &insns[b.first + b.count - 1];
            addr >= first && addr < last.address + u64::from(last.len)
        })
    };
    let pc_block = block_of(pc).ok_or(Unanalyzable::PcNotInFunction)?;
    let n = blocks.len();
    let succ = |b: &Block| b.successors.iter().map(|s| s.0).collect::<Vec<_>>();
    // Which blocks reach the PC's, and which the entry reaches.
    let mut preds: Vec<Vec<usize>> = vec![Vec::new(); n];
    for (i, b) in blocks.iter().enumerate() {
        for s in succ(b) {
            preds[s].push(i);
        }
    }
    let reaches_pc = flood(pc_block, |b| preds[b].clone(), n);
    let from_entry = flood(0, |b| succ(&blocks[b]), n);
    let relevant: Vec<bool> = (0..n).map(|b| reaches_pc[b] && from_entry[b]).collect();
    let mut branches = Vec::new();
    // Each block's kept edges (after the decisions).
    let mut kept: Vec<Vec<usize>> = blocks.iter().map(succ).collect();
    for (bi, b) in blocks.iter().enumerate() {
        if !relevant[bi] {
            continue;
        }
        let last_i = b.first + b.count - 1;
        let last = &insns[last_i];
        if last.flow != Flow::Branch {
            continue;
        }
        let taken = b
            .successors
            .iter()
            .find(|s| s.1 == Edge::Taken)
            .map(|s| s.0);
        let fall = b
            .successors
            .iter()
            .find(|s| s.1 == Edge::NotTaken)
            .map(|s| s.0);
        let in_loop = flood(bi, |x| succ(&blocks[x]), n)
            .iter()
            .enumerate()
            .any(|(x, &r)| r && x != bi && succ(&blocks[x]).contains(&bi))
            || succ(b).contains(&bi);
        let (way, why) = match (taken.map(|t| reaches_pc[t]), fall.map(|f| reaches_pc[f])) {
            (Some(true), Some(false) | None) => {
                (Way::Taken, "only its target leads to the PC".to_string())
            }
            (Some(false) | None, Some(true)) => {
                (Way::NotTaken, "only its way on leads to the PC".to_string())
            }
            _ => by_values(
                space, abi, &insns, &decoded, &blocks, bi, pc, pc_block, registers,
            ),
        };
        match way {
            Way::Taken => kept[bi].retain(|&s| Some(s) == taken),
            Way::NotTaken => kept[bi].retain(|&s| Some(s) == fall),
            Way::Unknown => {}
        }
        branches.push(Branch {
            address: last.address,
            way,
            why,
            in_loop,
        });
    }
    // The ways left: counted on the relevant blocks' kept edges.
    let reach_kept = flood(0, |b| kept[b].clone(), n);
    let mut kept_preds: Vec<Vec<usize>> = vec![Vec::new(); n];
    for (b, ks) in kept.iter().enumerate() {
        for &s in ks {
            kept_preds[s].push(b);
        }
    }
    let back_kept = flood(pc_block, |b| kept_preds[b].clone(), n);
    let live: Vec<bool> = (0..n).map(|b| reach_kept[b] && back_kept[b]).collect();
    let paths = count_paths(&kept, &live, pc_block);
    let blocks_ran = blocks
        .iter()
        .enumerate()
        .map(|(bi, b)| {
            let last = &insns[b.first + b.count - 1];
            let ran = if !live[bi] {
                Ran::No
            } else if bi == 0 || bi == pc_block || cuts(&kept, &live, bi, pc_block) {
                Ran::Yes
            } else {
                Ran::Maybe
            };
            BlockRan {
                start: insns[b.first].address,
                end: last.address + u64::from(last.len),
                ran,
            }
        })
        .collect();
    Ok(Report {
        function: start,
        pc,
        branches,
        blocks: blocks_ran,
        paths,
    })
}

/// The blocks reachable from `from` by `next`.
fn flood(from: usize, next: impl Fn(usize) -> Vec<usize>, n: usize) -> Vec<bool> {
    let mut seen = vec![false; n];
    let mut q = VecDeque::from([from]);
    while let Some(b) = q.pop_front() {
        if b >= n || seen[b] {
            continue;
        }
        seen[b] = true;
        q.extend(next(b));
    }
    seen
}

/// Whether every way from the entry to the PC's block goes through `cut`.
fn cuts(kept: &[Vec<usize>], live: &[bool], cut: usize, pc_block: usize) -> bool {
    let n = kept.len();
    let reach = flood(
        0,
        |b| {
            if b == cut {
                Vec::new()
            } else {
                kept[b].iter().copied().filter(|&s| live[s]).collect()
            }
        },
        n,
    );
    !reach[pc_block]
}

/// How many ways from the entry to the PC's block, on live blocks.
fn count_paths(kept: &[Vec<usize>], live: &[bool], pc_block: usize) -> Paths {
    let n = kept.len();
    // A cycle among live blocks: open.
    let mut state = vec![0u8; n];
    fn cyclic(b: usize, kept: &[Vec<usize>], live: &[bool], state: &mut [u8]) -> bool {
        state[b] = 1;
        for &s in &kept[b] {
            if !live[s] {
                continue;
            }
            if state[s] == 1 || (state[s] == 0 && cyclic(s, kept, live, state)) {
                return true;
            }
        }
        state[b] = 2;
        false
    }
    if live[0] && cyclic(0, kept, live, &mut state) {
        return Paths::Unbounded;
    }
    let mut memo: Vec<Option<u64>> = vec![None; n];
    fn ways(
        b: usize,
        pc: usize,
        kept: &[Vec<usize>],
        live: &[bool],
        memo: &mut [Option<u64>],
    ) -> u64 {
        if b == pc {
            return 1;
        }
        if let Some(m) = memo[b] {
            return m;
        }
        let mut total = 0u64;
        for &s in &kept[b] {
            if live[s] {
                total = total
                    .saturating_add(ways(s, pc, kept, live, memo))
                    .min(MAX_COUNT);
            }
        }
        memo[b] = Some(total);
        total
    }
    match ways(0, pc_block, kept, live, &mut memo) {
        0 | 1 => Paths::One,
        k => Paths::OneOf(k),
    }
}

/// A branch decided by the values its flags were set from, if they
/// survive to the PC.
#[allow(clippy::too_many_arguments)]
fn by_values(
    space: &AddressSpace<'_>,
    abi: Abi,
    insns: &[Insn],
    decoded: &[Option<Instruction>],
    blocks: &[Block],
    bi: usize,
    pc: u64,
    pc_block: usize,
    registers: &FrameRegisters,
) -> (Way, String) {
    let b = &blocks[bi];
    let last_i = b.first + b.count - 1;
    let Some(cond) = decoded[last_i]
        .as_ref()
        .and_then(|i| x64::condition(i.mnemonic()))
    else {
        return (Way::Unknown, "not a flags branch".into());
    };
    // The flag setter: the nearest instruction before the branch that
    // touches the flags.
    let mut setter = None;
    for k in (b.first..last_i).rev() {
        let Some(i) = &decoded[k] else {
            return (
                Way::Unknown,
                "an instruction before it doesn't decode".into(),
            );
        };
        match x64::flags(i) {
            None => {}
            Some(Some(s)) => {
                setter = Some((k, s));
                break;
            }
            Some(None) => {
                return (
                    Way::Unknown,
                    format!(
                        "its flags are set by an instruction not modeled ({})",
                        turbonop_x86::format::to_string(i)
                    ),
                );
            }
        }
    }
    let Some((si, how)) = setter else {
        return (Way::Unknown, "its flags are set in another block".into());
    };
    let Some(set) = &decoded[si] else {
        return (Way::Unknown, "its flags' instruction doesn't decode".into());
    };
    // What may run between the setter and the PC: the rest of its block,
    // every relevant block after it (the successors' that reach the PC),
    // and the PC's block up to the PC.
    let n = blocks.len();
    let succ = |x: usize| blocks[x].successors.iter().map(|s| s.0).collect::<Vec<_>>();
    let mut preds: Vec<Vec<usize>> = vec![Vec::new(); n];
    for (i, bl) in blocks.iter().enumerate() {
        for s in succ(i) {
            preds[s].push(i);
        }
        let _ = bl;
    }
    let reaches_pc = flood(pc_block, |x| preds[x].clone(), n);
    let after: BTreeSet<usize> = flood(bi, succ, n)
        .iter()
        .enumerate()
        .filter(|&(x, &r)| r && reaches_pc[x])
        .map(|(x, _)| x)
        .collect();
    let mut between: Vec<usize> = ((si + 1)..=last_i).collect();
    for &x in &after {
        let bl = &blocks[x];
        let end = bl.first + bl.count;
        for (k, insn) in insns.iter().enumerate().take(end).skip(bl.first) {
            // (The PC's own instruction hasn't run: in a caller, it's the
            // call; preserved registers were preserved across it.)
            if x == pc_block && insn.address >= pc {
                break;
            }
            between.push(k);
        }
        // (The block itself again, in a loop: the setter's block's start.)
        if x == bi {
            between.extend(bl.first..=si);
        }
    }
    let mut written = Writes::default();
    for &k in &between {
        let Some(i) = &decoded[k] else {
            written.all = true;
            break;
        };
        let w = x64::writes(i, abi);
        written.registers |= w.registers;
        written.all |= w.all;
        written.memory |= w.memory;
    }
    // The setter's operands' values, if they survive.
    let width = set.operands().iter().find_map(|o| match o {
        Operand::Reg(r) => u8::try_from(r.size()).ok(),
        Operand::Mem(m) => u8::try_from(m.size.bytes()).ok(),
        _ => None,
    });
    let Some(width) = width.filter(|w| *w > 0 && *w <= 8) else {
        return (Way::Unknown, "its flags' operands aren't general".into());
    };
    let value = |o: &Operand| -> Result<u64, String> {
        match o {
            Operand::Imm(i) => Ok(i.value),
            Operand::Reg(r) => {
                let d = x64::dwarf(*r).ok_or("a register that isn't general")?;
                if written.all || written.registers & (1 << d) != 0 {
                    return Err(format!("{} may be written on the way to the PC", r.name()));
                }
                match registers.get(d) {
                    Some(RegState::Known { value, .. }) => {
                        // (AH..BH: bits 8 to 15.)
                        let v = if r.class() == turbonop_x86::RegClass::Gpr8High {
                            value >> 8
                        } else {
                            value
                        };
                        #[allow(clippy::cast_possible_truncation)]
                        Ok(v as u64)
                    }
                    _ => Err(format!("{} isn't known at the PC", r.name())),
                }
            }
            Operand::Mem(m) => {
                if written.memory || written.all {
                    return Err("memory may be written on the way to the PC".into());
                }
                let mut addr = m.disp.cast_unsigned();
                for r in [m.base, m.index].into_iter().flatten() {
                    let d = x64::dwarf(r).ok_or("an address register that isn't general")?;
                    if written.registers & (1 << d) != 0 {
                        return Err(format!("{} may be written on the way to the PC", r.name()));
                    }
                    let Some(RegState::Known { value, .. }) = registers.get(d) else {
                        return Err(format!("{} isn't known at the PC", r.name()));
                    };
                    #[allow(clippy::cast_possible_truncation)]
                    let v = value as u64;
                    addr = addr.wrapping_add(if Some(r) == m.index {
                        v.wrapping_mul(u64::from(m.scale))
                    } else {
                        v
                    });
                }
                if m.is_rip_relative() {
                    return Err("a RIP-relative operand".into());
                }
                let mut b = [0u8; 8];
                let n = usize::from(width);
                space
                    .read(addr, &mut b[..n])
                    .ok_or_else(|| format!("memory at {addr:#x} isn't in the dump"))?;
                Ok(u64::from_le_bytes(b))
            }
            _ => Err("an operand that isn't a value".into()),
        }
    };
    let ops: Vec<&Operand> = set
        .operands()
        .iter()
        .filter(|o| !matches!(o, Operand::None))
        .collect();
    let text = turbonop_x86::format::to_string(set);
    let inputs = match how {
        FlagSetter::Compare | FlagSetter::Test => match (ops.first(), ops.get(1)) {
            (Some(a), Some(b)) => (value(a), value(b)),
            _ => return (Way::Unknown, format!("{text}: its operands")),
        },
        // The result is the destination's value now, if it survives.
        FlagSetter::Result => match ops.first() {
            Some(a) => (value(a), Ok(0)),
            None => return (Way::Unknown, format!("{text}: its operands")),
        },
    };
    match inputs {
        (Ok(a), Ok(b)) => match x64::decide(cond, how, a, b, width) {
            Some(true) => (
                Way::Taken,
                format!(
                    "{text} at {:#x}: {a:#x} and {b:#x}, which survive to the PC, take it",
                    insns[si].address
                ),
            ),
            Some(false) => (
                Way::NotTaken,
                format!(
                    "{text} at {:#x}: {a:#x} and {b:#x}, which survive to the PC, don't take it",
                    insns[si].address
                ),
            ),
            None => (
                Way::Unknown,
                format!("{text}: its result alone doesn't decide this condition"),
            ),
        },
        (Err(e), _) | (_, Err(e)) => (Way::Unknown, format!("{text}: {e}")),
    }
}
