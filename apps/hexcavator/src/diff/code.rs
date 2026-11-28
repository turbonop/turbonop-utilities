//! Comparing two copies of a function by their instructions, decoded
//! (x64, ARM64): the same instructions, but for where they refer to
//! things, and those the same things by name. Exact where comparing bytes
//! guesses: a relative displacement's instruction ends where it does (an
//! immediate after it), not at the field's end; ARM64's branches, `ADR`,
//! literal loads, and `ADRP` pairs (with the `ADD`, load, or store that
//! completes the address) are references too.

use super::image::{Same, Side};

/// What an address in each image names, for [`compare`]: `Some(Some(n))`
/// its names (and the offset into what has them), `Some(None)` inside
/// the image but named in only one, `None` outside it.
type Named = Option<Option<(Vec<String>, u32)>>;

/// Whether two addresses name the same thing; `None` if one is named in
/// only one image (so it can't be told), `Some(false)` if they differ.
fn same_place(x: &Named, y: &Named) -> Option<bool> {
    match (x, y) {
        (Some(Some(tx)), Some(Some(ty))) => {
            Some(tx.1 == ty.1 && tx.0.iter().any(|n| ty.0.contains(n)))
        }
        (Some(tx), Some(ty)) if tx.is_none() || ty.is_none() => None,
        _ => Some(false),
    }
}

/// The references each instruction makes (by its offset in the copy):
/// what the xref scan finds (ARM64's `ADRP` pairs at the instruction that
/// completes them; x64's RIP-relative operands, MSVC's tables read from
/// `__ImageBase`).
fn references(
    arch: turbonop_core::Arch,
    code: &[u8],
    at: u32,
) -> turbonop_hash::Map<u32, Vec<u64>> {
    let mut refs = Vec::new();
    turbonop_xref::scan_image(
        arch,
        turbonop_xref::Mode::default(),
        code,
        u64::from(at),
        Some(0),
        &mut refs,
    );
    let mut by: turbonop_hash::Map<u32, Vec<u64>> = turbonop_hash::Map::default();
    for r in refs {
        if let Some(o) = r
            .from
            .checked_sub(u64::from(at))
            .and_then(|o| u32::try_from(o).ok())
        {
            by.entry(o).or_default().push(r.to);
        }
    }
    by
}

/// What an instruction refers to: where, and for a memory access, how
/// many bytes, in elements of `step` (an `LDP`'s two registers' worth:
/// two adjacent globals, perhaps).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Extent {
    at: u64,
    len: u64,
    step: u64,
}

impl Extent {
    /// A place (a branch's target, an address computed).
    fn point(at: u64) -> Extent {
        Extent {
            at,
            len: 1,
            step: 1,
        }
    }
}

/// Explains a differing instruction's references: each pair of targets
/// the same place, over all of what is accessed (each element's start,
/// and the last byte: a load of two adjacent globals must load the same
/// two). `Some(unnamed)` if so (whether one was named in only one
/// image), `None` if not.
fn targets(a: &Side<'_>, b: &Side<'_>, tx: &[Extent], ty: &[Extent]) -> Option<bool> {
    targets_by(&|at| a.named(at), &|at| b.named(at), tx, ty)
}

/// [`targets`], naming addresses with `na` and `nb`.
fn targets_by(
    na: &dyn Fn(i64) -> Named,
    nb: &dyn Fn(i64) -> Named,
    tx: &[Extent],
    ty: &[Extent],
) -> Option<bool> {
    if tx.is_empty() || tx.len() != ty.len() {
        return None;
    }
    let mut unnamed = false;
    for (p, q) in tx.iter().zip(ty) {
        if p.len != q.len || p.step != q.step {
            return None;
        }
        let step = p.step.max(1);
        let probes = (0..p.len.div_ceil(step))
            .map(|k| k * step)
            .chain((p.len > 1).then(|| p.len - 1));
        for d in probes {
            let (np, nq) = (
                na(i64::try_from(p.at.checked_add(d)?).ok()?),
                nb(i64::try_from(q.at.checked_add(d)?).ok()?),
            );
            match same_place(&np, &nq) {
                Some(true) => {}
                None => unnamed = true,
                Some(false) => return None,
            }
        }
    }
    Some(unnamed)
}

/// How much an ARM64 load or store accesses (its transfer registers'
/// width, a narrow form's, times their count): `None` if it has no
/// memory operand, or registers whose width it doesn't say (vector
/// lists: a byte, then).
fn arm64_access(i: &turbonop_arm64::Instruction) -> Option<(u64, u64)> {
    use turbonop_arm64::{OperandKind as K, Register};
    // (Operands' kinds and registers alone: where it is doesn't matter.)
    let ops = i.detailed_operands(0);
    let mem = ops
        .iter()
        .position(|o| matches!(o.kind, K::Memory(_) | K::Label(_)))?;
    let name = i.encoding_name();
    let narrow = [
        ("LDRSB", 1),
        ("LDRSH", 2),
        ("LDRSW", 4),
        ("LDRB", 1),
        ("LDRH", 2),
        ("STRB", 1),
        ("STRH", 2),
        ("LDURSB", 1),
        ("LDURSH", 2),
        ("LDURSW", 4),
        ("LDURB", 1),
        ("LDURH", 2),
        ("STURB", 1),
        ("STURH", 2),
        ("LDARB", 1),
        ("LDARH", 2),
        ("STLRB", 1),
        ("STLRH", 2),
        ("LDAPRB", 1),
        ("LDAPRH", 2),
        ("LDXRB", 1),
        ("LDXRH", 2),
        ("STXRB", 1),
        ("STXRH", 2),
        ("LDPSW", 4),
    ]
    .iter()
    .find(|(m, _)| name.starts_with(&format!("{m}_")))
    .map(|&(_, n)| n);
    let mut count = 0u64;
    let mut width = 0u64;
    for o in &ops[..mem] {
        let K::Register(r) = o.kind else { continue };
        let w = match r.register {
            Register::X(_) | Register::Xzr | Register::D(_) => 8,
            Register::W(_) | Register::Wzr | Register::S(_) => 4,
            Register::Q(_) => 16,
            Register::H(_) => 2,
            Register::B(_) => 1,
            _ => return Some((1, 1)),
        };
        count += 1;
        width = w;
    }
    if count == 0 {
        return None;
    }
    // (A store-exclusive's status register comes first: not transferred.)
    if name.starts_with("STXR") || name.starts_with("STLXR") {
        count = count.saturating_sub(1).max(1);
    }
    let step = narrow.unwrap_or(width);
    Some((step * count, step))
}

/// Whether `x` and `y` differ in one run of at most `width` bytes.
fn one_field(x: &[u8], y: &[u8], width: usize) -> bool {
    let first = x.iter().zip(y).position(|(p, q)| p != q);
    let last = x.iter().zip(y).rposition(|(p, q)| p != q);
    match (first, last) {
        (Some(f), Some(l)) => l - f < width,
        _ => true,
    }
}

/// Compares the copies `x` (in `a`, at `ra`) and `y` (in `b`, at `rb`) of
/// a function by their decoded instructions: `None` where they can't be
/// (not x64's or ARM64's code, or bytes that don't decode alike), for the
/// byte comparison.
pub(super) fn compare(
    a: &Side<'_>,
    ra: u32,
    x: &[u8],
    b: &Side<'_>,
    rb: u32,
    y: &[u8],
) -> Option<Same> {
    use turbonop_pe::consts::machine;
    let arch = match a.pe.machine() {
        machine::AMD64 => turbonop_core::Arch::X64,
        machine::ARM64 => turbonop_core::Arch::Arm64,
        _ => return None,
    };
    if b.pe.machine() != a.pe.machine() || x.len() != y.len() || !a.in_code(ra) || !b.in_code(rb) {
        return None;
    }
    let pad = |c: u8| c == 0xCC || c == 0;
    let (rx, ry) = (references(arch, x, ra), references(arch, y, rb));
    let (mut moved, mut unnamed, mut padded) = (false, false, false);
    // ARM64 `ADRP`s that differ (their offset, and the register they
    // write): explained by an instruction that completes the address from
    // that register, later, alike in both (over all it accesses).
    let mut pending: Vec<(usize, turbonop_arm64::Register)> = Vec::new();
    let mut o = 0usize;
    while o < x.len() {
        // The rest padding in both: the copies differ in their padding.
        if x[o..] != y[o..] && x[o..].iter().all(|&c| pad(c)) && y[o..].iter().all(|&c| pad(c)) {
            padded = true;
            break;
        }
        let o32 = u32::try_from(o).ok()?;
        let len = match arch {
            turbonop_core::Arch::X64 => {
                let ix = turbonop_x86::decode(
                    &x[o..],
                    turbonop_x86::Mode::Bits64,
                    u64::from(ra) + o as u64,
                )
                .ok()?;
                let iy = turbonop_x86::decode(
                    &y[o..],
                    turbonop_x86::Mode::Bits64,
                    u64::from(rb) + o as u64,
                )
                .ok()?;
                if ix.len() != iy.len() {
                    return Some(Same::Different(o));
                }
                let n = ix.len();
                if x[o..o + n] != y[o..o + n] {
                    match x64_alike(
                        a,
                        b,
                        &ix,
                        &iy,
                        &x[o..o + n],
                        &y[o..o + n],
                        rx.get(&o32),
                        ry.get(&o32),
                    ) {
                        Some(u) => {
                            moved = true;
                            unnamed |= u;
                        }
                        None => return Some(Same::Different(o)),
                    }
                }
                n
            }
            _ => {
                let (Some(wx), Some(wy)) = (x.get(o..o + 4), y.get(o..o + 4)) else {
                    // (A tail shorter than an instruction: as bytes.)
                    if x[o..] == y[o..] {
                        break;
                    }
                    return Some(Same::Different(o));
                };
                let w = |s: &[u8]| u32::from_le_bytes([s[0], s[1], s[2], s[3]]);
                let (ax, ay) = (u64::from(ra) + o as u64, u64::from(rb) + o as u64);
                let decoded = || {
                    Some((
                        turbonop_arm64::decode(w(wx)).ok()?,
                        turbonop_arm64::decode(w(wy)).ok()?,
                    ))
                };
                // An address completed from a pending ADRP's register, the
                // same in both (all it accesses): that ADRP is explained.
                if !pending.is_empty()
                    && let (Some(sx), Some(sy)) = (rx.get(&o32), ry.get(&o32))
                    && let Some((ix, _)) = decoded()
                {
                    let ex = arm64_extents(&ix, sx);
                    let ey = arm64_extents(&ix, sy);
                    if targets(a, b, &ex, &ey).is_some() {
                        pending.retain(|&(_, r)| !arm64_uses(&ix, r));
                    }
                }
                if wx != wy {
                    let (ix, iy) = decoded()?;
                    match arm64_alike(a, b, (&ix, ax), (&iy, ay), rx.get(&o32), ry.get(&o32)) {
                        Arm64::Same(u) => {
                            moved = true;
                            unnamed |= u;
                        }
                        Arm64::Page(r) => {
                            moved = true;
                            pending.push((o, r));
                        }
                        Arm64::Different => return Some(Same::Different(o)),
                    }
                }
                4
            }
        };
        o += len;
    }
    // An ADRP whose page differs and that nothing after it completed alike.
    if let Some(&(at, _)) = pending.first() {
        return Some(Same::Different(at));
    }
    Some(if unnamed {
        Same::Unnamed
    } else if padded {
        Same::Padding
    } else if moved {
        Same::Moved
    } else {
        Same::Identical
    })
}

/// Whether two x64 instructions whose bytes differ are the same but for
/// where they refer to things (one field of their encoding), and those
/// the same places: `Some(unnamed)` if so.
#[allow(clippy::too_many_arguments, reason = "both sides of each")]
fn x64_alike(
    a: &Side<'_>,
    b: &Side<'_>,
    ix: &turbonop_x86::Instruction,
    iy: &turbonop_x86::Instruction,
    bx: &[u8],
    by: &[u8],
    rx: Option<&Vec<u64>>,
    ry: Option<&Vec<u64>>,
) -> Option<bool> {
    use turbonop_x86::Operand;
    if ix.mnemonic() != iy.mnemonic()
        || ix.operands().len() != iy.operands().len()
        || !one_field(bx, by, 8)
    {
        return None;
    }
    let next = |i: &turbonop_x86::Instruction| i.address() + i.len() as u64;
    let (mut tx, mut ty) = (Vec::new(), Vec::new());
    let mut via_scan = false;
    // (Each target, the same or not: an instruction moved has the same
    // absolute one in a different encoding, and what's there must be the
    // same.)
    for (p, q) in ix.operands().iter().zip(iy.operands()) {
        match (p, q) {
            (Operand::Rel(u), Operand::Rel(v)) => {
                tx.push(Extent::point(*u));
                ty.push(Extent::point(*v));
            }
            _ if p == q
                && !matches!(p, Operand::Mem(m) if m.base.is_some_and(|r| r.class() == turbonop_x86::RegClass::Ip)) =>
                {}
            (Operand::Mem(m), Operand::Mem(n)) => {
                let rip = |m: &turbonop_x86::Memory| {
                    m.base
                        .is_some_and(|r| r.class() == turbonop_x86::RegClass::Ip)
                };
                let (mut m2, mut n2) = (*m, *n);
                m2.disp = 0;
                n2.disp = 0;
                if m2 != n2 {
                    return None;
                }
                if rip(m) {
                    tx.push(x64_extent(next(ix).wrapping_add_signed(m.disp), m));
                    ty.push(x64_extent(next(iy).wrapping_add_signed(n.disp), n));
                } else {
                    // (A table read from a register holding __ImageBase:
                    // the scan's reference says where.)
                    via_scan = true;
                }
            }
            (Operand::Imm(_), Operand::Imm(_)) => via_scan = true,
            _ => return None,
        }
    }
    if via_scan {
        // (What the scan found at this instruction, sized as its memory
        // operand accesses, if it has one.)
        let mem = ix.operands().iter().find_map(|o| match o {
            Operand::Mem(m) => Some(*m),
            _ => None,
        });
        let sized = |t: u64| mem.as_ref().map_or(Extent::point(t), |m| x64_extent(t, m));
        let (sx, sy) = (rx?, ry?);
        let new_x: Vec<Extent> = sx
            .iter()
            .map(|&t| sized(t))
            .filter(|e| !tx.iter().any(|x: &Extent| x.at == e.at))
            .collect();
        let new_y: Vec<Extent> = sy
            .iter()
            .map(|&t| sized(t))
            .filter(|e| !ty.iter().any(|y: &Extent| y.at == e.at))
            .collect();
        tx.extend(new_x);
        ty.extend(new_y);
    }
    targets(a, b, &tx, &ty)
}

/// A memory operand's access at `at`: its size (a byte if it says none:
/// `lea`), in elements of up to 8 bytes (a vector load can span globals).
fn x64_extent(at: u64, m: &turbonop_x86::Memory) -> Extent {
    let len = u64::from(m.size.bytes()).max(1);
    Extent {
        at,
        len,
        step: len.min(8),
    }
}

/// How two ARM64 instructions whose words differ compare.
enum Arm64 {
    /// The same but where they refer to things, the same places (and
    /// whether one was named in only one image).
    Same(bool),
    /// The same `ADRP` (writing this register) but for its page: the
    /// instruction that completes its address decides.
    Page(turbonop_arm64::Register),
    Different,
}

fn arm64_alike(
    a: &Side<'_>,
    b: &Side<'_>,
    (ix, ax): (&turbonop_arm64::Instruction, u64),
    (iy, ay): (&turbonop_arm64::Instruction, u64),
    rx: Option<&Vec<u64>>,
    ry: Option<&Vec<u64>>,
) -> Arm64 {
    use turbonop_arm64::OperandKind as K;
    if ix.encoding_name() != iy.encoding_name() {
        return Arm64::Different;
    }
    let (ox, oy) = (ix.detailed_operands(ax), iy.detailed_operands(ay));
    if ox.len() != oy.len() {
        return Arm64::Different;
    }
    let adrp = ix.encoding_name().starts_with("ADRP");
    let (mut tx, mut ty) = (Vec::new(), Vec::new());
    let mut via_scan = false;
    // An ADRP's target is a page (moved, the same page is encoded
    // otherwise): the instruction that completes its address decides,
    // over all it accesses.
    if adrp {
        return match (ox.first().map(|o| o.kind), oy.first().map(|o| o.kind)) {
            (Some(K::Register(r)), Some(K::Register(s))) if r == s => Arm64::Page(r.register),
            _ => Arm64::Different,
        };
    }
    // (Each target, the same or not: an instruction moved has the same
    // absolute one in a different encoding, and what's there must be the
    // same.)
    for (p, q) in ox.iter().zip(oy.iter()) {
        match (p.kind, q.kind) {
            // (A literal load's label is what it loads: all of it.)
            (K::Label(u), K::Label(v)) => {
                tx.extend(arm64_extents(ix, &[u]));
                ty.extend(arm64_extents(ix, &[v]));
            }
            _ if p.kind == q.kind => {}
            (K::Immediate(_), K::Immediate(_)) => via_scan = true,
            (K::Memory(m), K::Memory(n)) => {
                let (mut m2, mut n2) = (m, n);
                m2.displacement = 0;
                n2.displacement = 0;
                if m2 != n2 {
                    return Arm64::Different;
                }
                via_scan = true;
            }
            _ => return Arm64::Different,
        }
    }
    if via_scan {
        let (Some(sx), Some(sy)) = (rx, ry) else {
            return Arm64::Different;
        };
        tx.extend(arm64_extents(ix, sx));
        ty.extend(arm64_extents(ix, sy));
    }
    match targets(a, b, &tx, &ty) {
        Some(u) => Arm64::Same(u),
        None => Arm64::Different,
    }
}

/// What an ARM64 instruction refers to at `targets`: all it accesses
/// (an `LDP`'s two registers' worth), or a place (`ADD`'s address, a
/// branch's target).
fn arm64_extents(i: &turbonop_arm64::Instruction, targets: &[u64]) -> Vec<Extent> {
    let name = i.encoding_name();
    let place = ["B", "CB", "TB", "ADR"].iter().any(|p| name.starts_with(p));
    targets
        .iter()
        .map(|&at| match arm64_access(i).filter(|_| !place) {
            Some((len, step)) => Extent { at, len, step },
            None => Extent::point(at),
        })
        .collect()
}

/// Whether `i` reads `reg` as an address's base (a memory operand's, or
/// `ADD`'s source): what completes an `ADRP` that wrote it.
fn arm64_uses(i: &turbonop_arm64::Instruction, reg: turbonop_arm64::Register) -> bool {
    use turbonop_arm64::OperandKind as K;
    // (Its registers alone: where it is doesn't matter.)
    i.detailed_operands(0)
        .iter()
        .enumerate()
        .any(|(k, o)| match o.kind {
            K::Memory(m) => m.base == reg,
            K::Register(r) => k > 0 && r.register == reg,
            _ => false,
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Names for addresses: each (start, size, name).
    fn namer(items: &'static [(i64, i64, &'static str)]) -> impl Fn(i64) -> Named {
        move |at| {
            let (start, _, name) = items.iter().find(|&&(s, n, _)| at >= s && at < s + n)?;
            Some(Some((
                vec![(*name).to_string()],
                u32::try_from(at - start).ok()?,
            )))
        }
    }

    /// A load of two adjacent globals (an `LDP`, 16 bytes in two 8-byte
    /// elements) is alike only if it loads the same two.
    #[test]
    fn a_pair_loads_the_same_two_globals() {
        let a = namer(&[(0x100, 8, "g1"), (0x108, 8, "g2")]);
        let same = namer(&[(0x200, 8, "g1"), (0x208, 8, "g2")]);
        let other = namer(&[(0x200, 8, "g1"), (0x208, 8, "g3")]);
        let ex = [Extent {
            at: 0x100,
            len: 16,
            step: 8,
        }];
        let ey = [Extent {
            at: 0x200,
            len: 16,
            step: 8,
        }];
        assert_eq!(targets_by(&a, &same, &ex, &ey), Some(false));
        assert_eq!(targets_by(&a, &other, &ex, &ey), None);
        // A single load of the first is alike either way.
        let px = [Extent {
            at: 0x100,
            len: 8,
            step: 8,
        }];
        let py = [Extent {
            at: 0x200,
            len: 8,
            step: 8,
        }];
        assert_eq!(targets_by(&a, &other, &px, &py), Some(false));
        // A 16-byte vector load across both (x64's movdqa), likewise.
        let vx = [Extent {
            at: 0x100,
            len: 16,
            step: 8,
        }];
        assert_eq!(targets_by(&a, &other, &vx, &ey), None);
        // And the last byte: a 12-byte access past the first global's
        // 8 into another.
        let short = namer(&[(0x200, 8, "g1"), (0x208, 4, "g2"), (0x20C, 4, "g4")]);
        let wx = [Extent {
            at: 0x104,
            len: 12,
            step: 4,
        }];
        let wy = [Extent {
            at: 0x204,
            len: 12,
            step: 4,
        }];
        let longer = namer(&[(0x100, 8, "g1"), (0x108, 8, "g2")]);
        assert_eq!(targets_by(&longer, &short, &wx, &wy), None);
    }

    /// What ARM64 loads and stores access: their registers' width, times
    /// their count; a narrow form's width.
    #[test]
    fn arm64_accesses() {
        let access = |w: u32| arm64_access(&turbonop_arm64::decode(w).unwrap());
        // LDP x0, x1, [x8, #16]
        assert_eq!(access(0xA941_0500), Some((16, 8)));
        // LDP w0, w1, [x8]
        assert_eq!(access(0x2940_0500), Some((8, 4)));
        // LDR x0, [x8, #8]
        assert_eq!(access(0xF940_0500), Some((8, 8)));
        // LDRB w0, [x8]
        assert_eq!(access(0x3940_0100), Some((1, 1)));
        // LDRH w0, [x8]
        assert_eq!(access(0x7940_0100), Some((2, 2)));
        // LDR q0, [x8]
        assert_eq!(access(0x3DC0_0100), Some((16, 16)));
        // STXR w2, x0, [x8]: the status register isn't stored.
        assert_eq!(access(0xC802_7D00), Some((8, 8)));
        // ADD x0, x8, #16: no access.
        assert_eq!(access(0x9100_4100), None);
    }
}
