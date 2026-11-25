//! Prints how turbonop-arm64 decodes words: the text, each operand and its
//! access, and the registers read and written.
//! `cargo run -p turbonop-arm64 --example operands -- A9BF7BFD F9400420`

fn main() {
    for arg in std::env::args().skip(1) {
        let word = u32::from_str_radix(arg.trim_start_matches("0x"), 16).expect("a hex word");
        let Some(inst) = turbonop_arm64::decode(word, 0x1000) else {
            println!("{word:08X}: not an instruction");
            continue;
        };
        println!(
            "{word:08X}: {} ({}, LLVM's {})",
            turbonop_arm64::format(&inst),
            inst.encoding_name(),
            inst.llvm_record()
        );
        for op in &inst.operands() {
            println!("    {:?} {:?}", op.access, op.kind);
        }
        println!(
            "    writes {:?}\n    reads {:?}\n    {:?}",
            inst.written_registers().iter().collect::<Vec<_>>(),
            inst.read_registers().iter().collect::<Vec<_>>(),
            inst.effects()
        );
    }
}
