//! Every encoding in golden.txt (tools/powerpc/oracle.py: encodings of
//! each instruction, random words, and random prefixed pairs, as
//! llvm-objdump writes them on 64-bit little- and big-endian and 32-bit
//! PowerPC) decoded and written the same, or not decoded where LLVM
//! decodes nothing.

use turbonop_global_alloc as _;

use turbonop_powerpc::format::format;
use turbonop_powerpc::{Features, decode};

#[test]
fn llvm_text() {
    let golden = include_str!("golden.txt");
    let ppc32 = Features::standard(false);
    let ppc64 = Features::standard(true);
    let mut wrong = Vec::new();
    let mut count = 0;
    for line in golden
        .lines()
        .filter(|l| !l.starts_with('#') && !l.is_empty())
    {
        let (head, want) = line.split_once('\t').unwrap();
        let mut it = head.split_whitespace();
        let (mode, size, word, at) = (
            it.next().unwrap(),
            it.next().unwrap(),
            it.next().unwrap(),
            it.next().unwrap(),
        );
        let w = u64::from_str_radix(word, 16).unwrap();
        let at = u64::from_str_radix(at, 16).unwrap();
        let big = mode != "PPC64LE";
        let f = if mode == "PPC32" { &ppc32 } else { &ppc64 };
        // (The words in memory: a prefixed instruction's prefix first.)
        let mut bytes = Vec::new();
        let words: Vec<u32> = if size == "8" {
            vec![
                u32::try_from(w >> 32).unwrap(),
                u32::try_from(w & 0xFFFF_FFFF).unwrap(),
            ]
        } else {
            vec![u32::try_from(w).unwrap()]
        };
        for x in words {
            bytes.extend(if big {
                x.to_be_bytes()
            } else {
                x.to_le_bytes()
            });
        }
        let got = match decode(&bytes, big, f) {
            Some(i) if usize::from(i.size) == bytes.len() => format(&i, at, f),
            // (A word of a pair that isn't a prefixed instruction: LLVM's
            // line for it is the first word's.)
            Some(i) if size == "8" => format(&i, at, f),
            _ => "<unknown>".into(),
        };
        let norm = |s: &str| s.split_whitespace().collect::<Vec<_>>().join(" ");
        count += 1;
        if norm(&got) != norm(want) {
            wrong.push(format!("{mode} {word}: want `{want}`, got `{got}`"));
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
