//! What x86-64 instructions do, as far as the path analysis needs: which
//! registers each may write (every register, for one not described: so a
//! value is never taken to survive one), whether it may write memory,
//! which set the flags a branch tests and how, and what a branch's
//! condition is.

use turbonop_abi::{Abi, Preserved, regs};
use turbonop_x86::{Instruction, Mnemonic, Operand, RegClass, Register};

/// A set of the 16 general registers (by DWARF number's bit), or all.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Writes {
    pub registers: u32,
    /// Every register (an instruction not described).
    pub all: bool,
    pub memory: bool,
}

/// A general register's DWARF number (any width of it: AL, AX, EAX, RAX
/// are RAX).
pub fn dwarf(r: Register) -> Option<u16> {
    match r.class() {
        RegClass::Gpr64 | RegClass::Gpr32 | RegClass::Gpr16 | RegClass::Gpr8 => {
            regs::x64::BY_ENCODING.get(usize::from(r.index())).copied()
        }
        // AH, CH, DH, BH: RAX, RCX, RDX, RBX's.
        RegClass::Gpr8High => regs::x64::BY_ENCODING
            .get(usize::from(r.index() & 3))
            .copied(),
        _ => None,
    }
}

fn bit(n: u16) -> u32 {
    1u32 << n
}

/// What `i` may write. Calls write what the convention lets a callee
/// write (the volatile registers), the stack pointer, and memory.
pub fn writes(i: &Instruction, abi: Abi) -> Writes {
    use Mnemonic as M;
    let ops = i.operands();
    let dest = |w: &mut Writes| match ops.first() {
        // (A vector or other register: not one of these.)
        Some(Operand::Reg(r)) => {
            if let Some(n) = dwarf(*r) {
                w.registers |= bit(n);
            }
        }
        Some(Operand::Mem(_)) => w.memory = true,
        _ => {}
    };
    let rsp = bit(regs::x64::RSP);
    let mut w = Writes::default();
    match i.mnemonic() {
        // No register or memory written (flags, at most).
        M::Cmp | M::Test | M::Nop | M::Jmp | M::Bt => {}
        m if is_jcc(m) => {}
        M::Mov
        | M::Movzx
        | M::Movsx
        | M::Movsxd
        | M::Lea
        | M::Add
        | M::Sub
        | M::And
        | M::Or
        | M::Xor
        | M::Inc
        | M::Dec
        | M::Neg
        | M::Not
        | M::Shl
        | M::Shr
        | M::Sar
        | M::Rol
        | M::Ror
        | M::Adc
        | M::Sbb
        | M::Bsf
        | M::Bsr
        | M::Popcnt
        | M::Lzcnt
        | M::Tzcnt
        | M::Bswap
        | M::Bts
        | M::Btr
        | M::Btc => dest(&mut w),
        m if is_cmov(m) || is_setcc(m) => dest(&mut w),
        M::Imul if ops.len() >= 2 => dest(&mut w),
        M::Imul | M::Mul | M::Div | M::Idiv => {
            w.registers |= bit(regs::x64::RAX) | bit(regs::x64::RDX);
        }
        M::Cdq | M::Cqo | M::Cwd => w.registers |= bit(regs::x64::RDX),
        M::Cdqe | M::Cwde | M::Cbw => w.registers |= bit(regs::x64::RAX),
        M::Xchg | M::Xadd => {
            for o in ops {
                match o {
                    Operand::Reg(r) => {
                        if let Some(n) = dwarf(*r) {
                            w.registers |= bit(n);
                        }
                    }
                    Operand::Mem(_) => w.memory = true,
                    _ => {}
                }
            }
        }
        M::Cmpxchg => {
            dest(&mut w);
            w.registers |= bit(regs::x64::RAX);
        }
        M::Push => {
            w.registers |= rsp;
            w.memory = true;
        }
        M::Pop => {
            dest(&mut w);
            w.registers |= rsp;
        }
        M::Leave => w.registers |= rsp | bit(regs::x64::RBP),
        M::Ret => w.registers |= rsp,
        M::Call => {
            for &n in &regs::x64::BY_ENCODING {
                if abi.preserved(n) == Preserved::Not {
                    w.registers |= bit(n);
                }
            }
            w.registers |= rsp;
            w.memory = true;
        }
        // (int 3, ud2, hlt: they don't return.)
        M::Int | M::Ud2 | M::Hlt => {}
        // Anything else: anything.
        _ => {
            w.all = true;
            w.memory = true;
        }
    }
    w
}

fn is_jcc(m: Mnemonic) -> bool {
    condition(m).is_some()
}

fn is_cmov(m: Mnemonic) -> bool {
    use Mnemonic as M;
    matches!(
        m,
        M::Cmovo
            | M::Cmovno
            | M::Cmovb
            | M::Cmovae
            | M::Cmove
            | M::Cmovne
            | M::Cmovbe
            | M::Cmova
            | M::Cmovs
            | M::Cmovns
            | M::Cmovp
            | M::Cmovnp
            | M::Cmovl
            | M::Cmovge
            | M::Cmovle
            | M::Cmovg
    )
}

fn is_setcc(m: Mnemonic) -> bool {
    use Mnemonic as M;
    matches!(
        m,
        M::Seto
            | M::Setno
            | M::Setb
            | M::Setae
            | M::Sete
            | M::Setne
            | M::Setbe
            | M::Seta
            | M::Sets
            | M::Setns
            | M::Setp
            | M::Setnp
            | M::Setl
            | M::Setge
            | M::Setle
            | M::Setg
    )
}

/// A conditional branch's condition.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Cond {
    O,
    No,
    B,
    Ae,
    E,
    Ne,
    Be,
    A,
    S,
    Ns,
    P,
    Np,
    L,
    Ge,
    Le,
    G,
}

/// A conditional jump's condition (`jcxz`-like jumps aren't: they test a
/// register, not the flags).
pub fn condition(m: Mnemonic) -> Option<Cond> {
    use Mnemonic as M;
    Some(match m {
        M::Jo => Cond::O,
        M::Jno => Cond::No,
        M::Jb => Cond::B,
        M::Jae => Cond::Ae,
        M::Je => Cond::E,
        M::Jne => Cond::Ne,
        M::Jbe => Cond::Be,
        M::Ja => Cond::A,
        M::Js => Cond::S,
        M::Jns => Cond::Ns,
        M::Jp => Cond::P,
        M::Jnp => Cond::Np,
        M::Jl => Cond::L,
        M::Jge => Cond::Ge,
        M::Jle => Cond::Le,
        M::Jg => Cond::G,
        _ => return None,
    })
}

/// How an instruction sets the flags a condition tests.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FlagSetter {
    /// `cmp a, b`: the flags of `a - b`.
    Compare,
    /// `test a, b`: the flags of `a & b` (CF and OF clear).
    Test,
    /// An arithmetic or logic instruction whose result is its destination:
    /// ZF, SF, PF are its result's (CF and OF aren't known from it).
    Result,
}

/// Whether `i` sets the flags, and how; None if it doesn't touch them.
/// (`Some(None)`: it changes them in a way this doesn't describe.)
pub fn flags(i: &Instruction) -> Option<Option<FlagSetter>> {
    use Mnemonic as M;
    match i.mnemonic() {
        M::Cmp => Some(Some(FlagSetter::Compare)),
        M::Test => Some(Some(FlagSetter::Test)),
        M::Add | M::Sub | M::And | M::Or | M::Xor | M::Inc | M::Dec | M::Neg => {
            Some(Some(FlagSetter::Result))
        }
        // Instructions that leave the flags alone.
        M::Mov
        | M::Movzx
        | M::Movsx
        | M::Movsxd
        | M::Lea
        | M::Push
        | M::Pop
        | M::Nop
        | M::Not
        | M::Xchg
        | M::Bswap => None,
        m if is_cmov(m) || is_setcc(m) || is_jcc(m) => None,
        // Anything else may set them.
        _ => Some(None),
    }
}

/// A condition's value, given the flags' inputs: `a` and `b` (a `cmp`'s or
/// `test`'s operands, or the result for [`FlagSetter::Result`]) at
/// `width` bytes. None where they don't decide it.
pub fn decide(cond: Cond, setter: FlagSetter, a: u64, b: u64, width: u8) -> Option<bool> {
    let bits = u32::from(width) * 8;
    let mask = if bits >= 64 {
        u64::MAX
    } else {
        (1u64 << bits) - 1
    };
    let sign = 1u64 << (bits - 1);
    let (a, b) = (a & mask, b & mask);
    let (result, cf, of) = match setter {
        FlagSetter::Compare => {
            let r = a.wrapping_sub(b) & mask;
            let cf = a < b;
            // (Overflow: the operands' signs differ, and the result's isn't a's.)
            let of = (a ^ b) & (a ^ r) & sign != 0;
            (r, Some(cf), Some(of))
        }
        FlagSetter::Test => (a & b, Some(false), Some(false)),
        FlagSetter::Result => (a, None, None),
    };
    let zf = result == 0;
    let sf = result & sign != 0;
    let pf = (result & 0xFF).count_ones() % 2 == 0;
    Some(match cond {
        Cond::E => zf,
        Cond::Ne => !zf,
        Cond::S => sf,
        Cond::Ns => !sf,
        Cond::P => pf,
        Cond::Np => !pf,
        Cond::O => of?,
        Cond::No => !of?,
        Cond::B => cf?,
        Cond::Ae => !cf?,
        Cond::Be => cf? || zf,
        Cond::A => !cf? && !zf,
        Cond::L => sf != of?,
        Cond::Ge => sf == of?,
        Cond::Le => zf || sf != of?,
        Cond::G => !zf && sf == of?,
    })
}
