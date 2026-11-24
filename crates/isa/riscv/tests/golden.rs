//! Every word in golden.txt (tools/riscv/oracle.py: encodings of each
//! instruction, and random words, as llvm-objdump writes them on RV32 and
//! RV64 with the standard extensions on) decoded and written the same, or
//! not decoded where LLVM decodes nothing.

use turbonop_riscv::format::format;
use turbonop_riscv::{Features, decode};

#[test]
fn llvm_text() {
    let golden = include_str!("golden.txt");
    let rv32 = Features::standard(false);
    let rv64 = Features::standard(true);
    let mut wrong = Vec::new();
    let mut count = 0;
    for line in golden
        .lines()
        .filter(|l| !l.starts_with('#') && !l.is_empty())
    {
        let (head, want) = line.split_once('\t').unwrap();
        let mut it = head.split_whitespace();
        let (xlen, size, word, at) = (
            it.next().unwrap(),
            it.next().unwrap(),
            it.next().unwrap(),
            it.next().unwrap(),
        );
        let w = u32::from_str_radix(word, 16).unwrap();
        let at = u64::from_str_radix(at, 16).unwrap();
        let bytes = w.to_le_bytes();
        let bytes = if size == "2" { &bytes[..2] } else { &bytes[..] };
        let f = if xlen == "RV64" { &rv64 } else { &rv32 };
        let got = match decode(bytes, f) {
            Some(i) => format(&i, at, f),
            None => "<unknown>".into(),
        };
        let norm = |s: &str| s.split_whitespace().collect::<Vec<_>>().join(" ");
        count += 1;
        if norm(&got) != norm(want) {
            wrong.push(format!("{xlen} {word}: want `{want}`, got `{got}`"));
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
