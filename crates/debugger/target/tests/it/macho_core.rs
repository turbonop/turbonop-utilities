//! Mach-O core files made here (no Mac to make real ones on yet): an ARM64
//! core with a thread, its exception state, an `addrable bits` note, and a
//! segment that's a dylib's (its header, UUID, and install name).

use turbonop_abi::{Os, regs};
use turbonop_core::Arch;
use turbonop_target::{DebugId, ImageId, MachCoreTarget, Target};

fn u32s(v: &[u32]) -> Vec<u8> {
    v.iter().flat_map(|x| x.to_le_bytes()).collect()
}

fn u64s(v: &[u64]) -> Vec<u8> {
    v.iter().flat_map(|x| x.to_le_bytes()).collect()
}

/// A command: its type, then its size, then `body`.
fn command(cmd: u32, body: &[u8]) -> Vec<u8> {
    let mut b = u32s(&[cmd, u32::try_from(body.len() + 8).unwrap()]);
    b.extend(body);
    b
}

fn segment(name: &[u8], vmaddr: u64, vmsize: u64, fileoff: u64, filesize: u64) -> Vec<u8> {
    let mut body = vec![0u8; 16];
    body[..name.len()].copy_from_slice(name);
    body.extend(u64s(&[vmaddr, vmsize, fileoff, filesize]));
    body.extend(u32s(&[7, 5, 0, 0]));
    command(0x19, &body)
}

const UUID: [u8; 16] = [9, 8, 7, 6, 5, 4, 3, 2, 1, 0, 0xa, 0xb, 0xc, 0xd, 0xe, 0xf];

/// A thin ARM64 dylib's first bytes: its header, __TEXT, its UUID, and its
/// install name.
fn dylib() -> Vec<u8> {
    let mut cmds = segment(b"__TEXT", 0, 0x4000, 0, 0x1000);
    cmds.extend(command(0x1B, &UUID));
    let mut id = u32s(&[24, 0, 0, 0]);
    id.extend(b"/usr/lib/libfoo.dylib\0\0\0");
    cmds.extend(command(0xD, &id));
    let mut b = u32s(&[
        0xFEED_FACF,
        0x0100_000C,
        0,
        6,
        3,
        u32::try_from(cmds.len()).unwrap(),
        0,
        0,
    ]);
    b.extend(cmds);
    b.resize(0x1000, 0);
    b
}

#[test]
fn arm64_core() {
    let lib = dylib();
    let stack = vec![0x5Au8; 0x100];
    // The thread: X0..X28 = 0x100+i, FP, LR, SP, PC; CPSR. Then the
    // exception state: FAR, ESR, the exception.
    let mut state: Vec<u64> = (0..29).map(|i| 0x100 + i).collect();
    state.extend([0x7000_0010, 0x1_0000_0100, 0x7000_0000, 0x1_0000_0200]);
    let mut thread = u32s(&[6, 68]);
    thread.extend(u64s(&state));
    thread.extend(u32s(&[0x6000_0000, 0]));
    thread.extend(u32s(&[7, 4]));
    thread.extend(u64s(&[0xDEAD]));
    thread.extend(u32s(&[0x9200_0046, 3]));
    // Commands: the dylib's segment, the stack's, the thread, the note.
    let header = 32;
    let n_cmds = 4;
    let cmds_len = 72 + 72 + (8 + thread.len()) + 40;
    let data_at = (header + cmds_len).next_multiple_of(0x1000);
    let lib_at = data_at;
    let stack_at = lib_at + lib.len();
    let note_at = stack_at + stack.len();
    let mut cmds = segment(b"", 0x1_0000_0000, 0x4000, lib_at as u64, lib.len() as u64);
    cmds.extend(segment(
        b"",
        0x7000_0000,
        0x100,
        stack_at as u64,
        stack.len() as u64,
    ));
    cmds.extend(command(4, &thread));
    let mut note = vec![0u8; 16];
    note[..13].copy_from_slice(b"addrable bits");
    note.extend(u64s(&[note_at as u64, 8]));
    cmds.extend(command(0x31, &note));
    assert_eq!(cmds.len(), cmds_len);
    let mut core = u32s(&[
        0xFEED_FACF,
        0x0100_000C,
        0,
        4,
        n_cmds,
        u32::try_from(cmds_len).unwrap(),
        0,
        0,
    ]);
    core.extend(cmds);
    core.resize(data_at, 0);
    core.extend(&lib);
    core.extend(&stack);
    core.extend(u32s(&[2, 47]));
    let t = MachCoreTarget::new(&core).unwrap();
    assert_eq!(t.arch(), Arch::Arm64);
    assert_eq!(t.os(), Some(Os::MacOs));
    assert_eq!(t.addressable_bits(), Some(47));
    let th = &t.threads()[0];
    assert_eq!(th.id, 1);
    assert_eq!(th.registers.get64(0), Some(0x100));
    assert_eq!(th.registers.get64(regs::arm64::X28), Some(0x100 + 28));
    assert_eq!(th.registers.get64(regs::arm64::FP), Some(0x7000_0010));
    assert_eq!(th.registers.get64(regs::arm64::LR), Some(0x1_0000_0100));
    assert_eq!(th.registers.get64(regs::arm64::SP), Some(0x7000_0000));
    assert_eq!(th.registers.get64(regs::arm64::PC), Some(0x1_0000_0200));
    assert_eq!(th.stack, Some(0x7000_0000..0x7000_0100));
    let e = t.exception().unwrap();
    assert_eq!(
        (e.thread, e.address, e.code, e.flags),
        (1, 0xDEAD, 0x9200_0046, 3)
    );
    // The module, from its header in the segment.
    let m = &t.modules()[0];
    assert_eq!(m.path, "/usr/lib/libfoo.dylib");
    assert_eq!(m.base, 0x1_0000_0000);
    assert_eq!(m.size, 0x4000);
    assert_eq!(m.image_id, Some(ImageId::Uuid(UUID)));
    assert_eq!(m.debug_id, Some(DebugId::Uuid(UUID)));
    // Memory.
    let mut b = [0u8; 4];
    assert_eq!(t.memory().read_at(0x7000_0080, &mut b), 4);
    assert_eq!(b, [0x5A; 4]);
    // Not a core: a dylib.
    assert!(MachCoreTarget::new(&lib).is_err());
}
