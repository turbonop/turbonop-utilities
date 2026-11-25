//! Core files written here, in each architecture's layouts.

use alloc::vec;
use alloc::vec::Vec;

use super::*;
use crate::ElfFile;
use crate::consts::ET_CORE;

/// A note's bytes (name and description padded to 4).
fn note(name: &[u8], kind: u32, desc: &[u8]) -> Vec<u8> {
    let mut b = Vec::new();
    b.extend(u32::try_from(name.len() + 1).unwrap().to_le_bytes());
    b.extend(u32::try_from(desc.len()).unwrap().to_le_bytes());
    b.extend(kind.to_le_bytes());
    b.extend(name);
    b.push(0);
    while !b.len().is_multiple_of(4) {
        b.push(0);
    }
    b.extend(desc);
    while !b.len().is_multiple_of(4) {
        b.push(0);
    }
    b
}

/// A little-endian core file: its notes, and one loaded segment.
fn core(wide: bool, machine: u16, notes: &[u8], load: (u64, &[u8])) -> Vec<u8> {
    let (eh, ph) = if wide { (64, 56) } else { (52, 32) };
    let notes_at = eh + 2 * ph;
    let load_at = notes_at + notes.len();
    let mut b = vec![0u8; eh];
    b[..4].copy_from_slice(b"\x7fELF");
    b[4] = if wide { 2 } else { 1 };
    b[5] = 1;
    b[6] = 1;
    b[16..18].copy_from_slice(&ET_CORE.to_le_bytes());
    b[18..20].copy_from_slice(&machine.to_le_bytes());
    b[20..24].copy_from_slice(&1u32.to_le_bytes());
    let w = |b: &mut Vec<u8>, at: usize, v: u64| {
        if wide {
            b[at..at + 8].copy_from_slice(&v.to_le_bytes());
        } else {
            b[at..at + 4].copy_from_slice(&u32::try_from(v).unwrap().to_le_bytes());
        }
    };
    if wide {
        w(&mut b, 32, eh as u64); // e_phoff
        b[52..54].copy_from_slice(&u16::try_from(eh).unwrap().to_le_bytes());
        b[54..56].copy_from_slice(&u16::try_from(ph).unwrap().to_le_bytes());
        b[56..58].copy_from_slice(&2u16.to_le_bytes());
    } else {
        w(&mut b, 28, eh as u64);
        b[40..42].copy_from_slice(&u16::try_from(eh).unwrap().to_le_bytes());
        b[42..44].copy_from_slice(&u16::try_from(ph).unwrap().to_le_bytes());
        b[44..46].copy_from_slice(&2u16.to_le_bytes());
    }
    let phdr = |b: &mut Vec<u8>, kind: u32, offset: usize, vaddr: u64, size: usize| {
        let mut p = vec![0u8; ph];
        p[..4].copy_from_slice(&kind.to_le_bytes());
        let put = |p: &mut Vec<u8>, at: usize, v: u64| {
            if wide {
                p[at..at + 8].copy_from_slice(&v.to_le_bytes());
            } else {
                p[at..at + 4].copy_from_slice(&u32::try_from(v).unwrap().to_le_bytes());
            }
        };
        if wide {
            put(&mut p, 8, offset as u64);
            put(&mut p, 16, vaddr);
            put(&mut p, 32, size as u64);
            put(&mut p, 40, size as u64);
        } else {
            put(&mut p, 4, offset as u64);
            put(&mut p, 8, vaddr);
            put(&mut p, 16, size as u64);
            put(&mut p, 20, size as u64);
        }
        b.extend(p);
    };
    phdr(&mut b, PT_NOTE, notes_at, 0, notes.len());
    phdr(&mut b, PT_LOAD, load_at, load.0, load.1.len());
    b.extend(notes);
    b.extend(load.1);
    b
}

/// An `elf_prstatus` with its registers `regs` (each `reg` bytes).
fn prstatus(wide: bool, pid: i32, regs: &[u64], reg: usize) -> Vec<u8> {
    let (ids, at) = if wide { (32, 112) } else { (24, 72) };
    let mut b = vec![0u8; at + regs.len() * reg + 8];
    b[0..4].copy_from_slice(&11i32.to_le_bytes()); // si_signo
    b[4..8].copy_from_slice(&1i32.to_le_bytes()); // si_code
    b[12..14].copy_from_slice(&11i16.to_le_bytes()); // pr_cursig
    b[ids..ids + 4].copy_from_slice(&pid.to_le_bytes());
    b[ids + 4..ids + 8].copy_from_slice(&1i32.to_le_bytes());
    for (i, &r) in regs.iter().enumerate() {
        let o = at + i * reg;
        if reg == 8 {
            b[o..o + 8].copy_from_slice(&r.to_le_bytes());
        } else {
            b[o..o + 4].copy_from_slice(&u32::try_from(r).unwrap().to_le_bytes());
        }
    }
    let fp = at + regs.len() * reg;
    b[fp..fp + 4].copy_from_slice(&1i32.to_le_bytes());
    b
}

fn words(wide: bool, values: &[u64]) -> Vec<u8> {
    values
        .iter()
        .flat_map(|&v| {
            if wide {
                v.to_le_bytes().to_vec()
            } else {
                u32::try_from(v).unwrap().to_le_bytes().to_vec()
            }
        })
        .collect()
}

#[test]
fn aarch64() {
    let regs: Vec<u64> = (0..34).map(|i| 0x1000 + i).collect();
    let mut notes = note(b"CORE", NT_PRSTATUS, &prstatus(true, 77, &regs, 8));
    let mut fp = vec![0u8; 528];
    fp[16..32].copy_from_slice(&7u128.to_le_bytes()); // V1
    fp[512..516].copy_from_slice(&0x10u32.to_le_bytes()); // FPSR
    notes.extend(note(b"CORE", NT_PRFPREG, &fp));
    let mut pac = 0x007F_0000_0000_0000u64.to_le_bytes().to_vec();
    pac.extend(0x007F_8000_0000_0000u64.to_le_bytes());
    notes.extend(note(b"LINUX", NT_ARM_PAC_MASK, &pac));
    // A second thread.
    notes.extend(note(b"CORE", NT_PRSTATUS, &prstatus(true, 78, &regs, 8)));
    // NT_FILE: two entries, page size 0x1000.
    let mut file = words(true, &[2, 0x1000, 0x4000, 0x5000, 0, 0x5000, 0x6000, 3]);
    file.extend(b"/bin/a\0/lib/b.so\0");
    notes.extend(note(b"CORE", NT_FILE, &file));
    notes.extend(note(
        b"CORE",
        NT_AUXV,
        &words(true, &[6, 0x1000, 25, 0x7000, 0, 0]),
    ));
    let mut si = vec![0u8; 128];
    si[0..4].copy_from_slice(&11i32.to_le_bytes());
    si[8..12].copy_from_slice(&2i32.to_le_bytes());
    si[16..24].copy_from_slice(&0xBADu64.to_le_bytes());
    notes.extend(note(b"CORE", NT_SIGINFO, &si));
    let mut ps = vec![0u8; 136];
    ps[1] = b'R';
    ps[16..20].copy_from_slice(&1000u32.to_le_bytes());
    ps[24..28].copy_from_slice(&77i32.to_le_bytes());
    ps[40..45].copy_from_slice(b"crash");
    ps[56..64].copy_from_slice(b"./crash ");
    notes.extend(note(b"CORE", NT_PRPSINFO, &ps));
    let data = core(true, EM_AARCH64, &notes, (0x4000, &[0xAB; 16]));
    let c = CoreFile::new(ElfFile::parse(&data).unwrap()).unwrap();
    let threads = c.threads();
    assert_eq!(threads.len(), 2);
    let t = &threads[0];
    assert_eq!(
        (t.status.signo, t.status.code, t.status.cursig),
        (11, 1, 11)
    );
    assert_eq!((t.status.pid, t.status.ppid), (77, 1));
    assert!(t.status.fpvalid);
    let GeneralRegisters::Arm64(g) = t.status.registers else {
        panic!("{:?}", t.status.registers)
    };
    assert_eq!(
        (g[0], g[30], g[31], g[32], g[33]),
        (0x1000, 0x101E, 0x101F, 0x1020, 0x1021)
    );
    // The FP note, and (named LINUX) the PAC masks, are the first thread's.
    assert_eq!(t.notes.len(), 2);
    let (v, fpsr, _) = arm64_fp_registers(t.note(NT_PRFPREG).unwrap(), Endian::Little).unwrap();
    assert_eq!((v[1], fpsr), (7, 0x10));
    assert_eq!(threads[1].status.pid, 78);
    assert!(threads[1].notes.is_empty());
    assert_eq!(
        c.pac_masks(),
        Some((0x007F_0000_0000_0000, 0x007F_8000_0000_0000))
    );
    let files = c.files();
    assert_eq!(files.len(), 2);
    assert_eq!(
        (files[0].start, files[0].end, files[0].offset),
        (0x4000, 0x5000, 0)
    );
    assert_eq!(files[0].path, b"/bin/a");
    assert_eq!(files[1].offset, 0x3000);
    assert_eq!(files[1].path, b"/lib/b.so");
    assert_eq!(c.auxv(), [(6, 0x1000), (25, 0x7000)]);
    assert_eq!(
        c.siginfo(),
        Some(SigInfo {
            signo: 11,
            errno: 0,
            code: 2,
            addr: Some(0xBAD)
        })
    );
    let p = c.process_info().unwrap();
    assert_eq!((p.sname, p.uid, p.pid), (b'R', 1000, 77));
    assert_eq!(p.fname, b"crash");
    assert_eq!(p.psargs, b"./crash ");
    assert_eq!(c.memory(), [(0x4000, &[0xABu8; 16][..])]);
}

#[test]
fn i386_and_arm() {
    for (machine, n) in [(EM_386, 17usize), (EM_ARM, 18)] {
        let regs: Vec<u64> = (0..n as u64).map(|i| 0x100 + i).collect();
        let mut notes = note(b"CORE", NT_PRSTATUS, &prstatus(false, 5, &regs, 4));
        let mut si = vec![0u8; 128];
        si[0..4].copy_from_slice(&7i32.to_le_bytes()); // SIGBUS
        si[12..16].copy_from_slice(&0x1234u32.to_le_bytes());
        notes.extend(note(b"CORE", NT_SIGINFO, &si));
        let mut ps = vec![0u8; 124];
        ps[8..10].copy_from_slice(&500u16.to_le_bytes());
        ps[12..16].copy_from_slice(&5i32.to_le_bytes());
        ps[28..31].copy_from_slice(b"app");
        notes.extend(note(b"CORE", NT_PRPSINFO, &ps));
        let mut file = words(false, &[1, 0x1000, 0x8000, 0x9000, 2]);
        file.extend(b"/x\0");
        notes.extend(note(b"CORE", NT_FILE, &file));
        if machine == EM_ARM {
            let mut vfp = vec![0u8; 260];
            vfp[8..16].copy_from_slice(&9u64.to_le_bytes()); // D1
            vfp[256..260].copy_from_slice(&0x3u32.to_le_bytes());
            notes.extend(note(b"LINUX", NT_ARM_VFP, &vfp));
        }
        let data = core(false, machine, &notes, (0x8000, &[1, 2, 3]));
        let c = CoreFile::new(ElfFile::parse(&data).unwrap()).unwrap();
        let threads = c.threads();
        assert_eq!(threads.len(), 1, "{machine}");
        let t = &threads[0];
        assert_eq!(t.status.pid, 5);
        match t.status.registers {
            GeneralRegisters::X86(g) => {
                assert_eq!(machine, EM_386);
                assert_eq!(
                    (g[x86::EBX], g[x86::EIP], g[x86::SS]),
                    (0x100, 0x10C, 0x110)
                );
            }
            GeneralRegisters::Arm(g) => {
                assert_eq!(machine, EM_ARM);
                assert_eq!((g[0], g[15], g[16]), (0x100, 0x10F, 0x110));
                let (d, fpscr) =
                    arm_vfp_registers(t.note(NT_ARM_VFP).unwrap(), Endian::Little).unwrap();
                assert_eq!((d[1], fpscr), (9, 3));
            }
            other => panic!("{other:?}"),
        }
        assert_eq!(c.siginfo().unwrap().addr, Some(0x1234));
        let p = c.process_info().unwrap();
        assert_eq!((p.uid, p.pid), (500, 5));
        assert_eq!(p.fname, b"app");
        assert_eq!(c.files()[0].offset, 0x2000);
        assert_eq!(c.memory()[0].0, 0x8000);
    }
}

#[test]
fn not_a_core_file() {
    let data = core(true, EM_X86_64, &[], (0, &[]));
    let mut exec = data.clone();
    exec[16] = 2; // ET_EXEC
    assert!(CoreFile::new(ElfFile::parse(&exec).unwrap()).is_none());
    let c = CoreFile::new(ElfFile::parse(&data).unwrap()).unwrap();
    assert!(c.threads().is_empty());
    assert!(c.files().is_empty());
    assert!(c.auxv().is_empty());
    assert_eq!(c.siginfo(), None);
    assert_eq!(c.process_info(), None);
    assert_eq!(c.pac_masks(), None);
    // (An empty load segment is no memory.)
    assert!(c.memory().is_empty());
}

#[test]
fn signals_without_addresses() {
    let mut si = vec![0u8; 128];
    si[0..4].copy_from_slice(&6i32.to_le_bytes()); // SIGABRT
    si[16..24].copy_from_slice(&0x55u64.to_le_bytes());
    let data = core(true, EM_X86_64, &note(b"CORE", NT_SIGINFO, &si), (0, &[]));
    let c = CoreFile::new(ElfFile::parse(&data).unwrap()).unwrap();
    assert_eq!(c.siginfo().unwrap().addr, None);
}

#[test]
fn x64_fp() {
    let mut d = vec![0u8; 512];
    d[24..28].copy_from_slice(&0x1F80u32.to_le_bytes());
    d[160 + 16 * 15..176 + 16 * 15].copy_from_slice(&5u128.to_le_bytes());
    let (xmm, mxcsr) = x64_fp_registers(&d).unwrap();
    assert_eq!((xmm[15], mxcsr), (5, 0x1F80));
    assert!(x64_fp_registers(&d[..100]).is_none());
}
