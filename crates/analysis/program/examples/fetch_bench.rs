//! Measures fetching from a symbol server, as a dump's first open does.
//!
//! `cargo run --release -p turbonop-program --example fetch_bench -- <command>`:
//!
//! - `keys <dir>... -o <file>`: the symbol server keys of every PE image in
//!   `dirs` (not their subdirectories), one per line: `pdb <key>` for its
//!   PDBs and `image <key>` for the image itself.
//! - `curl <keys> [-n N] [-kind pdb|image|all] [-parallel P] [-server URL]
//!   [-http1.1|-http2] [-keep DIR] [-tsv FILE] [-curl PATH]`: fetches the
//!   first `N` keys (evenly spaced through the file) by one `curl
//!   --parallel`, as `turbonop_program::server::fetch_batch` does. It writes
//!   what each transfer took (from curl's `--write-out`) to `FILE` and
//!   prints a summary. What's fetched is thrown away unless `-keep` says
//!   where to put it.
//!
//! - `streams <dir>`: what the PDBs in `dir` (as `-keep` left them) are
//!   made of, by kind of stream: how much of each a debugger reads first.
//!
//! Results name this machine's binaries: keep them out of the repository.

use turbonop_global_alloc as _;

use std::fmt::Write as _;
use std::path::{Path, PathBuf};
use std::time::Instant;

use turbonop_image::Image;
use turbonop_program::server::{self, ImageKey};

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match args.first().map(String::as_str) {
        Some("keys") => keys(&args[1..]),
        Some("curl") => curl(&args[1..]),
        Some("streams") => streams(&args[1..]),
        _ => {
            eprintln!("usage: fetch_bench keys <dir>... -o <file> | curl <keys> [options]");
            std::process::exit(2);
        }
    }
}

/// The value after `flag`, if given.
fn option<'a>(args: &'a [String], flag: &str) -> Option<&'a str> {
    let i = args.iter().position(|a| a == flag)?;
    args.get(i + 1).map(String::as_str)
}

fn keys(args: &[String]) {
    let out = option(args, "-o").expect("-o <file>");
    let mut dirs = Vec::new();
    let mut skip = false;
    for a in args {
        if std::mem::take(&mut skip) {
            continue;
        }
        if a == "-o" {
            skip = true;
            continue;
        }
        dirs.push(PathBuf::from(a));
    }
    let mut files: Vec<PathBuf> = dirs
        .iter()
        .flat_map(|d| std::fs::read_dir(d).into_iter().flatten().flatten())
        .map(|e| e.path())
        .filter(|p| {
            p.extension().and_then(|e| e.to_str()).is_some_and(|e| {
                ["dll", "exe", "sys", "drv", "ocx", "cpl"]
                    .iter()
                    .any(|x| e.eq_ignore_ascii_case(x))
            })
        })
        .collect();
    files.sort();
    let t = Instant::now();
    let lines = turbonop_par::map(&files, 8, |p| file_keys(p));
    let mut text = String::new();
    let (mut pdbs, mut images) = (0, 0);
    for l in lines.iter().flatten() {
        if l.starts_with("pdb ") {
            pdbs += 1;
        } else {
            images += 1;
        }
        let _ = writeln!(text, "{l}");
    }
    std::fs::write(out, text).expect("writable");
    eprintln!(
        "{} files: {pdbs} PDB keys, {images} image keys, in {:?}",
        files.len(),
        t.elapsed()
    );
}

/// `path`'s keys: its PDBs', then its own.
fn file_keys(path: &Path) -> Vec<String> {
    let Ok(map) = turbonop_mmap::Mmap::open(path) else {
        return Vec::new();
    };
    let Ok(image) = Image::parse(map.as_slice()) else {
        return Vec::new();
    };
    let mut keys: Vec<String> = image
        .debug_ids()
        .iter()
        .filter_map(server::ssqp_key)
        .map(|k| format!("pdb {k}"))
        .collect();
    if let Some(pe) = image.pe()
        && let Some(h) = pe.optional_header()
        && let Some(name) = path.file_name().and_then(|n| n.to_str())
    {
        let key = ImageKey::Pe {
            name: name.to_owned(),
            time_date_stamp: pe.file_header().time_date_stamp.get(),
            size_of_image: h.size_of_image(),
        };
        if let Some(k) = server::image_ssqp_key(&key) {
            keys.push(format!("image {k}"));
        }
    }
    keys
}

/// What curl said of one transfer.
#[derive(Default)]
struct Transfer {
    num: usize,
    code: u32,
    exit: u32,
    bytes: u64,
    redirects: u32,
    connects: u32,
    dns: f64,
    connect: f64,
    tls: f64,
    redirect: f64,
    first_byte: f64,
    total: f64,
    version: String,
    host: String,
}

const WRITE_OUT: &str = "%{urlnum}\t%{http_code}\t%{exitcode}\t%{size_download}\t%{num_redirects}\t%{num_connects}\t%{time_namelookup}\t%{time_connect}\t%{time_appconnect}\t%{time_redirect}\t%{time_starttransfer}\t%{time_total}\t%{http_version}\t%{url_effective}\\n";

fn curl(args: &[String]) {
    let keys_path = args.first().expect("a keys file");
    let n: usize = option(args, "-n").map_or(usize::MAX, |s| s.parse().expect("-n N"));
    let kind = option(args, "-kind").unwrap_or("pdb");
    let parallel = option(args, "-parallel").unwrap_or("64");
    let base = option(args, "-server").unwrap_or("https://msdl.microsoft.com/download/symbols");
    let keep = option(args, "-keep").map(PathBuf::from);
    let tsv = option(args, "-tsv");
    let curl = option(args, "-curl").unwrap_or("curl");
    let text = std::fs::read_to_string(keys_path).expect("a keys file");
    let mut all: Vec<&str> = text
        .lines()
        .filter_map(|l| l.split_once(' '))
        .filter(|(k, _)| kind == "all" || *k == kind)
        .map(|(_, key)| key)
        .collect();
    all.sort_unstable();
    all.dedup();
    // (Evenly spaced, so a small run is a sample of the whole.)
    let picked: Vec<&str> = if n >= all.len() {
        all
    } else {
        (0..n).map(|i| all[i * all.len() / n]).collect()
    };
    let scratch = std::env::temp_dir().join(format!("fetch-bench-{}", std::process::id()));
    std::fs::create_dir_all(&scratch).expect("a scratch directory");
    let null = if cfg!(windows) { "NUL" } else { "/dev/null" };
    let mut config = String::new();
    for (i, key) in picked.iter().enumerate() {
        let _ = writeln!(config, "url = \"{base}/{key}\"");
        let out = match &keep {
            Some(dir) => {
                let p = dir
                    .join(key.replace('/', "_"))
                    .with_extension(format!("{i}"));
                p.to_string_lossy().replace('\\', "/")
            }
            None => null.to_owned(),
        };
        let _ = writeln!(config, "output = \"{out}\"");
    }
    if let Some(dir) = &keep {
        std::fs::create_dir_all(dir).expect("-keep's directory");
    }
    let config_path = scratch.join("config");
    std::fs::write(&config_path, config).expect("writable");
    let mut cmd = std::process::Command::new(curl);
    cmd.args([
        "--parallel",
        "--parallel-max",
        parallel,
        "--silent",
        "--location",
        "--max-time",
        "600",
        "--user-agent",
        "turbonop-fetch-bench",
        "--write-out",
        WRITE_OUT,
    ]);
    if args.iter().any(|a| a == "-http1.1") {
        cmd.arg("--http1.1");
    }
    if args.iter().any(|a| a == "-http2") {
        cmd.arg("--http2");
    }
    for extra in args.iter().filter_map(|a| a.strip_prefix("-curl-arg=")) {
        cmd.arg(extra);
    }
    cmd.arg("--config").arg(&config_path);
    let t = Instant::now();
    let out = cmd.output().expect("curl runs");
    let wall = t.elapsed().as_secs_f64();
    let _ = std::fs::remove_dir_all(&scratch);
    let stdout = String::from_utf8_lossy(&out.stdout);
    let mut transfers: Vec<Transfer> = Vec::new();
    for line in stdout.lines() {
        let f: Vec<&str> = line.split('\t').collect();
        if f.len() < 14 {
            continue;
        }
        let num = |i: usize| f[i].parse::<f64>().unwrap_or(0.0);
        let host = f[13]
            .split("://")
            .nth(1)
            .and_then(|r| r.split('/').next())
            .unwrap_or("")
            .to_owned();
        transfers.push(Transfer {
            num: f[0].parse().unwrap_or(0),
            code: f[1].parse().unwrap_or(0),
            exit: f[2].parse().unwrap_or(0),
            bytes: f[3].parse().unwrap_or(0),
            redirects: f[4].parse().unwrap_or(0),
            connects: f[5].parse().unwrap_or(0),
            dns: num(6),
            connect: num(7),
            tls: num(8),
            redirect: num(9),
            first_byte: num(10),
            total: num(11),
            version: f[12].to_owned(),
            host,
        });
    }
    if let Some(path) = tsv {
        let mut s = String::from(
            "code\texit\tbytes\tredirects\tconnects\tdns\tconnect\ttls\tredirect\tfirst_byte\ttotal\tversion\thost\n",
        );
        for t in &transfers {
            let _ = writeln!(
                s,
                "{}\t{}\t{}\t{}\t{}\t{}\t{:.4}\t{:.4}\t{:.4}\t{:.4}\t{:.4}\t{:.4}\t{}\t{}",
                picked.get(t.num).copied().unwrap_or(""),
                t.code,
                t.exit,
                t.bytes,
                t.redirects,
                t.connects,
                t.dns,
                t.connect,
                t.tls,
                t.redirect,
                t.first_byte,
                t.total,
                t.version,
                t.host
            );
        }
        std::fs::write(path, s).expect("writable");
    }
    summarize(&transfers, picked.len(), wall);
}

fn percentiles(mut v: Vec<f64>) -> String {
    if v.is_empty() {
        return "-".into();
    }
    v.sort_by(f64::total_cmp);
    let p = |percent: usize| v[(v.len() - 1) * percent / 100];
    format!(
        "p50 {:.3}  p90 {:.3}  p99 {:.3}  max {:.3}",
        p(50),
        p(90),
        p(99),
        p(100)
    )
}

fn summarize(t: &[Transfer], asked: usize, wall: f64) {
    let ok: Vec<&Transfer> = t.iter().filter(|t| t.code == 200 && t.exit == 0).collect();
    let missing = t.iter().filter(|t| t.code == 404).count();
    let failed = t.len() - ok.len() - missing;
    let bytes: u64 = t.iter().map(|t| t.bytes).sum();
    let connects: u32 = t.iter().map(|t| t.connects).sum();
    let redirected = t.iter().filter(|t| t.redirects > 0).count();
    println!(
        "{asked} asked, {} reported: {} found, {missing} not found, {failed} failed",
        t.len(),
        ok.len()
    );
    println!(
        "wall {wall:.2} s; {:.1} MB, {:.1} MB/s; {connects} connections opened; {redirected} redirected",
        bytes as f64 / 1e6,
        bytes as f64 / 1e6 / wall
    );
    let mut versions: Vec<(String, usize)> = Vec::new();
    let mut hosts: Vec<(String, usize)> = Vec::new();
    for x in t {
        match versions.iter_mut().find(|(v, _)| *v == x.version) {
            Some(e) => e.1 += 1,
            None => versions.push((x.version.clone(), 1)),
        }
        match hosts.iter_mut().find(|(h, _)| *h == x.host) {
            Some(e) => e.1 += 1,
            None => hosts.push((x.host.clone(), 1)),
        }
    }
    println!("HTTP versions: {versions:?}");
    hosts.sort_by_key(|a| std::cmp::Reverse(a.1));
    println!("final hosts: {:?}", &hosts[..hosts.len().min(5)]);
    let col = |f: fn(&Transfer) -> f64, set: &[&Transfer]| {
        percentiles(set.iter().map(|t| f(t)).collect())
    };
    let all: Vec<&Transfer> = t.iter().collect();
    let miss: Vec<&Transfer> = t.iter().filter(|t| t.code == 404).collect();
    println!("found, seconds:");
    println!("  dns         {}", col(|t| t.dns, &ok));
    println!("  connect     {}", col(|t| t.connect, &ok));
    println!("  tls         {}", col(|t| t.tls, &ok));
    println!("  redirect    {}", col(|t| t.redirect, &ok));
    println!("  first byte  {}", col(|t| t.first_byte, &ok));
    println!("  total       {}", col(|t| t.total, &ok));
    println!("  body        {}", col(|t| t.total - t.first_byte, &ok));
    println!(
        "  size MB     {}",
        percentiles(ok.iter().map(|t| t.bytes as f64 / 1e6).collect())
    );
    if !miss.is_empty() {
        println!("not found, total seconds: {}", col(|t| t.total, &miss));
    }
    let _ = all;
}

/// The kinds of stream a PDB holds, by what reads them.
const KINDS: [&str; 10] = [
    "types (TPI)",
    "ids (IPI)",
    "DBI",
    "module symbols and lines",
    "symbol records",
    "globals hash",
    "publics hash",
    "frame data and FPO",
    "section headers and other DBI streams",
    "other (names, hashes, MSF directory)",
];

fn streams(args: &[String]) {
    let dir = args.first().expect("a directory");
    let files: Vec<PathBuf> = std::fs::read_dir(dir)
        .expect("readable")
        .flatten()
        .map(|e| e.path())
        .collect();
    let sums = turbonop_par::map(&files, 1, |p| {
        let mut kinds = [0u64; KINDS.len()];
        let Ok(data) = std::fs::read(p) else {
            return None;
        };
        let Ok(pdb) = turbonop_pdb::Pdb::parse(&data) else {
            return None;
        };
        let msf = pdb.msf();
        let mut kind_of = vec![9usize; msf.stream_count()];
        let mut set = |s: u32, k: usize| {
            if let Some(slot) = kind_of.get_mut(s as usize) {
                *slot = k;
            }
        };
        set(2, 0);
        set(4, 1);
        set(3, 2);
        if let Ok(dbi) = pdb.dbi() {
            for m in dbi.modules() {
                if let Some(s) = m.stream() {
                    set(s, 3);
                }
            }
            set(dbi.symbol_records_stream(), 4);
            set(dbi.globals_stream(), 5);
            set(dbi.publics_stream(), 6);
            for which in 0..11 {
                if let Some(s) = dbi.debug_stream(which) {
                    set(s, if which == 0 || which == 9 { 7 } else { 8 });
                }
            }
        }
        for (s, k) in kind_of.iter().enumerate() {
            kinds[*k] += u64::from(
                msf.stream_size(u32::try_from(s).unwrap_or(u32::MAX))
                    .unwrap_or(0),
            );
        }
        let streamed: u64 = kinds.iter().sum();
        kinds[9] += (data.len() as u64).saturating_sub(streamed);
        Some((data.len() as u64, kinds))
    });
    let mut total = 0u64;
    let mut kinds = [0u64; KINDS.len()];
    let mut n = 0;
    for (size, k) in sums.into_iter().flatten() {
        n += 1;
        total += size;
        for (a, b) in kinds.iter_mut().zip(k) {
            *a += b;
        }
    }
    println!("{n} PDBs, {:.1} MB", total as f64 / 1e6);
    for (name, k) in KINDS.iter().zip(kinds) {
        println!(
            "  {name:40} {:8.1} MB  {:5.1}%",
            k as f64 / 1e6,
            100.0 * k as f64 / total.max(1) as f64
        );
    }
}
