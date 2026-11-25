//! Targets made here: signal frames, and what's in no module.

use turbonop_abi::{Os, regs};
use turbonop_core::Arch;
use turbonop_frames::{Confidence, End, Method, Modules, RegState, Source, walk};
use turbonop_target::load::Loaded;
use turbonop_target::{
    AddressSpace, Capabilities, Exception, Memory, Module, Provenance, Registers, Snapshot, Target,
    Thread,
};

struct Captured(Vec<(u64, Vec<u8>)>);

impl Memory for Captured {
    fn read_at(&self, address: u64, buf: &mut [u8]) -> usize {
        for (start, bytes) in &self.0 {
            if address >= *start && address - start < bytes.len() as u64 {
                let from = usize::try_from(address - start).unwrap();
                let n = (bytes.len() - from).min(buf.len());
                buf[..n].copy_from_slice(&bytes[from..from + n]);
                return n;
            }
        }
        0
    }

    fn next_available(&self, address: u64) -> Option<u64> {
        self.0.iter().map(|r| r.0).filter(|&s| s > address).min()
    }

    fn provenance(&self) -> Provenance {
        Provenance::Dump
    }
}

struct Fake {
    arch: Arch,
    threads: Vec<Thread>,
    memory: Captured,
}

impl Target for Fake {
    fn arch(&self) -> Arch {
        self.arch
    }
    fn os(&self) -> Option<Os> {
        Some(Os::Linux)
    }
    fn snapshot(&self) -> Snapshot {
        Snapshot { generation: 0 }
    }
    fn capabilities(&self) -> Capabilities {
        Capabilities::DUMP
    }
    fn threads(&self) -> &[Thread] {
        &self.threads
    }
    fn modules(&self) -> &[Module] {
        &[]
    }
    fn exception(&self) -> Option<&Exception> {
        None
    }
    fn memory(&self) -> &dyn Memory {
        &self.memory
    }
}

fn run(t: &Fake) -> turbonop_frames::Stack {
    let loaded = Loaded {
        modules: Vec::new(),
    };
    let space = AddressSpace::new(t.memory(), Vec::new());
    let modules = Modules::new(&loaded, &space, &[]);
    walk(t, &space, &modules, &t.threads[0])
}

#[test]
fn x64_signal_frame() {
    let tramp = 0x7000_0000u64;
    let sp = 0x10_0000u64;
    let mut code = vec![0x48, 0xC7, 0xC0, 0x0F, 0x00, 0x00, 0x00, 0x0F, 0x05];
    code.resize(16, 0xCC);
    // The ucontext at SP: its registers 40 bytes in (R8..R15, RDI, RSI,
    // RBP, RBX, RDX, RAX, RCX, RSP, RIP, EFLAGS), each 0x100 + its index.
    let mut stack = vec![0u8; 40 + 18 * 8];
    for i in 0..18u64 {
        let at = usize::try_from(40 + 8 * i).unwrap();
        stack[at..at + 8].copy_from_slice(&(0x100 + i).to_le_bytes());
    }
    let mut r = Registers::new();
    r.set(regs::x64::RIP, Some(u128::from(tramp)));
    r.set(regs::x64::RSP, Some(u128::from(sp)));
    let t = Fake {
        arch: Arch::X64,
        threads: vec![Thread {
            id: 1,
            name: None,
            registers: r,
            stack: None,
            thread_pointer: None,
        }],
        memory: Captured(vec![(tramp, code), (sp, stack)]),
    };
    let s = run(&t);
    assert_eq!(s.frames.len(), 2, "{s:#x?}");
    let f = &s.frames[1];
    assert_eq!(f.method, Method::SignalFrame);
    assert_eq!(f.confidence, Confidence::Certain);
    // The interrupted code's RIP and RSP (indexes 16 and 15).
    assert_eq!((f.pc, f.sp), (0x110, 0x10F));
    assert!(!f.return_address);
    assert_eq!(f.registers.value(regs::x64::R8), Some(0x100));
    assert_eq!(f.registers.value(regs::x64::RAX), Some(0x10D));
    assert_eq!(
        f.registers.get(regs::x64::RBX),
        Some(RegState::known(
            0x10B,
            Source::SignalContext(sp + 40 + 11 * 8)
        ))
    );
    // Its code is in nothing: no caller found.
    assert_eq!(s.end, End::NoCaller);
}

#[test]
fn arm64_signal_frame() {
    let tramp = 0x7F00_0000u64;
    let sp = 0x20_0000u64;
    let code = vec![0x68, 0x11, 0x80, 0xD2, 0x01, 0x00, 0x00, 0xD4];
    // siginfo (128), then the ucontext's sigcontext 176 in: fault address,
    // X0..X30, SP, PC.
    let base = 128 + 176 + 8;
    let mut stack = vec![0u8; base + 33 * 8];
    for i in 0..33u64 {
        let at = base + usize::try_from(8 * i).unwrap();
        stack[at..at + 8].copy_from_slice(&(0x1000 + i).to_le_bytes());
    }
    let mut r = Registers::new();
    r.set(regs::arm64::PC, Some(u128::from(tramp)));
    r.set(regs::arm64::SP, Some(u128::from(sp)));
    let t = Fake {
        arch: Arch::Arm64,
        threads: vec![Thread {
            id: 1,
            name: None,
            registers: r,
            stack: None,
            thread_pointer: None,
        }],
        memory: Captured(vec![(tramp, code), (sp, stack)]),
    };
    let s = run(&t);
    let f = &s.frames[1];
    assert_eq!(f.method, Method::SignalFrame);
    assert_eq!((f.pc, f.sp), (0x1020, 0x101F));
    assert_eq!(f.registers.value(regs::arm64::LR), Some(0x101E));
    assert_eq!(f.registers.value(0), Some(0x1000));
}

#[test]
fn no_registers() {
    let t = Fake {
        arch: Arch::X64,
        threads: vec![Thread {
            id: 1,
            name: None,
            registers: Registers::new(),
            stack: None,
            thread_pointer: None,
        }],
        memory: Captured(Vec::new()),
    };
    let s = run(&t);
    assert!(s.frames.is_empty());
    assert_eq!(s.end, End::NoRegisters);
}
