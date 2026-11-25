//! A real core file (tools/coredump: crash.c, crashed in WSL), checked
//! against gdb's view of it (testdata/coredump/crash-x86_64.gdb.txt).

use turbonop_abi::{Os, regs::x64::*};
use turbonop_core::Arch;
use turbonop_image::{DebugId as ImageDebugId, Image};
use turbonop_target::{
    AddressSpace, CoreTarget, DebugId, ImageId, LoadedImage, Provenance, Target,
};

use crate::support::testdata;

#[test]
fn threads_and_signal() {
    let data = testdata("coredump/crash-x86_64.core");
    let t = CoreTarget::new(&data).unwrap();
    assert_eq!(t.arch(), Arch::X64);
    assert_eq!(t.os(), Some(Os::Linux));
    let threads = t.threads();
    assert_eq!(
        threads.iter().map(|t| t.id).collect::<Vec<_>>(),
        [464_366, 464_365]
    );
    // gdb's `info registers`, for the thread that crashed.
    let r = &threads[0].registers;
    for (n, v) in [
        (RAX, 0x64),
        (RBX, 0x73d5_5f98_a6c0),
        (RCX, 0x73d5_5f6a_3c1d),
        (RDX, 0),
        (RSI, 3),
        (RDI, 0x73d5_5f98_9e60),
        (RBP, 0x73d5_5f98_9e50),
        (RSP, 0x73d5_5f98_9e48),
        (R8, 0),
        (R9, 0x73d5_5f98_9e24),
        (R10, 8),
        (R11, 0x246),
        (R12, 0x73d5_5f98_a6c0),
        (R13, 0x7fff_3363_b7b0),
        (R14, 0x73d5_5f98_ace4),
        (R15, 0x7fff_3363_b8b7),
        (RIP, 0x5f30_1fdf_0200),
        (RFLAGS, 0x10202),
        (CS, 0x33),
        (SS, 0x2b),
        (FS_BASE, 0x73d5_5f98_a6c0),
        (GS_BASE, 0),
    ] {
        assert_eq!(r.get64(n), Some(v), "{n}");
    }
    assert_eq!(r.get64(MXCSR), Some(0x1F80));
    assert_eq!(threads[0].thread_pointer, Some(0x73d5_5f98_a6c0));
    let stack = threads[0].stack.clone().unwrap();
    assert!(stack.contains(&0x73d5_5f98_9e48), "{stack:x?}");
    // SIGSEGV, reading n->next->value: address 8.
    let e = t.exception().unwrap();
    assert_eq!((e.thread, e.code, e.address), (464_366, 11, 8));
    // The worker's stack, in memory: its return address into middle
    // (gdb's frame 1, 0x5f301fdf0220) is on the stack above RSP.
    let mut words = [0u8; 64];
    assert!(turbonop_target::read_exact(
        t.memory(),
        0x73d5_5f98_9e48,
        &mut words
    ));
    let found = words
        .chunks(8)
        .any(|w| u64::from_le_bytes(w.try_into().unwrap()) == 0x5f30_1fdf_0220);
    assert!(found, "middle's return address isn't on the stack");
}

#[test]
fn modules_from_mapped_files() {
    let data = testdata("coredump/crash-x86_64.core");
    let t = CoreTarget::new(&data).unwrap();
    let m = t.modules();
    let summary: Vec<_> = m
        .iter()
        .map(|m| (m.path.as_str(), m.base, m.size))
        .collect();
    assert_eq!(
        summary,
        [
            ("/tmp/da/crash-x86_64", 0x5f30_1fde_f000, 0x5000),
            (
                "/usr/lib/x86_64-linux-gnu/libc.so.6",
                0x73d5_5f60_0000,
                0x21_4000
            ),
            (
                "/usr/lib/x86_64-linux-gnu/ld-linux-x86-64.so.2",
                0x73d5_5f99_e000,
                0x3_e000
            ),
        ]
    );
    // The program's build ID, read from its headers in the core, is its
    // file's.
    let exe = testdata("coredump/crash-x86_64");
    let image = Image::parse(&exe).unwrap();
    let id = image
        .debug_ids()
        .into_iter()
        .find_map(|d| match d {
            ImageDebugId::BuildId(b) => Some(b.to_vec()),
            _ => None,
        })
        .unwrap();
    assert_eq!(m[0].image_id, Some(ImageId::BuildId(id.clone())));
    assert_eq!(m[0].debug_id, Some(DebugId::BuildId(id)));
    // The libraries' too (not checked against their files, not here).
    for lib in &m[1..] {
        assert!(
            matches!(lib.image_id, Some(ImageId::BuildId(ref b)) if b.len() == 20),
            "{}",
            lib.path
        );
    }
}

#[test]
fn loading_modules_files() {
    use turbonop_target::load::{Hints, load};
    let data = testdata("coredump/crash-x86_64.core");
    let t = CoreTarget::new(&data).unwrap();
    let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../testdata/coredump");
    // Only the fixtures' directory: not where the core says modules were
    // (on the machine that made it, /tmp/da is), nor the environment's
    // symbol directories or servers.
    let loaded = load(
        &t,
        &Hints {
            dirs: vec![dir.clone()],
            skip_recorded_paths: true,
            symbols: Some((Vec::new(), turbonop_program::server::Servers::default())),
            ..Hints::default()
        },
    );
    assert_eq!(loaded.modules.len(), 3);
    // The program: found beside the core (by name, checked by build ID),
    // with its DWARF.
    let exe = &loaded.modules[0];
    let found = exe.image.as_ref().unwrap();
    assert_eq!(found.path, dir.join("crash-x86_64"));
    assert!(matches!(exe.program, Some(Ok(_))));
    // The system's libraries aren't here: where they were looked for.
    let libc = &loaded.modules[1];
    let nf = libc.image.as_ref().unwrap_err();
    assert!(
        nf.tried.iter().any(|t| t.contains("libc.so.6")),
        "{:?}",
        nf.tried
    );
    assert!(libc.program.is_none());
    assert_eq!(loaded.counts(), (1, 1));
    // The address space: the program's code from its image.
    let space = loaded.address_space(&t);
    let mut code = [0u8; 4];
    assert_eq!(
        space.read(0x5f30_1fdf_0200, &mut code),
        Some(Provenance::Image)
    );
}

#[test]
fn code_from_the_image() {
    let data = testdata("coredump/crash-x86_64.core");
    let t = CoreTarget::new(&data).unwrap();
    let exe = testdata("coredump/crash-x86_64");
    let image = Image::parse(&exe).unwrap();
    let base = t.modules()[0].base;
    let loaded = LoadedImage::new(&image, base).unwrap();
    let space = AddressSpace::new(t.memory(), vec![loaded]);
    // The kernel didn't dump the program's code (a file mapping): the
    // image stands in for it, at the faulting instruction.
    let rip = 0x5f30_1fdf_0200;
    let mut code = [0u8; 4];
    assert_eq!(space.read(rip, &mut code), Some(Provenance::Image));
    // `mov eax, [rax+8]`-like: whatever it is, it's the file's bytes at
    // the instruction's offset (0x1200: the second mapping is offset
    // 0x1000).
    assert_eq!(&code, &exe[0x1200..0x1204]);
    // The headers were dumped: the core's own.
    let mut magic = [0u8; 4];
    assert_eq!(space.read(base, &mut magic), Some(Provenance::Dump));
    assert_eq!(&magic, b"\x7fELF");
}
