//! Comparing a function's or a datum's bytes in two images, modulo the
//! addresses in them.
//!
//! A linked image has no relocations left, so the fields that held them
//! are found again and compared by what they name (see
//! [`program`](super::program): a label and an offset) rather than by
//! value:
//!
//! - **Absolute addresses**, by the base relocations (`HIGHLOW`,
//!   `DIR64`): exact. Both images must have them at the same offsets.
//! - **PC-relative operands in code**, by decoding: x86-64's relative
//!   branches and calls and RIP-relative memory operands; ARM64's
//!   branches (`B`, `BL`, `B.cond`, `CBZ`, `TBZ`, ...), `ADR`, literal
//!   loads, and `ADRP` with the `ADD` or load and store that completes its
//!   address. The two instructions' other bytes must be equal, and both
//!   decodings must find the field at the same place.
//! - **Image-relative addresses** (RVAs, which nothing relocates: jump
//!   tables, unwind data, RTTI), by inference: where the bytes still
//!   differ, each 4-byte aligned word holding a difference is read as an
//!   RVA in both images, and accepted if both name the same thing. This
//!   can't prove the word is an RVA; it shows the two words agree if they
//!   are, and differing bytes that aren't two such RVAs are caught.
//!
//! Inside a procedure's code (as the PDB gives its extent), every
//! PC-relative operand's targets must name the same thing even where the
//! bytes are equal; outside, bytes that decode as instructions may be
//! data, and equal bytes are accepted.

use crate::report::{Report, show};

/// What the comparison needs to know of one image.
pub(crate) trait Names {
    /// The name of `rva` (a label and the distance from it), if it is in
    /// the image.
    fn name(&self, rva: u32) -> Option<(&[u8], u32)>;
    /// Whether `rva` is in a procedure's code.
    fn in_code(&self, rva: u32) -> bool;
    /// The image's base address.
    fn image_base(&self) -> u64;
    /// The base relocations in `[rva, rva + len)`: (RVA, size).
    fn relocations_in(&self, rva: u32, len: u32) -> &[(u32, u8)];
    /// Every symbol's name at the address `rva`'s label starts at: more
    /// than one where the linker folded identical functions (`/OPT:ICF`)
    /// or a symbol has aliases.
    fn aliases(&self, rva: u32) -> &[Box<[u8]>] {
        let _ = rva;
        &[]
    }
    /// The name of a thread-pointer offset (an ELF program's TLS
    /// variable, as local-exec code addresses it), if it is one.
    fn tls(&self, tpoff: i64) -> Option<(&[u8], u32)> {
        let _ = tpoff;
        None
    }
    /// The targets of a jump table at `rva`, if it is one that no label
    /// names (entries relative to the table, as GCC and clang write them
    /// for x86-64): its bytes follow the layout, its targets don't.
    fn jump_table(&self, rva: u32) -> Option<Vec<u32>> {
        let _ = rva;
        None
    }
}

/// Whether two addresses (RVAs, if in the images) name the same thing: the
/// same label and offset, or the same offset from addresses that share a
/// name (identical functions folded together hold several, and which one
/// labels the address follows which functions a link keeps).
pub(crate) fn same<A: Names + ?Sized, B: Names + ?Sized>(
    a: &A,
    ra: Option<u32>,
    b: &B,
    rb: Option<u32>,
) -> bool {
    let na = ra.and_then(|r| a.name(r));
    let nb = rb.and_then(|r| b.name(r));
    if na == nb {
        return true;
    }
    match (na, nb, ra, rb) {
        (Some((_, oa)), Some((_, ob)), Some(ra), Some(rb)) if oa == ob => {
            let (xa, xb) = (a.aliases(ra), b.aliases(rb));
            xa.iter().any(|x| xb.contains(x)) || same_jump_table(a, ra, b, rb)
        }
        (_, _, Some(ra), Some(rb)) => same_jump_table(a, ra, b, rb),
        _ => false,
    }
}

/// Whether `ra` and `rb` are jump tables whose entries name the same
/// targets.
fn same_jump_table<A: Names + ?Sized, B: Names + ?Sized>(a: &A, ra: u32, b: &B, rb: u32) -> bool {
    match (a.jump_table(ra), b.jump_table(rb)) {
        (Some(ta), Some(tb)) => {
            ta.len() == tb.len()
                && ta
                    .iter()
                    .zip(&tb)
                    .all(|(&x, &y)| a.name(x).is_some() && a.name(x) == b.name(y))
        }
        _ => false,
    }
}

/// One side of a comparison: an image, a region's address, and its bytes.
pub(crate) struct Side<'s, I: ?Sized> {
    pub image: &'s I,
    pub rva: u32,
    pub bytes: &'s [u8],
}

/// The instruction set to decode a region of code with.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Isa {
    X64,
    Arm64,
    /// No decoding: the region is data.
    Data,
}

/// Counts of how the differences were explained.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct Fields {
    pub absolute: u64,
    pub relative: u64,
    pub inferred: u64,
    /// Thread-pointer offsets (x86-64 local-exec TLS) naming the same
    /// variable.
    pub tls: u64,
}

fn word(b: &[u8], at: usize, size: usize) -> u64 {
    let mut v = [0u8; 8];
    v[..size].copy_from_slice(&b[at..at + size]);
    u64::from_le_bytes(v)
}

fn render(n: Option<(&[u8], u32)>) -> String {
    match n {
        Some((name, 0)) => show(name),
        Some((name, off)) => format!("{}+{off:#x}", show(name)),
        None => "nothing".into(),
    }
}

/// Compares two regions of equal length (see the module's
/// documentation). Failures are reported under `what`'s name.
pub(crate) fn compare<A: Names + ?Sized, B: Names + ?Sized>(
    a: &Side<'_, A>,
    b: &Side<'_, B>,
    isa: Isa,
    what: &[u8],
    rep: &mut Report,
) -> Fields {
    let len = a.bytes.len().min(b.bytes.len());
    let mut fields = Fields::default();
    let mut masked = vec![false; len];
    let mut failed = false;
    let mut fail = |rep: &mut Report, category: &str, detail: String| {
        if !failed {
            rep.fail(category, || format!("{}: {detail}", show(what)));
        }
        failed = true;
    };

    // Absolute addresses.
    let ra: Vec<(u32, u8)> = a
        .image
        .relocations_in(a.rva, len as u32)
        .iter()
        .map(|&(r, s)| (r - a.rva, s))
        .collect();
    let rb: Vec<(u32, u8)> = b
        .image
        .relocations_in(b.rva, len as u32)
        .iter()
        .map(|&(r, s)| (r - b.rva, s))
        .collect();
    if ra == rb {
        for &(at, size) in &ra {
            let (at, size) = (at as usize, usize::from(size));
            if at + size > len {
                continue;
            }
            let va = word(a.bytes, at, size);
            let vb = word(b.bytes, at, size);
            let ra = va
                .checked_sub(a.image.image_base())
                .and_then(|r| u32::try_from(r).ok());
            let rb = vb
                .checked_sub(b.image.image_base())
                .and_then(|r| u32::try_from(r).ok());
            let na = ra.and_then(|r| a.image.name(r));
            let nb = rb.and_then(|r| b.image.name(r));
            if !same(a.image, ra, b.image, rb) {
                fail(
                    rep,
                    "absolute addresses naming other functions or data",
                    format!("at +{at:#x}: {} and {}", render(na), render(nb)),
                );
            }
            fields.absolute += 1;
            masked[at..at + size].fill(true);
        }
    } else {
        fail(
            rep,
            "functions or data with base relocations at other places",
            format!("{} and {} relocations", ra.len(), rb.len()),
        );
    }

    // PC-relative operands.
    match isa {
        Isa::X64 => x64(a, b, &mut masked, &mut fields, &mut |c, d| fail(rep, c, d)),
        Isa::Arm64 => arm64(a, b, &mut masked, &mut fields, &mut |c, d| fail(rep, c, d)),
        Isa::Data => {}
    }

    // What still differs: image-relative words, or a difference.
    let mut at = 0;
    while at < len {
        if masked[at] || a.bytes[at] == b.bytes[at] {
            at += 1;
            continue;
        }
        // The aligned word holding the difference, read as an RVA in both.
        let misaligned = (a.rva as usize + at) & 3;
        let same_alignment = (a.rva & 3) == (b.rva & 3);
        let fits = at >= misaligned
            && at - misaligned + 4 <= len
            && same_alignment
            && !masked[at - misaligned..at - misaligned + 4].contains(&true);
        let start = at.saturating_sub(misaligned);
        if fits {
            let wa = word(a.bytes, start, 4) as u32;
            let wb = word(b.bytes, start, 4) as u32;
            let (na, nb) = (a.image.name(wa), b.image.name(wb));
            if na.is_some() && nb.is_some() && same(a.image, Some(wa), b.image, Some(wb)) {
                fields.inferred += 1;
                masked[start..start + 4].fill(true);
                at = start + 4;
                continue;
            }
        }
        fail(
            rep,
            "functions or data whose bytes differ",
            format!("at +{at:#x}: {:#04x} and {:#04x}", a.bytes[at], b.bytes[at]),
        );
        break;
    }
    fields
}

/// The position of a `size`-byte little-endian field in both encodings
/// that holds `da` in `a` and `db` in `b` (sign-extended), searched from
/// the end, at or after `from`.
fn field(a: &[u8], b: &[u8], from: usize, size: usize, da: i64, db: i64) -> Option<usize> {
    let read = |x: &[u8], p: usize| -> i64 {
        let v = word(x, p, size);
        let shift = 64 - 8 * size as u32;
        ((v << shift) as i64) >> shift
    };
    (from..=a.len().checked_sub(size)?)
        .rev()
        .find(|&p| read(a, p) == da && read(b, p) == db)
}

fn x64<A: Names + ?Sized, B: Names + ?Sized>(
    a: &Side<'_, A>,
    b: &Side<'_, B>,
    masked: &mut [bool],
    fields: &mut Fields,
    fail: &mut dyn FnMut(&str, String),
) {
    use turbonop_x86::{Mode, Operand, decode};
    let len = masked.len();
    let base_a = a.image.image_base() + u64::from(a.rva);
    let base_b = b.image.image_base() + u64::from(b.rva);
    let mut at = 0;
    while at < len {
        let (Ok(ia), Ok(ib)) = (
            decode(&a.bytes[at..len], Mode::Bits64, base_a + at as u64),
            decode(&b.bytes[at..len], Mode::Bits64, base_b + at as u64),
        ) else {
            at += 1;
            continue;
        };
        let n = ia.len();
        if n == 0 || n != ib.len() || ia.mnemonic() != ib.mnemonic() {
            at += 1;
            continue;
        }
        let (ea, eb) = (&a.bytes[at..at + n], &b.bytes[at..at + n]);
        let checked = a.image.in_code(a.rva + at as u32);
        for (oa, ob) in ia.operands().iter().zip(ib.operands()) {
            // (target, the field's encoded value, its possible sizes)
            let (ta, tb, sizes): (u64, u64, &[usize]) = match (oa, ob) {
                (Operand::Rel(ta), Operand::Rel(tb)) => (*ta, *tb, &[4, 1, 2]),
                (Operand::Mem(ma), Operand::Mem(mb))
                    if ma.is_rip_relative() && mb.is_rip_relative() =>
                {
                    (
                        ia.next_address().wrapping_add(ma.disp as u64),
                        ib.next_address().wrapping_add(mb.disp as u64),
                        &[4],
                    )
                }
                _ => continue,
            };
            let da = ta.wrapping_sub(ia.next_address()) as i64;
            let db = tb.wrapping_sub(ib.next_address()) as i64;
            let Some((p, size)) = sizes
                .iter()
                .find_map(|&s| field(ea, eb, 1, s, da, db).map(|p| (p, s)))
            else {
                continue;
            };
            let name = |img: &dyn Fn(u32) -> Option<(Vec<u8>, u32)>, t: u64, base: u64| {
                t.checked_sub(base)
                    .and_then(|r| u32::try_from(r).ok())
                    .and_then(img)
            };
            let na = name(
                &|r| a.image.name(r).map(|(n, o)| (n.to_vec(), o)),
                ta,
                a.image.image_base(),
            );
            let nb = name(
                &|r| b.image.name(r).map(|(n, o)| (n.to_vec(), o)),
                tb,
                b.image.image_base(),
            );
            if ea == eb && !checked {
                // Equal bytes outside a procedure: perhaps data.
            } else if !same(
                a.image,
                ta.checked_sub(a.image.image_base())
                    .and_then(|r| u32::try_from(r).ok()),
                b.image,
                tb.checked_sub(b.image.image_base())
                    .and_then(|r| u32::try_from(r).ok()),
            ) {
                if ea != eb || checked {
                    fail(
                        "code whose operands name other functions or data",
                        format!(
                            "at +{at:#x}: {} and {}",
                            render(na.as_ref().map(|(n, o)| (n.as_slice(), *o))),
                            render(nb.as_ref().map(|(n, o)| (n.as_slice(), *o)))
                        ),
                    );
                }
            } else {
                fields.relative += 1;
            }
            masked[at + p..at + p + size].fill(true);
        }
        // Thread-pointer offsets: displacements and immediates that name
        // the same TLS variable (another TLS layout moves them all).
        if ea != eb {
            for (oa, ob) in ia.operands().iter().zip(ib.operands()) {
                let (va, vb, size) = match (oa, ob) {
                    (Operand::Mem(ma), Operand::Mem(mb))
                        if !ma.is_rip_relative()
                            && matches!(ma.disp_size, 1 | 4)
                            && ma.disp_size == mb.disp_size =>
                    {
                        (ma.disp, mb.disp, usize::from(ma.disp_size))
                    }
                    (Operand::Imm(xa), Operand::Imm(xb)) if xa.size == 4 && xb.size == 4 => (
                        i64::from(xa.value as u32 as i32),
                        i64::from(xb.value as u32 as i32),
                        4,
                    ),
                    // (An imm32 sign-extended to 64 bits.)
                    (Operand::Imm(xa), Operand::Imm(xb)) if xa.size == 8 && xb.size == 8 => {
                        (xa.value as i64, xb.value as i64, 4)
                    }
                    (Operand::Imm(xa), Operand::Imm(xb)) if xa.size == 1 && xb.size == 1 => (
                        i64::from(xa.value as u8 as i8),
                        i64::from(xb.value as u8 as i8),
                        1,
                    ),
                    _ => continue,
                };
                if va == vb {
                    continue;
                }
                let (Some(na), Some(nb)) = (a.image.tls(va), b.image.tls(vb)) else {
                    continue;
                };
                if na != nb {
                    continue;
                }
                if let Some(p) = field(ea, eb, 1, size, va, vb)
                    && !masked[at + p..at + p + size].contains(&true)
                {
                    masked[at + p..at + p + size].fill(true);
                    fields.tls += 1;
                }
            }
        }
        at += n;
    }
}

/// An ARM64 instruction's PC-relative field: the mask of its bits.
fn arm64_label_mask(encoding: &str) -> Option<u32> {
    let e = encoding;
    if e.starts_with("B_only_branch_imm") || e.starts_with("BL_only_branch_imm") {
        Some(0x03FF_FFFF)
    } else if e.starts_with("ADR_only") || e.starts_with("ADRP_only") {
        Some(0x60FF_FFE0)
    } else if e.starts_with("TBZ_") || e.starts_with("TBNZ_") {
        Some(0x0007_FFE0)
    } else if e.starts_with("B_only_condbranch")
        || e.starts_with("BC_only_condbranch")
        || e.starts_with("CBZ_")
        || e.starts_with("CBNZ_")
        || e.contains("_loadlit")
    {
        Some(0x00FF_FFE0)
    } else {
        None
    }
}

/// The bits of an ADD (immediate) or a load or store's unsigned offset.
const ARM64_IMM12: u32 = 0x003F_FC00;

fn arm64<A: Names + ?Sized, B: Names + ?Sized>(
    a: &Side<'_, A>,
    b: &Side<'_, B>,
    masked: &mut [bool],
    fields: &mut Fields,
    fail: &mut dyn FnMut(&str, String),
) {
    use turbonop_arm64::{AddrMode, OperandKind, decode};
    let words = masked.len() / 4;
    let w = |x: &[u8], i: usize| {
        u32::from_le_bytes([x[4 * i], x[4 * i + 1], x[4 * i + 2], x[4 * i + 3]])
    };
    let va_a = |i: usize| a.image.image_base() + u64::from(a.rva) + 4 * i as u64;
    let va_b = |i: usize| b.image.image_base() + u64::from(b.rva) + 4 * i as u64;
    let name_a = |t: u64| {
        t.checked_sub(a.image.image_base())
            .and_then(|r| u32::try_from(r).ok())
            .and_then(|r| a.image.name(r))
            .map(|(n, o)| (n.to_vec(), o))
    };
    let name_b = |t: u64| {
        t.checked_sub(b.image.image_base())
            .and_then(|r| u32::try_from(r).ok())
            .and_then(|r| b.image.name(r))
            .map(|(n, o)| (n.to_vec(), o))
    };
    let mut check = |at: usize, ta: u64, tb: u64, mask: u32, equal: bool, checked: bool| {
        let (na, nb) = (name_a(ta), name_b(tb));
        let rva = |t: u64, base: u64| t.checked_sub(base).and_then(|r| u32::try_from(r).ok());
        if same(
            a.image,
            rva(ta, a.image.image_base()),
            b.image,
            rva(tb, b.image.image_base()),
        ) {
            fields.relative += 1;
        } else if !equal || checked {
            fail(
                "code whose operands name other functions or data",
                format!(
                    "at +{:#x}: {} and {}",
                    4 * at,
                    render(na.as_ref().map(|(n, o)| (n.as_slice(), *o))),
                    render(nb.as_ref().map(|(n, o)| (n.as_slice(), *o)))
                ),
            );
        }
        let _ = (at, mask);
    };
    // Marks a word's field bytes as explained.
    let mark = |masked: &mut [bool], at: usize, mask: u32| {
        for (k, byte) in mask.to_le_bytes().iter().enumerate() {
            if *byte != 0 {
                masked[4 * at + k] = true;
            }
        }
    };
    let mut i = 0;
    while i < words {
        let (wa, wb) = (w(a.bytes, i), w(b.bytes, i));
        let (Some(ia), Some(ib)) = (decode(wa, va_a(i)), decode(wb, va_b(i))) else {
            i += 1;
            continue;
        };
        let checked = a.image.in_code(a.rva + 4 * i as u32);
        let encoding = ia.encoding_name();
        if encoding != ib.encoding_name() {
            i += 1;
            continue;
        }
        let Some(mask) = arm64_label_mask(encoding) else {
            i += 1;
            continue;
        };
        if wa & !mask != wb & !mask {
            i += 1;
            continue;
        }
        let label = |ins: &turbonop_arm64::Instruction| {
            ins.operands().iter().find_map(|o| match o.kind {
                OperandKind::Label(t) => Some(t),
                _ => None,
            })
        };
        let (Some(ta), Some(tb)) = (label(&ia), label(&ib)) else {
            i += 1;
            continue;
        };
        if !encoding.starts_with("ADRP_only") {
            check(i, ta, tb, mask, wa == wb, checked);
            mark(masked, i, mask);
            i += 1;
            continue;
        }
        // ADRP: the page, completed by the next ADD or load or store whose
        // base is its register.
        let rd = wa & 0x1F;
        let mut completed = false;
        for j in i + 1..(i + 9).min(words) {
            let (ja, jb) = (w(a.bytes, j), w(b.bytes, j));
            let (Some(xa), Some(xb)) = (decode(ja, va_a(j)), decode(jb, va_b(j))) else {
                break;
            };
            if xa.encoding_name() != xb.encoding_name() || ja & !ARM64_IMM12 != jb & !ARM64_IMM12 {
                break;
            }
            let n = xa.encoding_name();
            let base = (ja >> 5) & 0x1F;
            let offset = |x: &turbonop_arm64::Instruction| -> Option<i64> {
                x.operands().iter().find_map(|o| match o.kind {
                    OperandKind::Mem(m) if m.mode == AddrMode::Offset && m.index.is_none() => {
                        Some(m.offset)
                    }
                    _ => None,
                })
            };
            let add = n.starts_with("ADD_64_addsub_imm") && base == rd;
            let ldst = base == rd && (n.contains("_ldst_pos"));
            if add || ldst {
                let (oa, ob) = if add {
                    let imm = |x: u32| {
                        i64::from((x >> 10) & 0xFFF) << if x & (1 << 22) != 0 { 12 } else { 0 }
                    };
                    (imm(ja), imm(jb))
                } else {
                    match (offset(&xa), offset(&xb)) {
                        (Some(oa), Some(ob)) => (oa, ob),
                        _ => break,
                    }
                };
                check(
                    i,
                    ta.wrapping_add(oa as u64),
                    tb.wrapping_add(ob as u64),
                    mask,
                    wa == wb && ja == jb,
                    checked,
                );
                mark(masked, i, mask);
                mark(masked, j, ARM64_IMM12);
                completed = true;
                break;
            }
            if xa
                .written_registers()
                .iter()
                .any(|r| r.id() == Some(rd as u8))
            {
                break;
            }
        }
        if !completed {
            // A page alone: names the page's start.
            check(i, ta, tb, mask, wa == wb, checked);
            mark(masked, i, mask);
        }
        i += 1;
    }
}

#[cfg(test)]
mod tests;
