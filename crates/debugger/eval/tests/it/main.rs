//! The integration tests, as one binary (one link, not one per file).
//!
//! The Windows dump of tools/minidump/crash.c, each frame's locals checked
//! against DbgEng's `dv /t /v` (testdata/minidump/crash-x64.dv.txt); and
//! the expression evaluator, on expressions made here.

use turbonop_global_alloc as _;

mod expr;

use std::path::{Path, PathBuf};

use turbonop_debuginfo::value::Value;
use turbonop_eval::{Local, Unavailable, locals};
use turbonop_frames::{Modules, walk_all};
use turbonop_program::server::Servers;
use turbonop_target::load::{Hints, load};
use turbonop_target::{MinidumpTarget, Provenance, Target};

fn testdata(path: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../../testdata")
        .join(path)
}

fn find<'a>(locals: &'a [Local], name: &str) -> &'a Local {
    locals
        .iter()
        .find(|l| l.name == name)
        .unwrap_or_else(|| panic!("no {name} in {locals:#?}"))
}

fn value(l: &Local) -> &Value {
    l.value
        .as_ref()
        .unwrap_or_else(|e| panic!("{}: {e}", l.name))
}

#[test]
fn windows_locals() {
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
    let arch = t.arch();
    // leaf: n isn't available (rcx was overwritten), depth = 3 (edx).
    let l = locals(s, 0, &modules, &space, arch).unwrap();
    assert!(find(&l, "n").value.is_err(), "{:?}", find(&l, "n"));
    let depth = find(&l, "depth");
    assert!(depth.param);
    assert_eq!(value(depth).text, "3");
    assert_eq!(depth.location, "rdx");
    assert_eq!(depth.provenance, None);
    // middle: n = 0x848757f770 (rbx, which leaf preserved), depth = 2
    // (edi), buffer: 64 'm's on the stack at 0x848757f700.
    let l = locals(s, 1, &modules, &space, arch).unwrap();
    let n = value(find(&l, "n"));
    assert_eq!(n.pointer, Some(0x84_8757_f770));
    assert_eq!(value(find(&l, "depth")).text, "2");
    let buffer = find(&l, "buffer");
    assert_eq!(buffer.type_name, "char[64]");
    assert!(
        value(buffer).text.contains(&"m".repeat(64)),
        "{}",
        value(buffer).text
    );
    assert_eq!(buffer.provenance, Some(Provenance::Dump));
    // outer: depth isn't available; n, the node, on the stack: {0, 42}.
    let l = locals(s, 2, &modules, &space, arch).unwrap();
    assert!(find(&l, "depth").value.is_err());
    let node = value(find(&l, "n"));
    let next = node.children.iter().find(|c| c.name == "next").unwrap();
    assert_eq!(next.text, "nullptr");
    let v = node.children.iter().find(|c| c.name == "value").unwrap();
    assert_eq!(v.text, "42");
    // worker: arg = 0, in its home space (0x848757f7c0).
    let l = locals(s, 3, &modules, &space, arch).unwrap();
    assert_eq!(value(find(&l, "arg")).text, "nullptr");
    // A frame whose module has no debug information here (kernel32).
    assert_eq!(
        locals(s, 4, &modules, &space, arch).err(),
        Some(Unavailable::NoDebugInfo)
    );
}

#[test]
fn linux_locals() {
    use turbonop_target::CoreTarget;
    let data = std::fs::read(testdata("coredump/crash-x86_64.core")).unwrap();
    let t = CoreTarget::new(&data).unwrap();
    let hints = Hints {
        dirs: vec![testdata("coredump")],
        skip_recorded_paths: true,
        symbols: Some((Vec::new(), Servers::default())),
        ..Hints::default()
    };
    let loaded = load(&t, &hints);
    let space = loaded.address_space(&t);
    let bases: Vec<u64> = t.modules().iter().map(|m| m.base).collect();
    let modules = Modules::new(&loaded, &space, &bases);
    let stacks = walk_all(&t, &space, &modules);
    let s = stacks.iter().find(|s| s.thread == 464_366).unwrap();
    let arch = t.arch();
    // gdb: leaf (n=0x73d55f989e60, depth=depth@entry=3); total in rax.
    let l = locals(s, 0, &modules, &space, arch).unwrap();
    assert_eq!(value(find(&l, "n")).pointer, Some(0x73d5_5f98_9e60));
    assert_eq!(value(find(&l, "depth")).text, "3");
    assert_eq!(value(find(&l, "total")).text, "100");
    // middle (n=n@entry=0x73d55f989e60, depth=depth@entry=2): entry
    // values, from outer's call site (n's relative to outer's frame base).
    let l = locals(s, 1, &modules, &space, arch).unwrap();
    assert_eq!(value(find(&l, "n")).pointer, Some(0x73d5_5f98_9e60));
    assert_eq!(value(find(&l, "depth")).text, "2");
    // outer (depth=depth@entry=1); n, the node {0, 42}, at its CFA-48.
    let l = locals(s, 2, &modules, &space, arch).unwrap();
    assert_eq!(value(find(&l, "depth")).text, "1");
    let node = value(find(&l, "n"));
    let v = node.children.iter().find(|c| c.name == "value").unwrap();
    assert_eq!(v.text, "42");
    assert_eq!(find(&l, "n").provenance, Some(Provenance::Dump));
    // worker (arg=<optimized out>): its entry value is libc's to give, and
    // libc isn't here.
    let l = locals(s, 3, &modules, &space, arch).unwrap();
    assert_eq!(
        find(&l, "arg").value.as_ref().err(),
        Some(&Unavailable::EntryValue)
    );
}

#[test]
fn windows_x86_locals() {
    // DbgEng's dv in middle: n = 0x0117fb1c (its stack argument: ecx, its
    // def-range's register, was clobbered by the call), depth = 2.
    let data = std::fs::read(testdata("minidump/crash-x86.dmp")).unwrap();
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
    let l = locals(s, 1, &modules, &space, t.arch()).unwrap();
    assert_eq!(value(find(&l, "n")).pointer, Some(0x0117_fb1c));
    assert_eq!(value(find(&l, "depth")).text, "2");
    assert!(value(find(&l, "buffer")).text.contains(&"m".repeat(64)));
    // leaf's are relative to x86's virtual frame (): not yet.
    let l = locals(s, 0, &modules, &space, t.arch()).unwrap();
    assert_eq!(
        find(&l, "depth").value.as_ref().err(),
        Some(&Unavailable::VirtualFrame)
    );
}
