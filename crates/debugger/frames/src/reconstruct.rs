//! Rebuilding a corrupt stack from what's left in its memory.
//!
//! When a walk fails (an overwritten return address, a smashed frame, a PC
//! in nothing), the stack still holds its older frames' return addresses.
//! [`reconstruct`] finds them and links them into the stacks they
//! probably were:
//!
//! 1. **Slots.** Every pointer-sized word in the range that points into a
//!    module ([`Slot`]): a return address only if the instruction before
//!    it is a call (and for a direct call, its target is noted).
//! 2. **Links.** From a return address R at slot A: R's function's unwind
//!    data, unwinding a frame at R with its stack pointer just above A,
//!    says where its caller's return address must be (and what it is);
//!    and a frame record (the saved frame pointer below A, pointing at the
//!    next record) says it independently. A direct call's target must be
//!    the function the frame below it is in (or a thunk that jumps there).
//! 3. **Chains.** Following links from each return address gives a chain;
//!    the longest are the probable stacks ([`Chain`]), each reaching a
//!    thread's entry point or not.
//!
//! A chain's [`ChainConfidence`]: certain only if it's the only one that
//! reaches a thread's entry point and every link is by unwind data;
//! one of *n* if *n* do; likely otherwise. (A chain's first frame may
//! itself be stale: a callee's frame that returned, its return address
//! left behind. Certainty is of the links, from where it starts.)

use turbonop_abi::{Abi, Os};
use turbonop_code::{Flow, decode};
use turbonop_core::Arch;
use turbonop_target::{AddressSpace, Target};

use crate::{Confidence, Frame, FrameRegisters, Method, Modules, RegState, Source, Walker};

/// A stack slot that points into a module.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Slot {
    /// Where it is.
    pub at: u64,
    /// What it holds (stripped of a pointer authentication code).
    pub value: u64,
    /// The module it points into.
    pub module: usize,
    /// Whether the instruction before it is a call (None: that code isn't
    /// available, neither in the dump nor in an image).
    pub after_call: Option<bool>,
    /// The call's target, for a direct call.
    pub call_target: Option<u64>,
}

/// How a chain's frame was reached from the one before it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LinkKind {
    /// The callee's unwind data says its caller's return address is here.
    UnwindData,
    /// The callee's frame record (its saved frame pointer) points here.
    FrameRecord,
}

/// A frame of a chain.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ChainFrame {
    /// The slot its return address is in.
    pub slot: u64,
    pub frame: Frame,
    /// How it was reached from the frame before it (None: the first).
    pub link: Option<LinkKind>,
}

/// How sure a chain is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ChainConfidence {
    /// It was this stack (from its first frame on).
    Certain,
    /// It was one of this many chains (each reaching a thread's entry).
    OneOf(usize),
    /// Probably this stack.
    Likely,
}

/// A probable stack.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Chain {
    pub frames: Vec<ChainFrame>,
    pub confidence: ChainConfidence,
    /// Whether its last frame is a thread's entry point (or its return
    /// address is the stack's last slot).
    pub reaches_entry: bool,
}

/// A stack rebuilt: its slots, and the probable stacks, likeliest first.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Reconstruction {
    pub slots: Vec<Slot>,
    pub chains: Vec<Chain>,
}

/// The most chains reported.
const MAX_CHAINS: usize = 8;

/// Functions threads start in (a chain ending in one reaches its entry).
const ENTRIES: &[&str] = &[
    "RtlUserThreadStart",
    "BaseThreadInitThunk",
    "start_thread",
    "__libc_start_main",
    "__libc_start_call_main",
    "_start",
    "thread_start",
    "_pthread_start",
    "clone",
    "__clone",
    "clone3",
    "__clone3",
    "__GI___clone3",
];

/// Rebuilds the stack in `range` (a thread's stack, from where its walk
/// failed, say).
pub fn reconstruct(
    target: &dyn Target,
    space: &AddressSpace<'_>,
    modules: &Modules<'_>,
    range: core::ops::Range<u64>,
) -> Reconstruction {
    let arch = target.arch();
    let Some(abi) = target.abi().or_else(|| Abi::new(arch, Os::Linux)) else {
        return Reconstruction {
            slots: Vec::new(),
            chains: Vec::new(),
        };
    };
    let w = Walker {
        target,
        space,
        modules,
        abi,
        arch,
        pc_reg: abi.program_counter(),
        sp_reg: abi.stack_pointer(),
    };
    let Some(size) = arch.pointer_size().map(u64::from) else {
        return Reconstruction {
            slots: Vec::new(),
            chains: Vec::new(),
        };
    };
    let start = range.start.next_multiple_of(size);
    let count = range.end.saturating_sub(start) / size;
    // Every slot, on every core.
    let indices: Vec<u64> = (0..count).collect();
    let slots: Vec<Slot> = turbonop_par::map(&indices, 4096, |&k| slot(&w, start + k * size))
        .into_iter()
        .flatten()
        .collect();
    // Each return address's links: where its caller's return address is.
    let links: Vec<Vec<(usize, LinkKind)>> =
        turbonop_par::map(&slots, 64, |s| links_from(&w, &slots, s, size));
    // The longest chain from each slot, from the top of the stack down.
    let n = slots.len();
    let mut best: Vec<(usize, Option<(usize, LinkKind)>)> = vec![(1, None); n];
    for i in (0..n).rev() {
        for &(j, kind) in &links[i] {
            if j > i && best[j].0 + 1 > best[i].0 {
                best[i] = (best[j].0 + 1, Some((j, kind)));
            }
        }
    }
    // Chains start at return addresses (after a call) no other links to.
    let mut linked_to = vec![false; n];
    for l in &links {
        for &(j, _) in l {
            linked_to[j] = true;
        }
    }
    let mut heads: Vec<usize> = (0..n)
        .filter(|&i| slots[i].after_call == Some(true) && !linked_to[i])
        .collect();
    heads.sort_by_key(|&i| core::cmp::Reverse(best[i].0));
    let mut chains: Vec<Chain> = Vec::new();
    for &h in heads.iter().take(MAX_CHAINS) {
        let mut frames = Vec::new();
        let mut at = Some((h, None));
        let mut all_unwind = true;
        while let Some((i, link)) = at {
            let s = &slots[i];
            if link == Some(LinkKind::FrameRecord) {
                all_unwind = false;
            }
            let regs = caller_registers(&w, s.value, s.at + size, Source::Scanned(s.at));
            let confidence = if link == Some(LinkKind::UnwindData) {
                Confidence::Certain
            } else {
                Confidence::Likely
            };
            frames.push(ChainFrame {
                slot: s.at,
                frame: w.frame(s.value, s.at + size, true, Method::Scan, confidence, regs),
                link,
            });
            at = best[i].1.map(|(j, kind)| (j, Some(kind)));
        }
        let last = frames.last().map(|f| &f.frame);
        let entry_name = last
            .and_then(|f| f.symbol.as_ref())
            .is_some_and(|s| ENTRIES.iter().any(|e| s.function.ends_with(e)));
        let at_top = frames.last().is_some_and(|f| f.slot + size >= range.end);
        chains.push(Chain {
            frames,
            confidence: if all_unwind {
                ChainConfidence::Certain
            } else {
                ChainConfidence::Likely
            },
            reaches_entry: entry_name || at_top,
        });
    }
    // Certainty is only for the one chain that reaches an entry by unwind
    // data alone; several are one of so many; the rest are likely.
    let reaching = chains
        .iter()
        .filter(|c| c.reaches_entry && c.confidence == ChainConfidence::Certain)
        .count();
    for c in &mut chains {
        c.confidence = match (c.reaches_entry, c.confidence, reaching) {
            (true, ChainConfidence::Certain, 1) => ChainConfidence::Certain,
            (true, ChainConfidence::Certain, n) => ChainConfidence::OneOf(n),
            _ => ChainConfidence::Likely,
        };
    }
    chains.sort_by_key(|c| {
        (
            !c.reaches_entry,
            c.confidence != ChainConfidence::Certain,
            core::cmp::Reverse(c.frames.len()),
        )
    });
    Reconstruction { slots, chains }
}

/// The slot at `at`, if it points into a module.
fn slot(w: &Walker<'_, '_, '_>, at: u64) -> Option<Slot> {
    let value = w.strip(w.word(at)?);
    let module = w.target.modules().iter().position(|m| m.contains(value))?;
    let (after_call, call_target) = call_before(w, value);
    Some(Slot {
        at,
        value,
        module,
        after_call,
        call_target,
    })
}

/// Whether the instruction before `ra` is a call (None: unknown), and its
/// target if direct.
fn call_before(w: &Walker<'_, '_, '_>, ra: u64) -> (Option<bool>, Option<u64>) {
    let longest: u64 = match w.arch {
        Arch::X86 | Arch::X64 => 8,
        _ => 4,
    };
    let Some(from) = ra.checked_sub(longest) else {
        return (None, None);
    };
    let mut code = [0u8; 8];
    let n = usize::try_from(longest).unwrap_or(8);
    if w.space.read(from, &mut code[..n]).is_none() {
        return (None, None);
    }
    for back in (2..=longest).rev() {
        let at = ra - back;
        let i = usize::try_from(longest - back).unwrap_or(0);
        // (ARM: a Thumb return address keeps its bit, and so is decoded.)
        let decoded_at = if w.arch == Arch::Arm && ra & 1 != 0 {
            at | 1
        } else {
            at
        };
        if let Some(insn) = decode(w.arch, &code[i..n], decoded_at)
            && insn.flow == Flow::Call
            && at + u64::from(insn.len) == (ra & !u64::from(w.arch == Arch::Arm))
        {
            return (Some(true), insn.target);
        }
        if !matches!(w.arch, Arch::X86 | Arch::X64) {
            break;
        }
    }
    (Some(false), None)
}

/// A caller's registers as a chain knows them: its PC and stack pointer.
fn caller_registers(w: &Walker<'_, '_, '_>, pc: u64, sp: u64, from: Source) -> FrameRegisters {
    let mut r = FrameRegisters::new();
    r.set(w.pc_reg, Some(RegState::known(u128::from(pc), from)));
    r.set(w.sp_reg, Some(RegState::known(u128::from(sp), from)));
    r
}

/// Where the return address at slot `s` leads: the slots its caller's
/// return address is in, by its function's unwind data and by its frame
/// record.
fn links_from(
    w: &Walker<'_, '_, '_>,
    slots: &[Slot],
    s: &Slot,
    size: u64,
) -> Vec<(usize, LinkKind)> {
    let find = |at: u64| slots.binary_search_by_key(&at, |x| x.at).ok();
    let mut out = Vec::new();
    // By unwind data: a frame at the return address, its stack pointer
    // just above the slot (x86: the return popped it), unwound.
    if matches!(w.arch, Arch::X86 | Arch::X64) {
        let callee = w.frame(
            s.value,
            s.at + size,
            true,
            Method::Scan,
            Confidence::Likely,
            caller_registers(w, s.value, s.at + size, Source::Scanned(s.at)),
        );
        if let Some(Some(caller)) = w.by_unwind_data(&callee)
            && let Some(j) = find(caller.sp - size)
            && slots[j].value == caller.pc
            && consistent(w, &slots[j], &callee)
        {
            out.push((j, LinkKind::UnwindData));
        }
    }
    // By frame record: the record below the slot (the saved frame pointer,
    // then this return address), whose saved frame pointer points at the
    // caller's record.
    if let Some(saved) = s.at.checked_sub(size).and_then(|a| w.word(a))
        && saved > s.at
        && saved % size == 0
        && let Some(j) = find(saved + size)
        && !out.iter().any(|&(k, _)| k == j)
    {
        let callee = w.frame(
            s.value,
            s.at + size,
            true,
            Method::Scan,
            Confidence::Likely,
            FrameRegisters::new(),
        );
        if consistent(w, &slots[j], &callee) {
            out.push((j, LinkKind::FrameRecord));
        }
    }
    out
}

/// Whether the caller's call (before `caller`'s return address) can have
/// called `callee`'s function: an indirect call can; a direct one, if its
/// target is the function (or a thunk, which jumps on).
fn consistent(w: &Walker<'_, '_, '_>, caller: &Slot, callee: &Frame) -> bool {
    let Some(target) = caller.call_target else {
        return true;
    };
    let Some(sym) = &callee.symbol else {
        return true;
    };
    let start = callee.lookup().wrapping_sub(sym.offset);
    if target == start {
        return true;
    }
    // A thunk: the target jumps.
    let mut code = [0u8; 16];
    if w.space.read(target, &mut code).is_some()
        && let Some(i) = decode(w.arch, &code, target)
    {
        return i.flow == Flow::Jump;
    }
    // (Its code unknown: allowed.)
    true
}
