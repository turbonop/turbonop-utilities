//! Fingerprints: for each function and datum of an image, a value that is
//! the same for two that do the same thing and refer to things that are
//! the same, wherever each is.
//!
//! Each node starts from a local hash ([`code`], [`data`], [`named`]) and
//! the ordered list of nodes it refers to. Code's local hash is of its
//! instructions with every address masked out: a branch within the
//! function counts as its offset from the function's start; anything else
//! it refers to (a call's target, the data an operand reaches) is a
//! reference, in the order the code makes them (RISC-V's `AUIPC`, and
//! the `LUI` an address is built on, count by their registers alone).
//! Data's is of its bytes with
//! its pointers masked out, which are its references. What is known by its
//! name (an import) is its name.
//!
//! [`refine`] then gives each node a class: its local hash and its
//! references' classes, in order, rehashed until the classes stop
//! splitting. Two functions identical but for their callees get different
//! classes unless the callees' classes are the same; cycles (recursion)
//! need no special case. This is how linkers fold identical code (lld's
//! ICF), and it makes the classes comparable across images: nothing in
//! them depends on an address.

#![warn(missing_docs)]

#[cfg(test)]
use turbonop_global_alloc as _;

use std::hash::Hasher;

use rustc_hash::FxHasher;
use turbonop_xref::{Arch, CodeRef, Via};

/// A node's local hash and what it refers to (node indexes, in order).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Item {
    /// The hash of its own bytes (references masked out).
    pub local: u64,
    /// The nodes it refers to, in order.
    pub refs: Vec<u32>,
}

/// Where an address is, for [`code`] and [`data`]: in another node, or in
/// none known.
pub trait Resolve {
    /// The node `address` is in (an address code computes or accesses).
    fn node_at(&self, address: u64) -> Option<u32>;
    /// The node starting at `address` (a call's or branch's target).
    fn node_starting_at(&self, address: u64) -> Option<u32>;
}

fn hasher(tag: u8) -> FxHasher {
    let mut h = FxHasher::default();
    h.write_u8(tag);
    h
}

/// A datum known by its name (an imported function's address table entry,
/// say): its name alone.
pub fn named(name: &str) -> Item {
    let mut h = hasher(b'n');
    h.write(name.as_bytes());
    h.write_usize(name.len());
    Item {
        local: h.finish(),
        refs: Vec::new(),
    }
}

/// Data: its bytes (`bytes`, of a node `size` long; past `bytes`, zero),
/// with its pointers masked out: `pointers` are each pointer's offset,
/// width, and the node it points to (if any), in order.
pub fn data(bytes: &[u8], size: u64, pointers: &[(u64, u32, Option<u32>)]) -> Item {
    let mut h = hasher(b'd');
    h.write_u64(size);
    let mut at = 0usize;
    let mut refs = Vec::new();
    for &(offset, width, to) in pointers {
        let Ok(start) = usize::try_from(offset) else {
            break;
        };
        if start < at || start > bytes.len() {
            continue;
        }
        h.write(&bytes[at..start]);
        h.write_u8(0xFE);
        h.write_u32(width);
        match to {
            Some(n) => refs.push(n),
            None => h.write_u8(0xFD),
        }
        at = (start + width as usize).min(bytes.len());
    }
    h.write(&bytes[at.min(bytes.len())..]);
    Item {
        local: h.finish(),
        refs,
    }
}

/// Code: the function `[start, start + code.len())` in `arch`, with
/// `image_base` as for [`turbonop_xref::scan_image`], and `absolute`
/// the addresses its instructions hold that relocations name (32-bit x86:
/// each one's slot and the address there), sorted by slot.
pub fn code(
    arch: Arch,
    mode: turbonop_code::Mode,
    code: &[u8],
    start: u64,
    image_base: Option<u64>,
    absolute: &[(u64, u64)],
    resolve: &impl Resolve,
) -> Item {
    let mut refs = Vec::new();
    // (x86's are found as the code is decoded below: decoded once.)
    if !matches!(arch, Arch::X86 | Arch::X64) {
        turbonop_xref::scan_image(arch, mode, code, start, image_base, &mut refs);
    }
    let big = mode.big;
    let end = start + code.len() as u64;
    let mut item = Item::default();
    let mut h = hasher(b'c');
    // What an address the code uses is: a place in the function, or a
    // reference to another node.
    let target = |h: &mut FxHasher, to: u64, via: Via, item: &mut Item| {
        if (start..end).contains(&to) {
            h.write_u8(1);
            h.write_u64(to - start);
            return;
        }
        let node = match via {
            Via::Call | Via::Jump => resolve.node_starting_at(to),
            // (An address used any other way: where it falls.)
            _ => resolve.node_at(to),
        };
        match node {
            Some(n) => {
                h.write_u8(2);
                item.refs.push(n);
            }
            None => h.write_u8(3),
        }
    };
    let mut r = 0;
    match arch {
        Arch::X86 | Arch::X64 => {
            let mode = if arch == Arch::X86 {
                turbonop_x86::Mode::Bits32
            } else {
                turbonop_x86::Mode::Bits64
            };
            let mut sweep = turbonop_xref::X86Sweep::new(image_base.filter(|_| arch == Arch::X64));
            let mut a = 0;
            let mut at = 0usize;
            while at < code.len() {
                let pc = start + at as u64;
                let Ok(inst) = turbonop_x86::decode(&code[at..], mode, pc) else {
                    h.write_u8(0xFF);
                    h.write(&code[at..]);
                    break;
                };
                let next = pc + inst.len() as u64;
                // The references this instruction makes, and the absolute
                // addresses in it.
                refs.clear();
                sweep.step(&inst, pc, &mut refs);
                let mine = &refs[..];
                let first = a;
                while a < absolute.len() && absolute[a].0 < next {
                    a += 1;
                }
                let abs = &absolute[first..a];
                let masked = !mine.is_empty() || !abs.is_empty();
                x86_instruction(&mut h, &inst, masked);
                for c in mine {
                    target(&mut h, c.to, c.via, &mut item);
                }
                for &(_, to) in abs {
                    target(&mut h, to, Via::Address, &mut item);
                }
                at += inst.len().max(1);
            }
        }
        Arch::Arm64 => {
            for (i, w) in code.chunks(4).enumerate() {
                let pc = start + i as u64 * 4;
                let Ok(w) = <[u8; 4]>::try_from(w) else {
                    h.write(w);
                    break;
                };
                let word = u32::from_le_bytes(w);
                let mine = take_refs(&refs, &mut r, pc);
                let Some(inst) = turbonop_arm64::decode(word, pc) else {
                    h.write_u32(word);
                    continue;
                };
                h.write_u32(word & !arm64_address_bits(inst.encoding_name(), !mine.is_empty()));
                for c in mine {
                    target(&mut h, c.to, c.via, &mut item);
                }
            }
        }
        Arch::Arm => {
            let mut at = 0usize;
            for (off, inst) in thumb(code) {
                at = off + usize::from(inst.size);
                let pc = start + off as u64;
                let mine = take_refs(&refs, &mut r, pc);
                // (What holds an address, PC-relative or built, is left
                // out: its encoding and registers are the instruction.)
                if addresses(&inst) || !mine.is_empty() {
                    h.write(inst.encoding.name.as_bytes());
                    arm32_registers(&mut h, &inst);
                } else {
                    h.write_u32(inst.word);
                }
                for c in mine {
                    target(&mut h, c.to, c.via, &mut item);
                }
            }
            // (Then what isn't code: a literal pool, as it is.)
            h.write(code.get(at..).unwrap_or_default());
        }
        Arch::RiscV32
        | Arch::RiscV64
        | Arch::LoongArch32
        | Arch::LoongArch64
        | Arch::PowerPc
        | Arch::PowerPc64
        | Arch::Mips
        | Arch::Mips64 => {
            // (All decoded by tables: what holds an address PC-relative,
            // what an absolute one's upper part, and the base registers
            // an addressing instruction adds to.)
            type Table = (
                Vec<(usize, turbonop_riscv::Instruction)>,
                fn(&str) -> bool,
                &'static [&'static str],
                &'static [&'static str],
            );
            let (insts, pc_relative, upper, base): Table = match arch {
                Arch::RiscV32 | Arch::RiscV64 => {
                    (riscv(arch, code), |n| n == "AUIPC", &["LUI"], &["rs1"])
                }
                Arch::LoongArch32 | Arch::LoongArch64 => (
                    loongarch(arch, code),
                    |n| matches!(n, "PCALAU12I" | "PCADDU12I" | "PCADDU18I" | "PCADDI"),
                    &["LU12I_W"],
                    &["rj"],
                ),
                Arch::Mips | Arch::Mips64 => (
                    mips(arch, mode, code),
                    |n| {
                        matches!(
                            n,
                            "AUIPC" | "ALUIPC" | "ADDIUPC" | "LWPC" | "LWUPC" | "LDPC"
                        )
                    },
                    &["LUi"],
                    &["rs", "base"],
                ),
                _ => (
                    powerpc(arch, big, code),
                    // (The prefixed PC-relative forms: `pld`'s `PLDpc`.)
                    |n| n.starts_with('P') && n.ends_with("pc"),
                    &["ADDIS", "LIS"],
                    &["RA", "RS"],
                ),
            };
            // The upper parts whose registers an addressing instruction
            // then uses (they are an address's).
            let mut masked = vec![false; insts.len()];
            let mut uppers = [None::<usize>; 32];
            let referring: std::collections::HashSet<u64> = refs.iter().map(|c| c.from).collect();
            for (k, (off, i)) in insts.iter().enumerate() {
                let pc = start + *off as u64;
                if pc_relative(i.name()) {
                    masked[k] = true;
                }
                if referring.contains(&pc) {
                    masked[k] = true;
                    // (A PowerPC load's or store's base: its memory
                    // operand's.)
                    let memory_base = matches!(arch, Arch::PowerPc | Arch::PowerPc64)
                        .then(|| turbonop_powerpc::memory(i))
                        .flatten()
                        .and_then(|(_, r)| gpr(turbonop_powerpc::Operand::Reg(r)));
                    if let Some(rs) =
                        memory_base.or_else(|| base.iter().find_map(|b| i.operand(b).and_then(gpr)))
                        && let Some(l) = uppers[usize::from(rs)]
                    {
                        masked[l] = true;
                    }
                }
                for op in i.operands().iter().take(usize::from(i.inst().defs)) {
                    if let Some(rd) = gpr(*op) {
                        uppers[usize::from(rd)] = upper.contains(&i.name()).then_some(k);
                    }
                }
            }
            let mut at = 0usize;
            for (k, (off, i)) in insts.iter().enumerate() {
                at = off + usize::from(i.size);
                let pc = start + *off as u64;
                let mine = take_refs(&refs, &mut r, pc);
                table_instruction(&mut h, i, masked[k]);
                for c in mine {
                    target(&mut h, c.to, c.via, &mut item);
                }
            }
            h.write(code.get(at..).unwrap_or_default());
        }
        // (Code no decoder reads: its bytes, as they are.)
        _ => h.write(code),
    }
    h.write_usize(code.len());
    item.local = h.finish();
    item
}

/// RISC-V code's instructions (compressed ones by their full forms), with
/// their offsets, to the first that doesn't decode.
fn riscv(arch: Arch, code: &[u8]) -> Vec<(usize, turbonop_riscv::Instruction)> {
    let features = turbonop_riscv::Features::standard(arch == Arch::RiscV64);
    let mut out = Vec::new();
    let mut at = 0usize;
    while at < code.len() {
        let Some(i) = turbonop_riscv::decode(&code[at..], &features) else {
            break;
        };
        let size = i.size;
        let full = turbonop_riscv::uncompress(&i, &features).unwrap_or(i);
        out.push((at, full));
        at += usize::from(size);
    }
    out
}

/// LoongArch code's instructions, with their offsets, to the first that
/// doesn't decode.
fn loongarch(arch: Arch, code: &[u8]) -> Vec<(usize, turbonop_loongarch::Instruction)> {
    let features = if arch == Arch::LoongArch64 {
        turbonop_loongarch::Features::la64()
    } else {
        turbonop_loongarch::Features::la32()
    };
    code.as_chunks::<4>()
        .0
        .iter()
        .map_while(|w| turbonop_loongarch::decode(w, &features))
        .enumerate()
        .map(|(n, i)| (4 * n, i))
        .collect()
}

/// PowerPC code's instructions (a prefixed one's 8 bytes as one), its
/// words big-endian (`big`) or not, with their offsets, to the first that
/// doesn't decode.
fn powerpc(arch: Arch, big: bool, code: &[u8]) -> Vec<(usize, turbonop_powerpc::Instruction)> {
    let features = turbonop_powerpc::Features::standard(arch == Arch::PowerPc64);
    let mut out = Vec::new();
    let mut at = 0usize;
    while at < code.len() {
        let Some(i) = turbonop_powerpc::decode(&code[at..], big, &features) else {
            break;
        };
        out.push((at, i));
        at += usize::from(i.size);
    }
    out
}

/// MIPS code's instructions, its words big-endian (`big`) or not, with
/// their offsets, to the first that doesn't decode.
fn mips(
    arch: Arch,
    mode: turbonop_code::Mode,
    code: &[u8],
) -> Vec<(usize, turbonop_mips::Instruction)> {
    let features = turbonop_code::mips_features_in(arch, mode);
    let big = mode.big;
    let mut out = Vec::new();
    let mut at = 0;
    while let Some(i) = code
        .get(at..)
        .and_then(|c| turbonop_mips::decode(c, big, &features))
    {
        let n = turbonop_mips::size(&i);
        out.push((at, i));
        at += n;
    }
    out
}

/// An integer register's number (RISC-V's, LoongArch's, PowerPC's, and
/// MIPS's tables all number theirs file 0).
fn gpr(o: turbonop_riscv::Operand) -> Option<u8> {
    o.reg().filter(|r| r.file == 0).map(|r| r.n)
}

/// A RISC-V or LoongArch instruction: its operation and operands; if
/// `masked` (it holds part of an address), its operation and registers
/// alone.
fn table_instruction(h: &mut FxHasher, i: &turbonop_riscv::Instruction, masked: bool) {
    h.write(i.name().as_bytes());
    for op in i.operands() {
        match op {
            turbonop_riscv::Operand::Reg(r) => {
                h.write_u8(b'r');
                table_reg(h, *r);
            }
            turbonop_riscv::Operand::Imm(v) => {
                h.write_u8(b'i');
                if !masked {
                    h.write_i64(*v);
                }
            }
        }
    }
}

fn table_reg(h: &mut FxHasher, r: turbonop_riscv::Reg) {
    h.write_u8(r.file);
    h.write_u8(r.n);
}

/// 32-bit ARM Thumb-2 code's instructions, with their offsets, to the
/// first that doesn't decode.
fn thumb(code: &[u8]) -> impl Iterator<Item = (usize, turbonop_arm32::Instruction)> + '_ {
    let half = |at: usize| {
        code.get(at..at + 2)
            .map(|b| u16::from_le_bytes([b[0], b[1]]))
    };
    let mut at = 0usize;
    std::iter::from_fn(move || {
        let first = half(at)?;
        let second = if turbonop_arm32::is_t32_wide(first) {
            Some(half(at + 2)?)
        } else {
            None
        };
        let i = turbonop_arm32::decode_t32(first, second)?;
        let off = at;
        at += usize::from(i.size);
        Some((off, i))
    })
}

/// Whether a 32-bit ARM instruction holds (part of) an address: a
/// branch's or literal's offset, `ADR`'s, or `MOVW`'s and `MOVT`'s halves.
fn addresses(i: &turbonop_arm32::Instruction) -> bool {
    let n = i.encoding.name;
    n.starts_with("B_")
        || n.starts_with("BL_i")
        || n.starts_with("CBZ")
        || n.starts_with("CBNZ")
        || n.starts_with("LDR_l")
        || n.starts_with("ADR")
        || n == "MOV_i_T3"
        || n.starts_with("MOVT")
}

/// A 32-bit ARM instruction's registers (its `Rd`, `Rn`, `Rt`, and `Rm`
/// fields), into `h`.
fn arm32_registers(h: &mut FxHasher, i: &turbonop_arm32::Instruction) {
    for name in ["Rd", "Rdn", "Rn", "Rt", "Rm"] {
        h.write_u32(i.field(name).unwrap_or(u32::MAX));
    }
}

/// The references at `pc`, advancing `r` past them (`refs` is in
/// instruction order).
fn take_refs<'a>(refs: &'a [CodeRef], r: &mut usize, pc: u64) -> &'a [CodeRef] {
    while *r < refs.len() && refs[*r].from < pc {
        *r += 1;
    }
    let first = *r;
    while *r < refs.len() && refs[*r].from == pc {
        *r += 1;
    }
    &refs[first..*r]
}

/// Hashes an x86 instruction: its operation and operands, with
/// displacements and immediates that may be addresses left out if
/// `masked` (the instruction refers to something).
fn x86_instruction(h: &mut FxHasher, inst: &turbonop_x86::Instruction, masked: bool) {
    use turbonop_x86::Operand;
    h.write_u16(inst.mnemonic() as u16);
    h.write_u16(inst.prefixes());
    h.write_u8(inst.mask().unwrap_or(0xFF));
    h.write_u8(u8::from(inst.zeroing()));
    h.write_u8(inst.vector_length());
    let reg = |h: &mut FxHasher, r: Option<turbonop_x86::Register>| match r {
        Some(r) => {
            h.write_u8(r.class() as u8);
            h.write_u8(r.index());
        }
        None => h.write_u8(0xFF),
    };
    for op in inst.operands() {
        match op {
            Operand::Reg(r) => {
                h.write_u8(b'r');
                reg(h, Some(*r));
            }
            Operand::Imm(i) => {
                h.write_u8(b'i');
                h.write_u8(i.size);
                if !masked {
                    h.write_u64(i.value);
                }
            }
            Operand::Mem(m) => {
                h.write_u8(b'm');
                reg(h, m.segment);
                reg(h, m.base);
                reg(h, m.index);
                h.write_u8(m.scale);
                h.write_u8(m.size as u8);
                h.write_u8(m.address_size);
                h.write_u8(m.broadcast);
                // (A RIP-relative displacement depends on where the code is,
                // whatever it reaches.)
                if !masked && !m.is_rip_relative() {
                    h.write_i64(m.disp);
                }
            }
            Operand::Rel(_) => h.write_u8(b'l'),
            Operand::Far {
                segment,
                offset,
                offset_size,
            } => {
                h.write_u8(b'f');
                h.write_u16(*segment);
                h.write_u32(*offset);
                h.write_u8(*offset_size);
            }
            Operand::None => h.write_u8(b'-'),
            // (Operand kinds yet to come: their kind, not their value.)
            _ => h.write_u8(b'?'),
        }
    }
}

/// The bits of an ARM64 instruction that hold an address, for `encoding`:
/// always a PC-relative one (a branch's or literal's offset, `ADR`'s, a
/// page), which depends on where the code is; a page offset (`ADD`, a
/// load or store after `ADRP`) if the instruction makes a reference
/// (`refers`).
fn arm64_address_bits(encoding: &str, refers: bool) -> u32 {
    const IMM19: u32 = 0x7FFFF << 5;
    const IMMLO: u32 = 3 << 29;
    const IMM12: u32 = 0xFFF << 10;
    match encoding {
        "ADRP_only_pcreladdr" | "ADR_only_pcreladdr" => IMM19 | IMMLO,
        "BL_only_branch_imm" | "B_only_branch_imm" => 0x3FF_FFFF,
        "TBZ_only_testbranch" | "TBNZ_only_testbranch" => 0x3FFF << 5,
        e if e.ends_with("condbranch") || e.ends_with("_compbranch") || e.ends_with("_loadlit") => {
            IMM19
        }
        e if refers && (e.ends_with("ADD_64_addsub_imm") || e.ends_with("_ldst_pos")) => IMM12,
        _ => 0,
    }
}

/// A function's shape: its instructions with what makes one instantiation
/// of a template differ from another left out (every constant and
/// displacement, operand widths and scales, and what it refers to), so
/// that two of one shape differ only in constants, the sizes of what they
/// work on, or what they call. `None` for code no decoder here reads.
///
/// - x86: each instruction's operation (prefixes aside) and its operands'
///   kinds, with their registers' numbers but not their widths (`eax` and
///   `rax` alike, `xmm0` and `ymm0` alike).
/// - ARM64: each instruction's encoding less its width (`ADD_64_addsub_imm`
///   and `ADD_32_addsub_imm` alike), and its first two register fields.
/// - 32-bit ARM (Thumb-2): each instruction's operation (its encoding less
///   the encoding's number: `ADD_i_T1` and `ADD_i_T3` alike, 16 bits wide
///   or 32), and its registers.
/// - RISC-V: each instruction's operation (a compressed one's full form's:
///   `c.addi` and `addi` alike) and its registers.
pub fn shape(arch: Arch, mode: turbonop_code::Mode, code: &[u8], start: u64) -> Option<u64> {
    let big = mode.big;
    let mut h = hasher(b's');
    let mut count = 0usize;
    match arch {
        Arch::X86 | Arch::X64 => {
            use turbonop_x86::Operand;
            let mode = if arch == Arch::X86 {
                turbonop_x86::Mode::Bits32
            } else {
                turbonop_x86::Mode::Bits64
            };
            let reg = |h: &mut FxHasher, r: Option<turbonop_x86::Register>| {
                h.write_u8(r.map_or(0xFF, turbonop_x86::Register::index));
            };
            let mut at = 0usize;
            while at < code.len() {
                let pc = start + at as u64;
                let Ok(inst) = turbonop_x86::decode(&code[at..], mode, pc) else {
                    h.write_u8(0xFF);
                    break;
                };
                h.write_u16(inst.mnemonic() as u16);
                for op in inst.operands() {
                    match op {
                        Operand::Reg(r) => {
                            h.write_u8(b'r');
                            reg(&mut h, Some(*r));
                        }
                        Operand::Imm(_) => h.write_u8(b'i'),
                        Operand::Mem(m) => {
                            h.write_u8(b'm');
                            reg(&mut h, m.base);
                            reg(&mut h, m.index);
                        }
                        Operand::Rel(_) => h.write_u8(b'l'),
                        Operand::None => h.write_u8(b'-'),
                        _ => h.write_u8(b'?'),
                    }
                }
                count += 1;
                at += inst.len().max(1);
            }
        }
        Arch::Arm64 => {
            for (i, w) in code.as_chunks::<4>().0.iter().enumerate() {
                let word = u32::from_le_bytes(*w);
                match turbonop_arm64::decode(word, start + i as u64 * 4) {
                    Some(inst) => {
                        for part in inst.encoding_name().split('_') {
                            if part != "32" && part != "64" {
                                h.write(part.as_bytes());
                            }
                            h.write_u8(b'_');
                        }
                        h.write_u32(word & 0x3FF);
                    }
                    None => h.write_u8(0xFF),
                }
                count += 1;
            }
        }
        Arch::Arm => {
            for (_, inst) in thumb(code) {
                let n = inst.encoding.name;
                // (`ADD_i_T3`: `ADD_i`.)
                let op = n.rsplit_once('_').map_or(n, |(op, _)| op);
                h.write(op.as_bytes());
                arm32_registers(&mut h, &inst);
                count += 1;
            }
        }
        Arch::RiscV32
        | Arch::RiscV64
        | Arch::LoongArch32
        | Arch::LoongArch64
        | Arch::PowerPc
        | Arch::PowerPc64
        | Arch::Mips
        | Arch::Mips64 => {
            let insts = match arch {
                Arch::RiscV32 | Arch::RiscV64 => riscv(arch, code),
                Arch::LoongArch32 | Arch::LoongArch64 => loongarch(arch, code),
                Arch::Mips | Arch::Mips64 => mips(arch, mode, code),
                _ => powerpc(arch, big, code),
            };
            for (_, i) in insts {
                h.write(i.name().as_bytes());
                for op in i.operands() {
                    if let turbonop_riscv::Operand::Reg(r) = op {
                        table_reg(&mut h, *r);
                    }
                }
                count += 1;
            }
        }
        _ => return None,
    }
    h.write_usize(count);
    Some(h.finish())
}

/// The class of each item: its local hash and its references' classes,
/// rehashed until the number of classes stops growing (at most `rounds`
/// times). Each round's class includes the last's, so classes only split.
pub fn refine(items: &[Item], rounds: usize) -> Vec<u64> {
    let mut classes: Vec<u64> = items.iter().map(|i| i.local).collect();
    let mut count = distinct(&classes);
    for _ in 0..rounds {
        // (Each item's next class from this round's: on every core.)
        let next: Vec<u64> = turbonop_par::map_range(items.len(), 16 * 1024, |i| {
            let item = &items[i];
            let mut h = hasher(b'r');
            h.write_u64(classes[i]);
            h.write_usize(item.refs.len());
            for &r in &item.refs {
                h.write_u64(classes.get(r as usize).copied().unwrap_or(0));
            }
            h.finish()
        });
        let n = distinct(&next);
        classes = next;
        if n == count {
            break;
        }
        count = n;
    }
    classes
}

fn distinct(v: &[u64]) -> usize {
    let mut s: Vec<u64> = v.to_vec();
    turbonop_par::sort_by(&mut s, u64::cmp);
    s.dedup();
    s.len()
}

#[cfg(test)]
mod tests;
