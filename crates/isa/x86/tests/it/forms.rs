//! An enumeration of encodings that reaches (nearly) every opcode table
//! form: every map, opcode, and mandatory prefix, with each ModRM register
//! form and a memory form, both W values, every vector length, and the
//! EVEX and APX payload bits that select forms. It finds one register-form
//! and one memory-form encoding of each form, in each mode.

use std::collections::BTreeMap;
use std::sync::OnceLock;
use turbonop_x86::{Mode, Operand, RegClass, Register, decode};

/// The bytes after the opcode: ModRM register forms (all 64 for the legacy
/// maps, whose `!XX` forms need exact values), and a memory form with a
/// SIB byte (`[rax+rcx]`, which VSIB and tile forms need), then filler for
/// displacements and immediates.
fn modrm_tails(all: bool) -> Vec<Vec<u8>> {
    let mut tails = Vec::new();
    for reg in 0..8u8 {
        if all {
            for rm in 0..8u8 {
                tails.push(vec![0xC0 | reg << 3 | rm]);
            }
        } else {
            tails.push(vec![0xC0 | reg << 3 | 1]);
            tails.push(vec![0xC0 | reg << 3]);
        }
        tails.push(vec![reg << 3 | 4, 0x08]);
    }
    tails
}

const FILLER: [u8; 12] = [
    0x11, 0x22, 0x33, 0x44, 0x55, 0x66, 0x77, 0x88, 0x19, 0x2A, 0x3B, 0x4C,
];

/// Whether a one-byte opcode is a prefix (which would make a sample of
/// another instruction).
fn is_prefix(opcode: u8, is64: bool) -> bool {
    matches!(
        opcode,
        0x26 | 0x2E | 0x36 | 0x3E | 0x64 | 0x65 | 0x66 | 0x67 | 0xF0 | 0xF2 | 0xF3
    ) || is64 && (opcode & 0xF0 == 0x40 || opcode == 0xD5)
}

/// Every candidate encoding for one mode, in a fixed order, grouped into
/// independent batches.
fn batches(mode: Mode) -> Vec<Vec<Vec<u8>>> {
    let is64 = mode == Mode::Bits64;
    let mut out = Vec::new();
    let all_modrm = modrm_tails(true);
    let some_modrm = modrm_tails(false);

    // Legacy and REX.
    let p1: [&[u8]; 7] = [
        &[],
        &[0x66],
        &[0xF3],
        &[0xF2],
        &[0x66, 0xF2],
        &[0x66, 0xF3],
        &[0xF0],
    ];
    let p2: [&[u8]; 2] = [&[], &[0x67]];
    let rex: &[&[u8]] = if is64 {
        &[&[], &[0x48], &[0x41], &[0x44], &[0x47], &[0x4F]]
    } else {
        &[&[]]
    };
    let escapes: [&[u8]; 4] = [&[], &[0x0F], &[0x0F, 0x38], &[0x0F, 0x3A]];
    for escape in escapes {
        for a in p1 {
            for b in p2 {
                let mut batch = Vec::new();
                for r in rex {
                    for opcode in 0..=255u8 {
                        if escape.is_empty() && is_prefix(opcode, is64) {
                            continue;
                        }
                        for tail in &all_modrm {
                            let mut v = [a, b, r, escape].concat();
                            v.push(opcode);
                            v.extend_from_slice(tail);
                            v.extend_from_slice(&FILLER);
                            batch.push(v);
                        }
                    }
                }
                out.push(batch);
            }
        }
    }
    // 3DNow!: 0F 0F ModRM [SIB] suffix.
    let mut batch = Vec::new();
    for a in [&[][..], &[0x66]] {
        for suffix in 0..=255u8 {
            for tail in &some_modrm {
                let mut v = [a, &[0x0F, 0x0F]].concat();
                v.extend_from_slice(tail);
                v.push(suffix);
                v.extend_from_slice(&FILLER);
                batch.push(v);
            }
        }
    }
    out.push(batch);
    // REX2.
    if is64 {
        let mut batch = Vec::new();
        for a in [&[][..], &[0x66]] {
            for payload in [0x00u8, 0x08, 0x80, 0x88, 0x11, 0x44] {
                for opcode in 0..=255u8 {
                    for tail in &all_modrm {
                        let mut v = [a, &[0xD5, payload]].concat();
                        v.push(opcode);
                        v.extend_from_slice(tail);
                        v.extend_from_slice(&FILLER);
                        batch.push(v);
                    }
                }
            }
        }
        out.push(batch);
    }
    // VEX (three-byte, so W and every map are reachable) and XOP.
    // (In 64-bit mode, also with R, X, and B set, for registers 8-15.)
    let rxb: &[u8] = if is64 { &[0xE0, 0x00] } else { &[0xE0] };
    for (lead, maps) in [(0xC4u8, &[1u8, 2, 3, 5, 7][..]), (0x8F, &[8, 9, 10])] {
        for &m in maps {
            let mut batch = Vec::new();
            for w in 0..2u8 {
                for l in 0..2u8 {
                    for pp in 0..4u8 {
                        for vvvv in [0xFu8, 0x5] {
                            let p = w << 7 | vvvv << 3 | l << 2 | pp;
                            for &r in rxb {
                                for opcode in 0..=255u8 {
                                    for tail in &all_modrm {
                                        let mut v = vec![lead, r | m, p, opcode];
                                        v.extend_from_slice(tail);
                                        v.extend_from_slice(&FILLER);
                                        batch.push(v);
                                    }
                                }
                            }
                        }
                    }
                }
            }
            out.push(batch);
        }
    }
    // EVEX.
    for m in 1..=7u8 {
        let rxb: &[u8] = if is64 { &[0xE0, 0x00] } else { &[0xE0] };
        if m == 4 && !is64 {
            continue;
        }
        let p2s: Vec<u8> = if m == 4 {
            // APX: ND (b), V4, NF (aaa bit 2), and CCMP/CTEST's condition.
            (0..32).collect()
        } else {
            let mut v = Vec::new();
            for l in 0..3u8 {
                for b in 0..2u8 {
                    for (z, aaa) in [(0u8, 0u8), (0, 1), (1, 1)] {
                        v.push(z << 7 | l << 5 | b << 4 | 0x08 | aaa);
                    }
                }
            }
            v
        };
        for w in 0..2u8 {
            let mut batch = Vec::new();
            for pp in 0..4u8 {
                for vvvv in [0xFu8, 0x5] {
                    let p1 = w << 7 | vvvv << 3 | 0x04 | pp;
                    for &p2 in &p2s {
                        for &r in rxb {
                            // (R' stays set: EVEX.R' is APX's R4.)
                            let p0 = (r & 0xE0) | 0x10 | m;
                            for opcode in 0..=255u8 {
                                for tail in &some_modrm {
                                    let mut v = vec![0x62, p0, p1, p2, opcode];
                                    v.extend_from_slice(tail);
                                    v.extend_from_slice(&FILLER);
                                    batch.push(v);
                                }
                            }
                        }
                    }
                }
            }
            out.push(batch);
        }
    }
    out
}

/// A form's best register-form and memory-form encodings.
#[derive(Default, Clone)]
pub struct Found {
    pub reg: Option<(i32, Vec<u8>)>,
    pub mem: Option<(i32, Vec<u8>)>,
}

impl Found {
    fn offer(&mut self, mem: bool, score: i32, bytes: &[u8]) {
        let slot = if mem { &mut self.mem } else { &mut self.reg };
        if slot.as_ref().is_none_or(|(s, _)| score > *s) {
            *slot = Some((score, bytes.to_vec()));
        }
    }
}

/// How well an encoding shows which registers an instruction reads and
/// writes: its explicit registers (and address registers) should differ
/// from one another and from the low eight general-purpose registers and
/// xmm0, which instructions use implicitly.
fn score(inst: &turbonop_x86::Instruction) -> i32 {
    let mut regs = Vec::new();
    for op in inst.operands() {
        match op {
            Operand::Reg(r) => regs.push(r.full()),
            Operand::Mem(m) => {
                regs.extend([m.base, m.index].into_iter().flatten().map(Register::full));
            }
            _ => {}
        }
    }
    let mut score = 0;
    for (i, r) in regs.iter().enumerate() {
        if regs[..i].contains(r) {
            score -= 10;
        }
        let low = match r.class() {
            RegClass::Gpr64 | RegClass::Zmm => r.index() < 8,
            _ => false,
        };
        if low {
            score -= 1;
        }
    }
    score
}

/// The best encodings found of each form, by form, in `mode` (enumerated
/// once).
pub fn found(mode: Mode) -> &'static BTreeMap<u16, Found> {
    static FOUND: [OnceLock<BTreeMap<u16, Found>>; 2] = [OnceLock::new(), OnceLock::new()];
    let i = usize::from(mode == Mode::Bits32);
    FOUND[i].get_or_init(|| enumerate(mode))
}

fn enumerate(mode: Mode) -> BTreeMap<u16, Found> {
    let batches = batches(mode);
    let results: Vec<BTreeMap<u16, Found>> = std::thread::scope(|s| {
        let handles: Vec<_> = batches
            .iter()
            .map(|batch| {
                s.spawn(move || {
                    let mut found: BTreeMap<u16, Found> = BTreeMap::new();
                    for bytes in batch {
                        let Ok(inst) = decode(bytes, mode, 0x1000) else {
                            continue;
                        };
                        let mem = inst.operands().iter().any(|o| matches!(o, Operand::Mem(_)));
                        found.entry(inst.form()).or_default().offer(
                            mem,
                            score(&inst),
                            &bytes[..inst.len()],
                        );
                    }
                    found
                })
            })
            .collect();
        handles.into_iter().map(|h| h.join().unwrap()).collect()
    });
    let mut all: BTreeMap<u16, Found> = BTreeMap::new();
    for batch in results {
        for (form, f) in batch {
            let slot = all.entry(form).or_default();
            for (mem, sample) in [(false, f.reg), (true, f.mem)] {
                if let Some((score, bytes)) = sample {
                    slot.offer(mem, score, &bytes);
                }
            }
        }
    }
    all
}

fn hex(bytes: &[u8]) -> String {
    bytes
        .iter()
        .map(|b| format!("{b:02X}"))
        .collect::<Vec<_>>()
        .join(" ")
}

/// The sample encodings, one per line (`<mode> <hex>`): each form's
/// encodings in 64-bit mode, and in 32-bit mode those of forms not found in
/// 64-bit mode.
pub fn samples() -> Vec<String> {
    let (f64, f32) = (found(Mode::Bits64), found(Mode::Bits32));
    let mut lines = Vec::new();
    for (mode, found) in [("64", f64), ("32", f32)] {
        for (form, f) in found {
            if mode == "32" && f64.contains_key(form) {
                continue;
            }
            for (_, bytes) in [&f.reg, &f.mem].into_iter().flatten() {
                lines.push(format!("{mode} {}", hex(bytes)));
            }
        }
    }
    lines
}

/// Writes the samples for the XED oracle (`tools/x86access`).
#[test]
#[ignore]
fn generate_samples() {
    let path = std::env::var("SAMPLES").unwrap_or_else(|_| "access-samples.txt".to_string());
    std::fs::write(&path, samples().join("\n") + "\n").unwrap();
}

/// Forms no encoding selects: `91+r` also makes a `98` form, which
/// `cbw`/`cwde`/`cdqe` take.
pub const UNREACHABLE: &[&str] = &["00-legacy.txt:149 xchg rAX,Zv"];

#[test]
fn every_form_is_reachable() {
    let (f64, f32) = (found(Mode::Bits64), found(Mode::Bits32));
    let count = u16::try_from(turbonop_x86::form_count()).unwrap();
    let missing: Vec<&str> = (0..count)
        .filter(|f| !f64.contains_key(f) && !f32.contains_key(f))
        .map(|f| turbonop_x86::form_origin(f).unwrap())
        .filter(|o| !UNREACHABLE.contains(o))
        .collect();
    assert!(
        missing.is_empty(),
        "{} forms unreached:\n{}",
        missing.len(),
        missing.join("\n")
    );
}
