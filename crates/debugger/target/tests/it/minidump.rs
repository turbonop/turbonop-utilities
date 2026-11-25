use turbonop_abi::{Os, regs};
use turbonop_core::Arch;
use turbonop_minidump::{cpu, platform};
use turbonop_target::minidump::uuid_of_guid;
use turbonop_target::{DebugId, ImageId, MinidumpTarget, ModuleSource, Provenance, Target};

use crate::support::{ModuleSpec, ThreadSpec, Writer, amd64_context, arm64_context};

fn rsds(guid: [u8; 16], age: u32, name: &str) -> Vec<u8> {
    let mut b = b"RSDS".to_vec();
    b.extend(guid);
    b.extend(age.to_le_bytes());
    b.extend(name.as_bytes());
    b.push(0);
    b
}

const GUID: [u8; 16] = [
    0x10, 0x32, 0x54, 0x76, 0x98, 0xBA, 0xDC, 0xFE, 1, 2, 3, 4, 5, 6, 7, 8,
];

#[test]
fn windows_x64() {
    let mut w = Writer::new();
    w.system_info(cpu::AMD64, platform::WIN32_NT);
    let ctx1 = amd64_context(0x7FF6_0000_1010, 0x1000);
    let ctx2 = amd64_context(0x7FF6_0000_2020, 0x2000);
    let stack = [0xAAu8; 0x40];
    w.threads(&[
        ThreadSpec {
            id: 100,
            teb: 0x7000,
            stack_start: 0x1000,
            stack: &stack,
            context: &ctx1,
        },
        ThreadSpec {
            id: 200,
            teb: 0,
            stack_start: 0,
            stack: &[],
            context: &ctx2,
        },
    ]);
    w.thread_names(&[(200, "worker")]);
    let cv = rsds(GUID, 3, r"c:\build\app.pdb");
    w.modules(&[ModuleSpec {
        base: 0x7FF6_0000_0000,
        size: 0x5000,
        time_date_stamp: 0x6543_2100,
        name: r"C:\app\app.exe",
        cv: &cv,
    }]);
    w.memory(&[(0x1000, &stack), (0x9000, &[1, 2, 3, 4])]);
    let fault = amd64_context(0x7FF6_0000_3030, 0x1008);
    w.exception(100, 0xC000_0005, 0x7FF6_0000_3030, &fault);
    let data = w.finish();
    let t = MinidumpTarget::new(&data).unwrap();
    assert_eq!(t.arch(), Arch::X64);
    assert_eq!(t.os(), Some(Os::Windows));
    assert_eq!(t.abi().unwrap().home_space(), 32);
    assert_eq!(t.snapshot().generation, 0);
    assert!(
        t.capabilities()
            .has(turbonop_target::Capabilities::READ_MEMORY)
    );
    assert!(!t.capabilities().has(turbonop_target::Capabilities::RUN));
    let threads = t.threads();
    assert_eq!(threads.len(), 2);
    assert_eq!(threads[0].id, 100);
    assert_eq!(
        threads[0].registers.get64(regs::x64::RIP),
        Some(0x7FF6_0000_1010)
    );
    assert_eq!(threads[0].registers.get64(regs::x64::RSP), Some(0x1000));
    assert_eq!(threads[0].stack, Some(0x1000..0x1040));
    assert_eq!(threads[0].thread_pointer, Some(0x7000));
    assert_eq!(threads[0].name, None);
    assert_eq!(threads[1].name.as_deref(), Some("worker"));
    assert_eq!(threads[1].stack, None);
    assert_eq!(threads[1].thread_pointer, None);
    let m = &t.modules()[0];
    assert_eq!(m.path, r"C:\app\app.exe");
    assert_eq!(m.file_name(), "app.exe");
    assert_eq!(
        m.image_id,
        Some(ImageId::Pe {
            time_date_stamp: 0x6543_2100,
            size_of_image: 0x5000
        })
    );
    assert_eq!(
        m.debug_id,
        Some(DebugId::Pdb {
            guid: GUID,
            age: 3,
            path: r"c:\build\app.pdb".into()
        })
    );
    assert_eq!(m.source, ModuleSource::ModuleList);
    assert!(t.module_at(0x7FF6_0000_4FFF).is_some());
    assert!(t.module_at(0x7FF6_0000_5000).is_none());
    let e = t.exception().unwrap();
    assert_eq!(
        (e.thread, e.code, e.address),
        (100, 0xC000_0005, 0x7FF6_0000_3030)
    );
    assert_eq!(e.parameters, [1, 0xdead]);
    let r = e.registers.as_ref().unwrap();
    assert_eq!(r.get64(regs::x64::RIP), Some(0x7FF6_0000_3030));
    // Memory.
    let mut buf = [0u8; 4];
    let mem = t.memory();
    assert_eq!(mem.read_at(0x9000, &mut buf), 4);
    assert_eq!(buf, [1, 2, 3, 4]);
    assert_eq!(mem.read_at(0x9002, &mut buf), 2);
    assert_eq!(mem.read_at(0x8000, &mut buf), 0);
    assert_eq!(mem.next_available(0x1040), Some(0x9000));
    assert_eq!(mem.next_available(0x9000), None);
    assert_eq!(mem.provenance(), Provenance::Dump);
}

#[test]
fn linux_arm64_breakpad() {
    let mut w = Writer::new();
    w.system_info(cpu::ARM64, platform::LINUX);
    let ctx = arm64_context(0x5555_0000_1000, 0x7FFF_0000);
    w.threads(&[ThreadSpec {
        id: 7,
        teb: 0,
        stack_start: 0,
        stack: &[],
        context: &ctx,
    }]);
    let build_id: Vec<u8> = (1..=20).collect();
    let mut bpel = b"LEpB".to_vec();
    bpel.extend(&build_id);
    // An older Breakpad's record: RSDS, the build ID's first 16 bytes.
    let old = rsds(GUID, 0, "");
    w.modules(&[
        ModuleSpec {
            base: 0x5555_0000_0000,
            size: 0x4000,
            time_date_stamp: 0,
            name: "/usr/bin/app",
            cv: &bpel,
        },
        ModuleSpec {
            base: 0x7F00_0000_0000,
            size: 0x2000,
            time_date_stamp: 0,
            name: "/lib/libold.so",
            cv: &old,
        },
    ]);
    // The maps: app (in the list), libc (not: found here), an anonymous
    // mapping, a non-executable file, the stack.
    let maps = "\
5555_00000000-555500001000 r--p 00000000 08:01 11 /usr/bin/app
555500001000-555500004000 r-xp 00001000 08:01 11 /usr/bin/app
7f1000000000-7f1000028000 r--p 00000000 08:01 22 /usr/lib/aarch64-linux-gnu/libc.so.6
7f1000028000-7f10001b0000 r-xp 00028000 08:01 22 /usr/lib/aarch64-linux-gnu/libc.so.6
7f10001b0000-7f10001c0000 rw-p 00000000 00:00 0
7f10001c0000-7f10001d0000 rw-p 001b0000 08:01 22 /usr/lib/aarch64-linux-gnu/libc.so.6
7f2000000000-7f2000001000 r--p 00000000 08:01 33 /usr/share/locale/data
7ffffff00000-800000000000 rw-p 00000000 00:00 0 [stack]
"
    .replace('_', "");
    w.stream(0x4767_0009, maps.as_bytes());
    let data = w.finish();
    let t = MinidumpTarget::new(&data).unwrap();
    assert_eq!(t.arch(), Arch::Arm64);
    assert_eq!(t.os(), Some(Os::Linux));
    assert_eq!(
        t.threads()[0].registers.get64(regs::arm64::PC),
        Some(0x5555_0000_1000)
    );
    let modules = t.modules();
    assert_eq!(modules.len(), 3);
    assert_eq!(modules[0].path, "/usr/bin/app");
    assert_eq!(
        modules[0].image_id,
        Some(ImageId::BuildId(build_id.clone()))
    );
    assert_eq!(modules[0].debug_id, Some(DebugId::BuildId(build_id)));
    assert_eq!(modules[1].path, "/lib/libold.so");
    assert_eq!(modules[1].debug_id, Some(DebugId::BuildId(GUID.to_vec())));
    let libc = &modules[2];
    assert_eq!(libc.path, "/usr/lib/aarch64-linux-gnu/libc.so.6");
    assert_eq!(libc.source, ModuleSource::MemoryMap);
    assert_eq!((libc.base, libc.size), (0x7F10_0000_0000, 0x1D_0000));
    assert_eq!(libc.image_id, None);
}

#[test]
fn macos_crashpad_uuid() {
    let mut w = Writer::new();
    w.system_info(cpu::ARM64, platform::MAC_OS_X);
    let cv = rsds(GUID, 0, "app");
    w.modules(&[ModuleSpec {
        base: 0x1_0000_0000,
        size: 0x8000,
        time_date_stamp: 0,
        name: "/Applications/App.app/Contents/MacOS/App",
        cv: &cv,
    }]);
    let data = w.finish();
    let t = MinidumpTarget::new(&data).unwrap();
    assert_eq!(t.os(), Some(Os::MacOs));
    let uuid = [
        0x76, 0x54, 0x32, 0x10, 0xBA, 0x98, 0xFE, 0xDC, 1, 2, 3, 4, 5, 6, 7, 8,
    ];
    assert_eq!(uuid_of_guid(GUID), uuid);
    assert_eq!(t.modules()[0].image_id, Some(ImageId::Uuid(uuid)));
    assert_eq!(t.modules()[0].debug_id, Some(DebugId::Uuid(uuid)));
    // No threads, no exception: none.
    assert!(t.threads().is_empty());
    assert!(t.exception().is_none());
    // No architecture from a context: the system info's.
    assert_eq!(t.arch(), Arch::Arm64);
}

#[test]
fn not_a_minidump() {
    assert!(MinidumpTarget::new(b"MDMQ").is_err());
    assert!(MinidumpTarget::new(&[]).is_err());
}

/// Writes the synthetic minidumps in testdata/minidump (the fuzzer's seeds,
/// and examples): `cargo test -p turbonop-target -- --ignored write_fixtures`.
#[test]
#[ignore]
fn write_fixtures() {
    let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../testdata/minidump");
    std::fs::create_dir_all(&dir).unwrap();
    // Windows x64: a thread with a stack, a module, an exception.
    let mut w = Writer::new();
    w.system_info(cpu::AMD64, platform::WIN32_NT);
    let ctx = amd64_context(0x7FF6_0000_1010, 0x1000);
    let stack: Vec<u8> = (0..=255).collect();
    w.threads(&[ThreadSpec {
        id: 100,
        teb: 0x7000,
        stack_start: 0x1000,
        stack: &stack,
        context: &ctx,
    }]);
    w.thread_names(&[(100, "main")]);
    let cv = rsds(GUID, 3, r"c:\build\app.pdb");
    w.modules(&[ModuleSpec {
        base: 0x7FF6_0000_0000,
        size: 0x5000,
        time_date_stamp: 0x6543_2100,
        name: r"C:\app\app.exe",
        cv: &cv,
    }]);
    w.memory(&[(0x1000, &stack)]);
    w.exception(100, 0xC000_0005, 0x7FF6_0000_1010, &ctx);
    std::fs::write(dir.join("synthetic-windows-x64.dmp"), w.finish()).unwrap();
    // Linux ARM64: Breakpad's build ID, and the process maps.
    let mut w = Writer::new();
    w.system_info(cpu::ARM64, platform::LINUX);
    let ctx = arm64_context(0x5555_0000_1000, 0x7FFF_0000);
    w.threads(&[ThreadSpec {
        id: 7,
        teb: 0,
        stack_start: 0x7FFF_0000,
        stack: &stack,
        context: &ctx,
    }]);
    let mut bpel = b"LEpB".to_vec();
    bpel.extend(1..=20u8);
    w.modules(&[ModuleSpec {
        base: 0x5555_0000_0000,
        size: 0x4000,
        time_date_stamp: 0,
        name: "/usr/bin/app",
        cv: &bpel,
    }]);
    w.memory(&[(0x7FFF_0000, &stack)]);
    w.stream(0x4767_0009, b"555500000000-555500004000 r-xp 00000000 08:01 11 /usr/bin/app\n7f1000000000-7f1000028000 r-xp 00000000 08:01 22 /usr/lib/libc.so.6\n");
    w.exception(7, 11, 0x5555_0000_1000, &ctx);
    std::fs::write(dir.join("synthetic-linux-arm64.dmp"), w.finish()).unwrap();
}
