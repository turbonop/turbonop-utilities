//! THIRD_PARTY_NOTICES.txt, compressed (deflate), for `hexcavator licenses`.

use std::path::PathBuf;

fn main() {
    println!("cargo::rerun-if-changed=THIRD_PARTY_NOTICES.txt");
    let text = std::fs::read("THIRD_PARTY_NOTICES.txt").expect("THIRD_PARTY_NOTICES.txt");
    let packed = miniz_oxide::deflate::compress_to_vec(&text, 10);
    let out = PathBuf::from(std::env::var_os("OUT_DIR").expect("OUT_DIR"));
    std::fs::write(out.join("notices.deflate"), packed).expect("notices.deflate");
}
