//! The integration tests, as one binary (one link, not one per file):
//! functions assembled here (whose ways are known), and the Windows dump's
//! frames.

use std::path::{Path, PathBuf};

use turbonop_abi::{Abi, Os, regs::x64::*};
use turbonop_core::Arch;
use turbonop_frames::{FrameRegisters, Modules, RegState, Source, walk_all};
use turbonop_paths::x64::{Cond, FlagSetter, decide};
use turbonop_paths::{Paths, Ran, Way, analyze};
use turbonop_program::server::Servers;
use turbonop_target::load::{Hints, load};
use turbonop_target::{AddressSpace, Memory, MinidumpTarget, Provenance, Target};

struct Code(u64, Vec<u8>);

impl Memory for Code {
    fn read_at(&self, address: u64, buf: &mut [u8]) -> usize {
        let Some(off) = address.checked_sub(self.0) else {
            return 0;
        };
        let Some(d) = usize::try_from(off).ok().and_then(|o| self.1.get(o..)) else {
            return 0;
        };
        let n = d.len().min(buf.len());
        buf[..n].copy_from_slice(&d[..n]);
        n
    }
    fn next_available(&self, address: u64) -> Option<u64> {
        (address < self.0).then_some(self.0)
    }
    fn provenance(&self) -> Provenance {
        Provenance::Dump
    }
}

/// `cmp edi, 5; jle L; <middle>; mov eax, 1; jmp M; L: mov eax, 2; M: mov
/// [0], eax (the crash); ret` at 0x1000, with `middle` (bytes) on the way
/// not taken.
fn function(middle: &[u8]) -> (Vec<u8>, u64) {
    let mut c = vec![0x83, 0xFF, 0x05]; // cmp edi, 5
    let skip = u8::try_from(middle.len() + 7).unwrap();
    c.extend([0x7E, skip]); // jle L
    c.extend(middle);
    c.extend([0xB8, 1, 0, 0, 0, 0xEB, 0x05]); // mov eax, 1; jmp M
    c.extend([0xB8, 2, 0, 0, 0]); // L: mov eax, 2
    let crash = 0x1000 + c.len() as u64;
    c.extend([0x89, 0x04, 0x25, 0, 0, 0, 0, 0xC3]); // M: mov [0], eax; ret
    (c, crash)
}

fn registers(edi: Option<u64>) -> FrameRegisters {
    let mut r = FrameRegisters::new();
    if let Some(v) = edi {
        r.set(RDI, Some(RegState::known(u128::from(v), Source::Context)));
    }
    r.set(RAX, Some(RegState::known(2, Source::Context)));
    r
}

fn run(middle: &[u8], edi: Option<u64>) -> turbonop_paths::Report {
    let (code, crash) = function(middle);
    let len = code.len() as u64;
    let memory = Code(0x1000, code);
    let space = AddressSpace::new(&memory, Vec::new());
    let abi = Abi::new(Arch::X64, Os::Linux).unwrap();
    analyze(&space, Arch::X64, abi, 0x1000, len, crash, &registers(edi)).unwrap()
}

#[test]
fn a_value_that_survives_decides() {
    // edi = 7 at the crash, and nothing writes it: 7 > 5, jle not taken.
    let r = run(&[], Some(7));
    assert_eq!(r.branches.len(), 1);
    let b = &r.branches[0];
    assert_eq!(b.address, 0x1003);
    assert_eq!(b.way, Way::NotTaken, "{}", b.why);
    assert!(b.why.contains("0x7"), "{}", b.why);
    assert!(!b.in_loop);
    assert_eq!(r.paths, Paths::One);
    let ran: Vec<(u64, Ran)> = r.blocks.iter().map(|b| (b.start, b.ran)).collect();
    assert_eq!(
        ran,
        [
            (0x1000, Ran::Yes),
            (0x1005, Ran::Yes),
            (0x100c, Ran::No),
            (0x1011, Ran::Yes)
        ]
    );
    // edi = 3 (and as signed, -1): taken.
    assert_eq!(run(&[], Some(3)).branches[0].way, Way::Taken);
    assert_eq!(run(&[], Some(0xFFFF_FFFF)).branches[0].way, Way::Taken);
}

#[test]
fn what_isnt_known_isnt_claimed() {
    // edi unknown: neither way, and two ways left.
    let r = run(&[], None);
    assert_eq!(r.branches[0].way, Way::Unknown);
    assert!(
        r.branches[0].why.contains("isn't known"),
        "{}",
        r.branches[0].why
    );
    assert_eq!(r.paths, Paths::OneOf(2));
    assert!(r.blocks.iter().filter(|b| b.ran == Ran::Maybe).count() == 2);
    // edi written on one way (mov edi, 1): its value at the crash says
    // nothing of its value at the cmp.
    let r = run(&[0xBF, 1, 0, 0, 0], Some(1));
    assert_eq!(r.branches[0].way, Way::Unknown);
    assert!(
        r.branches[0].why.contains("may be written"),
        "{}",
        r.branches[0].why
    );
    // An instruction not modeled on the way (cpuid): anything may be
    // written.
    let r = run(&[0x0F, 0xA2], Some(7));
    assert_eq!(r.branches[0].way, Way::Unknown);
    // A call on the way: it clobbers rdi (volatile), and rbx is preserved.
    let r = run(&[0xE8, 0, 0, 0, 0], Some(7));
    assert_eq!(r.branches[0].way, Way::Unknown);
}

#[test]
fn control_flow_alone_decides() {
    // cmp edi, 5; jle L; ret; L: mov [0], eax: only the target reaches the
    // crash, whatever edi is.
    let code = vec![
        0x83, 0xFF, 0x05, 0x7E, 0x01, 0xC3, 0x89, 0x04, 0x25, 0, 0, 0, 0, 0xC3,
    ];
    let memory = Code(0x1000, code.clone());
    let space = AddressSpace::new(&memory, Vec::new());
    let abi = Abi::new(Arch::X64, Os::Windows).unwrap();
    let r = analyze(
        &space,
        Arch::X64,
        abi,
        0x1000,
        code.len() as u64,
        0x1006,
        &registers(None),
    )
    .unwrap();
    assert_eq!(r.branches[0].way, Way::Taken);
    assert!(r.branches[0].why.contains("only its target"));
    assert_eq!(r.paths, Paths::One);
    // Not in the code: not analyzable.
    assert!(
        analyze(
            &space,
            Arch::X64,
            abi,
            0x1000,
            code.len() as u64,
            0x9000,
            &registers(None)
        )
        .is_err()
    );
    assert!(
        analyze(
            &space,
            Arch::Arm64,
            abi,
            0x1000,
            4,
            0x1000,
            &registers(None)
        )
        .is_err()
    );
}

#[test]
fn conditions() {
    let c = |cond, a, b| decide(cond, FlagSetter::Compare, a, b, 4);
    assert_eq!(c(Cond::E, 5, 5), Some(true));
    assert_eq!(c(Cond::Ne, 5, 5), Some(false));
    assert_eq!(c(Cond::L, 0xFFFF_FFFF, 0), Some(true)); // -1 < 0
    assert_eq!(c(Cond::B, 0xFFFF_FFFF, 0), Some(false)); // unsigned
    assert_eq!(c(Cond::A, 7, 5), Some(true));
    assert_eq!(c(Cond::Be, 5, 5), Some(true));
    assert_eq!(c(Cond::G, 0x7FFF_FFFF, 0xFFFF_FFFF), Some(true)); // max > -1
    assert_eq!(c(Cond::O, 0x8000_0000, 1), Some(true)); // min - 1 overflows
    assert_eq!(c(Cond::S, 0, 1), Some(true));
    assert_eq!(
        decide(Cond::E, FlagSetter::Test, 0b1010, 0b0101, 1),
        Some(true)
    );
    assert_eq!(
        decide(Cond::Ne, FlagSetter::Test, 0b1010, 0b0010, 1),
        Some(true)
    );
    assert_eq!(decide(Cond::B, FlagSetter::Test, 1, 1, 1), Some(false));
    // A result alone decides ZF and SF conditions, not CF or OF's.
    assert_eq!(decide(Cond::E, FlagSetter::Result, 0, 0, 8), Some(true));
    assert_eq!(decide(Cond::B, FlagSetter::Result, 0, 0, 8), None);
    assert_eq!(decide(Cond::P, FlagSetter::Compare, 3, 0, 1), Some(true)); // 0b11: even
}

fn testdata(path: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../../testdata")
        .join(path)
}

#[test]
fn windows_middle() {
    // middle: if (n->value == 42) return leaf(...) + ...; the frame's PC
    // is the call to leaf, so its branch went the way that calls leaf.
    let data = std::fs::read(testdata("minidump/crash-x64.dmp")).unwrap();
    let t = MinidumpTarget::new(&data).unwrap();
    let hints = Hints {
        dirs: vec![testdata("minidump")],
        skip_recorded_paths: true,
        symbols: Some((Vec::new(), Servers::default())),
        ..Hints::default()
    };
    let loaded = load(&t, &hints);
    let space = loaded.address_space(&t);
    let bases: Vec<u64> = t.modules().iter().map(|m| m.base).collect();
    let modules = Modules::new(&loaded, &space, &bases);
    let stacks = walk_all(&t, &space, &modules);
    let e = t.exception().unwrap();
    let s = stacks.iter().find(|s| s.thread == e.thread).unwrap();
    let f = &s.frames[1];
    let sym = f.symbol.as_ref().unwrap();
    assert_eq!(sym.function, "middle");
    let start = f.lookup() - sym.offset;
    let abi = t.abi().unwrap();
    let r = analyze(
        &space,
        Arch::X64,
        abi,
        start,
        0x4c,
        f.lookup(),
        &f.registers,
    )
    .unwrap();
    assert!(!r.branches.is_empty(), "{r:#?}");
    assert!(r.branches.iter().all(|b| b.way != Way::Unknown), "{r:#?}");
    assert_eq!(r.paths, Paths::One);
}
