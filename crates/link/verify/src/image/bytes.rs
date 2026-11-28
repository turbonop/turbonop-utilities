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
    /// The end of the procedure whose code holds `rva`, if one does.
    fn procedure_end(&self, rva: u32) -> Option<u32> {
        let _ = rva;
        None
    }
    /// Whether [`tls`](Self::tls) takes an offset in the image's TLS
    /// section (Windows: code addresses a variable by its `SECREL` from the
    /// thread's block, which it reads from the TLS array at `gs:[58h]`),
    /// rather than from the thread pointer.
    fn tls_in_section(&self) -> bool {
        false
    }
    /// The size of the TLS block, rounded up to its alignment (x86-64's
    /// thread pointer's distance from the block's start), if there is one.
    fn tls_block(&self) -> Option<u64> {
        None
    }
    /// The `len` bytes at `rva`, if its name is its contents (a constant
    /// or string with no symbol: see `elf::program`).
    fn content(&self, rva: u32, len: u32) -> Option<&[u8]> {
        let _ = (rva, len);
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
    // Bytes that decoded as PC-relative operands naming the same thing in
    // both, outside every procedure's code: there they may be data (a jump
    // table, say), so they explain a difference only where reading the
    // word as an RVA doesn't.
    let mut soft = vec![false; len];
    // Operands in a procedure's code that name other things: differences,
    // unless the bytes are a table's (MSVC puts a switch's jump table in
    // the procedure, after its code) and its words explain them.
    let mut pending: Vec<Pending> = Vec::new();
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
        Isa::X64 => x64(
            a,
            b,
            &mut masked,
            &mut soft,
            &mut pending,
            &mut fields,
            &mut |c, d| fail(rep, c, d),
        ),
        Isa::Arm64 => arm64(a, b, &mut masked, &mut soft, &mut pending, &mut fields),
        Isa::Data => {}
    }

    // What still differs: image-relative words, or a difference.
    let mut differs: Option<usize> = None;
    let mut at = 0;
    while at < len {
        if masked[at] || a.bytes[at] == b.bytes[at] {
            at += 1;
            continue;
        }
        // The aligned word holding the difference, read as an RVA in both.
        let misaligned = (a.rva as usize + at) & 3;
        let same_alignment = (a.rva & 3) == (b.rva & 3);
        let start = at.saturating_sub(misaligned);
        // (The word may hold bytes explained already, as fields of what
        // they decoded to: a table in a procedure's code decodes as
        // instructions too. Two words that differ and name the same thing
        // in both images are the better explanation.)
        let fits = at >= misaligned && at - misaligned + 4 <= len && same_alignment;
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
        // In data, an RVA needn't be aligned: the C++ exception handler's
        // compressed tables (`__CxxFrameHandler4`'s) hold them at any byte.
        if isa == Isa::Data
            && let Some(s) = (at.saturating_sub(3)..=at).find(|&s| {
                s + 4 <= len && s != start && {
                    let wa = word(a.bytes, s, 4) as u32;
                    let wb = word(b.bytes, s, 4) as u32;
                    a.image.name(wa).is_some()
                        && b.image.name(wb).is_some()
                        && same(a.image, Some(wa), b.image, Some(wb))
                        && !(s..s + 4).any(|k| masked[k] && a.bytes[k] != b.bytes[k])
                }
            })
        {
            fields.inferred += 1;
            masked[s..s + 4].fill(true);
            at = s + 4;
            continue;
        }
        if !soft[at] && differs.is_none() {
            differs = Some(at);
        }
        at += 1;
    }
    for p in pending {
        let bytes = p.at..p.at + p.len;
        let equal = bytes.clone().all(|k| a.bytes[k] == b.bytes[k]);
        let explained = bytes.clone().all(|k| a.bytes[k] == b.bytes[k] || masked[k]);
        if equal || !explained {
            fail(
                rep,
                "code whose operands name other functions or data",
                p.detail,
            );
        }
    }
    if let Some(at) = differs {
        fail(
            rep,
            "functions or data whose bytes differ",
            format!(
                "at +{at:#x}: {:#04x} and {:#04x} (RVAs {:#x} and {:#x})",
                a.bytes[at],
                b.bytes[at],
                a.rva as usize + at,
                b.rva as usize + at
            ),
        );
    }
    fields
}

/// An operand in a procedure's code whose targets name other things (see
/// `pending` in [`compare`]): its field, and what it names in each.
struct Pending {
    at: usize,
    len: usize,
    detail: String,
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
    hard: &mut [bool],
    soft: &mut [bool],
    pending: &mut Vec<Pending>,
    fields: &mut Fields,
    fail: &mut dyn FnMut(&str, String),
) {
    use turbonop_x86::{Mnemonic, Mode, Operand, RegClass, Register, decode};
    let len = hard.len();
    let base_a = a.image.image_base() + u64::from(a.rva);
    let base_b = b.image.image_base() + u64::from(b.rva);
    // A TLS access as mold relaxes the dynamic models to local-exec:
    // `mov %fs:(...),R; sub $S,R`, S the TLS block's aligned size, leaves R
    // at the block's start (where `__tls_get_addr` returned the module's
    // block), and `d(R)` then addresses the variable at offset d of the
    // block (`lea d(%rax),%rax`), after copies of R too (`mov %rax,%rcx`).
    // Another TLS layout moves d (and S): each is named in its own program.
    // The registers known to hold the block's start are followed through
    // copies and forgotten when written; at any instruction that could
    // write one other than as its first operand (or jumps), all are.
    let fs = Register::new(RegClass::Segment, 4);
    let mut fs_loaded: Option<Register> = None;
    let mut block: Vec<Register> = Vec::new();
    // Windows: where the code reads the TLS array (`gs:[58h]`), in both. A
    // `SECREL` (a displacement or an immediate) is taken as one only near
    // such a read: MSVC loads it a few instructions before or after.
    let in_section = a.image.tls_in_section() && b.image.tls_in_section();
    let tls_reads: Vec<usize> = if in_section {
        let theirs = tls_array_reads(&b.bytes[..len], base_b);
        tls_array_reads(&a.bytes[..len], base_a)
            .into_iter()
            .filter(|p| theirs.binary_search(p).is_ok())
            .collect()
    } else {
        Vec::new()
    };
    let near_tls_read = |at: usize| {
        let i = tls_reads.partition_point(|&p| p + TLS_WINDOW < at);
        tls_reads.get(i).is_some_and(|&p| p <= at + TLS_WINDOW)
    };
    // MSVC's switch tables: `mov ecx, [rdx + rax*4 + T]` (and `movzx eax,
    // byte ptr [rdx + rax + T]`, an index into it), the image base in rdx
    // and T an RVA in the procedure, the same distance into it in both.
    // MSVC puts them after the procedure's code, up to its end (as the PDB
    // gives it), so from the first one found to that end the bytes are
    // data: not decoded, compared as words (RVAs of the cases).
    let mut tables: Option<(usize, usize)> = None;
    let mut at = 0;
    while at < len {
        if let Some((from, to)) = tables
            && at >= from
            && at < to
        {
            at = to;
            tables = None;
            fs_loaded = None;
            block.clear();
            continue;
        }
        let (Ok(ia), Ok(ib)) = (
            decode(&a.bytes[at..len], Mode::Bits64, base_a + at as u64),
            decode(&b.bytes[at..len], Mode::Bits64, base_b + at as u64),
        ) else {
            at += 1;
            fs_loaded = None;
            block.clear();
            continue;
        };
        let n = ia.len();
        if n == 0 || n != ib.len() || ia.mnemonic() != ib.mnemonic() {
            at += 1;
            fs_loaded = None;
            block.clear();
            continue;
        }
        let checked = a.image.in_code(a.rva + at as u32);
        // Bytes outside a procedure that decode across the start of one
        // (data before it, read as an instruction) would put the decoding
        // of its code out of step: it starts at the procedure.
        if !checked
            && let Some(k) = (1..n).find(|&k| a.image.in_code(a.rva + (at + k) as u32))
        {
            at += k;
            fs_loaded = None;
            block.clear();
            continue;
        }
        let (ea, eb) = (&a.bytes[at..at + n], &b.bytes[at..at + n]);
        // What an instruction outside a procedure explains is only
        // perhaps explained (see `soft` in `compare`).
        let masked: &mut [bool] = if checked { &mut *hard } else { &mut *soft };
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
            // A constant or string named by its contents: those can run on
            // past it into what the layout puts after it. A load of a size
            // reads that many bytes, so the same bytes there are the same.
            let accessed = match (oa, ob) {
                (Operand::Mem(ma), Operand::Mem(mb)) if ma.size == mb.size => {
                    u32::from(ma.size.bytes())
                }
                _ => 0,
            };
            let rva = |t: u64, base: u64| t.checked_sub(base).and_then(|r| u32::try_from(r).ok());
            let same_contents = accessed > 0
                && match (
                    rva(ta, a.image.image_base()).and_then(|r| a.image.content(r, accessed)),
                    rva(tb, b.image.image_base()).and_then(|r| b.image.content(r, accessed)),
                ) {
                    (Some(ca), Some(cb)) => ca == cb,
                    _ => false,
                };
            if ea == eb && !checked {
                // Equal bytes outside a procedure: perhaps data.
            } else if same_contents {
                fields.relative += 1;
            } else if !same(
                a.image,
                ta.checked_sub(a.image.image_base())
                    .and_then(|r| u32::try_from(r).ok()),
                b.image,
                tb.checked_sub(b.image.image_base())
                    .and_then(|r| u32::try_from(r).ok()),
            ) {
                // (Outside a procedure, the bytes are left for the
                // comparison of words to explain, or not.)
                if checked {
                    pending.push(Pending {
                        at: at + p,
                        len: size,
                        detail: format!(
                            "at +{at:#x}: {} and {} (RVAs {:#x} and {:#x})",
                            render(na.as_ref().map(|(n, o)| (n.as_slice(), *o))),
                            render(nb.as_ref().map(|(n, o)| (n.as_slice(), *o))),
                            a.rva as usize + at,
                            b.rva as usize + at
                        ),
                    });
                }
                continue;
            } else {
                fields.relative += 1;
            }
            masked[at + p..at + p + size].fill(true);
        }
        // Thread-pointer offsets: displacements and immediates that name
        // the same TLS variable (another TLS layout moves them all).
        if ea != eb && (!in_section || near_tls_read(at)) {
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
        // Windows: a `SECREL` MSVC loads as `mov r32, imm32` near a read of
        // the TLS array names its variable even where the bytes are equal
        // (another layout of the template would move it).
        if ea == eb
            && in_section
            && checked
            && ia.mnemonic() == Mnemonic::Mov
            && near_tls_read(at)
            && let [Operand::Reg(r), Operand::Imm(xa)] = ia.operands()
            && xa.size == 4
            && indexes_soon(&a.bytes[at + n..len], base_a + (at + n) as u64, *r)
        {
            let v = i64::from(xa.value as u32);
            if let (Some(na), Some(nb)) = (a.image.tls(v), b.image.tls(v))
                && na != nb
            {
                fail(
                    "code whose TLS offsets name other variables",
                    format!("at +{at:#x}: {} and {}", render(Some(na)), render(Some(nb))),
                );
            }
        }
        for (oa, ob) in ia.operands().iter().zip(ib.operands()) {
            if let (Operand::Mem(ma), Operand::Mem(mb)) = (oa, ob)
                && !ma.is_rip_relative()
                && ma.base.is_some()
                && ma.index.is_some()
                && matches!(ma.scale, 1 | 4)
                && ma.disp_size == 4
                && (mb.base, mb.index, mb.scale, mb.disp_size) == (ma.base, ma.index, ma.scale, 4)
                && let (Ok(ta), Ok(tb)) = (u32::try_from(ma.disp), u32::try_from(mb.disp))
                && let (Some(oa), Some(ob)) = (ta.checked_sub(a.rva), tb.checked_sub(b.rva))
                && oa == ob
                && (oa as usize) > at
                && (oa as usize) < len
                && a.image.in_code(ta)
                && b.image.in_code(tb)
            {
                let from = oa as usize;
                let to = a
                    .image
                    .procedure_end(ta)
                    .zip(b.image.procedure_end(tb))
                    .filter(|&(ea, eb)| ea - a.rva == eb - b.rva)
                    .map_or(len, |(ea, _)| ((ea - a.rva) as usize).min(len));
                tables = Some(match tables {
                    Some((f, t)) if f <= from => (f, t.max(to)),
                    Some((_, t)) => (from, t.max(to)),
                    None => (from, to),
                });
            }
        }
        // Image-relative displacements (`mov ecx, [rax+rcx*4+T]`, the image
        // base in rax and T the RVA of a jump table, as MSVC indexes a
        // switch's): by inference, as image-relative words are below.
        if ea != eb {
            for (oa, ob) in ia.operands().iter().zip(ib.operands()) {
                if let (Operand::Mem(ma), Operand::Mem(mb)) = (oa, ob)
                    && !ma.is_rip_relative()
                    && !mb.is_rip_relative()
                    && ma.disp_size == 4
                    && mb.disp_size == 4
                    && ma.disp != mb.disp
                    && ma.base == mb.base
                    && ma.index == mb.index
                    && let (Ok(ra), Ok(rb)) = (u32::try_from(ma.disp), u32::try_from(mb.disp))
                    && a.image.name(ra).is_some()
                    && same(a.image, Some(ra), b.image, Some(rb))
                    && let Some(p) = field(ea, eb, 1, 4, ma.disp, mb.disp)
                    && !masked[at + p..at + p + 4].contains(&true)
                {
                    masked[at + p..at + p + 4].fill(true);
                    fields.inferred += 1;
                }
            }
        }
        // mold's relaxed TLS accesses (above).
        let mut mask = |p: Option<usize>, size: usize, fields: &mut Fields| {
            if let Some(p) = p
                && !masked[at + p..at + p + size].contains(&true)
            {
                masked[at + p..at + p + size].fill(true);
                fields.tls += 1;
            }
        };
        let (sa, sb) = (a.image.tls_block(), b.image.tls_block());
        if let (Some(sa), Some(sb)) = (sa, sb) {
            for (oa, ob) in ia.operands().iter().zip(ib.operands()) {
                if let (Operand::Mem(ma), Operand::Mem(mb)) = (oa, ob)
                    && ma.base.is_some_and(|r| block.contains(&r))
                    && mb.base == ma.base
                    && ma.index.is_none()
                    && mb.index.is_none()
                    && matches!(ma.disp_size, 1 | 4)
                    && ma.disp_size == mb.disp_size
                {
                    let tpoff = |d: i64, s: u64| d.checked_sub(i64::try_from(s).ok()?);
                    let na = tpoff(ma.disp, sa).and_then(|t| a.image.tls(t));
                    let nb = tpoff(mb.disp, sb).and_then(|t| b.image.tls(t));
                    match (na, nb) {
                        (Some(x), Some(y)) if x == y => {
                            if ma.disp != mb.disp {
                                let size = usize::from(ma.disp_size);
                                mask(field(ea, eb, 1, size, ma.disp, mb.disp), size, fields);
                            }
                        }
                        (None, None) => {}
                        // (Even where the bytes are the same.)
                        _ => fail(
                            "code whose TLS offsets name other variables",
                            format!("at +{at:#x}: {} and {}", render(na), render(nb)),
                        ),
                    }
                }
            }
        }
        let loaded = fs_loaded.take();
        match (ia.mnemonic(), ia.operands(), ib.operands()) {
            (
                Mnemonic::Mov,
                [Operand::Reg(r), Operand::Mem(ma)],
                [Operand::Reg(rb), Operand::Mem(mb)],
            ) if r == rb
                && r.class() == RegClass::Gpr64
                && ma.segment == Some(fs)
                && mb.segment == Some(fs) =>
            {
                block.retain(|x| x != r);
                fs_loaded = Some(*r);
            }
            (
                Mnemonic::Sub,
                [Operand::Reg(r), Operand::Imm(xa)],
                [Operand::Reg(rb), Operand::Imm(xb)],
            ) if loaded == Some(*r) && r == rb && sa == Some(xa.value) && sb == Some(xb.value) => {
                block.push(*r);
                if xa.value != xb.value {
                    let (va, vb) = (xa.value as i64, xb.value as i64);
                    mask(field(ea, eb, 1, 4, va, vb), 4, fields);
                }
            }
            // A copy.
            (
                Mnemonic::Mov,
                [Operand::Reg(to), Operand::Reg(from)],
                [Operand::Reg(to_b), Operand::Reg(from_b)],
            ) if to == to_b && from == from_b => {
                let held = block.contains(from) && to.class() == RegClass::Gpr64;
                block.retain(|x| x != to);
                if held {
                    block.push(*to);
                }
            }
            // Instructions that write only their first operand (if that).
            (
                Mnemonic::Mov
                | Mnemonic::Lea
                | Mnemonic::Add
                | Mnemonic::Sub
                | Mnemonic::And
                | Mnemonic::Or
                | Mnemonic::Xor
                | Mnemonic::Cmp
                | Mnemonic::Test
                | Mnemonic::Movsxd
                | Mnemonic::Movzx
                | Mnemonic::Movsx,
                ops,
                _,
            ) => {
                if let Some(Operand::Reg(r)) = ops.first() {
                    // (Any width of it: `mov %ecx,...` writes all of rcx.)
                    block.retain(|x| x.index() != r.index() || !is_gpr(r.class()));
                }
            }
            _ => block.clear(),
        }
        at += n;
    }
}

/// How far (in bytes) from a read of the TLS array a `SECREL` may be.
const TLS_WINDOW: usize = 48;

/// Whether one of the next few instructions of `code` addresses memory
/// as `[base + r]` (or `[r + index]`), `r` the register (of any width) a
/// `SECREL` was loaded into: MSVC's `movzx eax, byte ptr [rcx+rax]`, rcx
/// the thread's block.
fn indexes_soon(code: &[u8], va: u64, r: turbonop_x86::Register) -> bool {
    use turbonop_x86::{Mode, Operand, decode};
    let mut at = 0;
    for _ in 0..6 {
        let Ok(i) = decode(&code[at..], Mode::Bits64, va + at as u64) else {
            return false;
        };
        let hit = i.operands().iter().any(|o| {
            matches!(o, Operand::Mem(m) if m.base.is_some() && m.index.is_some()
                && (m.base.is_some_and(|b| b.index() == r.index())
                    || m.index.is_some_and(|x| x.index() == r.index())))
        });
        if hit {
            return true;
        }
        at += i.len().max(1);
        if at >= code.len() {
            return false;
        }
    }
    false
}

/// The offsets of the instructions in `code` that read the TLS array
/// (`gs:[58h]`, x64 Windows), decoded from the start.
fn tls_array_reads(code: &[u8], va: u64) -> Vec<usize> {
    use turbonop_x86::{Mode, Operand, RegClass, Register, decode};
    let gs = Register::new(RegClass::Segment, 5);
    let mut out = Vec::new();
    // (Cheap: most code has no gs prefix at all.)
    if !code.contains(&0x65) {
        return out;
    }
    let mut at = 0;
    while at < code.len() {
        let Ok(i) = decode(&code[at..], Mode::Bits64, va + at as u64) else {
            at += 1;
            continue;
        };
        if i.operands().iter().any(|o| {
            matches!(o, Operand::Mem(m) if m.segment == Some(gs) && m.base.is_none() && m.index.is_none() && m.disp == 0x58)
        }) {
            out.push(at);
        }
        at += i.len().max(1);
    }
    out
}

/// Whether registers of `c` are general-purpose (of any width).
fn is_gpr(c: turbonop_x86::RegClass) -> bool {
    use turbonop_x86::RegClass;
    matches!(
        c,
        RegClass::Gpr8 | RegClass::Gpr8High | RegClass::Gpr16 | RegClass::Gpr32 | RegClass::Gpr64
    )
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
    hard: &mut [bool],
    soft: &mut [bool],
    pending: &mut Vec<Pending>,
    fields: &mut Fields,
) {
    use turbonop_arm64::{AddrMode, OperandKind, decode};
    let words = hard.len() / 4;
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
    // Whether the field is explained: the targets name the same thing.
    // Inside a procedure, a field that names other things is a difference
    // unless the word is a table's (see `pending` in `compare`); outside
    // one, it is left for the comparison of words.
    let mut check = |at: usize, ta: u64, tb: u64, checked: bool| -> bool {
        let (na, nb) = (name_a(ta), name_b(tb));
        let rva = |t: u64, base: u64| t.checked_sub(base).and_then(|r| u32::try_from(r).ok());
        if same(
            a.image,
            rva(ta, a.image.image_base()),
            b.image,
            rva(tb, b.image.image_base()),
        ) {
            fields.relative += 1;
            return true;
        }
        if checked {
            pending.push(Pending {
                at: 4 * at,
                len: 4,
                detail: format!(
                    "at +{:#x}: {} and {}",
                    4 * at,
                    render(na.as_ref().map(|(n, o)| (n.as_slice(), *o))),
                    render(nb.as_ref().map(|(n, o)| (n.as_slice(), *o)))
                ),
            });
        }
        false
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
        let masked: &mut [bool] = if checked { &mut *hard } else { &mut *soft };
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
            if check(i, ta, tb, checked) {
                mark(masked, i, mask);
            }
            i += 1;
            continue;
        }
        // ADRP: the page, completed by each ADD or load or store whose base
        // is its register (up to one that writes it).
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
                if check(i, ta.wrapping_add(oa as u64), tb.wrapping_add(ob as u64), checked) {
                    mark(masked, i, mask);
                    mark(masked, j, ARM64_IMM12);
                }
                completed = true;
                // (Later loads and stores may use the page too, up to one
                // that writes its register.)
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
            if check(i, ta, tb, checked) {
                mark(masked, i, mask);
            }
        }
        i += 1;
    }
}

#[cfg(test)]
mod tests;
