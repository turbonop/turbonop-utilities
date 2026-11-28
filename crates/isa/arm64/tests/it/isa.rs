//! ARM64 through `turbonop_isa`'s traits: control flow, attributes,
//! registers, memory, operands, and text, of known words; and over every
//! encoding, the register accesses agree with the read and write sets.

use turbonop_arm64::{Arm64, Instruction, Register, opcodes};
use turbonop_isa::{
    Access, Attributes, Base, ControlFlow, DecodeError, FlowKind, Format, Isa, Memory,
    MemoryAccess, Offset, Operand, Operands, OtherBits, Reference, Registers, Resolver, Target,
    TokenKind, TokenSink, Unresolved, Update,
};

const AT: u64 = 0x1000;

fn insn(word: u32) -> Instruction {
    Arm64
        .decode(&(), &word.to_le_bytes(), AT)
        .unwrap_or_else(|e| panic!("{word:08x}: {e}"))
}

fn flow(word: u32) -> ControlFlow {
    Arm64.flow(&(), &insn(word), AT)
}

#[test]
fn control_flow() {
    let jump = |t| ControlFlow::new(FlowKind::Jump, Target::Direct(t));
    let call = |t| ControlFlow::new(FlowKind::Call, t);
    assert_eq!(flow(0x1400_0002), jump(AT + 8)); // b
    assert_eq!(flow(0x9400_0004), call(Target::Direct(AT + 16))); // bl
    assert_eq!(flow(0x5400_0040), jump(AT + 8).when(true)); // b.eq
    assert_eq!(flow(0xb400_0040), jump(AT + 8).when(true)); // cbz x0
    assert_eq!(flow(0x3600_0040), jump(AT + 8).when(true)); // tbz w0, #0
    assert_eq!(flow(0xd65f_03c0), ControlFlow::RETURN); // ret
    assert_eq!(
        flow(0xd61f_0200),
        ControlFlow::new(FlowKind::Jump, Target::Indirect)
    ); // br x16
    assert_eq!(flow(0xd63f_0100), call(Target::Indirect)); // blr x8
    assert_eq!(flow(0xd400_0001), ControlFlow::SYSTEM_CALL); // svc #0
    assert_eq!(flow(0xd420_0000), ControlFlow::TRAP); // brk #0
    assert_eq!(flow(0x0000_0000), ControlFlow::TRAP); // udf #0
    assert_eq!(flow(0xd503_201f), ControlFlow::NEXT); // nop
    assert_eq!(flow(0x9100_0420), ControlFlow::NEXT); // add x0, x1, #1
    // (adr and a literal load name addresses, but don't branch.)
    assert_eq!(flow(0x1000_0040), ControlFlow::NEXT);
    assert_eq!(flow(0x5800_0040), ControlFlow::NEXT);
}

#[test]
fn attributes() {
    let a = |w| Arm64.attributes(&insn(w));
    assert!(a(0xd503_201f).contains(Attributes::NOP));
    assert!(a(0x9100_0420).is_empty());
    // ldaxr x0, [x1]; ldadd x2, x3, [x4]: atomic, and may order.
    assert!(a(0xc85f_fc20).contains(Attributes::ATOMIC | Attributes::BARRIER));
    assert!(a(0xf822_0083).contains(Attributes::ATOMIC | Attributes::BARRIER));
    // dmb ish orders, and runs at EL0; ldar orders, and isn't atomic.
    assert!(a(0xd503_3bbf).contains(Attributes::BARRIER | Attributes::SYSTEM));
    assert!(!a(0xd503_3bbf).contains(Attributes::PRIVILEGED));
    assert!(a(0xc8df_fc20).contains(Attributes::BARRIER));
    assert!(!a(0xc8df_fc20).contains(Attributes::ATOMIC));
    // mrs x0, tpidr_el0 is EL0's; msr sctlr_el1, x0 isn't; nor are eret
    // and hvc.
    assert!(!a(0xd53b_d040).contains(Attributes::PRIVILEGED));
    assert!(a(0xd518_1000).contains(Attributes::PRIVILEGED | Attributes::SYSTEM));
    assert!(a(0xd69f_03e0).contains(Attributes::PRIVILEGED));
    assert!(a(0xd400_0002).contains(Attributes::PRIVILEGED));
    // fadd d0, d1, d2 may raise a floating-point exception.
    assert!(a(0x1e62_2820).contains(Attributes::FP_EXCEPTION));
}

fn accesses(word: u32) -> Vec<(Register, Access, OtherBits, bool)> {
    let mut v: Vec<_> = Arm64
        .register_accesses(&(), &insn(word), AT)
        .map(|a| (a.register, a.access, a.other_bits, a.implicit))
        .collect();
    v.sort_by_key(|a| format!("{a:?}"));
    v
}

#[test]
fn registers() {
    let (r, w, rw) = (Access::READ, Access::WRITE, Access::READ_WRITE);
    let (keep, zero) = (OtherBits::Preserved, OtherBits::Zeroed);
    // add x0, x1, #1
    assert_eq!(
        accesses(0x9100_0420),
        [
            (Register::X(0), w, zero, false),
            (Register::X(1), r, keep, false)
        ]
    );
    // adds w0, w1, #1: w0 (zeroing x0's top), and NZCV, implicitly.
    assert_eq!(
        accesses(0x3100_0420),
        [
            (Register::Nzcv, w, zero, true),
            (Register::W(0), w, zero, false),
            (Register::W(1), r, keep, false)
        ]
    );
    // add x0, x0, x1: x0 once, read and written.
    assert_eq!(
        accesses(0x8b01_0000),
        [
            (Register::X(0), rw, zero, false),
            (Register::X(1), r, keep, false)
        ]
    );
    // blr x30: x30 read (its operand) and written (the link), each once;
    // and SP, which LLVM's calls say they use (more than the
    // architecture says: the sets over-approximate).
    assert_eq!(
        accesses(0xd63f_03c0),
        [
            (Register::Sp, r, keep, true),
            (Register::X(30), r, keep, false),
            (Register::X(30), w, zero, true)
        ]
    );
    // b.eq reads NZCV.
    assert_eq!(accesses(0x5400_0040), [(Register::Nzcv, r, keep, true)]);
    // cmp x0, x1: the zero register it writes is no register at all.
    let (reads, writes) = Arm64.register_sets(&(), &insn(0xeb01_001f), AT);
    assert!(reads.contains(Register::X(0)) && reads.contains(Register::W(1)));
    assert!(writes.contains(Register::Nzcv) && writes.len() == 1);
}

fn memory(word: u32) -> Vec<MemoryAccess<Register>> {
    Arm64.memory_accesses(&(), &insn(word), AT).collect()
}

#[test]
fn memory_accesses() {
    let at = |base, displacement| MemoryAccess::new(Access::READ, base, displacement, None);
    // ldr x0, [x1, #8]
    assert_eq!(memory(0xf940_0420), [at(Base::Register(Register::X(1)), 8)]);
    // ldr x0, [x1], #8: at x1, which is then advanced.
    let post = MemoryAccess {
        update: Update::Post(Offset::Immediate(8)),
        ..at(Base::Register(Register::X(1)), 0)
    };
    assert_eq!(memory(0xf840_8420), [post]);
    // ldr x0, [x1, #8]!
    let pre = MemoryAccess {
        update: Update::Pre,
        ..at(Base::Register(Register::X(1)), 8)
    };
    assert_eq!(memory(0xf840_8c20), [pre]);
    // str x0, [sp, #16]
    let store = MemoryAccess {
        access: Access::WRITE,
        ..at(Base::Register(Register::Sp), 16)
    };
    assert_eq!(memory(0xf900_0be0), [store]);
    // ldr x0, <literal>: PC-relative, resolved.
    let literal = memory(0x5800_0040);
    #[allow(clippy::cast_possible_wrap)]
    let expected = at(Base::Pc, (AT + 8) as i64);
    assert_eq!(literal, [expected]);
    assert_eq!(literal[0].address(), Some(AT + 8));
    // ldr z0, [x0, #1, mul vl]: in vector lengths.
    let sve = memory(0x8580_4000);
    assert_eq!(sve.len(), 1);
    assert!(sve[0].vector_length_units && sve[0].address().is_none());
    // add and branches touch no memory.
    assert!(memory(0x9100_0420).is_empty() && memory(0x1400_0002).is_empty());
}

#[test]
fn operands() {
    let ops = |w| -> Vec<Operand<Register>> { Arm64.operands(&(), &insn(w), AT).collect() };
    // adrp x0, <page> (one page on): an address, from the instruction's
    // own.
    assert_eq!(
        ops(0xb000_0000)[1],
        Operand::Address((AT & !0xFFF) + 0x1000)
    );
    // b: a target; add: registers, then an immediate.
    assert_eq!(ops(0x1400_0002), [Operand::Target(AT + 8)]);
    let add = ops(0x9100_0420);
    assert_eq!(add[0], Operand::Register(Register::X(0), Access::WRITE));
    assert!(matches!(add[2], Operand::Immediate(i) if i.value == 1));
}

#[test]
fn text() {
    struct Names;
    impl Resolver for Names {
        fn resolve(&self, r: Reference, out: &mut dyn TokenSink) -> bool {
            if r == Reference::Address(AT + 8) {
                out.token(TokenKind::Name, "target");
                return true;
            }
            false
        }
    }
    #[derive(Default)]
    struct Kinds(Vec<(TokenKind, String)>);
    impl TokenSink for Kinds {
        fn token(&mut self, kind: TokenKind, text: &str) {
            self.0.push((kind, text.to_string()));
        }
    }
    let mut s = String::new();
    Arm64.format(
        &(),
        &insn(0x1400_0002),
        AT,
        Default::default(),
        &Names,
        &mut s,
    );
    assert_eq!(s.split_whitespace().collect::<Vec<_>>(), ["b", "target"]);
    let mut k = Kinds::default();
    Arm64.format(
        &(),
        &insn(0xd503_201f),
        AT,
        Default::default(),
        &Unresolved,
        &mut k,
    );
    assert_eq!(k.0, [(TokenKind::Mnemonic, "nop".to_string())]);
}

#[test]
fn opcodes_are_numbers_with_names() {
    let add = insn(0x9100_0420);
    assert_eq!(add.opcode(), opcodes::ADD_64_ADDSUB_IMM);
    assert_eq!(add.opcode().name(), "ADD_64_addsub_imm");
    assert_eq!(add.opcode().mnemonic(), "add");
    // An alias is its own opcode: mov x0, x1 (orr x0, xzr, x1).
    assert_eq!(insn(0xaa01_03e0).opcode(), opcodes::MOV_ORR_64_LOG_SHIFT);
    assert_eq!(insn(0x5400_0040).opcode().mnemonic(), "b");
    let n = turbonop_arm64::Opcode::all().count();
    assert_eq!(
        u32::try_from(n).unwrap(),
        <turbonop_arm64::Opcode as turbonop_isa::Opcode>::COUNT
    );
    assert!(turbonop_arm64::Opcode::all().all(|o| !o.mnemonic().is_empty()));
}

#[test]
fn decode_errors() {
    assert_eq!(
        Arm64.decode(&(), &[0x1f, 0x20], AT),
        Err(DecodeError::Truncated { needed: 4 })
    );
    // An unallocated encoding.
    assert_eq!(
        turbonop_arm64::decode(0xffff_ffff),
        Err(DecodeError::Undefined)
    );
}

/// Over every encoding (its fixed bits, and those with every other bit
/// set): the accesses name exactly the registers the read and write sets
/// have, and the traits' other answers don't panic.
#[test]
fn accesses_agree_with_the_sets_for_every_encoding() {
    let mut checked = 0;
    for (name, mask, value) in turbonop_arm64::encodings() {
        for word in [value, value | !mask] {
            let Ok(i) = turbonop_arm64::decode(word) else {
                continue;
            };
            checked += 1;
            let (reads, writes) = Arm64.register_sets(&(), &i, AT);
            let (old_r, old_w) = (i.read_registers(), i.written_registers());
            for id in 0..128u8 {
                let Some(r) = Register::from_id(id) else {
                    continue;
                };
                assert_eq!(
                    reads.contains(r),
                    old_r.contains(r),
                    "{name} {word:08x}: reads {r:?}"
                );
                assert_eq!(
                    writes.contains(r),
                    old_w.contains(r),
                    "{name} {word:08x}: writes {r:?}"
                );
            }
            let _ = (Arm64.flow(&(), &i, AT), Arm64.attributes(&i));
            let _ = Arm64.memory_accesses(&(), &i, AT).count();
            let _ = Arm64.operands(&(), &i, AT).count();
        }
    }
    assert!(checked > 8_000, "too few: {checked}");
}
