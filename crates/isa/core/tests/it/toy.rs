//! The vocabulary's pieces, and a toy instruction set that implements
//! every trait, read by tools written against the traits alone (as real
//! tools are).

use turbonop_isa::*;

/// The toy's registers: r0 to r7 (64-bit), their low halves h0 to h7, a
/// zero register, and a flags register.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
enum Reg {
    R(u8),
    H(u8),
    Zero,
    Flags,
}

impl Register for Reg {
    const STORAGE_COUNT: u16 = 9;

    fn storage(self) -> Option<u16> {
        match self {
            Reg::R(n) | Reg::H(n) => Some(u16::from(n)),
            Reg::Zero => None,
            Reg::Flags => Some(8),
        }
    }

    fn bits(self) -> Bits {
        match self {
            Reg::H(_) => Bits::low(32),
            Reg::Flags => Bits::low(4),
            _ => Bits::low(64),
        }
    }

    fn from_storage(s: u16) -> Option<Reg> {
        match s {
            0..=7 => u8::try_from(s).ok().map(Reg::R),
            8 => Some(Reg::Flags),
            _ => None,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
enum Op {
    Nop,
    Add,
    Jump,
    BranchZero,
    Return,
    Load,
}

impl Opcode for Op {
    const COUNT: u32 = 6;

    fn index(self) -> u32 {
        self as u32
    }

    fn mnemonic(self) -> &'static str {
        ["nop", "add", "jmp", "bz", "ret", "ld"][self as usize]
    }
}

/// An instruction: its opcode byte and up to two operand bytes.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
struct Insn {
    bytes: [u8; 3],
    len: u8,
}

/// The toy: `00` nop; `01 ab` add ra, rb (sets flags); `02 d` jmp
/// (relative to the next); `03 d` bz (if flags' zero); `04` ret; `05 ab
/// d` ld ha, [rb + d].
#[derive(Clone, Debug)]
struct Toy;

impl Toy {
    fn op(i: &Insn) -> Op {
        [
            Op::Nop,
            Op::Add,
            Op::Jump,
            Op::BranchZero,
            Op::Return,
            Op::Load,
        ][usize::from(i.bytes[0])]
    }

    fn target(i: &Insn, address: u64) -> u64 {
        (address + u64::from(i.len)).wrapping_add_signed(i64::from(i.bytes[1] as i8))
    }
}

impl Isa for Toy {
    type Instruction = Insn;
    type Opcode = Op;
    type Context = ();

    const MIN_LENGTH: usize = 1;
    const MAX_LENGTH: usize = 3;
    const ALIGNMENT: usize = 1;

    fn context(&self, _: &[u8], _: u64) {}

    fn decode(&self, _: &(), bytes: &[u8], _: u64) -> Result<Insn, DecodeError> {
        let first = *bytes.first().ok_or(DecodeError::Truncated { needed: 1 })?;
        let len: u8 = match first {
            0 | 4 => 1,
            1..=3 => 2,
            5 => 3,
            _ => return Err(DecodeError::Undefined),
        };
        let b = bytes
            .get(..usize::from(len))
            .ok_or(DecodeError::Truncated { needed: len })?;
        let mut i = Insn { bytes: [0; 3], len };
        i.bytes[..usize::from(len)].copy_from_slice(b);
        Ok(i)
    }

    fn bytes(i: &Insn) -> &[u8] {
        &i.bytes[..usize::from(i.len)]
    }

    fn opcode(i: &Insn) -> Op {
        Toy::op(i)
    }

    fn flow(&self, _: &(), i: &Insn, address: u64) -> ControlFlow {
        match Toy::op(i) {
            Op::Jump => ControlFlow::new(FlowKind::Jump, Target::Direct(Toy::target(i, address))),
            Op::BranchZero => {
                ControlFlow::new(FlowKind::Jump, Target::Direct(Toy::target(i, address))).when(true)
            }
            Op::Return => ControlFlow::RETURN,
            _ => ControlFlow::NEXT,
        }
    }

    fn attributes(&self, i: &Insn) -> Attributes {
        if Toy::op(i) == Op::Nop {
            Attributes::NOP
        } else {
            Attributes::NONE
        }
    }
}

fn reg_a(i: &Insn) -> u8 {
    i.bytes[1] >> 4 & 7
}

fn reg_b(i: &Insn) -> u8 {
    i.bytes[1] & 7
}

impl Registers for Toy {
    type Register = Reg;
    type RegisterSet = BitSet<Reg, 1>;

    fn register_accesses(
        &self,
        _: &(),
        i: &Insn,
        _: u64,
    ) -> impl Iterator<Item = RegisterAccess<Reg>> {
        let explicit = |register, access, other_bits| RegisterAccess {
            register,
            access,
            other_bits,
            implicit: false,
        };
        let mut out = [None; 3];
        match Toy::op(i) {
            Op::Add => {
                out[0] = Some(explicit(
                    Reg::R(reg_a(i)),
                    Access::READ_WRITE,
                    OtherBits::Preserved,
                ));
                out[1] = Some(explicit(
                    Reg::R(reg_b(i)),
                    Access::READ,
                    OtherBits::Preserved,
                ));
                out[2] = Some(RegisterAccess {
                    register: Reg::Flags,
                    access: Access::WRITE,
                    other_bits: OtherBits::Preserved,
                    implicit: true,
                });
            }
            Op::BranchZero => {
                out[0] = Some(RegisterAccess {
                    register: Reg::Flags,
                    access: Access::READ,
                    other_bits: OtherBits::Preserved,
                    implicit: true,
                });
            }
            Op::Load => {
                out[0] = Some(explicit(Reg::H(reg_a(i)), Access::WRITE, OtherBits::Zeroed));
                out[1] = Some(explicit(
                    Reg::R(reg_b(i)),
                    Access::READ,
                    OtherBits::Preserved,
                ));
            }
            _ => {}
        }
        out.into_iter().flatten()
    }
}

impl Memory for Toy {
    fn memory_accesses(&self, _: &(), i: &Insn, _: u64) -> impl Iterator<Item = MemoryAccess<Reg>> {
        (Toy::op(i) == Op::Load)
            .then(|| {
                MemoryAccess::new(
                    Access::READ,
                    Base::Register(Reg::R(reg_b(i))),
                    i64::from(i.bytes[2] as i8),
                    Some(4),
                )
            })
            .into_iter()
    }
}

impl Operands for Toy {
    fn operands(&self, cx: &(), i: &Insn, address: u64) -> impl Iterator<Item = Operand<Reg>> {
        let mut out = [None; 2];
        match Toy::op(i) {
            Op::Add => {
                out[0] = Some(Operand::Register(Reg::R(reg_a(i)), Access::READ_WRITE));
                out[1] = Some(Operand::Register(Reg::R(reg_b(i)), Access::READ));
            }
            Op::Jump | Op::BranchZero => {
                out[0] = self
                    .flow(cx, i, address)
                    .direct_target()
                    .map(Operand::Target);
            }
            Op::Load => {
                out[0] = Some(Operand::Register(Reg::H(reg_a(i)), Access::WRITE));
                out[1] = self
                    .memory_accesses(cx, i, address)
                    .next()
                    .map(Operand::Memory);
            }
            _ => {}
        }
        out.into_iter().flatten()
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
struct Plain;

impl Format for Toy {
    type Syntax = Plain;

    fn format<S: TokenSink + ?Sized, N: Resolver + ?Sized>(
        &self,
        cx: &(),
        i: &Insn,
        address: u64,
        _: Plain,
        names: &N,
        out: &mut S,
    ) {
        use core::fmt::Write as _;
        out.token(TokenKind::Mnemonic, Toy::op(i).mnemonic());
        for (k, o) in self.operands(cx, i, address).enumerate() {
            out.token(TokenKind::Punctuation, if k == 0 { " " } else { ", " });
            let mut text = String::new();
            match o {
                Operand::Register(Reg::R(n), _) => {
                    let _ = write!(text, "r{n}");
                    out.token(TokenKind::Register, &text);
                }
                Operand::Register(Reg::H(n), _) => {
                    let _ = write!(text, "h{n}");
                    out.token(TokenKind::Register, &text);
                }
                Operand::Target(a) => {
                    let mut named = String::new();
                    if names.resolve(Reference::Address(a), &mut named) {
                        out.token(TokenKind::Name, &named);
                    } else {
                        let _ = write!(text, "{a:#x}");
                        out.token(TokenKind::Address, &text);
                    }
                }
                Operand::Memory(m) => {
                    let Base::Register(Reg::R(b)) = m.base else {
                        unreachable!()
                    };
                    let _ = write!(text, "[r{b}{:+}]", m.displacement);
                    out.token(TokenKind::Number, &text);
                }
                _ => out.token(TokenKind::Keyword, "?"),
            }
        }
    }
}

// --- Tools, written against the traits alone ---

/// Every instruction of `code`, in order, to the first that doesn't
/// decode.
fn sweep<I: Isa>(isa: &I, code: &[u8], address: u64) -> Vec<(u64, I::Instruction)> {
    let cx = isa.context(code, address);
    let mut out = Vec::new();
    let mut at = 0;
    while at < code.len() {
        let Ok(i) = isa.decode(&cx, &code[at..], address + at as u64) else {
            break;
        };
        out.push((address + at as u64, i));
        at += I::len(&i);
    }
    out
}

/// The addresses that start basic blocks.
fn leaders<I: Isa>(isa: &I, code: &[u8], address: u64) -> Vec<u64> {
    let cx = isa.context(code, address);
    let mut out = vec![address];
    for (at, i) in sweep(isa, code, address) {
        let f = isa.flow(&cx, &i, at);
        out.extend(f.direct_target());
        let next = at + I::len(&i) as u64;
        if f.ends_block() && next < address + code.len() as u64 {
            out.push(next);
        }
    }
    out.sort_unstable();
    out.dedup();
    out
}

/// The registers each instruction writes that a later one reads.
fn live_writes<I: Registers>(isa: &I, code: &[u8], address: u64) -> I::RegisterSet {
    let cx = isa.context(code, address);
    let (mut written, mut used) = (I::RegisterSet::default(), I::RegisterSet::default());
    for (at, i) in sweep(isa, code, address) {
        let (r, w) = isa.register_sets(&cx, &i, at);
        used = used.union(&r.intersection(&written));
        written = written.union(&w);
    }
    used
}

/// Each constant address loaded from or stored to, and each load's base.
fn bases<I: Memory>(isa: &I, code: &[u8], address: u64) -> Vec<Base<I::Register>> {
    let cx = isa.context(code, address);
    sweep(isa, code, address)
        .into_iter()
        .flat_map(|(at, i)| isa.memory_accesses(&cx, &i, at).collect::<Vec<_>>())
        .map(|m| m.base)
        .collect()
}

/// The text of each instruction.
fn listing<I: Format>(isa: &I, code: &[u8], address: u64, names: &dyn Resolver) -> Vec<String> {
    let cx = isa.context(code, address);
    sweep(isa, code, address)
        .into_iter()
        .map(|(at, i)| {
            let mut s = String::new();
            isa.format(&cx, &i, at, I::Syntax::default(), names, &mut s);
            s
        })
        .collect()
}

const CODE: &[u8] = &[
    0x01, 0x12, // add r1, r2
    0x03, 0x03, // bz +3
    0x05, 0x31, 0xfc, // ld h3, [r1-4]
    0x00, // nop
    0x02, 0xfa, // jmp -6
    0x04, // ret
];

#[test]
fn tools_read_the_toy_through_the_traits() {
    let i = sweep(&Toy, CODE, 0x100);
    assert_eq!(i.len(), 6);
    assert_eq!(Toy::opcode(&i[2].1), Op::Load);
    assert_eq!(Toy::bytes(&i[2].1), &[0x05, 0x31, 0xfc]);
    assert_eq!(leaders(&Toy, CODE, 0x100), [0x100, 0x104, 0x107, 0x10a]);
    // r1 (written by the add, read by the load) and the flags (by bz).
    let live = live_writes(&Toy, CODE, 0x100);
    assert!(live.contains(Reg::R(1)) && live.contains(Reg::Flags));
    assert!(!live.contains(Reg::R(2)));
    assert_eq!(live.len(), 2);
    assert_eq!(bases(&Toy, CODE, 0x100), [Base::Register(Reg::R(1))]);
}

#[test]
fn text_names_what_the_resolver_names() {
    struct Names;
    impl Resolver for Names {
        fn resolve(&self, r: Reference, out: &mut dyn TokenSink) -> bool {
            if r == Reference::Address(0x104) {
                out.token(TokenKind::Name, "loop");
                return true;
            }
            false
        }
    }
    let plain = listing(&Toy, CODE, 0x100, &Unresolved);
    assert_eq!(
        plain,
        [
            "add r1, r2",
            "bz 0x107",
            "ld h3, [r1-4]",
            "nop",
            "jmp 0x104",
            "ret"
        ]
    );
    assert_eq!(listing(&Toy, CODE, 0x100, &Names)[4], "jmp loop");
}

#[test]
fn errors_say_why() {
    let cx = ();
    assert_eq!(
        Toy.decode(&cx, &[], 0),
        Err(DecodeError::Truncated { needed: 1 })
    );
    assert_eq!(
        Toy.decode(&cx, &[5, 0], 0),
        Err(DecodeError::Truncated { needed: 3 })
    );
    assert_eq!(Toy.decode(&cx, &[9], 0), Err(DecodeError::Undefined));
}

#[test]
fn flow_falls_through_unless_it_cannot() {
    let jump = ControlFlow::new(FlowKind::Jump, Target::Direct(4));
    assert!(!jump.falls_through() && jump.ends_block());
    assert!(jump.when(true).falls_through() && jump.when(true).ends_block());
    let call = ControlFlow::new(FlowKind::Call, Target::Indirect);
    assert!(call.falls_through() && !call.ends_block());
    assert!(ControlFlow::SYSTEM_CALL.falls_through());
    assert!(!ControlFlow::TRAP.falls_through() && ControlFlow::TRAP.ends_block());
    assert!(ControlFlow::RETURN.when(true).falls_through());
    assert_eq!(ControlFlow::NEXT.direct_target(), None);
}

#[test]
fn access_kinds() {
    assert!(Access::READ_WRITE.reads() && Access::READ_WRITE.writes());
    assert!(Access::MAY_WRITE.may_write() && !Access::MAY_WRITE.writes());
    assert!(!Access::NONE.may_read() && !Access::NONE.may_write());
    assert_eq!(
        Access::READ.union(Access::MAY_WRITE),
        Access::READ_MAY_WRITE
    );
}

#[test]
fn bit_sets_by_storage() {
    let mut s = BitSet::<Reg, 2>::default();
    s.insert_storage(0);
    s.insert_storage(63);
    s.insert_storage(64);
    s.insert_storage(127);
    assert_eq!(s.storages().collect::<Vec<_>>(), [0, 63, 64, 127]);
    assert!(s.contains_storage(64) && !s.contains_storage(65) && !s.contains_storage(500));
    s.remove_storage(63);
    s.remove_storage(500);
    assert_eq!(s.len(), 3);
    // A register is in a set by its storage: h1 is r1's.
    let mut t = BitSet::<Reg, 1>::default();
    t.insert(Reg::R(1));
    assert!(t.contains(Reg::H(1)));
    t.insert(Reg::Zero);
    assert_eq!(t.len(), 1);
    assert!(!t.contains(Reg::Zero));
    let mut u = BitSet::<Reg, 1>::default();
    u.insert(Reg::Flags);
    u.insert(Reg::H(1));
    assert_eq!(t.union(&u).storages().collect::<Vec<_>>(), [1, 8]);
    assert_eq!(u.difference(&t).storages().collect::<Vec<_>>(), [8]);
    assert!(t.difference(&u).is_empty());
    assert_eq!(
        RegisterSet::registers(&t.union(&u)).collect::<Vec<_>>(),
        [Reg::R(1), Reg::Flags]
    );
}

#[test]
fn bits_overlap() {
    assert!(Bits::low(32).overlaps(Bits::low(64)));
    assert!(
        !Bits {
            offset: 8,
            width: 8
        }
        .overlaps(Bits::low(8))
    );
}

#[test]
fn memory_addresses() {
    let m = MemoryAccess::<Reg>::new(Access::READ, Base::Pc, 0x1000, Some(8));
    assert_eq!(m.address(), Some(0x1000));
    let m = MemoryAccess::new(Access::READ, Base::Register(Reg::R(1)), 8, Some(8));
    assert_eq!(m.address(), None);
}
