//! The addresses code refers to, found by disassembling it.
//!
//! - Direct calls and branches (x86 `call`/`jmp`/`jcc` with a relative
//!   target; ARM64 `B`, `BL`, `B.cond`, `BC.cond`, `CBZ`/`CBNZ`,
//!   `TBZ`/`TBNZ`).
//! - x86-64: every RIP-relative memory operand without a segment or index
//!   (`lea rax, [rip + x]`, `mov eax, [rip + x]`, `call [rip + x]`); with
//!   [`scan_image`], operands relative to the image's address too.
//! - ARM64: `ADRP` page addresses completed by the `ADD` (immediate) or
//!   unsigned-offset load or store that uses them, and addresses an `ADD`
//!   computes from those (an object's fields, an array, by immediate,
//!   unscaled, or register offset); `ADR`, and literal loads
//!   (`LDR x0, label`). A register's page is forgotten when an
//!   instruction may write the register, so an unrelated value is never
//!   taken for one.
//!
//! - 32-bit ARM (Thumb-2: Windows on ARM's only code, and Linux's armhf
//!   default): `BL`, `BLX`, `B`, `CBZ`/`CBNZ`; `ADR`; literal loads
//!   (`LDR r0, [pc, #n]`), the address of the pool's word and (as code
//!   holds an absolute address there) the address it holds; and `MOVW`
//!   with `MOVT` to one register, a 32-bit address in two halves.
//!
//! - RISC-V (compressed instructions by their full forms): `JAL` and the
//!   branches; `AUIPC` (a PC-relative upper part) or `LUI` (an absolute
//!   one) completed by the `ADDI`, load, store, or `JALR` (a far call or
//!   tail call) that uses them, and the loads and stores relative to an
//!   address an `ADDI` computed so. A register's part is forgotten when an
//!   instruction writes the register.
//!
//! 32-bit x86 code holds absolute addresses, which the image's relocations
//! (not disassembly) find. Decoding is a linear sweep that stops at the
//! first invalid instruction (data in a function's range, such as a
//! literal pool or jump table, can follow its code).

/// How code is read (its words' byte order, MIPS's image's flags).
pub use hexcavator_code::Mode;
/// The instruction sets (x86, x64, ARM64, 32-bit ARM, RISC-V, LoongArch,
/// PowerPC, and MIPS code is scanned; others have no references found).
pub use hexcavator_core::Arch;

/// How code refers to an address.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[non_exhaustive]
pub enum Via {
    /// A direct call.
    Call,
    /// A direct branch (within a function, or a tail call).
    Jump,
    /// An address an operand accesses (data loaded or stored, an import
    /// address table entry called through).
    Address,
    /// An address an instruction computes without accessing it (x86 `lea`,
    /// ARM64 `ADR` and `ADRP` with `ADD`): a datum's or a function whose
    /// address is taken.
    AddressOf,
}

/// One reference: the instruction's address, the address it refers to,
/// and how.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CodeRef {
    /// The referring instruction's address.
    pub from: u64,
    /// The address it refers to.
    pub to: u64,
    /// How.
    pub via: Via,
}

/// Appends the references of `code`, which is at `address`, to `out` (its
/// words little-endian: see [`scan_image`] for big-endian code).
pub fn scan(arch: Arch, code: &[u8], address: u64, out: &mut Vec<CodeRef>) {
    scan_image(arch, Mode::default(), code, address, None, out);
}

/// [`scan`], knowing where the image starts (`image_base`, in the same
/// address space as `address`): x86-64 code then also refers to tables by
/// their offset from a register holding the image's address, as MSVC
/// compiles them (`lea r9, [__ImageBase]`, then `[r9 + rcx*4 + table]`).
/// A register holds the image's address from a `lea` of exactly it until
/// an instruction writes the register (its first operand). Calls don't end
/// it: the sweep is in address order, not control flow, and code after a
/// call can be reached without one; a compiler reloads the register before
/// using it again after a call that changes it, and the reload is a write.
/// `mode` says how the code is read (its words' byte order; MIPS's
/// image's flags: see [`Mode`]).
pub fn scan_image(
    arch: Arch,
    mode: Mode,
    code: &[u8],
    address: u64,
    image_base: Option<u64>,
    out: &mut Vec<CodeRef>,
) {
    scan_absolute(arch, mode, code, address, image_base, 0, out);
}

/// [`scan_image`], with the absolute addresses code builds (32-bit ARM's
/// `MOVW` and `MOVT`, and its literal pools' words) moved by `bias` to the
/// nodes' address space: a PE image's preferred base, whose addresses are
/// relative to it.
pub fn scan_absolute(
    arch: Arch,
    mode: Mode,
    code: &[u8],
    address: u64,
    image_base: Option<u64>,
    bias: u64,
    out: &mut Vec<CodeRef>,
) {
    scan_with(
        arch,
        mode,
        code,
        address,
        image_base,
        bias,
        &Memory::NONE,
        out,
    );
}

/// What an image's words hold, for code that loads them.
#[derive(Clone, Copy)]
pub struct Memory<'a> {
    /// A GOT entry's pointer, by its address (if it is one's).
    pub got: &'a dyn Fn(u64) -> Option<u64>,
    /// A word in code's sections (a literal: PowerPC's offset to its GOT),
    /// by its address.
    pub literal: &'a dyn Fn(u64) -> Option<u64>,
}

impl Memory<'_> {
    /// Nothing known.
    pub const NONE: Memory<'static> = Memory {
        got: &|_| None,
        literal: &|_| None,
    };
}

/// [`scan_absolute`], knowing what the image's GOT entries and code's
/// literals hold (`memory`): code that loads a GOT entry has its value,
/// an address (as PIC code loads its data's addresses, and MIPS's its
/// data's 64 KiB pages'); one that loads a literal, its value (32-bit
/// PowerPC's PIC code its GOT's offset).
#[allow(clippy::too_many_arguments)]
pub fn scan_with(
    arch: Arch,
    mode: Mode,
    code: &[u8],
    address: u64,
    image_base: Option<u64>,
    bias: u64,
    memory: &Memory<'_>,
    out: &mut Vec<CodeRef>,
) {
    let got = memory.got;
    match arch {
        Arch::X86 => x86(code, address, hexcavator_x86::Mode::Bits32, None, out),
        Arch::X64 => x86(code, address, hexcavator_x86::Mode::Bits64, image_base, out),
        Arch::Arm64 => arm64(code, address, out),
        Arch::Arm => thumb(code, address & !1, bias, out),
        Arch::RiscV32 | Arch::RiscV64 => riscv(code, address, arch == Arch::RiscV64, out),
        Arch::LoongArch32 | Arch::LoongArch64 => {
            loongarch(code, address, arch == Arch::LoongArch64, out);
        }
        Arch::PowerPc | Arch::PowerPc64 => {
            powerpc(
                code,
                address,
                mode.big,
                arch == Arch::PowerPc64,
                memory,
                out,
            );
        }
        Arch::Mips | Arch::Mips64 => mips(code, address, mode, arch, got, out),
        // (No scanner for the others yet: no references.)
        _ => {}
    }
}

fn x86(
    code: &[u8],
    address: u64,
    mode: hexcavator_x86::Mode,
    image_base: Option<u64>,
    out: &mut Vec<CodeRef>,
) {
    let mut sweep = X86Sweep::new(image_base);
    let mut at = 0usize;
    while at < code.len() {
        let pc = address.wrapping_add(at as u64);
        let Ok(inst) = hexcavator_x86::decode(&code[at..], mode, pc) else {
            return;
        };
        sweep.step(&inst, pc, out);
        at += inst.len().max(1);
    }
}

/// The x86 sweep of [`scan_image`] an instruction at a time, for a caller
/// that decodes the code itself (and would otherwise decode it twice):
/// each instruction's references, given in order from the code's start
/// until one doesn't decode.
pub struct X86Sweep {
    image_base: Option<u64>,
    /// The registers holding the image's address, by number; and what
    /// they are at the targets of jumps ahead, where the sweep merges them
    /// in (a register holds it there if it does on any path seen).
    based: u32,
    /// Those that held it anywhere so far.
    ever: u32,
    ahead: Vec<(u64, u32)>,
}

impl X86Sweep {
    /// A sweep of x86 code; `image_base`, x64's (see [`scan_image`]).
    pub fn new(image_base: Option<u64>) -> Self {
        Self {
            image_base,
            based: 0,
            ever: 0,
            ahead: Vec::new(),
        }
    }

    /// The references of `inst`, at `pc`, pushed to `out`.
    pub fn step(&mut self, inst: &hexcavator_x86::Instruction, pc: u64, out: &mut Vec<CodeRef>) {
        use hexcavator_x86::{Flow, Mnemonic, Operand, RegClass, Register};
        let gpr = |r: Register| {
            matches!(
                r.class(),
                RegClass::Gpr8
                    | RegClass::Gpr8High
                    | RegClass::Gpr16
                    | RegClass::Gpr32
                    | RegClass::Gpr64
            )
            .then(|| r.full().index())
            .filter(|&i| i < 32)
        };
        let holds_base = |based: u32, r: Option<Register>| {
            r.and_then(gpr).is_some_and(|i| based & (1 << i) != 0)
        };
        if !self.ahead.is_empty() {
            let based = &mut self.based;
            self.ahead.retain(|&(target, regs)| {
                if target == pc {
                    *based |= regs;
                }
                target > pc
            });
        }
        if self.based != 0
            && matches!(inst.flow(), Flow::Jump | Flow::ConditionalJump)
            && let Some(target) = inst.branch_target()
            && target > pc
        {
            self.ahead.push((target, self.based));
        }
        let branch = matches!(inst.flow(), Flow::Call | Flow::Jump | Flow::ConditionalJump);
        let ops = inst.operands();
        let access = if inst.mnemonic() == Mnemonic::Lea {
            Via::AddressOf
        } else {
            Via::Address
        };
        if let Some(base) = self.image_base {
            for op in ops {
                // [base + index*scale + table], or [index*scale + table]
                // with the image's address added after (`lea rax,
                // [rax*4 + table]` then `add rax, rdx`): a register holds
                // it, and no base register makes the displacement absolute.
                // A register that held it anywhere in the function counts
                // too with an offset past the image's first page (a jump
                // table's cases run in an order the sweep can't see, so a
                // write in one case doesn't end it in the next; a field's
                // offset is rarely that large).
                let Operand::Mem(m) = op else {
                    continue;
                };
                let sticky = m.disp >= 0x1000;
                let held = |r| holds_base(self.based, r) || (sticky && holds_base(self.ever, r));
                if !m.is_rip_relative()
                    && m.disp > 0
                    && (held(m.base)
                        || (m.scale <= 1 && held(m.index))
                        || (m.base.is_none() && m.index.is_some() && (self.based | self.ever) != 0))
                {
                    out.push(CodeRef {
                        from: pc,
                        to: base.wrapping_add(m.disp as u64),
                        via: access,
                    });
                }
            }
            // What the instruction writes no longer holds the image's
            // address, unless it is a lea of it.
            let lea_of_base = match ops {
                [Operand::Reg(r), Operand::Mem(m)]
                    if inst.mnemonic() == Mnemonic::Lea && m.is_rip_relative() =>
                {
                    (inst.next_address().wrapping_add_signed(m.disp) == base)
                        .then(|| gpr(*r))
                        .flatten()
                }
                _ => None,
            };
            if let Some(Operand::Reg(r)) = ops.first()
                && let Some(i) = gpr(*r)
            {
                self.based &= !(1 << i);
            }
            if let Some(i) = lea_of_base {
                self.based |= 1 << i;
                self.ever |= 1 << i;
            }
        }
        for op in ops {
            match op {
                Operand::Rel(target) if branch => out.push(CodeRef {
                    from: pc,
                    to: *target,
                    via: if inst.flow() == Flow::Call {
                        Via::Call
                    } else {
                        Via::Jump
                    },
                }),
                Operand::Mem(m)
                    if m.is_rip_relative() && m.segment.is_none() && m.index.is_none() =>
                {
                    let to = inst.next_address().wrapping_add_signed(m.disp);
                    if to != 0 {
                        out.push(CodeRef {
                            from: pc,
                            to,
                            via: access,
                        });
                    }
                }
                _ => {}
            }
        }
    }
}

/// Sign-extends the low `bits` bits of `v`.
fn sext(v: u32, bits: u32) -> i64 {
    let shift = 64 - bits;
    (i64::from(v) << shift) >> shift
}

/// 32-bit ARM's Thumb-2 code (see the module's documentation); `bias`
/// is taken from the absolute addresses it builds.
fn thumb(code: &[u8], address: u64, bias: u64, out: &mut Vec<CodeRef>) {
    use hexcavator_arm32::{decode_t32, format::branch_target, is_t32_wide};
    let half = |at: usize| {
        code.get(at..at + 2)
            .map(|b| u16::from_le_bytes([b[0], b[1]]))
    };
    let word = |at: u64| {
        let i = usize::try_from(at.checked_sub(address)?).ok()?;
        let b = code.get(i..i.checked_add(4)?)?;
        Some(u32::from_le_bytes([b[0], b[1], b[2], b[3]]))
    };
    // Each register's low half, from a MOVW not yet overwritten; and the
    // word a literal load put in it (position-independent code adds the
    // PC to it: `ldr r4, [pc, #n]`, then `add r4, pc`).
    let mut low = [None::<u32>; 16];
    let mut loaded = [None::<u32>; 16];
    let mut at = 0usize;
    while let Some(first) = half(at) {
        let second = if is_t32_wide(first) {
            match half(at + 2) {
                Some(s) => Some(s),
                None => return,
            }
        } else {
            None
        };
        let Some(i) = decode_t32(first, second) else {
            return;
        };
        let pc = address.wrapping_add(at as u64);
        at += usize::from(i.size);
        let n = i.encoding.name;
        let mut push = |to: u64, via: Via| out.push(CodeRef { from: pc, to, via });
        if let Some(to) = branch_target(&i, pc) {
            push(
                to,
                if n.starts_with("BL_i") {
                    Via::Call
                } else {
                    Via::Jump
                },
            );
            continue;
        }
        let f = |name: &str| i.field(name).unwrap_or(0);
        // (Thumb's PC reads as the instruction's address and 4, a word's.)
        let base = pc.wrapping_add(4) & !3;
        match n {
            "LDR_l_T1" | "LDR_l_T2" => {
                let off = if n == "LDR_l_T1" {
                    i64::from(f("imm8") << 2)
                } else if i.field("U") == Some(0) {
                    -i64::from(f("imm12"))
                } else {
                    i64::from(f("imm12"))
                };
                let lit = base.wrapping_add_signed(off);
                push(lit, Via::Address);
                // (What a literal pool holds is an absolute address, as
                // often as not; one of the image's, if it's anything.)
                let rt = f("Rt") as usize & 15;
                let w = word(lit);
                if let Some(w) = w {
                    push((u64::from(w) & !1).wrapping_sub(bias), Via::AddressOf);
                }
                low[rt] = None;
                loaded[rt] = w;
                continue;
            }
            // `add rd, pc`: a position-independent address, from the
            // offset loaded into it.
            "ADD_r_T2" if f("Rm") == 15 => {
                let rd = (f("DN") << 3 | f("Rdn")) as usize & 15;
                if let Some(w) = loaded[rd].take() {
                    // (An offset either way, in a 32-bit address space.)
                    let at = pc
                        .wrapping_add(4)
                        .wrapping_add_signed(i64::from(w.cast_signed()));
                    push(at & 0xffff_ffff, Via::Address);
                }
                low[rd] = None;
                continue;
            }
            "ADR_T1" => push(base.wrapping_add(u64::from(f("imm8") << 2)), Via::AddressOf),
            "ADR_T2" | "ADR_T3" => {
                let imm = u64::from(f("i") << 11 | f("imm3") << 8 | f("imm8"));
                push(
                    if n == "ADR_T2" {
                        base.wrapping_sub(imm)
                    } else {
                        base.wrapping_add(imm)
                    },
                    Via::AddressOf,
                );
            }
            "MOV_i_T3" => {
                let imm = f("imm4") << 12 | f("i") << 11 | f("imm3") << 8 | f("imm8");
                low[f("Rd") as usize & 15] = Some(imm);
                continue;
            }
            "MOVT_T1" => {
                let rd = f("Rd") as usize & 15;
                if let Some(lo) = low[rd].take() {
                    let hi = f("imm4") << 12 | f("i") << 11 | f("imm3") << 8 | f("imm8");
                    // (A function's address has its Thumb bit set.)
                    push(
                        (u64::from(hi << 16 | lo) & !1).wrapping_sub(bias),
                        Via::AddressOf,
                    );
                }
                continue;
            }
            _ => {}
        }
        // Anything else that writes a register ends its half address,
        // and its loaded word.
        for r in ["Rd", "Rt", "Rdn"] {
            if let Some(v) = i.field(r) {
                low[v as usize & 15] = None;
                loaded[v as usize & 15] = None;
            }
        }
    }
}

fn riscv(code: &[u8], address: u64, rv64: bool, out: &mut Vec<CodeRef>) {
    use hexcavator_riscv::Operand;
    let features = hexcavator_riscv::Features::standard(rv64);
    // Each register's upper part (from AUIPC or LUI), and its address
    // (from an ADDI to an upper part), until it's written.
    let mut upper = [None::<u64>; 32];
    let mut addrs = [None::<u64>; 32];
    // (An address on RV32 wraps at 32 bits.)
    let wrap = |v: u64| if rv64 { v } else { v & 0xFFFF_FFFF };
    let mut at = 0usize;
    while at < code.len() {
        let pc = address.wrapping_add(at as u64);
        let Some(compact) = hexcavator_riscv::decode(&code[at..], &features) else {
            return;
        };
        at += usize::from(compact.size);
        let full = hexcavator_riscv::uncompress(&compact, &features);
        let i = full.as_ref().unwrap_or(&compact);
        let x = |n: &str| {
            i.operand(n)
                .and_then(hexcavator_riscv::gpr_of)
                .map(usize::from)
        };
        let imm = |n: &str| i.operand(n).and_then(Operand::imm);
        let mut push = |to: u64, via: Via| out.push(CodeRef { from: pc, to, via });
        let name = i.name();
        // What the instruction leaves in its destination: an upper part,
        // an address, or neither.
        let mut set_upper = None;
        let mut set_addr = None;
        match name {
            "AUIPC" | "LUI" => {
                // (The upper 20 bits, sign-extended from bit 31.)
                let hi = sext(
                    u32::try_from(imm("imm20").unwrap_or(0)).unwrap_or(0) << 12,
                    32,
                );
                let base = if name == "AUIPC" { pc } else { 0 };
                set_upper = Some(wrap(base.wrapping_add_signed(hi)));
            }
            "JAL" => {
                if let Some(off) = imm("imm20") {
                    let via = if x("rd") == Some(0) {
                        Via::Jump
                    } else {
                        Via::Call
                    };
                    push(wrap(pc.wrapping_add_signed(off)), via);
                }
            }
            "BEQ" | "BNE" | "BLT" | "BGE" | "BLTU" | "BGEU" => {
                if let Some(off) = imm("imm12") {
                    push(wrap(pc.wrapping_add_signed(off)), Via::Jump);
                }
            }
            "JALR" => {
                if let (Some(b), Some(off)) = (x("rs1").and_then(|r| upper[r]), imm("imm12")) {
                    let via = if x("rd") == Some(0) {
                        Via::Jump
                    } else {
                        Via::Call
                    };
                    push(wrap(b.wrapping_add_signed(off)), via);
                }
            }
            "ADDI" => {
                if let (Some(b), Some(off)) = (x("rs1").and_then(|r| upper[r]), imm("imm12")) {
                    let to = wrap(b.wrapping_add_signed(off));
                    push(to, Via::AddressOf);
                    set_addr = Some(to);
                }
            }
            "LB" | "LH" | "LW" | "LD" | "LBU" | "LHU" | "LWU" | "SB" | "SH" | "SW" | "SD"
            | "FLH" | "FLW" | "FLD" | "FLQ" | "FSH" | "FSW" | "FSD" | "FSQ" => {
                let base = x("rs1").and_then(|r| upper[r].or(addrs[r]));
                if let (Some(b), Some(off)) = (base, imm("imm12")) {
                    push(wrap(b.wrapping_add_signed(off)), Via::Address);
                }
            }
            _ => {}
        }
        // The registers it writes forget what they held (but what it set).
        for (k, op) in i
            .operands()
            .iter()
            .enumerate()
            .take(usize::from(i.inst().defs))
        {
            if let Some(r) = hexcavator_riscv::gpr_of(*op) {
                let r = usize::from(r);
                upper[r] = if k == 0 { set_upper } else { None };
                addrs[r] = if k == 0 { set_addr } else { None };
            }
        }
        // (x0 holds nothing.)
        upper[0] = None;
        addrs[0] = None;
    }
}

/// LoongArch's references: `b`, `bl` and the conditional branches; and
/// addresses built in a register, then used: `pcalau12i` (the 4 KiB page,
/// PC-relative), `pcaddu12i` and `pcaddu18i` (PC-relative upper parts),
/// `lu12i.w` (an absolute one), each with an `addi`, `ori`, load, store,
/// or `jirl` (`call36`) after; and `pcaddi` (an address, PC-relative).
fn loongarch(code: &[u8], address: u64, la64: bool, out: &mut Vec<CodeRef>) {
    let features = if la64 {
        hexcavator_loongarch::Features::la64()
    } else {
        hexcavator_loongarch::Features::la32()
    };
    // Each register's upper part, and its address (from an `addi` or
    // `ori` to an upper part, or `pcaddi`), until it's written.
    let mut upper = [None::<u64>; 32];
    let mut addrs = [None::<u64>; 32];
    // (An address on LA32 wraps at 32 bits.)
    let wrap = |v: u64| if la64 { v } else { v & 0xFFFF_FFFF };
    for (n, w) in code.as_chunks::<4>().0.iter().enumerate() {
        let pc = address.wrapping_add(n as u64 * 4);
        let Some(i) = hexcavator_loongarch::decode(w, &features) else {
            return;
        };
        let x = |name: &str| {
            i.operand(name)
                .and_then(hexcavator_loongarch::gpr_of)
                .map(usize::from)
        };
        // (Each instruction here has one immediate.)
        let imm = i.operands().iter().find_map(|o| o.imm());
        let mut push = |to: u64, via: Via| out.push(CodeRef { from: pc, to, via });
        let name = i.name();
        let mut set_upper = None;
        let mut set_addr = None;
        match name {
            "PCALAU12I" | "PCADDU12I" | "PCADDU18I" | "LU12I_W" => {
                let (base, shift) = match name {
                    "PCALAU12I" => (pc & !0xFFF, 12),
                    "PCADDU18I" => (pc, 18),
                    "PCADDU12I" => (pc, 12),
                    _ => (0, 12),
                };
                let hi = imm.unwrap_or(0) << shift;
                set_upper = Some(wrap(base.wrapping_add_signed(hi)));
            }
            "PCADDI" => {
                let to = wrap(pc.wrapping_add_signed(imm.unwrap_or(0) << 2));
                push(to, Via::AddressOf);
                set_addr = Some(to);
            }
            "B" | "BL" | "BEQ" | "BNE" | "BLT" | "BGE" | "BLTU" | "BGEU" | "BEQZ" | "BNEZ"
            | "BCEQZ" | "BCNEZ" => {
                if let Some(to) = hexcavator_loongarch::branch_target(&i, pc) {
                    let via = if name == "BL" { Via::Call } else { Via::Jump };
                    push(wrap(to), via);
                }
            }
            "JIRL" => {
                if let (Some(b), Some(off)) = (x("rj").and_then(|r| upper[r]), imm) {
                    let via = if x("rd") == Some(0) {
                        Via::Jump
                    } else {
                        Via::Call
                    };
                    push(wrap(b.wrapping_add_signed(off)), via);
                }
            }
            "ADDI_W" | "ADDI_D" | "ORI" => {
                if let (Some(b), Some(off)) = (x("rj").and_then(|r| upper[r]), imm) {
                    let to = if name == "ORI" {
                        b | off.cast_unsigned()
                    } else {
                        b.wrapping_add_signed(off)
                    };
                    let to = wrap(to);
                    push(to, Via::AddressOf);
                    set_addr = Some(to);
                }
            }
            "LD_B" | "LD_H" | "LD_W" | "LD_D" | "LD_BU" | "LD_HU" | "LD_WU" | "ST_B" | "ST_H"
            | "ST_W" | "ST_D" | "LDPTR_W" | "LDPTR_D" | "STPTR_W" | "STPTR_D" | "FLD_S"
            | "FLD_D" | "FST_S" | "FST_D" | "VLD" | "VST" | "XVLD" | "XVST" | "PRELD" => {
                let base = x("rj").and_then(|r| upper[r].or(addrs[r]));
                if let (Some(b), Some(off)) = (base, imm) {
                    push(wrap(b.wrapping_add_signed(off)), Via::Address);
                }
            }
            _ => {}
        }
        // The registers it writes forget what they held (but what it set).
        for (k, op) in i
            .operands()
            .iter()
            .enumerate()
            .take(usize::from(i.inst().defs))
        {
            if let Some(r) = hexcavator_loongarch::gpr_of(*op) {
                let r = usize::from(r);
                upper[r] = if k == 0 { set_upper } else { None };
                addrs[r] = if k == 0 { set_addr } else { None };
            }
        }
        // ($zero holds nothing.)
        upper[0] = None;
        addrs[0] = None;
    }
}

/// PowerPC's references: `b`, `bl`, `bc` and its kin (their targets,
/// relative or absolute); and addresses built in a register, then used:
/// `lis` (an absolute upper half), or `addis` to a register
/// whose value is known (an upper half above it: the TOC pointer, `r2`,
/// as an ELFv2 function's global entry computes it from `r12`, its own
/// address), each with an `addi`, `ori`, load, or store after; and the
/// prefixed instructions' PC-relative addresses (`pld 3, x@pcrel`,
/// `paddi 3, 0, x@pcrel, 1`). `r12` holds the function's address at its
/// start (the ELFv2 ABI's global entry).
#[allow(clippy::too_many_lines)]
fn powerpc(
    code: &[u8],
    address: u64,
    big: bool,
    ppc64: bool,
    memory: &Memory<'_>,
    out: &mut Vec<CodeRef>,
) {
    let features = hexcavator_powerpc::Features::standard(ppc64);
    // The link register, after a call (`bcl 20, 31, .+4`: the PC).
    let mut lr = None::<u64>;
    // Where a call just past itself lands (over a literal).
    let mut resume = None::<u64>;
    // Each register's value, if known, and whether it's an upper half
    // (from `addis`), until it's written.
    let mut known = [None::<(u64, bool)>; 32];
    known[12] = Some((address, false));
    let wrap = |v: u64| if ppc64 { v } else { v & 0xFFFF_FFFF };
    let mut at = 0usize;
    while at < code.len() {
        let pc = address.wrapping_add(at as u64);
        let Some(i) = hexcavator_powerpc::decode(&code[at..], big, &features) else {
            // (A literal a call steps over, `bl .+8`, as PIC code reads its
            // address: on, after it.)
            match resume.filter(|&t| t > pc) {
                Some(t) => {
                    at = usize::try_from(t - address).unwrap_or(usize::MAX);
                    continue;
                }
                None => return,
            }
        };
        at += usize::from(i.size);
        let reg = |name: &str| {
            i.operand(name)
                .and_then(hexcavator_powerpc::gpr_of)
                .map(usize::from)
        };
        let imm = |name: &str| i.operand(name).and_then(hexcavator_powerpc::Operand::imm);
        // (A load's or store's displacement and base.)
        let mem = hexcavator_powerpc::memory(&i)
            .and_then(|(d, r)| Some((d, usize::from(hexcavator_powerpc::gpr(r)?))));
        let mut push = |to: u64, via: Via| out.push(CodeRef { from: pc, to, via });
        let name = i.name();
        let mut set = None;
        let flow = hexcavator_code::powerpc_flow(&i, pc, &features).0;
        let next_lr = (flow == hexcavator_code::Flow::Call).then(|| wrap(pc + 4));
        if let Some(to) = hexcavator_powerpc::branch_target(&i, pc, &features) {
            push(
                wrap(to),
                if flow == hexcavator_code::Flow::Call {
                    Via::Call
                } else {
                    Via::Jump
                },
            );
        } else if name.starts_with('P') && name.ends_with("pc") {
            // A prefixed PC-relative instruction: `pld`, `plwz`, `pstd`,
            // `paddi`, ... (its displacement the address's offset).
            if let Some(d) = mem.map(|(d, _)| d).or_else(|| imm("SI")) {
                let to = wrap(pc.wrapping_add_signed(d));
                if name.starts_with("PADDI") || name.starts_with("PLI") {
                    push(to, Via::AddressOf);
                    set = Some((to, false));
                } else {
                    push(to, Via::Address);
                }
            }
        } else {
            match name {
                "LIS" => {
                    let hi = imm("D").unwrap_or(0) << 16;
                    set = Some((wrap(hi.cast_unsigned()), true));
                }
                "ADDIS" => {
                    // (RA r0 is read as 0: `lis`.)
                    let ra = reg("RA");
                    let hi = imm("D").unwrap_or(0) << 16;
                    let from = match ra {
                        Some(0) => Some(0),
                        Some(r) => known[r].map(|(v, _)| v),
                        None => None,
                    };
                    set = from.map(|b| (wrap(b.wrapping_add_signed(hi)), true));
                }
                "MFLR" => set = lr.map(|v| (v, false)),
                "ADD4" => {
                    if let (Some(a), Some(b)) = (reg("RA"), reg("RB"))
                        && let (Some((x, _)), Some((y, _))) = (known[a], known[b])
                    {
                        set = Some((wrap(x.wrapping_add(y)), false));
                    }
                }
                "ADDI" | "ORI" => {
                    let src = if name == "ORI" { reg("RS") } else { reg("RA") };
                    if let (Some(r), Some(lo)) = (src, imm("D"))
                        && let Some((b, upper)) = known[r]
                    {
                        let to = if name == "ORI" {
                            b | (lo.cast_unsigned() & 0xFFFF)
                        } else {
                            b.wrapping_add_signed(lo)
                        };
                        let to = wrap(to);
                        // (r2, the TOC pointer, and r12 at entry: the
                        // ABI's, not an object's address.)
                        if upper {
                            push(to, Via::AddressOf);
                        }
                        set = Some((to, false));
                    }
                }
                _ => {
                    // A load or store: `lwz 3, x@toc@l(4)`; or a load of a
                    // GOT entry (an address, which it takes) or of a
                    // literal (its value) from a register whose value is
                    // known.
                    let load = matches!(name, "LWZ" | "LD");
                    if let Some((d, r)) = mem
                        && let Some((b, upper)) = known[r]
                    {
                        let at = wrap(b.wrapping_add_signed(d));
                        if let Some(v) = (memory.got)(at).filter(|_| load) {
                            push(v, Via::AddressOf);
                            set = Some((v, true));
                        } else if let Some(v) = (memory.literal)(at).filter(|_| load) {
                            set = Some((v, false));
                        } else if upper {
                            push(at, Via::Address);
                        }
                    }
                }
            }
        }
        // The registers it writes forget what they held (but what it set).
        for (k, op) in i
            .operands()
            .iter()
            .enumerate()
            .take(usize::from(i.inst().defs))
        {
            if let Some(r) = hexcavator_powerpc::gpr_of(*op) {
                known[usize::from(r)] = if k == 0 { set } else { None };
            }
        }
        // (A call leaves r12 and the volatile registers unknown; r2 is
        // restored.)
        if flow == hexcavator_code::Flow::Call {
            for r in [0, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12] {
                known[r] = None;
            }
        }
        // (The link register: a call's return address; `mtlr` sets it to
        // what isn't followed.)
        if next_lr.is_some() || name == "MTLR" {
            lr = next_lr;
        }
        resume = hexcavator_powerpc::branch_target(&i, pc, &features)
            .filter(|&t| flow == hexcavator_code::Flow::Call && t > pc + 4 && t <= pc + 16);
    }
}

/// MIPS's references: its branches and jumps (`j`'s and `jal`'s region
/// taken from their delay slot's address), and addresses built in a
/// register, then used: from `lui`, through `addiu`, `daddiu`, `ori`, and
/// `dsll` (a 64-bit address takes six: `lui`, `daddiu`, `dsll`, `daddiu`,
/// `dsll`, and its use), to a load or store, a `jalr` or `jr` through it,
/// or any other use (an address computed: a pointer passed); and Release
/// 6's PC-relative ones (`auipc`, `addiupc`, `lwpc`, ...). `$gp` holds
/// the image's `$gp` (`mode`'s), its small data and GOT `$gp`-relative,
/// whatever reloads it; `$t9` holds the function's address at its start
/// (PIC code computes `$gp` from it, with `addu`). A call's delay slot
/// runs before it, with the registers as they were.
fn mips(
    code: &[u8],
    address: u64,
    mode: Mode,
    arch: Arch,
    got: &dyn Fn(u64) -> Option<u64>,
    out: &mut Vec<CodeRef>,
) {
    use hexcavator_code::Flow;
    let features = hexcavator_code::mips_features_in(arch, mode);
    let big = mode.big;
    let mips64 = arch == Arch::Mips64;
    let wrap = |v: u64| if mips64 { v } else { v & 0xFFFF_FFFF };
    // Each register's value, if built from a `lui` (or PC-relative); and
    // the address it holds that nothing has used yet (from where).
    let mut known = [None::<u64>; 32];
    let mut pending = [None::<(u64, u64)>; 32];
    known[25] = Some(address);
    known[28] = mode.mips_gp;
    let flush = |pending: &mut [Option<(u64, u64)>; 32], r: usize, out: &mut Vec<CodeRef>| {
        if let Some((from, to)) = pending[r].take() {
            out.push(CodeRef {
                from,
                to,
                via: Via::AddressOf,
            });
        }
    };
    // (A call's clobbers, after its delay slot.)
    let mut clobber = false;
    let mut at = 0usize;
    while at < code.len() {
        let pc = address.wrapping_add(at as u64);
        let Some(i) = hexcavator_mips::decode(&code[at..], big, &features) else {
            break;
        };
        at += hexcavator_mips::size(&i);
        let reg = |name: &str| {
            i.operand(name)
                .and_then(hexcavator_mips::gpr_of)
                .map(usize::from)
        };
        let imm = |name: &str| i.operand(name).and_then(hexcavator_mips::Operand::imm);
        let (flow, target) = hexcavator_code::mips_flow(&i, pc);
        // (microMIPS's by their standard forms' names: their operands'
        // are the same.)
        let name = match i.name() {
            "LUi_MM" | "LUI_MMR6" => "LUi",
            "ADDiu_MM" => "ADDiu",
            "ORi_MM" => "ORi",
            "ADDu_MM" | "ADDU16_MM" => "ADDu",
            "AUIPC_MMR6" => "AUIPC",
            "ALUIPC_MMR6" => "ALUIPC",
            "ADDIUPC_MM" | "ADDIUPC_MMR6" => "ADDIUPC",
            "LWPC_MMR6" => "LWPC",
            "LW_MM" | "LW16_MM" | "LWSP_MM" | "LWGP_MM" => "LW",
            other => other,
        };
        // What it sets its destination to, and the registers whose values
        // it continues building (not a use of them).
        let mut set = None;
        let mut building = [None::<usize>; 1];
        let mut push = |to: u64, via: Via| out.push(CodeRef { from: pc, to, via });
        if let Some(to) = target {
            push(
                wrap(to),
                if flow == Flow::Call {
                    Via::Call
                } else {
                    Via::Jump
                },
            );
        } else {
            match name {
                "LUi" => {
                    let hi = sext(
                        u32::try_from(imm("imm16").unwrap_or(0) & 0xFFFF).unwrap_or(0) << 16,
                        32,
                    );
                    set = Some(wrap(hi.cast_unsigned()));
                }
                "AUIPC" | "ALUIPC" => {
                    let hi = imm("imm").unwrap_or(0) << 16;
                    let base = if name == "ALUIPC" { pc & !0xFFFF } else { pc };
                    set = Some(wrap(base.wrapping_add_signed(hi)));
                }
                "ADDIUPC" => {
                    // (microMIPS's from its address's word.)
                    let base = if hexcavator_mips::is_micromips(&i) {
                        pc & !3
                    } else {
                        pc
                    };
                    let to = wrap(base.wrapping_add_signed(imm("imm").unwrap_or(0)));
                    push(to, Via::AddressOf);
                }
                "LWPC" | "LWUPC" | "LDPC" => {
                    let base = if name == "LDPC" { pc & !7 } else { pc };
                    push(
                        wrap(base.wrapping_add_signed(imm("imm").unwrap_or(0))),
                        Via::Address,
                    );
                }
                "ADDiu" | "DADDiu" | "ORi" => {
                    if let (Some(r), Some(lo)) = (reg("rs"), imm("imm16"))
                        && let Some(b) = known[r]
                    {
                        let to = if name == "ORi" {
                            b | (lo.cast_unsigned() & 0xFFFF)
                        } else {
                            b.wrapping_add_signed(lo)
                        };
                        set = Some(wrap(to));
                        building[0] = Some(r);
                    }
                }
                "ADDu" | "DADDu" => {
                    if let (Some(a), Some(b)) = (reg("rs"), reg("rt"))
                        && let (Some(x), Some(y)) = (known[a], known[b])
                    {
                        set = Some(wrap(x.wrapping_add(y)));
                        building[0] = Some(a);
                    }
                }
                "DSLL" | "DSLL32" => {
                    if let (Some(r), Some(sa)) = (reg("rt"), imm("shamt"))
                        && let Some(b) = known[r]
                    {
                        let sa = sa + if name == "DSLL32" { 32 } else { 0 };
                        set = Some(b.checked_shl(u32::try_from(sa).unwrap_or(64)).unwrap_or(0));
                        building[0] = Some(r);
                    }
                }
                _ => {
                    // A load or store (`lw $2, %lo(x)($1)`), or a jump or call
                    // through a register whose value is known.
                    if let (Some(r), Some(off)) = (reg("base"), imm("offset"))
                        && let Some(b) = known[r]
                    {
                        let at = wrap(b.wrapping_add_signed(off));
                        // (A GOT entry's load: its value, an address, as
                        // the register's.)
                        match got(at).filter(|_| matches!(name, "LW" | "LD")) {
                            Some(v) => set = Some(v),
                            None => push(at, Via::Address),
                        }
                        pending[r] = None;
                        building[0] = Some(r);
                    } else if matches!(flow, Flow::Call | Flow::Jump)
                        && let Some(r) = reg("rs")
                        && let Some(to) = known[r]
                    {
                        push(
                            to,
                            if flow == Flow::Call {
                                Via::Call
                            } else {
                                Via::Jump
                            },
                        );
                        pending[r] = None;
                        building[0] = Some(r);
                    }
                }
            }
        }
        let defs = usize::from(i.inst().defs);
        // What it reads otherwise uses the addresses there.
        for op in i.operands().iter().skip(defs) {
            if let Some(r) = hexcavator_mips::gpr_of(*op).map(usize::from)
                && building[0] != Some(r)
            {
                flush(&mut pending, r, out);
            }
        }
        if let Some(r) = building[0] {
            pending[r] = None;
        }
        // The registers it writes forget what they held (but what it set).
        for (k, op) in i.operands().iter().enumerate().take(defs) {
            if let Some(r) = hexcavator_mips::gpr_of(*op).map(usize::from) {
                flush(&mut pending, r, out);
                known[r] = if k == 0 { set } else { None };
                // (`$gp` is the image's, however it's set: computed, or
                // reloaded after a call.)
                if r == 28 && mode.mips_gp.is_some() {
                    known[r] = mode.mips_gp;
                    continue;
                }
                if k == 0
                    && let Some(to) = set
                    && building[0].is_some()
                {
                    pending[r] = Some((pc, to));
                }
            }
        }
        known[0] = None;
        pending[0] = None;
        // (A call leaves the caller-saved registers unknown, once its delay
        // slot has run.)
        let mut forget = |known: &mut [Option<u64>; 32], pending: &mut [Option<(u64, u64)>; 32]| {
            for r in (1..16).chain([24, 25, 31]) {
                flush(pending, r, out);
                known[r] = None;
            }
        };
        if clobber {
            forget(&mut known, &mut pending);
        }
        clobber = flow == Flow::Call;
        if clobber && !hexcavator_mips::has_delay_slot(&i) {
            forget(&mut known, &mut pending);
            clobber = false;
        }
    }
    for r in 0..32 {
        flush(&mut pending, r, out);
    }
}

fn arm64(code: &[u8], address: u64, out: &mut Vec<CodeRef>) {
    // Each register's page, from an ADRP not yet overwritten; and each
    // register's address, from an ADD to a page or to another address (an
    // object's fields and arrays are reached from it).
    let mut pages = [None::<u64>; 32];
    let mut addrs = [None::<u64>; 32];
    for (i, w) in code.as_chunks::<4>().0.iter().enumerate() {
        let word = u32::from_le_bytes([w[0], w[1], w[2], w[3]]);
        let pc = address.wrapping_add(i as u64 * 4);
        let Some(inst) = hexcavator_arm64::decode(word, pc) else {
            return;
        };
        let name = inst.encoding_name();
        let rd = (word & 31) as usize;
        let rn = ((word >> 5) & 31) as usize;
        let adr_imm = || sext(((word >> 5) & 0x7FFFF) << 2 | ((word >> 29) & 3), 21);
        let mut push = |to: u64, via: Via| out.push(CodeRef { from: pc, to, via });
        // The address an ADD computes, which its destination then holds.
        let mut computed = None;
        match name {
            "ADRP_only_pcreladdr" => {
                pages[rd] = Some((pc & !0xFFF).wrapping_add_signed(adr_imm() << 12));
                continue;
            }
            "BL_only_branch_imm" | "B_only_branch_imm" => {
                let to = pc.wrapping_add_signed(sext(word & 0x3FF_FFFF, 26) << 2);
                push(
                    to,
                    if name.starts_with("BL") {
                        Via::Call
                    } else {
                        Via::Jump
                    },
                );
                continue;
            }
            "B_only_condbranch" | "BC_only_condbranch" => {
                push(
                    pc.wrapping_add_signed(sext((word >> 5) & 0x7FFFF, 19) << 2),
                    Via::Jump,
                );
                continue;
            }
            "TBZ_only_testbranch" | "TBNZ_only_testbranch" => {
                push(
                    pc.wrapping_add_signed(sext((word >> 5) & 0x3FFF, 14) << 2),
                    Via::Jump,
                );
                continue;
            }
            _ => {}
        }
        if name.ends_with("_compbranch") {
            push(
                pc.wrapping_add_signed(sext((word >> 5) & 0x7FFFF, 19) << 2),
                Via::Jump,
            );
            continue;
        }
        if name == "ADR_only_pcreladdr" {
            push(pc.wrapping_add_signed(adr_imm()), Via::AddressOf);
        } else if name.ends_with("_loadlit") {
            push(
                pc.wrapping_add_signed(sext((word >> 5) & 0x7FFFF, 19) << 2),
                Via::Address,
            );
        } else if name.ends_with("ADD_64_addsub_imm") {
            if let Some(base) = pages[rn].or(addrs[rn]) {
                let shift = if (word >> 22) & 1 == 1 { 12 } else { 0 };
                let imm = u64::from((word >> 10) & 0xFFF) << shift;
                let to = base.wrapping_add(imm);
                push(to, Via::AddressOf);
                computed = Some(to);
            }
        } else if name.ends_with("_ldst_regoff")
            && let Some(base) = pages[rn].or(addrs[rn])
        {
            // An array's element: the array.
            push(base, Via::Address);
        } else if name.ends_with("_ldst_unscaled")
            && let Some(base) = addrs[rn]
        {
            push(
                base.wrapping_add_signed(sext((word >> 12) & 0x1FF, 9)),
                Via::Address,
            );
        } else if name.ends_with("_ldst_pos")
            && let Some(page) = pages[rn].or(addrs[rn])
        {
            let size = (word >> 30) & 3;
            let simd = (word >> 26) & 1 == 1;
            let opc = (word >> 22) & 3;
            // A 128-bit SIMD access scales by 16, the rest by their size.
            let scale = if simd && size == 0 && opc & 2 != 0 {
                4
            } else {
                size
            };
            let imm = u64::from((word >> 10) & 0xFFF) << scale;
            push(page.wrapping_add(imm), Via::Address);
        }
        // What the instruction may have written is no longer a page.
        // (Stores and branches write no general register at bits 0-4;
        // loads of SIMD registers don't either.)
        let simd_load = name.starts_with("LD") && (word >> 26) & 1 == 1;
        let writes_rd = !(name.starts_with("ST")
            || name.starts_with('B')
            || name.starts_with("CB")
            || name.starts_with("TB")
            || name.starts_with("RET")
            || name.starts_with("PRFM")
            || simd_load);
        if writes_rd {
            pages[rd] = None;
            addrs[rd] = None;
        }
        if name.starts_with("LDP") && !simd_load {
            pages[((word >> 10) & 31) as usize] = None;
            addrs[((word >> 10) & 31) as usize] = None;
        }
        // Writeback (pre- or post-indexed) changes the base.
        if name.contains("_ldst_immpre") || name.contains("_ldst_immpost") {
            pages[rn] = None;
            addrs[rn] = None;
        }
        if let Some(to) = computed {
            addrs[rd] = Some(to);
        }
    }
}
