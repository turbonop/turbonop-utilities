//! Every word in golden.txt (tools/loongarch/oracle.py: encodings of each
//! instruction, and random words, as llvm-objdump writes them on LA32 and
//! LA64) decoded and written the same, or not decoded where LLVM decodes
//! nothing.

use turbonop_loongarch::format::format;
use turbonop_loongarch::{Features, decode};

#[test]
fn llvm_text() {
    let golden = include_str!("golden.txt");
    let la32 = Features::la32();
    let la64 = Features::la64();
    let mut wrong = Vec::new();
    let mut count = 0;
    for line in golden
        .lines()
        .filter(|l| !l.starts_with('#') && !l.is_empty())
    {
        let (head, want) = line.split_once('\t').unwrap();
        let mut it = head.split_whitespace();
        let (base, word, at) = (it.next().unwrap(), it.next().unwrap(), it.next().unwrap());
        let w = u32::from_str_radix(word, 16).unwrap();
        let at = u64::from_str_radix(at, 16).unwrap();
        let f = if base == "LA64" { &la64 } else { &la32 };
        let got = match decode(&w.to_le_bytes(), f) {
            Some(i) => format(&i, at, f),
            None => "<unknown>".into(),
        };
        let norm = |s: &str| s.split_whitespace().collect::<Vec<_>>().join(" ");
        count += 1;
        if norm(&got) != norm(want) {
            wrong.push(format!("{base} {word}: want `{want}`, got `{got}`"));
        }
    }
    assert!(count > 10_000, "{count}");
    assert!(
        wrong.is_empty(),
        "{} of {count} wrong:\n{}",
        wrong.len(),
        wrong[..wrong.len().min(60)].join("\n")
    );
}

// (Tests allocate with mimalloc, as the programs do.)
#[cfg(not(target_family = "wasm"))]
#[global_allocator]
static TEST_ALLOCATOR: mimalloc::MiMalloc = mimalloc::MiMalloc;
