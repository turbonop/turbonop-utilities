//! The library check's stages, one at a time, for measuring each one's
//! CPU time from outside (run it once per stage and subtract):
//!
//!     stages <stage> <original.lib> <rewritten.lib>
//!
//! Stage 0 maps the two files; 1 also reads the original's members; 2 the
//! rewritten's too; 3 compares them as `turbolib verify` does. Each prints
//! its wall time on standard error.
use turbonop_global_alloc as _;
use turbonop_verify::library as v;
fn main() {
    let a: Vec<String> = std::env::args().collect();
    if a.len() != 4 {
        eprintln!("usage: stages <0-3> <original.lib> <rewritten.lib>");
        std::process::exit(2);
    }
    let stage: u32 = a[1].parse().unwrap();
    let o = turbonop_mmap::Mmap::open(std::path::Path::new(&a[2])).unwrap();
    let r = turbonop_mmap::Mmap::open(std::path::Path::new(&a[3])).unwrap();
    let opts = v::Options::default();
    if stage == 0 {
        return;
    }
    let t = std::time::Instant::now();
    let ro = v::read_rewritten(&o, &opts).unwrap();
    eprintln!("read original {:?}", t.elapsed());
    if stage == 1 {
        std::process::exit(0);
    }
    let t = std::time::Instant::now();
    let rr = v::read_rewritten(&r, &opts).unwrap();
    eprintln!("read rewritten {:?}", t.elapsed());
    if stage == 2 {
        std::process::exit(0);
    }
    let t = std::time::Instant::now();
    let rep = v::compare(&ro, &rr, &opts);
    eprintln!("compare {:?}", t.elapsed());
    drop(rep);
    std::process::exit(0);
}
