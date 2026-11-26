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

fn run(a: &Fake, ra: u32, x: &[u8], b: &Fake, rb: u32, y: &[u8], isa: Isa) -> (Report, Fields) {
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
