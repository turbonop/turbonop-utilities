//! Times reading a PDB's inline sites and attributing their bytes:
//! `cargo run --release -p turbonop-debuginfo --example inline_timing -- <pdb>`.

use turbonop_global_alloc as _;

fn main() {
    let path = std::env::args().nth(1).expect("a PDB");
    let data = std::fs::read(&path).expect("readable");
    let debug = turbonop_debuginfo::DebugInfo::parse(&data).expect("a PDB");
    let t = std::time::Instant::now();
    let found = debug.inline_sites().expect("inline sites");
    eprintln!("sites ({}): {:?}", found.sites.len(), t.elapsed());
    let t = std::time::Instant::now();
    let own = turbonop_debuginfo::exclusive_sizes(found.sites);
    eprintln!("exclusive ({}): {:?}", own.len(), t.elapsed());
}
