//! Every word in golden.txt (tools/mips/oracle.py: encodings of each
//! instruction, and random words, as llvm-objdump writes them in several
//! configurations) decoded and written the same, or not decoded where
//! LLVM decodes nothing.

use turbonop_mips::format::format;
use turbonop_mips::{Features, decode};

/// Each configuration's byte order and features (as llvm-objdump has
/// them: its default CPU's, the ISA level's, and those given it).
fn config(name: &str) -> (bool, Features) {
    let (big, names): (bool, &[&str]) = match name {
        "MIPS32R2" => (true, &["mips32", "mips32r2"]),
        "MIPS64R2EL" => (false, &["mips64", "mips64r2"]),
        "MIPS32R6EL" => (false, &["mips32", "mips32r6"]),
        "MIPS64R6" => (true, &["mips64", "mips64r6"]),
        "MIPS32R2EL+ASE" => (
            false,
            &[
                "mips32", "mips32r2", "msa", "dsp", "dspr2", "virt", "mt", "eva", "mcu",
            ],
        ),
        "MIPS64R6+ASE" => (
            true,
            &[
                "mips64", "mips64r6", "msa", "dsp", "dspr2", "dspr3", "virt", "mt", "eva", "crc",
                "ginv",
            ],
        ),
        "OCTEON" => (true, &["mips64", "mips64r2", "cnmips", "cnmipsp"]),
        _ => panic!("{name}"),
    };
    (big, Features::of(names))
}

#[test]
fn llvm_text() {
    let golden = include_str!("golden.txt");
    let mut wrong = Vec::new();
    let mut count = 0;
    let mut configs = std::collections::HashMap::new();
    for line in golden
        .lines()
        .filter(|l| !l.starts_with('#') && !l.is_empty())
    {
        let (head, want) = line.split_once('\t').unwrap();
        let mut it = head.split_whitespace();
        let (name, word, at) = (it.next().unwrap(), it.next().unwrap(), it.next().unwrap());
        let (big, f) = configs.entry(name).or_insert_with(|| config(name));
        let w = u32::from_str_radix(word, 16).unwrap();
        let at = u64::from_str_radix(at, 16).unwrap();
        let bytes = if *big {
            w.to_be_bytes()
        } else {
            w.to_le_bytes()
        };
        let got = match decode(&bytes, *big, f) {
            Some(i) => format(&i, at, f),
            None => "<unknown>".into(),
        };
        let norm = |s: &str| s.split_whitespace().collect::<Vec<_>>().join(" ");
        count += 1;
        if norm(&got) != norm(want) {
            wrong.push(format!(
                "{name} {word}: want `{want}`, got `{}`",
                norm(&got)
            ));
        }
    }
    assert!(count > 10_000, "{count}");
    assert!(
        wrong.is_empty(),
        "{} of {count} wrong:\n{}",
        wrong.len(),
        wrong[..wrong.len().min(80)].join("\n")
    );
}

/// Each microMIPS configuration's byte order and features.
fn mm_config(name: &str) -> (bool, Features) {
    let (big, names): (bool, &[&str]) = match name {
        "MM32R2" => (true, &["mips32", "mips32r2", "micromips"]),
        "MM32R2EL" => (false, &["mips32", "mips32r2", "micromips"]),
        "MM32R6" => (true, &["mips32", "mips32r6", "micromips"]),
        "MM32R6EL" => (false, &["mips32", "mips32r6", "micromips"]),
        "MM32R2+ASE" => (
            false,
            &[
                "mips32",
                "mips32r2",
                "micromips",
                "dsp",
                "dspr2",
                "fp64",
                "virt",
                "mt",
                "eva",
            ],
        ),
        _ => panic!("{name}"),
    };
    (big, Features::of(names))
}

/// Every item in golden-micromips.txt (tools/mips/oracle.py: encodings of
/// each microMIPS instruction, and random halfwords) decoded and written
/// as llvm-objdump writes it, or not decoded where it decodes nothing.
#[test]
fn llvm_micromips_text() {
    let golden = include_str!("golden-micromips.txt");
    let mut wrong = Vec::new();
    let mut count = 0;
    let mut configs = std::collections::HashMap::new();
    for line in golden
        .lines()
        .filter(|l| !l.starts_with('#') && !l.is_empty())
    {
        let (head, want) = line.split_once('\t').unwrap();
        let mut it = head.split_whitespace();
        let (name, item, at) = (it.next().unwrap(), it.next().unwrap(), it.next().unwrap());
        let (big, f) = configs.entry(name).or_insert_with(|| mm_config(name));
        let v = u32::from_str_radix(item, 16).unwrap();
        let at = u64::from_str_radix(at, 16).unwrap();
        let mut bytes = Vec::new();
        let [a, b, c, d] = v.to_be_bytes();
        for h in [
            u16::from_be_bytes([a, b]),
            u16::from_be_bytes([c, d]),
            0x0C00,
            0x0C00,
        ] {
            bytes.extend(if *big {
                h.to_be_bytes()
            } else {
                h.to_le_bytes()
            });
        }
        let got = match turbonop_mips::decode_micromips(&bytes, *big, f) {
            Some(i) => format(&i, at, f),
            None => "<unknown>".into(),
        };
        let norm = |s: &str| s.split_whitespace().collect::<Vec<_>>().join(" ");
        count += 1;
        if norm(&got) != norm(want) {
            wrong.push(format!(
                "{name} {item}: want `{want}`, got `{}`",
                norm(&got)
            ));
        }
    }
    assert!(count > 10_000, "{count}");
    assert!(
        wrong.is_empty(),
        "{} of {count} wrong:\n{}",
        wrong.len(),
        wrong[..wrong.len().min(120)].join("\n")
    );
}

// (Tests allocate with mimalloc, as the programs do.)
#[cfg(not(target_family = "wasm"))]
#[global_allocator]
static TEST_ALLOCATOR: mimalloc::MiMalloc = mimalloc::MiMalloc;
