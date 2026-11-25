//! Decodes a file's bytes from `offset` for `length` (hexadecimal) as
//! 64-bit code, linearly, and reports how fast: the decoder's throughput.
//!
//!     cargo run --release -p turbonop-x86 --example sweep -- clang.exe 400 630EF97

use turbonop_global_alloc as _;

use turbonop_x86::{Mode, decode};

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let [path, offset, length] = &args[..] else {
        eprintln!("usage: sweep <file> <offset> <length> (hexadecimal)");
        std::process::exit(2);
    };
    let hex = |s: &str| usize::from_str_radix(s.trim_start_matches("0x"), 16).unwrap_or(0);
    let data = std::fs::read(path).unwrap_or_default();
    let (from, len) = (hex(offset), hex(length));
    let code = &data[from.min(data.len())..(from + len).min(data.len())];
    let start = std::time::Instant::now();
    let (mut at, mut count, mut invalid) = (0usize, 0u64, 0u64);
    while at < code.len() {
        match decode(&code[at..], Mode::Bits64, at as u64) {
            Ok(i) => {
                at += i.len().max(1);
                count += 1;
            }
            Err(_) => {
                at += 1;
                invalid += 1;
            }
        }
    }
    let t = start.elapsed();
    println!(
        "{count} instructions ({invalid} invalid bytes) in {t:?}: {:.1} ns each",
        t.as_nanos() as f64 / count.max(1) as f64
    );
}
