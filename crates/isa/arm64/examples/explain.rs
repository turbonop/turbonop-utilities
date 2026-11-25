//! How turbonop-arm64 decodes words: every encoding that matches, and how each
//! renders them, operand by operand.
//! `cargo run -p turbonop-arm64 --example explain -- 3C72F89F 7F7EE7AB`

use turbonop_global_alloc as _;

fn main() {
    for arg in std::env::args().skip(1) {
        let word = u32::from_str_radix(arg.trim_start_matches("0x"), 16).expect("a hex word");
        let shown = turbonop_arm64::decode(word, 0).map(|i| turbonop_arm64::format(&i));
        println!("{word:08X}: {shown:?}");
        print!("{}", turbonop_arm64::explain(word));
    }
}
