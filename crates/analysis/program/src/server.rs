//! Debug information from symbol servers, when asked for: Microsoft's
//! (and anything that serves its layout, SSQP: `msdl.microsoft.com`,
//! `symstore` shares over HTTP, .NET's symbol servers) and debuginfod.
//!
//! Nothing is asked of a server unless servers are given
//! ([`set_servers`]: the tools' `--symbol-server` and `--debuginfod`, or
//! `TURBONOP_SYMBOL_SERVER` and `TURBONOP_DEBUGINFOD`). debuginfod's own
//! `DEBUGINFOD_URLS` is not followed: distributions set it for every
//! user, and asking a server tells it which builds are being looked at.
//!
//! Files are fetched with the system's `curl` (on Windows 10 and later,
//! macOS, and Linux), limited in size and time, and kept in memory, or
//! in the symbol cache if one is given (in the servers' layout, where the
//! lookup finds them next time: see `turbonop_debuginfo::candidate_paths_in`).
//! What a server sends is hostile like any input, and is used only if it
//! matches the image (`DebugInfo::matches`).

use std::path::{Path, PathBuf};
use std::sync::RwLock;

use turbonop_image::DebugId;
use turbonop_mmap::Mmap;

/// Where to ask for debug information, and where to keep what is found.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Servers {
    /// Servers with Microsoft's layout (SSQP), by base URL.
    pub symbol_servers: Vec<String>,
    /// debuginfod servers, by base URL.
    pub debuginfod: Vec<String>,
    /// Where to keep what is fetched (else it is kept in memory only).
    pub cache: Option<PathBuf>,
}

impl Servers {
    /// Whether there is anything to ask.
    pub fn is_empty(&self) -> bool {
        self.symbol_servers.is_empty() && self.debuginfod.is_empty()
    }

    /// The servers the environment names: `TURBONOP_SYMBOL_SERVER`
    /// (separated by `;`), `TURBONOP_DEBUGINFOD` (by spaces, as
    /// `DEBUGINFOD_URLS`), and `TURBONOP_SYMBOL_CACHE`.
    pub fn from_env() -> Self {
        let var = |n: &str| std::env::var(n).ok().filter(|v| !v.trim().is_empty());
        Servers {
            symbol_servers: var("TURBONOP_SYMBOL_SERVER")
                .map(|v| {
                    v.split(';')
                        .map(str::trim)
                        .filter(|s| !s.is_empty())
                        .map(String::from)
                        .collect()
                })
                .unwrap_or_default(),
            debuginfod: var("TURBONOP_DEBUGINFOD")
                .map(|v| v.split_whitespace().map(String::from).collect())
                .unwrap_or_default(),
            cache: var("TURBONOP_SYMBOL_CACHE").map(PathBuf::from),
        }
    }
}

static SERVERS: RwLock<Option<Servers>> = RwLock::new(None);

/// The servers to ask (none: nothing is asked).
pub fn set_servers(servers: Servers) {
    *SERVERS
        .write()
        .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(servers);
}

/// The servers set (else the environment's).
pub fn servers() -> Servers {
    SERVERS
        .read()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .clone()
        .unwrap_or_else(Servers::from_env)
}

/// The largest file fetched (a debug file can be large; a server that
/// sends more is refused).
pub const MAX_FILE: u64 = 4 << 30;

/// The longest a fetch may take, in seconds.
const MAX_SECONDS: u32 = 600;

/// The key a server with Microsoft's layout keeps `id`'s file under, if
/// it has one: `app.pdb/<GUID><age>/app.pdb`,
/// `_.debug/elf-buildid-sym-<id>/_.debug`,
/// `_.dwarf/mach-uuid-sym-<uuid>/_.dwarf`.
pub fn ssqp_key(id: &DebugId<'_>) -> Option<String> {
    let hex = |b: &[u8]| b.iter().map(|b| format!("{b:02x}")).collect::<String>();
    match *id {
        DebugId::Pdb { guid, age, path } => {
            let path = String::from_utf8_lossy(path);
            let name = path.rsplit(['/', '\\']).next()?.to_ascii_lowercase();
            if !safe_name(&name) {
                return None;
            }
            let key = turbonop_debuginfo::symbol_store_key(&guid, age);
            Some(format!("{name}/{key}/{name}"))
        }
        DebugId::Pdb20 {
            signature,
            age,
            path,
        } => {
            let path = String::from_utf8_lossy(path);
            let name = path.rsplit(['/', '\\']).next()?.to_ascii_lowercase();
            if !safe_name(&name) {
                return None;
            }
            Some(format!("{name}/{signature:08X}{age:x}/{name}"))
        }
        DebugId::BuildId(b) | DebugId::AltLink { build_id: b, .. } if b.len() >= 2 => {
            Some(format!("_.debug/elf-buildid-sym-{}/_.debug", hex(b)))
        }
        DebugId::Uuid(u) => Some(format!("_.dwarf/mach-uuid-sym-{}/_.dwarf", hex(&u))),
        _ => None,
    }
}

/// What identifies an image file (not its debug information) on a
/// symbol server: what a crash dump records of a module.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum ImageKey {
    /// A PE image: its file name, `TimeDateStamp`, and `SizeOfImage`.
    Pe {
        /// Its file name (or path).
        name: String,
        /// Its file header's `TimeDateStamp`.
        time_date_stamp: u32,
        /// Its optional header's `SizeOfImage`.
        size_of_image: u32,
    },
    /// An ELF image: its file name and build ID.
    BuildId {
        /// Its file name (or path).
        name: String,
        /// Its `NT_GNU_BUILD_ID` (or the prefix of it older Breakpad
        /// recorded).
        build_id: Vec<u8>,
    },
    /// A Mach-O image: its file name and UUID.
    Uuid {
        /// Its file name (or path).
        name: String,
        /// Its `LC_UUID`.
        uuid: [u8; 16],
    },
}

impl ImageKey {
    /// The image's file name (as the dump recorded it, its directories
    /// dropped).
    pub fn name(&self) -> &str {
        let (ImageKey::Pe { name, .. }
        | ImageKey::BuildId { name, .. }
        | ImageKey::Uuid { name, .. }) = self;
        name.rsplit(['/', '\\']).next().unwrap_or(name)
    }

    /// The key's index directory, the part between the names
    /// (`<TIMESTAMP><size>`, `elf-buildid-<id>`, `mach-uuid-<uuid>`).
    pub fn index(&self) -> String {
        let hex = |b: &[u8]| b.iter().map(|b| format!("{b:02x}")).collect::<String>();
        match self {
            ImageKey::Pe {
                time_date_stamp,
                size_of_image,
                ..
            } => format!("{time_date_stamp:08X}{size_of_image:x}"),
            ImageKey::BuildId { build_id, .. } => format!("elf-buildid-{}", hex(build_id)),
            ImageKey::Uuid { uuid, .. } => format!("mach-uuid-{}", hex(uuid)),
        }
    }
}

/// The key a server with Microsoft's layout keeps an image under:
/// `app.exe/<TIMESTAMP><size>/app.exe`, `libc.so.6/elf-buildid-<id>/libc.so.6`,
/// `App/mach-uuid-<uuid>/App` (names lowercase, as SSQP's keys are).
pub fn image_ssqp_key(key: &ImageKey) -> Option<String> {
    let name = key.name().to_ascii_lowercase();
    if !safe_name(&name) {
        return None;
    }
    if let ImageKey::BuildId { build_id, .. } = key
        && build_id.len() < 2
    {
        return None;
    }
    Some(format!("{name}/{}/{name}", key.index()))
}

/// The path a debuginfod server serves an image at, if it can (ELF only:
/// `buildid/<id>/executable`).
pub fn image_debuginfod_path(key: &ImageKey) -> Option<String> {
    match key {
        ImageKey::BuildId { build_id, .. } if build_id.len() >= 2 => {
            let hex: String = build_id.iter().map(|b| format!("{b:02x}")).collect();
            Some(format!("buildid/{hex}/executable"))
        }
        _ => None,
    }
}

/// Where the servers have an image: (URL, where the cache keeps it).
pub fn image_locations(servers: &Servers, key: &ImageKey) -> Vec<(String, Option<PathBuf>)> {
    let mut out = Vec::new();
    if let Some(k) = image_ssqp_key(key) {
        for s in &servers.symbol_servers {
            let cached = servers
                .cache
                .as_ref()
                .map(|c| k.split('/').fold(c.clone(), |p, part| p.join(part)));
            out.push((format!("{}/{k}", s.trim_end_matches('/')), cached));
        }
    }
    if let Some(path) = image_debuginfod_path(key) {
        for s in &servers.debuginfod {
            let cached = servers.cache.as_ref().map(|c| {
                path.trim_start_matches("buildid/")
                    .split('/')
                    .fold(c.clone(), |p, part| p.join(part))
            });
            out.push((format!("{}/{path}", s.trim_end_matches('/')), cached));
        }
    }
    out
}

/// The path a debuginfod server serves `id`'s debug file at, if it can.
pub fn debuginfod_path(id: &DebugId<'_>) -> Option<String> {
    match *id {
        DebugId::BuildId(b) | DebugId::AltLink { build_id: b, .. } if b.len() >= 2 => {
            let hex: String = b.iter().map(|b| format!("{b:02x}")).collect();
            Some(format!("buildid/{hex}/debuginfo"))
        }
        _ => None,
    }
}

/// A name from an image that goes into a URL and a path: letters,
/// digits, and `._-+` only (no separators, no `..`).
fn safe_name(name: &str) -> bool {
    !name.is_empty()
        && name != "."
        && name != ".."
        && name
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"._-+".contains(&b))
}

/// Where the servers have `id`'s file: (URL, where the cache keeps it).
pub fn locations(servers: &Servers, id: &DebugId<'_>) -> Vec<(String, Option<PathBuf>)> {
    let mut out = Vec::new();
    if let Some(key) = ssqp_key(id) {
        for s in &servers.symbol_servers {
            let cached = servers
                .cache
                .as_ref()
                .map(|c| key.split('/').fold(c.clone(), |p, part| p.join(part)));
            out.push((format!("{}/{key}", s.trim_end_matches('/')), cached));
        }
    }
    if let Some(path) = debuginfod_path(id) {
        for s in &servers.debuginfod {
            // (debuginfod's own cache layout: <id>/debuginfo.)
            let cached = servers.cache.as_ref().map(|c| {
                path.trim_start_matches("buildid/")
                    .split('/')
                    .fold(c.clone(), |p, part| p.join(part))
            });
            out.push((format!("{}/{path}", s.trim_end_matches('/')), cached));
        }
    }
    out
}

/// Fetches `url` (with `curl`): into `cached` if given (and returns it
/// mapped), else into memory. `None` if it isn't there, or anything
/// fails.
pub fn fetch(url: &str, cached: Option<&Path>) -> Option<Mmap> {
    if !(url.starts_with("https://") || url.starts_with("http://")) {
        return None;
    }
    let mut cmd = std::process::Command::new("curl");
    cmd.args([
        "--fail",
        "--silent",
        "--location",
        "--proto",
        "=https,http",
        "--proto-redir",
        "=https,http",
        "--max-redirs",
        "5",
        "--max-time",
        &MAX_SECONDS.to_string(),
        "--max-filesize",
        &MAX_FILE.to_string(),
        "--user-agent",
        concat!("turbonop/", env!("CARGO_PKG_VERSION")),
    ]);
    match cached {
        Some(path) => {
            let dir = path.parent()?;
            std::fs::create_dir_all(dir).ok()?;
            // (Written beside it, then renamed: a half-fetched file is never
            // found.)
            let mut tmp = path.as_os_str().to_owned();
            tmp.push(format!(".{}.part", std::process::id()));
            let tmp = PathBuf::from(tmp);
            let ok = cmd
                .arg("--output")
                .arg(&tmp)
                .arg("--")
                .arg(url)
                .stdin(std::process::Stdio::null())
                .stdout(std::process::Stdio::null())
                .stderr(std::process::Stdio::null())
                .status()
                .is_ok_and(|s| s.success());
            if !ok || std::fs::rename(&tmp, path).is_err() {
                let _ = std::fs::remove_file(&tmp);
                return None;
            }
            Mmap::open_named(path).ok()
        }
        None => {
            let out = cmd
                .arg("--output")
                .arg("-")
                .arg("--")
                .arg(url)
                .stdin(std::process::Stdio::null())
                .stderr(std::process::Stdio::null())
                .output()
                .ok()?;
            (out.status.success() && !out.stdout.is_empty()).then(|| Mmap::from(out.stdout))
        }
    }
}

/// The most transfers one batch runs at once.
const PARALLEL: usize = 64;

/// A file to fetch: its URL, and where the cache keeps it (else it's
/// kept in memory).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Request {
    /// Where to fetch it from.
    pub url: String,
    /// Where to keep it (else in memory).
    pub cached: Option<PathBuf>,
}

/// Fetches every request at once: what each got, in order (as [`fetch`]
/// would have, one by one).
///
/// One `curl` runs them all (`--parallel`), so transfers to a server share
/// its connections (HTTP/2 multiplexes them), and none waits for another.
/// A `curl` too old for that (before 7.66) has them fetched by separate
/// processes, many at once.
pub fn fetch_batch(requests: &[Request]) -> Vec<Option<Mmap>> {
    match requests.len() {
        0 => return Vec::new(),
        1 => return vec![fetch(&requests[0].url, requests[0].cached.as_deref())],
        _ => {}
    }
    if let Some(out) = fetch_parallel(requests) {
        return out;
    }
    fetch_each(requests)
}

/// [`fetch_batch`] by separate `curl` processes, [`PARALLEL`] at once.
fn fetch_each(requests: &[Request]) -> Vec<Option<Mmap>> {
    use std::sync::Mutex;
    use std::sync::atomic::{AtomicUsize, Ordering};
    let next = AtomicUsize::new(0);
    let out: Vec<Mutex<Option<Mmap>>> = requests.iter().map(|_| Mutex::new(None)).collect();
    std::thread::scope(|s| {
        for _ in 0..PARALLEL.min(requests.len()) {
            s.spawn(|| {
                loop {
                    let i = next.fetch_add(1, Ordering::Relaxed);
                    let Some(r) = requests.get(i) else { break };
                    let got = fetch(&r.url, r.cached.as_deref());
                    *out[i]
                        .lock()
                        .unwrap_or_else(std::sync::PoisonError::into_inner) = got;
                }
            });
        }
    });
    out.into_iter()
        .map(|m| {
            m.into_inner()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
        })
        .collect()
}

/// A string as a `curl` config file quotes it.
fn config_quoted(s: &str) -> String {
    let mut q = String::with_capacity(s.len() + 2);
    q.push('"');
    for c in s.chars() {
        match c {
            '"' => q.push_str("\\\""),
            '\\' => q.push_str("\\\\"),
            '\n' | '\r' | '\t' => q.push(' '),
            c => q.push(c),
        }
    }
    q.push('"');
    q
}

/// [`fetch_batch`] by one `curl --parallel`, if this `curl` can.
fn fetch_parallel(requests: &[Request]) -> Option<Vec<Option<Mmap>>> {
    use std::fmt::Write as _;
    static BATCH: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let n = BATCH.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let scratch = std::env::temp_dir().join(format!("turbonop-fetch-{}-{n}", std::process::id()));
    std::fs::create_dir_all(&scratch).ok()?;
    let _cleanup = RemoveOnDrop(scratch.clone());
    // Each transfer's output: beside its cache path (then renamed, so a
    // half-fetched file is never found), or in the scratch directory.
    let outputs: Vec<PathBuf> = requests
        .iter()
        .enumerate()
        .map(|(i, r)| match &r.cached {
            Some(p) => {
                let mut tmp = p.as_os_str().to_owned();
                tmp.push(format!(".{}.{n}.part", std::process::id()));
                PathBuf::from(tmp)
            }
            None => scratch.join(format!("{i}.part")),
        })
        .collect();
    let mut config = String::new();
    let mut sent = Vec::new();
    for (i, r) in requests.iter().enumerate() {
        let ok = r.url.starts_with("https://") || r.url.starts_with("http://");
        let dir_ok = outputs[i]
            .parent()
            .is_some_and(|d| std::fs::create_dir_all(d).is_ok());
        if !ok || !dir_ok {
            continue;
        }
        let _ = writeln!(config, "url = {}", config_quoted(&r.url));
        let _ = writeln!(
            config,
            "output = {}",
            config_quoted(&outputs[i].to_string_lossy())
        );
        sent.push(i);
    }
    if sent.is_empty() {
        return Some(requests.iter().map(|_| None).collect());
    }
    let config_path = scratch.join("config");
    std::fs::write(&config_path, config).ok()?;
    let out = std::process::Command::new("curl")
        .args([
            "--parallel",
            "--parallel-max",
            &PARALLEL.to_string(),
            "--fail",
            "--silent",
            "--location",
            "--proto",
            "=https,http",
            "--proto-redir",
            "=https,http",
            "--max-redirs",
            "5",
            "--max-time",
            &MAX_SECONDS.to_string(),
            "--max-filesize",
            &MAX_FILE.to_string(),
            "--user-agent",
            concat!("turbonop/", env!("CARGO_PKG_VERSION")),
            "--write-out",
            "%{urlnum} %{http_code} %{exitcode}\\n",
            "--config",
        ])
        .arg(&config_path)
        .stdin(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .output()
        .ok()?;
    // (curl's "unknown option": too old for --parallel or %{exitcode}.)
    if out.status.code() == Some(2) || (out.stdout.is_empty() && !sent.is_empty()) {
        for o in &outputs {
            let _ = std::fs::remove_file(o);
        }
        return None;
    }
    let mut got: Vec<Option<Mmap>> = requests.iter().map(|_| None).collect();
    for line in String::from_utf8_lossy(&out.stdout).lines() {
        let mut f = line.split_whitespace();
        let (Some(num), Some(code), Some(exit)) = (f.next(), f.next(), f.next()) else {
            continue;
        };
        let (Ok(num), Ok(code)) = (num.parse::<usize>(), code.parse::<u32>()) else {
            continue;
        };
        let Some(&i) = sent.get(num) else { continue };
        let output = &outputs[i];
        if exit != "0" || !(200..300).contains(&code) {
            let _ = std::fs::remove_file(output);
            continue;
        }
        got[i] = match &requests[i].cached {
            Some(path) => {
                if std::fs::rename(output, path).is_err() {
                    let _ = std::fs::remove_file(output);
                    continue;
                }
                Mmap::open_named(path).ok()
            }
            None => std::fs::read(output)
                .ok()
                .filter(|d| !d.is_empty())
                .map(Mmap::from),
        };
    }
    // (Outputs of transfers curl didn't report.)
    for (i, o) in outputs.iter().enumerate() {
        if got[i].is_none() {
            let _ = std::fs::remove_file(o);
        }
    }
    Some(got)
}

/// Removes a directory, and what's in it, when dropped.
struct RemoveOnDrop(PathBuf);

impl Drop for RemoveOnDrop {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn image_keys() {
        let pe = ImageKey::Pe {
            name: r"C:\Windows\System32\NTDLL.dll".into(),
            time_date_stamp: 0x0A1B_2C3D,
            size_of_image: 0x1F_8000,
        };
        assert_eq!(pe.name(), "NTDLL.dll");
        assert_eq!(
            image_ssqp_key(&pe).as_deref(),
            Some("ntdll.dll/0A1B2C3D1f8000/ntdll.dll")
        );
        assert_eq!(image_debuginfod_path(&pe), None);
        let elf = ImageKey::BuildId {
            name: "/usr/lib/libc.so.6".into(),
            build_id: vec![0xab, 0xcd, 0xef],
        };
        assert_eq!(
            image_ssqp_key(&elf).as_deref(),
            Some("libc.so.6/elf-buildid-abcdef/libc.so.6")
        );
        assert_eq!(
            image_debuginfod_path(&elf).as_deref(),
            Some("buildid/abcdef/executable")
        );
        let mach = ImageKey::Uuid {
            name: "/System/Library/Frameworks/Foundation.framework/Foundation".into(),
            uuid: [0x0f; 16],
        };
        assert_eq!(
            image_ssqp_key(&mach).as_deref(),
            Some("foundation/mach-uuid-0f0f0f0f0f0f0f0f0f0f0f0f0f0f0f0f/foundation")
        );
        // Names that would leave the key's directory.
        let bad = ImageKey::Pe {
            name: "a b.dll".into(),
            time_date_stamp: 1,
            size_of_image: 1,
        };
        assert_eq!(image_ssqp_key(&bad), None);
        let servers = Servers {
            symbol_servers: vec!["https://msdl.microsoft.com/download/symbols".into()],
            debuginfod: vec!["https://debuginfod.example/".into()],
            cache: Some(PathBuf::from("/c")),
        };
        let l = image_locations(&servers, &elf);
        assert_eq!(l.len(), 2);
        assert_eq!(
            l[0].0,
            "https://msdl.microsoft.com/download/symbols/libc.so.6/elf-buildid-abcdef/libc.so.6"
        );
        assert_eq!(
            l[0].1,
            Some(
                Path::new("/c")
                    .join("libc.so.6")
                    .join("elf-buildid-abcdef")
                    .join("libc.so.6")
            )
        );
        assert_eq!(
            l[1].0,
            "https://debuginfod.example/buildid/abcdef/executable"
        );
        assert_eq!(
            l[1].1,
            Some(Path::new("/c").join("abcdef").join("executable"))
        );
        assert_eq!(image_locations(&servers, &pe).len(), 1);
        // NB10 PDBs.
        let nb10 = DebugId::Pdb20 {
            signature: 0x3A2B_1C0D,
            age: 2,
            path: br"c:\old\App.pdb",
        };
        assert_eq!(
            ssqp_key(&nb10).as_deref(),
            Some("app.pdb/3A2B1C0D2/app.pdb")
        );
    }

    #[test]
    fn batches_refuse_what_fetch_refuses() {
        assert!(fetch_batch(&[]).is_empty());
        let got = fetch_batch(&[
            Request {
                url: "file:///etc/passwd".into(),
                cached: None,
            },
            Request {
                url: "-o/tmp/x".into(),
                cached: None,
            },
        ]);
        assert_eq!(got.len(), 2);
        assert!(got.iter().all(Option::is_none));
        assert_eq!(config_quoted(r#"a"b\c"#), r#""a\"b\\c""#);
    }

    #[test]
    fn keys() {
        let guid = [
            0xFD, 0x0D, 0xB9, 0x1C, 0xB9, 0x4C, 0xA8, 0x4B, 0xBA, 0x9C, 0xAE, 0x62, 0x63, 0x7E,
            0x69, 0x30,
        ];
        let pdb = DebugId::Pdb {
            guid,
            age: 1,
            path: br"D:\a\CoreCLR.pdb",
        };
        assert_eq!(
            ssqp_key(&pdb).as_deref(),
            Some("coreclr.pdb/1CB90DFD4CB94BA8BA9CAE62637E69301/coreclr.pdb")
        );
        let id = DebugId::BuildId(&[0x2e, 0x52, 0x99]);
        assert_eq!(
            ssqp_key(&id).as_deref(),
            Some("_.debug/elf-buildid-sym-2e5299/_.debug")
        );
        assert_eq!(
            debuginfod_path(&id).as_deref(),
            Some("buildid/2e5299/debuginfo")
        );
        let mach = DebugId::Uuid([0xca; 16]);
        assert_eq!(
            ssqp_key(&mach).as_deref(),
            Some("_.dwarf/mach-uuid-sym-cacacacacacacacacacacacacacacaca/_.dwarf")
        );
        assert_eq!(debuginfod_path(&mach), None);
        // Names that would leave the key's directory, or the URL's path.
        for bad in [&b"../x.pdb"[..], b"a?b.pdb", b"a b.pdb", b"..", b""] {
            let id = DebugId::Pdb {
                guid,
                age: 1,
                path: bad,
            };
            let key = ssqp_key(&id);
            assert!(
                key.as_deref().is_none_or(|k| k.split('/').all(safe_name)),
                "{bad:?}: {key:?}"
            );
        }
        assert_eq!(
            ssqp_key(&DebugId::Pdb {
                guid,
                age: 1,
                path: b"x/.."
            }),
            None
        );
    }

    #[test]
    fn locations_and_cache_paths() {
        let servers = Servers {
            symbol_servers: vec!["https://msdl.microsoft.com/download/symbols/".into()],
            debuginfod: vec!["https://debuginfod.example".into()],
            cache: Some(PathBuf::from("/c")),
        };
        let id = DebugId::BuildId(&[0xab, 0xcd]);
        let l = locations(&servers, &id);
        assert_eq!(
            l[0].0,
            "https://msdl.microsoft.com/download/symbols/_.debug/elf-buildid-sym-abcd/_.debug"
        );
        assert_eq!(
            l[0].1,
            Some(
                Path::new("/c")
                    .join("_.debug")
                    .join("elf-buildid-sym-abcd")
                    .join("_.debug")
            )
        );
        assert_eq!(l[1].0, "https://debuginfod.example/buildid/abcd/debuginfo");
        assert_eq!(l[1].1, Some(Path::new("/c").join("abcd").join("debuginfo")));
        assert!(locations(&Servers::default(), &id).is_empty());
        // Only HTTP(S) is fetched.
        assert!(fetch("file:///etc/passwd", None).is_none());
        assert!(fetch("-o/tmp/x", None).is_none());
    }
}
