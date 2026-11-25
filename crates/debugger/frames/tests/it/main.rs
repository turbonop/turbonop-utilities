//! The integration tests, as one binary (one link, not one per file).
//!
//! Real dumps of tools/minidump/crash.c and tools/coredump/crash.c (the
//! same program, crashed on Windows and on Linux), walked and checked
//! against DbgEng's and gdb's stacks (testdata/minidump/crash-x64.cdb.txt,
//! testdata/coredump/crash-x86_64.gdb.txt). Only the programs' own files
//! are found (not the system's libraries), so frames past them come from
//! the fallbacks.

mod synthetic;

use std::path::{Path, PathBuf};

use turbonop_abi::regs::x64::*;
use turbonop_frames::{Confidence, End, Frame, Method, Modules, RegState, Source, walk_all};
use turbonop_program::server::Servers;
use turbonop_target::load::{Hints, Loaded, load};
use turbonop_target::{CoreTarget, MinidumpTarget, Target};

fn testdata(path: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../../testdata")
        .join(path)
}

/// Only `dir` is searched: not where the dump says modules were, nor the
/// environment's symbol directories or servers.
fn hermetic(dir: &str) -> Hints {
    Hints {
        dirs: vec![testdata(dir)],
        skip_recorded_paths: true,
        symbols: Some((Vec::new(), Servers::default())),
        ..Hints::default()
    }
}

fn function(f: &Frame) -> &str {
    f.symbol.as_ref().map_or("", |s| s.function.as_str())
}

fn line(f: &Frame) -> u32 {
    f.symbol.as_ref().map_or(0, |s| s.line)
}

fn stacks(target: &dyn Target, loaded: &Loaded) -> Vec<turbonop_frames::Stack> {
    let space = loaded.address_space(target);
    let bases: Vec<u64> = target.modules().iter().map(|m| m.base).collect();
    let modules = Modules::new(loaded, &space, &bases);
    walk_all(target, &space, &modules)
}

#[test]
fn windows_crash() {
    let data = std::fs::read(testdata("minidump/crash-x64.dmp")).unwrap();
    let t = MinidumpTarget::new(&data).unwrap();
    let loaded = load(&t, &hermetic("minidump"));
    let exe = t
        .modules()
        .iter()
        .position(|m| m.file_name().eq_ignore_ascii_case("crash-x64.exe"))
        .unwrap();
    assert!(loaded.modules[exe].image.is_ok());
    assert!(matches!(loaded.modules[exe].program, Some(Ok(_))), "no PDB");
    let all = stacks(&t, &loaded);
    let e = t.exception().unwrap();
    let crashed = all.iter().find(|s| s.thread == e.thread).unwrap();
    let f = &crashed.frames;
    // DbgEng's `kn` from the exception's context: leaf+0x13, middle+0x33,
    // outer+0x22, worker+0xe, then kernel32 and ntdll.
    let expect = [
        (0x7ff7_eed9_101b, 0x84_8757_f6d8, "leaf", 34),
        (0x7ff7_eed9_1053, 0x84_8757_f6e0, "middle", 41),
        (0x7ff7_eed9_108e, 0x84_8757_f750, "outer", 48),
        (0x7ff7_eed9_115e, 0x84_8757_f790, "worker", 53),
    ];
    for (i, (pc, sp, name, line_)) in expect.iter().enumerate() {
        assert_eq!(f[i].pc, *pc, "frame {i}");
        assert_eq!(f[i].sp, *sp, "frame {i}");
        assert_eq!(function(&f[i]), *name, "frame {i}");
        assert_eq!(line(&f[i]), *line_, "frame {i}");
        assert_eq!(f[i].confidence, Confidence::Certain, "frame {i}");
        assert_eq!(f[i].module, Some(exe));
    }
    assert_eq!(f[0].method, Method::Context);
    // (leaf, at /O1, is a leaf: no unwind data; middle has some.)
    assert_eq!(f[1].method, Method::Leaf);
    assert_eq!(f[2].method, Method::Pdata);
    // worker's caller: kernel32!BaseThreadInitThunk+0x17, by worker's
    // unwind data (certain), though kernel32's image isn't here.
    assert_eq!(f[4].pc, 0x7ffa_ca64_dda7);
    assert_eq!(f[4].sp, 0x84_8757_f7c0);
    assert_eq!(f[4].confidence, Confidence::Certain);
    assert!(f[4].symbol.is_none());
    // Past it, only heuristics: likely, if anything.
    assert!(f[5..].iter().all(|f| f.confidence == Confidence::Likely));
    // The registers' provenance: RSP is the CFA; RIP the return address;
    // RAX (volatile) clobbered by the call; RBX (preserved) carried from
    // the context, or read where leaf's callers saved it.
    let r = &f[1].registers;
    assert!(matches!(
        r.get(RSP),
        Some(RegState::Known {
            from: Source::Cfa,
            ..
        })
    ));
    assert!(matches!(
        r.get(RIP),
        Some(RegState::Known {
            from: Source::ReturnAddress,
            ..
        })
    ));
    assert_eq!(r.get(RAX), Some(RegState::Clobbered));
    assert_eq!(r.get(RCX), Some(RegState::Clobbered));
    assert!(
        matches!(r.get(RBX), Some(RegState::Known { .. })),
        "{:?}",
        r.get(RBX)
    );
    // Every other thread waits in ntdll: its frames are its context's,
    // then heuristics.
    for s in all.iter().filter(|s| s.thread != e.thread) {
        assert!(!s.frames.is_empty());
        assert_eq!(s.frames[0].method, Method::Context);
    }
}

#[test]
fn linux_crash() {
    let data = std::fs::read(testdata("coredump/crash-x86_64.core")).unwrap();
    let t = CoreTarget::new(&data).unwrap();
    let loaded = load(&t, &hermetic("coredump"));
    let all = stacks(&t, &loaded);
    // gdb's `thread apply all bt` for the thread that crashed: leaf,
    // middle, outer, worker (crash.c:30, 37, 44, 49), then start_thread.
    let crashed = all.iter().find(|s| s.thread == 464_366).unwrap();
    let f = &crashed.frames;
    let expect = [
        (0x5f30_1fdf_0200, "leaf", 30),
        (0x5f30_1fdf_0220, "middle", 37),
        (0x5f30_1fdf_0263, "outer", 44),
        (0x5f30_1fdf_028b, "worker", 49),
    ];
    for (i, (pc, name, line_)) in expect.iter().enumerate() {
        assert_eq!(f[i].pc, *pc, "frame {i}");
        assert_eq!(function(&f[i]), *name, "frame {i}");
        assert_eq!(line(&f[i]), *line_, "frame {i}");
        assert_eq!(f[i].confidence, Confidence::Certain, "frame {i}");
    }
    assert_eq!(f[1].method, Method::Cfi);
    // worker's caller: start_thread (libc's, not here), by worker's CFI.
    assert_eq!(f[4].pc, 0x73d5_5f6a_3dba);
    assert_eq!(f[4].confidence, Confidence::Certain);
    // The main thread: in libc (not here), so its frames are heuristic
    // past its context; main's frame (crash.c:59) is among them, found by
    // a scan or the frame pointer chain.
    let main = all.iter().find(|s| s.thread == 464_365).unwrap();
    assert_eq!(main.frames[0].method, Method::Context);
    let found_main = main.frames.iter().find(|f| function(f) == "main");
    let found_main = found_main.unwrap_or_else(|| {
        panic!(
            "{:#x?}",
            main.frames
                .iter()
                .map(|f| (f.pc, f.method))
                .collect::<Vec<_>>()
        )
    });
    assert_eq!(found_main.pc, 0x5f30_1fdf_0305);
    assert_eq!(found_main.confidence, Confidence::Likely);
    assert_ne!(main.end, End::NoRegisters);
}

#[test]
fn windows_reconstructed() {
    use turbonop_frames::reconstruct::{LinkKind, reconstruct};
    let data = std::fs::read(testdata("minidump/crash-x64.dmp")).unwrap();
    let t = MinidumpTarget::new(&data).unwrap();
    let loaded = load(&t, &hermetic("minidump"));
    let space = loaded.address_space(&t);
    let bases: Vec<u64> = t.modules().iter().map(|m| m.base).collect();
    let modules = Modules::new(&loaded, &space, &bases);
    // The crashing thread's stack, from the exception's stack pointer up,
    // as if its walk had failed at once: middle, outer, worker, then
    // kernel32, each linked by its callee's unwind data.
    let e = t.exception().unwrap();
    let sp = e.registers.as_ref().unwrap().get64(RSP).unwrap();
    let thread = t.threads().iter().find(|th| th.id == e.thread).unwrap();
    let end = thread.stack.clone().unwrap().end;
    let r = reconstruct(&t, &space, &modules, sp..end);
    let best = &r.chains[0];
    let pcs: Vec<u64> = best.frames.iter().map(|f| f.frame.pc).collect();
    assert_eq!(
        &pcs[..4],
        [
            0x7ff7_eed9_1053,
            0x7ff7_eed9_108e,
            0x7ff7_eed9_115e,
            0x7ffa_ca64_dda7
        ],
        "{:#x?}",
        r.chains
            .iter()
            .map(|c| c.frames.iter().map(|f| f.frame.pc).collect::<Vec<_>>())
            .collect::<Vec<_>>()
    );
    assert_eq!(best.frames[0].slot, 0x84_8757_f6d8);
    assert_eq!(best.frames[0].link, None);
    assert!(
        best.frames[1..4]
            .iter()
            .all(|f| f.link == Some(LinkKind::UnwindData))
    );
    assert_eq!(function(&best.frames[0].frame), "middle");
    // Without kernel32 and ntdll here, nothing reaches the thread's entry.
    assert!(!best.reaches_entry);
    assert_eq!(
        best.confidence,
        turbonop_frames::reconstruct::ChainConfidence::Likely
    );
    // Its slots: the return addresses after calls, and (kernel32's,
    // whose code isn't here) unknown.
    let s = r.slots.iter().find(|s| s.at == 0x84_8757_f6d8).unwrap();
    assert_eq!(s.after_call, Some(true));
    assert!(s.call_target.is_some());
    let k32 = r
        .slots
        .iter()
        .find(|s| s.value == 0x7ffa_ca64_dda7)
        .unwrap();
    assert_eq!(k32.after_call, None);
    // The main thread: wmain, then __scrt_common_main_seh, by unwind data.
    let main = &t.threads()[0];
    let stack = main.stack.clone().unwrap();
    let r = reconstruct(&t, &space, &modules, stack);
    let found = r.chains.iter().any(|c| {
        c.frames.windows(2).any(|w| {
            w[0].frame.pc == 0x7ff7_eed9_1142
                && w[1].frame.pc == 0x7ff7_eed9_142f
                && w[1].link == Some(LinkKind::UnwindData)
        })
    });
    assert!(found);
}
