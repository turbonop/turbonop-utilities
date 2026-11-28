//! Region comparisons on hand-made images: each a few labels, and code
//! or data placed at different addresses in the two.

use super::*;

const BASE: u64 = 0x1_4000_0000;

/// An image of labels (sorted by address) and base relocations.
struct Fake {
    labels: Vec<(u32, &'static [u8])>,
    relocations: Vec<(u32, u8)>,
    code: (u32, u32),
}

impl Names for Fake {
    fn name(&self, rva: u32) -> Option<(&[u8], u32)> {
        if rva >= 0x10_0000 {
            return None;
        }
        let i = self.labels.partition_point(|l| l.0 <= rva).checked_sub(1)?;
        Some((self.labels[i].1, rva - self.labels[i].0))
    }

    fn in_code(&self, rva: u32) -> bool {
        rva >= self.code.0 && rva < self.code.0 + self.code.1
    }

    fn image_base(&self) -> u64 {
        BASE
    }

    fn relocations_in(&self, rva: u32, len: u32) -> &[(u32, u8)] {
        let from = self.relocations.partition_point(|r| r.0 < rva);
        let to = self.relocations.partition_point(|r| r.0 < rva + len);
        &self.relocations[from..to]
    }
}

/// Two images: `f` at 0x1000 in the first and 0x2000 in the second, `g`
/// at 0x3000 and 0x5000, `h` at 0x4000 and 0x3000, `d` (data) at 0x8000
/// and 0x9000.
fn images() -> (Fake, Fake) {
    let a = Fake {
        labels: vec![
            (0x1000, b"f"),
            (0x3000, b"g"),
            (0x4000, b"h"),
            (0x8000, b"d"),
        ],
        relocations: Vec::new(),
        code: (0x1000, 0x100),
    };
    let b = Fake {
        labels: vec![
            (0x2000, b"f"),
            (0x3000, b"h"),
            (0x5000, b"g"),
            (0x9000, b"d"),
        ],
        relocations: Vec::new(),
        code: (0x2000, 0x100),
    };
    (a, b)
}

fn rel32(next: u32, target: u32) -> [u8; 4] {
    (target.wrapping_sub(next) as i32).to_le_bytes()
}

/// `call target` at `at` (5 bytes), then `lea rax, [rip + data]` (7),
/// then `ret`.
fn code(at: u32, target: u32, data: u32) -> Vec<u8> {
    let mut v = vec![0xE8];
    v.extend(rel32(at + 5, target));
    v.extend([0x48, 0x8D, 0x05]);
    v.extend(rel32(at + 12, data));
    v.push(0xC3);
    v
}

fn run<N: Names>(a: &N, ra: u32, x: &[u8], b: &N, rb: u32, y: &[u8], isa: Isa) -> (Report, Fields) {
    let mut rep = Report::new();
    let f = compare(
        &Side {
            image: a,
            rva: ra,
            bytes: x,
        },
        &Side {
            image: b,
            rva: rb,
            bytes: y,
        },
        isa,
        b"f",
        &mut rep,
    );
    (rep, f)
}

#[test]
fn x64_operands_are_compared_by_what_they_name() {
    let (a, b) = images();
    let x = code(0x1000, 0x3000, 0x8010);
    let y = code(0x2000, 0x5000, 0x9010);
    assert_ne!(x, y);
    let (rep, f) = run(&a, 0x1000, &x, &b, 0x2000, &y, Isa::X64);
    assert!(rep.passed(), "{}", rep.text());
    assert_eq!(f.relative, 2);
    // The call goes to h in the second image.
    let y = code(0x2000, 0x3000, 0x9010);
    let (rep, _) = run(&a, 0x1000, &x, &b, 0x2000, &y, Isa::X64);
    assert!(
        rep.failed("code whose operands name other functions or data"),
        "{}",
        rep.text()
    );
    // Another offset in the datum.
    let y = code(0x2000, 0x5000, 0x9014);
    let (rep, _) = run(&a, 0x1000, &x, &b, 0x2000, &y, Isa::X64);
    assert!(!rep.passed());
}

#[test]
fn x64_equal_bytes_inside_a_procedure_still_name_their_targets() {
    // Both at 0x1000, the call's target moved: the bytes are equal, but
    // the second calls h.
    let (a, mut b) = images();
    b.labels = vec![
        (0x1000, b"f"),
        (0x3000, b"h"),
        (0x5000, b"g"),
        (0x9000, b"d"),
    ];
    b.code = (0x1000, 0x100);
    let x = code(0x1000, 0x3000, 0x8010);
    let (rep, _) = run(&a, 0x1000, &x, &b, 0x1000, &x, Isa::X64);
    assert!(
        rep.failed("code whose operands name other functions or data"),
        "{}",
        rep.text()
    );
    // Outside a procedure, equal bytes are taken as they are.
    let (mut a, mut b) = (a, b);
    a.code = (0, 0);
    b.code = (0, 0);
    let (rep, _) = run(&a, 0x1000, &x, &b, 0x1000, &x, Isa::X64);
    assert!(rep.passed(), "{}", rep.text());
}

#[test]
fn other_bytes_are_caught() {
    let (a, b) = images();
    let x = code(0x1000, 0x3000, 0x8010);
    let mut y = code(0x2000, 0x5000, 0x9010);
    y[12] = 0xCC;
    let (rep, _) = run(&a, 0x1000, &x, &b, 0x2000, &y, Isa::X64);
    assert!(
        rep.failed("functions or data whose bytes differ"),
        "{}",
        rep.text()
    );
}

#[test]
fn absolute_addresses_by_base_relocations() {
    let (mut a, mut b) = images();
    a.relocations = vec![(0x8008, 8)];
    b.relocations = vec![(0x9008, 8)];
    let datum = |target: u64| {
        let mut v = vec![1u8; 8];
        v.extend((BASE + target).to_le_bytes());
        v
    };
    let (rep, f) = run(
        &a,
        0x8000,
        &datum(0x3004),
        &b,
        0x9000,
        &datum(0x5004),
        Isa::Data,
    );
    assert!(rep.passed(), "{}", rep.text());
    assert_eq!(f.absolute, 1);
    let (rep, _) = run(
        &a,
        0x8000,
        &datum(0x3004),
        &b,
        0x9000,
        &datum(0x3004),
        Isa::Data,
    );
    assert!(rep.failed("absolute addresses naming other functions or data"));
    // A relocation in one only.
    b.relocations.clear();
    let (rep, _) = run(
        &a,
        0x8000,
        &datum(0x3004),
        &b,
        0x9000,
        &datum(0x5004),
        Isa::Data,
    );
    assert!(rep.failed("functions or data with base relocations at other places"));
}

#[test]
fn image_relative_words_are_inferred() {
    let (a, b) = images();
    // A table of RVAs: f+0x10 and g, at 4-byte aligned places.
    let table = |f: u32, g: u32| {
        let mut v = 7u32.to_le_bytes().to_vec();
        v.extend((f + 0x10).to_le_bytes());
        v.extend(g.to_le_bytes());
        v
    };
    let (rep, f) = run(
        &a,
        0x8000,
        &table(0x1000, 0x3000),
        &b,
        0x9000,
        &table(0x2000, 0x5000),
        Isa::Data,
    );
    assert!(rep.passed(), "{}", rep.text());
    assert_eq!(f.inferred, 2);
    let (rep, _) = run(
        &a,
        0x8000,
        &table(0x1000, 0x3000),
        &b,
        0x9000,
        &table(0x2000, 0x4000),
        Isa::Data,
    );
    assert!(rep.failed("functions or data whose bytes differ"));
    // (Equal words are taken as they are: an RVA can't be told from an
    // integer, so one that names something else at the same value isn't
    // caught here.)
    // Misaligned regions don't read words they don't hold.
    let (rep, _) = run(&a, 0x8001, &[1, 2, 3], &b, 0x9001, &[1, 2, 4], Isa::Data);
    assert!(rep.failed("functions or data whose bytes differ"));
}

/// `mov ecx, [rax + rcx*4 + table]`, then `ret`: MSVC's switch, the image
/// base in rax and `table` the jump table's RVA.
fn switch(table: u32) -> Vec<u8> {
    let mut v = vec![0x8B, 0x8C, 0x88];
    v.extend(table.to_le_bytes());
    v.push(0xC3);
    v
}

#[test]
fn x64_image_relative_displacements_are_inferred() {
    let (a, b) = images();
    // The table at f+0x40 in each (the displacement is misaligned).
    let (rep, f) = run(
        &a,
        0x1000,
        &switch(0x1040),
        &b,
        0x2000,
        &switch(0x2040),
        Isa::X64,
    );
    assert!(rep.passed(), "{}", rep.text());
    assert_eq!(f.inferred, 1);
    // Another place in the second.
    let (rep, _) = run(
        &a,
        0x1000,
        &switch(0x1040),
        &b,
        0x2000,
        &switch(0x2044),
        Isa::X64,
    );
    assert!(
        rep.failed("functions or data whose bytes differ"),
        "{}",
        rep.text()
    );
    // Another function in the second.
    let (rep, _) = run(
        &a,
        0x1000,
        &switch(0x3000),
        &b,
        0x2000,
        &switch(0x3000 + 0x1000),
        Isa::X64,
    );
    assert!(!rep.passed(), "{}", rep.text());
}

/// ARM64 words.
fn bl(at: u32, target: u32) -> u32 {
    0x9400_0000 | ((target.wrapping_sub(at) >> 2) & 0x03FF_FFFF)
}

fn adrp(at: u32, target: u32, rd: u32) -> u32 {
    let pages = (i64::from(target >> 12) - i64::from(at >> 12)) as u32;
    0x9000_0000 | ((pages & 3) << 29) | (((pages >> 2) & 0x7_FFFF) << 5) | rd
}

fn add_imm(rd: u32, rn: u32, imm: u32) -> u32 {
    0x9100_0000 | ((imm & 0xFFF) << 10) | (rn << 5) | rd
}

fn ldr_x(rt: u32, rn: u32, offset: u32) -> u32 {
    0xF940_0000 | (((offset / 8) & 0xFFF) << 10) | (rn << 5) | rt
}

fn words(w: &[u32]) -> Vec<u8> {
    w.iter().flat_map(|x| x.to_le_bytes()).collect()
}

/// `bl g; adrp x0, d+0x10; add x0, x0, :lo12:d+0x10; adrp x1, d+8;
/// ldr x2, [x1, :lo12:d+8]; ret`.
fn arm(at: u32, g: u32, d: u32) -> Vec<u8> {
    words(&[
        bl(at, g),
        adrp(at + 4, d + 0x10, 0),
        add_imm(0, 0, (d + 0x10) & 0xFFF),
        adrp(at + 12, d + 8, 1),
        ldr_x(2, 1, (d + 8) & 0xFFF),
        0xD65F_03C0,
    ])
}

#[test]
fn arm64_operands_are_compared_by_what_they_name() {
    let (a, mut b) = images();
    // d at 0x9100 in the second, so that its page offset differs too.
    b.labels = vec![
        (0x2000, b"f"),
        (0x3000, b"h"),
        (0x5000, b"g"),
        (0x9100, b"d"),
    ];
    let x = arm(0x1000, 0x3000, 0x8000);
    let y = arm(0x2000, 0x5000, 0x9100);
    let (rep, f) = run(&a, 0x1000, &x, &b, 0x2000, &y, Isa::Arm64);
    assert!(rep.passed(), "{}", rep.text());
    assert_eq!(f.relative, 3);
    // The load reads another offset.
    let mut y2 = y.clone();
    y2[16..20].copy_from_slice(&ldr_x(2, 1, (0x9100 + 16) & 0xFFF).to_le_bytes());
    let (rep, _) = run(&a, 0x1000, &x, &b, 0x2000, &y2, Isa::Arm64);
    assert!(!rep.passed());
    // The call goes to h.
    let y3 = arm(0x2000, 0x3000, 0x9100);
    let (rep, _) = run(&a, 0x1000, &x, &b, 0x2000, &y3, Isa::Arm64);
    assert!(
        rep.failed("code whose operands name other functions or data"),
        "{}",
        rep.text()
    );
}

/// A [`Fake`] with a TLS block and constants named by their contents.
struct Extra {
    base: Fake,
    /// The TLS block's aligned size, and its variables: (offset, name).
    block: u64,
    tls: Vec<(u64, &'static [u8])>,
    /// Constants without labels: (address, bytes).
    constants: Vec<(u32, Vec<u8>)>,
}

impl Extra {
    fn constant(&self, rva: u32) -> Option<&[u8]> {
        let (at, bytes) = self
            .constants
            .iter()
            .find(|c| c.0 <= rva && rva < c.0 + 64)?;
        bytes.get((rva - at) as usize..)
    }
}

impl Names for Extra {
    fn name(&self, rva: u32) -> Option<(&[u8], u32)> {
        match self.constant(rva) {
            Some(b) => Some((b, crate::elf::program::CONTENT)),
            None => self.base.name(rva),
        }
    }

    fn in_code(&self, rva: u32) -> bool {
        self.base.in_code(rva)
    }

    fn image_base(&self) -> u64 {
        BASE
    }

    fn relocations_in(&self, rva: u32, len: u32) -> &[(u32, u8)] {
        self.base.relocations_in(rva, len)
    }

    fn tls(&self, tpoff: i64) -> Option<(&[u8], u32)> {
        let off = u64::try_from(tpoff + self.block as i64).ok()?;
        let (at, name) = self.tls.iter().rev().find(|t| t.0 <= off)?;
        Some((name, (off - at) as u32))
    }

    fn tls_block(&self) -> Option<u64> {
        Some(self.block)
    }

    fn content(&self, rva: u32, len: u32) -> Option<&[u8]> {
        self.constant(rva)?.get(..len as usize)
    }
}

fn extra(base: Fake, block: u64, var_at: u64, constant: u32, tail: u8) -> Extra {
    let mut bytes: Vec<u8> = (1..=16).collect();
    bytes.extend([tail; 48]);
    Extra {
        base,
        block,
        tls: vec![(0, b"other"), (var_at, b"Head"), (var_at + 8, b"after")],
        constants: vec![(constant, bytes)],
    }
}

/// `xor eax,eax; mov rax,fs:[rax]; sub rax,S; mov rcx,rax; (between);
/// lea rcx,[rcx+d]; ret`, as mold relaxes a dynamic TLS access.
fn mold_tls(size: u8, d: u8, between: &[u8]) -> Vec<u8> {
    let mut v = vec![
        0x31, 0xC0, 0x64, 0x48, 0x8B, 0x00, 0x48, 0x2D, size, 0, 0, 0,
    ];
    v.extend([0x48, 0x89, 0xC1]);
    v.extend(between);
    v.extend([0x48, 0x8D, 0x89, d, 0, 0, 0, 0xC3]);
    v
}

#[test]
fn mold_tls_offsets_are_named_in_each_block() {
    let (a, b) = images();
    // The variable at 0x20 of a 0x38-byte block, and at 0x18 of 0x30.
    let (a, b) = (
        extra(a, 0x38, 0x20, 0x8100, 0xAA),
        extra(b, 0x30, 0x18, 0x9100, 0xBB),
    );
    let x = mold_tls(0x38, 0x20, &[]);
    let y = mold_tls(0x30, 0x18, &[]);
    let (rep, f) = run(&a, 0x1000, &x, &b, 0x2000, &y, Isa::X64);
    assert!(rep.passed(), "{}", rep.text());
    assert_eq!(f.tls, 2);
    // A copy's register written in between (`mov rcx,[rbp-8]`): no longer
    // the block's start.
    let w = [0x48, 0x8B, 0x4D, 0xF8];
    let (rep, _) = run(
        &a,
        0x1000,
        &mold_tls(0x38, 0x20, &w),
        &b,
        0x2000,
        &mold_tls(0x30, 0x18, &w),
        Isa::X64,
    );
    assert!(
        rep.failed("functions or data whose bytes differ"),
        "{}",
        rep.text()
    );
    // Another register written (`mov rdx,[rbp-8]`): still is.
    let w = [0x48, 0x8B, 0x55, 0xF8];
    let (rep, _) = run(
        &a,
        0x1000,
        &mold_tls(0x38, 0x20, &w),
        &b,
        0x2000,
        &mold_tls(0x30, 0x18, &w),
        Isa::X64,
    );
    assert!(rep.passed(), "{}", rep.text());
    // Another variable (at the same offset of the other block).
    let (rep, _) = run(
        &a,
        0x1000,
        &x,
        &b,
        0x2000,
        &mold_tls(0x30, 0x20, &[]),
        Isa::X64,
    );
    assert!(
        rep.failed("code whose TLS offsets name other variables"),
        "{}",
        rep.text()
    );
    // A block size that isn't the program's.
    let (rep, _) = run(
        &a,
        0x1000,
        &x,
        &b,
        0x2000,
        &mold_tls(0x28, 0x18, &[]),
        Isa::X64,
    );
    assert!(!rep.passed(), "{}", rep.text());
}

#[test]
fn constants_named_by_contents_compare_as_far_as_they_are_read() {
    let (a, b) = images();
    // The same 16 bytes, then other bytes.
    let (a, b) = (
        extra(a, 0x38, 0x20, 0x8100, 0xAA),
        extra(b, 0x30, 0x18, 0x9100, 0xBB),
    );
    // `movdqa xmm0, [rip+c]` (16 bytes read); `lea rax, [rip+c]` (none).
    let load = |at: u32, c: u32, op: &[u8]| {
        let mut v = op.to_vec();
        v.extend(rel32(at + op.len() as u32 + 4, c));
        v.push(0xC3);
        v
    };
    let movdqa = [0x66, 0x0F, 0x6F, 0x05];
    let (rep, f) = run(
        &a,
        0x1000,
        &load(0x1000, 0x8100, &movdqa),
        &b,
        0x2000,
        &load(0x2000, 0x9100, &movdqa),
        Isa::X64,
    );
    assert!(rep.passed(), "{}", rep.text());
    assert_eq!(f.relative, 1);
    // Read from one byte further: 16 bytes of which the last differs.
    let (rep, _) = run(
        &a,
        0x1000,
        &load(0x1000, 0x8101, &movdqa),
        &b,
        0x2000,
        &load(0x2000, 0x9101, &movdqa),
        Isa::X64,
    );
    assert!(
        rep.failed("code whose operands name other functions or data"),
        "{}",
        rep.text()
    );
    // An address taken: what follows may be read too.
    let lea = [0x48, 0x8D, 0x05];
    let (rep, _) = run(
        &a,
        0x1000,
        &load(0x1000, 0x8100, &lea),
        &b,
        0x2000,
        &load(0x2000, 0x9100, &lea),
        Isa::X64,
    );
    assert!(
        rep.failed("code whose operands name other functions or data"),
        "{}",
        rep.text()
    );
}
