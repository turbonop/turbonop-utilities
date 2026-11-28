//! Words dumpbin's golden file has none of, as LLVM's disassembler
//! (llvm-mc, `-mattr=+bti,+sme`) writes them: BTI's targets, and SMSTART's
//! and SMSTOP's (MSR SVCRSM, SVCRZA, SVCRSMZA). `None`: LLVM writes them
//! only as an unallocated PSTATE field's MSR (`msr S0_3_C4_C1_3, xzr`).

const CASES: &[(u32, Option<&str>)] = &[
    (0xD503_241F, Some("bti")),
    (0xD503_245F, Some("bti c")),
    (0xD503_249F, Some("bti j")),
    (0xD503_24DF, Some("bti jc")),
    (0xD503_437F, Some("smstart sm")),
    (0xD503_457F, Some("smstart za")),
    (0xD503_477F, Some("smstart")),
    (0xD503_427F, Some("smstop sm")),
    (0xD503_447F, Some("smstop za")),
    (0xD503_467F, Some("smstop")),
    (0xD503_417F, None),
    (0xD503_407F, None),
    (0xD503_4B7F, None),
];

#[test]
fn as_llvm_writes_them() {
    let mut failures = Vec::new();
    for &(word, want) in CASES {
        let got = turbonop_arm64::decode(word).ok().map(|i| {
            turbonop_arm64::format(&i, 0)
                .split_whitespace()
                .collect::<Vec<_>>()
                .join(" ")
        });
        if got.as_deref() != want {
            failures.push(format!("{word:08X}: expected {want:?}, got {got:?}"));
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}
