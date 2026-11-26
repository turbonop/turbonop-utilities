//! Running the `ar` personality: from the command line to what is printed,
//! written, and returned, as GNU `ar` 2.46 and `llvm-ar` 24 behave (read in
//! their sources, and measured: see `docs/turbolib.md`, phase 6).
//!
//! Where the two differ in what they write:
//!
//! - **Members already in the archive** keep their header fields with GNU
//!   `ar` (a `ranlib` keeps an archive's dates and modes); `llvm-ar`
//!   rewrites them in deterministic mode (`0`, `0`, `0`, `644`) and keeps
//!   them otherwise.
//! - **`s` and `ranlib`** always rewrite the archive with GNU `ar`;
//!   `llvm-ar` leaves an archive that has a symbol table as it is.
//! - **`r` and `q` with nothing changed**: GNU `ar` writes nothing (but an
//!   empty archive if there was none); `llvm-ar` writes the archive again.
//! - **The format**: GNU `ar` writes the GNU dialect; `llvm-ar`, its
//!   `--format`, else the old archive's, else the first new member's (ELF:
//!   GNU; Mach-O: Darwin; COFF: Microsoft's, which this personality doesn't
//!   write: use the `llvm-lib` one).

use super::args::{self, Command, Flavor, Op, Position};
use crate::output;
use crate::settings::{Fold, Locals, Precomp, Pull, Settings};
use std::io;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use turbonop_archive::unix::symbols::member_symbols;
use turbonop_archive::unix::{Archiver, Dialect, Entry, Layout, Options};
use turbonop_archive::write::{Fields, IndexSymbol};
use turbonop_archive::{Archive, MemberKind};
use turbonop_mmap::Mmap;

/// The environment a run sees.
#[derive(Clone, Debug, Default)]
pub struct Env {
    /// The current directory, against which relative paths are read.
    pub cwd: PathBuf,
    /// `TURBOLIB_OPTIONS`.
    pub turbolib_options: Option<String>,
    /// `SOURCE_DATE_EPOCH` (GNU `ar`, without `D`: the members' dates).
    pub source_date_epoch: Option<u64>,
    /// `SYM64_THRESHOLD` (`llvm-ar`).
    pub sym64_threshold: Option<u64>,
    /// The time, in seconds since 1970 (for `U`'s symbol table date).
    pub now: u64,
    /// Standard input, for `-M`.
    pub stdin: Option<Vec<u8>>,
    /// `TZ`, for the dates `t v` prints in local time.
    pub tz: Option<String>,
}

impl Env {
    /// The process's environment (standard input read only for `-M`).
    pub fn from_process(mri: bool) -> Env {
        let number = |name: &str| std::env::var(name).ok().and_then(|v| v.trim().parse().ok());
        Env {
            cwd: std::env::current_dir().unwrap_or_default(),
            turbolib_options: std::env::var("TURBOLIB_OPTIONS").ok(),
            source_date_epoch: number("SOURCE_DATE_EPOCH"),
            sym64_threshold: number("SYM64_THRESHOLD"),
            now: std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map_or(0, |d| d.as_secs()),
            stdin: mri.then(|| {
                let mut v = Vec::new();
                let _ = io::Read::read_to_end(&mut io::stdin(), &mut v);
                v
            }),
            tz: std::env::var("TZ").ok(),
        }
    }
}

/// What a run printed and returned.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Outcome {
    pub stdout: Vec<u8>,
    pub stderr: String,
    pub exit: i32,
}

/// Why a run stopped: the message, printed as the tool prints its errors.
#[derive(Debug)]
pub struct Fatal(pub String);

/// The state of one run.
pub struct Run<'e> {
    pub flavor: Flavor,
    /// The program's name, as the tools put it before their messages.
    pub argv0: String,
    pub env: &'e Env,
    pub stdout: Vec<u8>,
    pub stderr: String,
    pub settings: Settings,
    /// GNU `ar`: an entry a file names that isn't in the archive is a
    /// message, not an error, but then the exit status is still 0.
    status: i32,
}

impl<'e> Run<'e> {
    /// A run of `flavor`, named `argv0`, in `env`.
    pub fn new(flavor: Flavor, argv0: &str, env: &'e Env) -> Self {
        Run {
            flavor,
            argv0: argv0.to_string(),
            env,
            stdout: Vec::new(),
            stderr: String::new(),
            settings: Settings::default(),
            status: 0,
        }
    }

    /// A warning or message on standard error, worded as the tool words
    /// it.
    pub fn warn(&mut self, text: &str) {
        let line = if self.flavor.is_llvm() {
            format!("{}: warning: {text}\n", self.argv0)
        } else {
            format!("{}: {text}\n", self.argv0)
        };
        self.stderr.push_str(&line);
    }

    /// An error that stops the run.
    pub fn fatal(&self, text: impl Into<String>) -> Fatal {
        let text = text.into();
        Fatal(if self.flavor.is_llvm() {
            format!("{}: error: {text}", self.argv0)
        } else {
            format!("{}: {text}", self.argv0)
        })
    }

    fn print(&mut self, s: &str) {
        self.stdout.extend_from_slice(s.as_bytes());
    }

    fn archiver(&self) -> Archiver {
        if self.flavor.is_llvm() {
            Archiver::LlvmAr
        } else {
            Archiver::GnuAr
        }
    }

    fn resolve(&self, p: &str) -> PathBuf {
        let p = Path::new(p);
        if p.is_absolute() {
            p.to_path_buf()
        } else {
            self.env.cwd.join(p)
        }
    }
}

/// A member's bytes: part of a mapped file.
#[derive(Clone, Debug)]
pub struct Bytes {
    pub map: Arc<Mmap>,
    pub start: usize,
    pub len: usize,
}

impl Bytes {
    pub fn get(&self) -> &[u8] {
        &self.map.as_slice()[self.start..self.start + self.len]
    }

    fn whole(map: Arc<Mmap>) -> Bytes {
        let len = map.as_slice().len();
        Bytes { map, start: 0, len }
    }
}

/// A member of the archive being written.
#[derive(Clone, Debug)]
pub struct Item {
    /// Its name as stored (a thin archive's: the path from the archive's
    /// directory).
    pub name: String,
    /// A GNU thin archive's member of another archive: its header there.
    pub origin: Option<u64>,
    pub fields: Fields,
    /// Its size (the file's, for a thin archive's member).
    pub size: u64,
    /// Its bytes (`None`: a thin archive's member whose file isn't there).
    pub bytes: Option<Bytes>,
    /// Whether it came from the archive (rather than a file named now).
    pub old: bool,
}

impl Item {
    /// Its bytes (none for a thin archive's member whose file isn't there).
    pub fn data(&self) -> &[u8] {
        self.bytes.as_ref().map_or(&[], Bytes::get)
    }
}

/// The archive on disk, read.
pub struct Old {
    pub thin: bool,
    pub dialect: Dialect,
    pub has_symtab: bool,
    pub members: Vec<Item>,
    /// Each member's data offset in the archive (for `O`).
    pub data_offsets: Vec<u64>,
}

/// The fields of a file added now.
fn new_fields(run: &Run<'_>, meta: &std::fs::Metadata, deterministic: bool) -> Fields {
    if deterministic {
        return Fields::new(b"0", b"0", b"0", b"644").unwrap_or(Fields::BLANK);
    }
    let mtime = meta
        .modified()
        .ok()
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .map_or(0, |d| d.as_secs());
    let (uid, gid, mode) = unix_ids(meta);
    let (date, uid, gid, mode) = if run.flavor.is_llvm() {
        (mtime, uid % 1_000_000, gid % 1_000_000, mode & 0o7777)
    } else {
        (run.env.source_date_epoch.unwrap_or(mtime), uid, gid, mode)
    };
    let d = date.to_string();
    let u = uid.to_string();
    let g = gid.to_string();
    let m = format!("{mode:o}");
    let cut = |s: &str, n: usize| s.as_bytes()[..s.len().min(n)].to_vec();
    Fields::new(&cut(&d, 12), &cut(&u, 6), &cut(&g, 6), &cut(&m, 8)).unwrap_or(Fields::BLANK)
}

#[cfg(unix)]
fn unix_ids(meta: &std::fs::Metadata) -> (u64, u64, u64) {
    use std::os::unix::fs::MetadataExt;
    (
        u64::from(meta.uid()),
        u64::from(meta.gid()),
        u64::from(meta.mode()),
    )
}

#[cfg(not(unix))]
fn unix_ids(meta: &std::fs::Metadata) -> (u64, u64, u64) {
    let mode = if meta.permissions().readonly() {
        0o100_444
    } else {
        0o100_644
    };
    (0, 0, mode)
}

/// The last component of a path (after its last `/`; `\` too on
/// Windows).
fn basename(p: &str) -> &str {
    let seps: &[char] = if cfg!(windows) { &['/', '\\'] } else { &['/'] };
    p.rsplit(seps).next().unwrap_or(p)
}

/// `path` made absolute against `cwd`, `.` and `..` removed.
fn lexical_absolute(cwd: &Path, path: &str) -> Vec<String> {
    let mut parts: Vec<String> = Vec::new();
    let p = Path::new(path);
    let full = if p.is_absolute() {
        p.to_path_buf()
    } else {
        cwd.join(p)
    };
    for c in full.components() {
        match c {
            std::path::Component::ParentDir => {
                parts.pop();
            }
            std::path::Component::Normal(s) => parts.push(s.to_string_lossy().into_owned()),
            _ => {}
        }
    }
    parts
}

/// The path of `file` from the directory of `archive` (both relative to
/// `cwd`), as a thin archive stores it.
pub fn relative_to_archive(cwd: &Path, archive: &str, file: &str) -> String {
    let to = lexical_absolute(cwd, file);
    let mut from = lexical_absolute(cwd, archive);
    from.pop();
    let common = from.iter().zip(&to).take_while(|(a, b)| a == b).count();
    let mut out: Vec<&str> = vec![".."; from.len() - common];
    out.extend(to[common..].iter().map(String::as_str));
    out.join("/")
}

/// Reads the archive at `path`.
pub fn read_old(run: &Run<'_>, path: &str) -> Result<Option<Old>, Fatal> {
    let full = run.resolve(path);
    let map = match Mmap::open(&full) {
        Ok(m) => Arc::new(m),
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(run.fatal(format!("{path}: {e}"))),
    };
    let data = map.as_slice();
    let archive = Archive::parse(data).map_err(|_| {
        run.fatal(if run.flavor.is_llvm() {
            format!("unable to load '{path}': file too small to be an archive")
        } else {
            format!("{path}: file format not recognized")
        })
    })?;
    let thin = archive.is_thin();
    let mut members = Vec::new();
    let mut data_offsets = Vec::new();
    let mut has_symtab = false;
    let mut bsd = false;
    let dir = Path::new(path).parent().map(Path::to_path_buf);
    for m in archive.members() {
        match m.kind {
            MemberKind::SymbolTable => {
                has_symtab = true;
                bsd |= m.name.starts_with(b"__.SYMDEF");
                continue;
            }
            MemberKind::LongNames | MemberKind::EcSymbolTable => continue,
            MemberKind::File => {}
        }
        bsd |= usize::try_from(m.header_offset)
            .ok()
            .and_then(|o| data.get(o..o + 3))
            == Some(b"#1/");
        let name = String::from_utf8_lossy(&m.name).into_owned();
        let fields = Fields::of(&m).unwrap_or(Fields::BLANK);
        let bytes = if thin {
            // The member's file, from the archive's directory.
            let rel = if Path::new(&name).is_absolute() {
                PathBuf::from(&name)
            } else {
                dir.as_ref()
                    .map_or_else(|| PathBuf::from(&name), |d| d.join(&name))
            };
            Mmap::open(run.resolve(&rel.to_string_lossy()))
                .ok()
                .map(|f| Bytes::whole(Arc::new(f)))
                .and_then(|b| match m.origin {
                    None => Some(b),
                    Some(at) => {
                        let inner = Archive::parse(b.get()).ok()?.member_at_offset(at)?;
                        Some(Bytes {
                            start: usize::try_from(inner.data_offset).ok()?,
                            len: inner.data.len(),
                            map: b.map,
                        })
                    }
                })
        } else {
            Some(Bytes {
                map: map.clone(),
                start: usize::try_from(m.data_offset).unwrap_or(0),
                len: m.data.len(),
            })
        };
        data_offsets.push(m.data_offset);
        members.push(Item {
            name,
            origin: m.origin,
            fields,
            size: if thin { m.size } else { m.data.len() as u64 },
            bytes,
            old: true,
        });
    }
    Ok(Some(Old {
        thin,
        dialect: if bsd { Dialect::Bsd } else { Dialect::Gnu },
        has_symtab,
        members,
        data_offsets,
    }))
}

/// What to write: the members, and how.
pub struct Plan {
    pub members: Vec<Item>,
    pub thin: bool,
    pub dialect: Dialect,
    pub symtab: bool,
    pub deterministic: bool,
}

/// Opens a file named on the command line as a new member.
pub fn new_item(
    run: &Run<'_>,
    archive: &str,
    file: &str,
    thin: bool,
    full_path: bool,
    truncate: bool,
    deterministic: bool,
) -> Result<Item, Fatal> {
    let path = run.resolve(file);
    let meta =
        std::fs::metadata(&path).map_err(|e| run.fatal(format!("{file}: {}", io_text(&e))))?;
    if meta.is_dir() {
        return Err(run.fatal(format!("{file}: Is a directory")));
    }
    let map = Mmap::open(&path).map_err(|e| run.fatal(format!("{file}: {}", io_text(&e))))?;
    let name = if thin {
        let absolute = Path::new(file).is_absolute();
        if run.flavor.is_llvm() {
            if absolute {
                file.replace('\\', "/")
            } else {
                relative_to_archive(&run.env.cwd, archive, file)
            }
        } else if absolute || Path::new(archive).is_absolute() {
            file.to_string()
        } else {
            relative_to_archive(&run.env.cwd, archive, file)
        }
    } else if full_path && !run.flavor.is_llvm() {
        file.to_string()
    } else {
        let b = basename(file);
        if truncate && !run.flavor.is_llvm() {
            b.chars().take(15).collect()
        } else {
            b.to_string()
        }
    };
    let map = Arc::new(map);
    Ok(Item {
        name,
        origin: None,
        fields: new_fields(run, &meta, deterministic),
        size: map.as_slice().len() as u64,
        bytes: Some(Bytes::whole(map)),
        old: false,
    })
}

/// The text of an I/O error as `strerror` gives it.
pub fn io_text(e: &io::Error) -> String {
    match e.kind() {
        io::ErrorKind::NotFound => "No such file or directory".into(),
        io::ErrorKind::PermissionDenied => "Permission denied".into(),
        _ => {
            let s = e.to_string();
            s.split(" (os error").next().unwrap_or(&s).to_string()
        }
    }
}

/// Whether the name `given` (a file named now) matches the member name
/// `member`, as `flavor` compares them.
fn matches(
    run: &Run<'_>,
    c: &Command,
    archive: &str,
    thin: bool,
    given: &str,
    member: &str,
) -> bool {
    if run.flavor.is_llvm() {
        if c.full_path || thin {
            let given = if thin && !Path::new(given).is_absolute() {
                relative_to_archive(&run.env.cwd, archive, given)
            } else {
                given.replace('\\', "/")
            };
            given == member.replace('\\', "/")
        } else {
            basename(given) == basename(member)
        }
    } else {
        let norm = |s: &str| -> String {
            if c.full_path {
                s.to_string()
            } else if c.truncate {
                basename(s).chars().take(15).collect()
            } else {
                basename(s).to_string()
            }
        };
        norm(given) == norm(member)
    }
}

/// The members a GNU or llvm-ar write operation leaves.
fn plan_members(
    run: &Run<'_>,
    c: &Command,
    archive: &str,
    old: Option<&Old>,
    log: &mut String,
) -> Result<(Vec<Item>, bool), Fatal> {
    let op = c.op.unwrap_or(Op::Replace);
    let thin = c.thin || old.is_some_and(|o| o.thin);
    let deterministic = c.deterministic.unwrap_or(true);
    let mut members: Vec<Item> = old.map(|o| o.members.clone()).unwrap_or_default();
    if run.flavor.is_llvm() && deterministic {
        for m in &mut members {
            m.fields = Fields::new(b"0", b"0", b"0", b"644").unwrap_or(Fields::BLANK);
        }
    } else if run.flavor.is_llvm() {
        // Its permissions alone, again.
        for m in &mut members {
            let mode = std::str::from_utf8(m.fields.mode())
                .ok()
                .and_then(|t| u32::from_str_radix(t, 8).ok())
                .unwrap_or(0o644)
                & 0o7777;
            let text = format!("{mode:o}");
            m.fields = Fields::new(
                m.fields.date(),
                m.fields.uid(),
                m.fields.gid(),
                text.as_bytes(),
            )
            .unwrap_or(m.fields);
        }
    }
    let new = |run: &Run<'_>, f: &str| {
        new_item(
            run,
            archive,
            f,
            thin,
            c.full_path,
            c.truncate,
            deterministic,
        )
    };
    let position = |members: &[Item]| -> Result<usize, Fatal> {
        match &c.position {
            None => Ok(members.len()),
            Some(Position::After(p) | Position::Before(p)) => {
                let after = matches!(c.position, Some(Position::After(_)));
                match members.iter().position(|m| {
                    if run.flavor.is_llvm() {
                        matches(run, c, archive, thin, p, &m.name)
                    } else {
                        m.name == *p
                    }
                }) {
                    Some(i) => Ok(if after { i + 1 } else { i }),
                    None if run.flavor.is_llvm() => Err(run.fatal("insertion point not found")),
                    // GNU: not found, at the end.
                    None => Ok(members.len()),
                }
            }
        }
    };
    let mut changed = false;
    match op {
        Op::Delete => {
            let mut counts = vec![0u32; c.files.len()];
            for (fi, f) in c.files.iter().enumerate() {
                if !run.flavor.is_llvm() && f == "__.SYMDEF" {
                    continue;
                }
                let mut found = false;
                let mut k = 0;
                while k < members.len() {
                    let hit = if run.flavor.is_llvm() {
                        matches(run, c, archive, thin, f, &members[k].name)
                    } else {
                        let norm = if c.full_path { f.as_str() } else { basename(f) };
                        norm == members[k].name
                    };
                    if hit {
                        counts[fi] += 1;
                        if c.count.is_none_or(|n| n == counts[fi]) {
                            if c.verbose && !run.flavor.is_llvm() {
                                log.push_str(&format!("d - {f}\n"));
                            }
                            members.remove(k);
                            found = true;
                            changed = true;
                            break;
                        }
                    }
                    k += 1;
                }
                if !found && c.verbose && !run.flavor.is_llvm() {
                    log.push_str(&format!("No member named `{f}'\n"));
                }
            }
        }
        Op::Move => {
            let mut moved = Vec::new();
            for f in &c.files {
                let Some(i) = members
                    .iter()
                    .position(|m| matches(run, c, archive, thin, f, &m.name))
                else {
                    if run.flavor.is_llvm() {
                        continue;
                    }
                    return Err(run.fatal(format!("no entry {f} in archive {archive}!")));
                };
                if c.verbose && !run.flavor.is_llvm() {
                    log.push_str(&format!("m - {f}\n"));
                }
                moved.push(members.remove(i));
            }
            if run.flavor.is_llvm() {
                // In the archive's order, at the position.
                let mut ordered: Vec<Item> = Vec::new();
                if let Some(old) = old {
                    for o in &old.members {
                        if let Some(k) = moved.iter().position(|m| m.name == o.name) {
                            ordered.push(moved.remove(k));
                        }
                    }
                }
                ordered.append(&mut moved);
                let at = position(&members)?;
                members.splice(at..at, ordered);
            } else {
                for m in moved {
                    let at = position(&members)?;
                    members.insert(at, m);
                }
            }
            changed = true;
        }
        Op::QuickAppend | Op::Replace => {
            let quick = op == Op::QuickAppend;
            let mut appended: Vec<Item> = Vec::new();
            let mut fresh = vec![false; members.len()];
            for f in &c.files {
                let flatten = thin || (run.flavor.is_llvm() && c.add_library);
                if !quick {
                    // The first member of that name the archive had.
                    let at = members.iter().zip(&fresh).position(|(m, &is_new)| {
                        !is_new && m.old && matches(run, c, archive, thin, f, &m.name)
                    });
                    if let Some(i) = at {
                        if c.update {
                            let newer = std::fs::metadata(run.resolve(f))
                                .ok()
                                .and_then(|m| m.modified().ok())
                                .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                                .map(|d| d.as_secs());
                            let theirs = std::str::from_utf8(members[i].fields.date())
                                .ok()
                                .and_then(|d| d.parse::<u64>().ok())
                                .unwrap_or(0);
                            match newer {
                                Some(t) if t > theirs => {}
                                Some(_) => continue,
                                None if run.flavor.is_llvm() => {
                                    return Err(
                                        run.fatal(format!("{f}: No such file or directory"))
                                    );
                                }
                                None => continue,
                            }
                        }
                        let item = new(run, f)?;
                        if c.verbose && !run.flavor.is_llvm() {
                            log.push_str(&format!("r - {f}\n"));
                        }
                        members.remove(i);
                        fresh.remove(i);
                        let at = match &c.position {
                            None => i,
                            Some(_) => position(&members)?,
                        };
                        members.insert(at, item);
                        fresh.insert(at, true);
                        changed = true;
                        continue;
                    }
                }
                let items = add_file(run, c, archive, f, flatten, thin, deterministic)?;
                if c.verbose && !run.flavor.is_llvm() {
                    log.push_str(&format!("a - {f}\n"));
                }
                if run.flavor.is_llvm() {
                    appended.extend(items);
                } else {
                    for item in items {
                        let at = position(&members)?;
                        members.insert(at, item);
                        fresh.insert(at, true);
                    }
                }
                changed = true;
            }
            if run.flavor.is_llvm() {
                let at = position(&members)?;
                members.splice(at..at, appended);
            }
        }
        _ => {}
    }
    Ok((members, changed))
}

/// The members a file named now adds: the file, or (flattening) the
/// members of the archive it is.
fn add_file(
    run: &Run<'_>,
    c: &Command,
    archive: &str,
    file: &str,
    flatten: bool,
    thin: bool,
    deterministic: bool,
) -> Result<Vec<Item>, Fatal> {
    let item = new_item(
        run,
        archive,
        file,
        thin,
        c.full_path,
        c.truncate,
        deterministic,
    )?;
    let bytes = item.data();
    if !(flatten && Archive::is_archive(bytes)) {
        return Ok(vec![item]);
    }
    let inner =
        Archive::parse(bytes).map_err(|_| run.fatal(format!("{file}: malformed archive")))?;
    // llvm-ar flattens a thin archive into a thin one, any archive with L;
    // GNU ar, any archive into a thin one.
    if run.flavor.is_llvm() && thin && !inner.is_thin() {
        return Ok(vec![item]);
    }
    let map = item.bytes.as_ref().map(|b| b.map.clone());
    let mut out = Vec::new();
    let dir = Path::new(file).parent().map(Path::to_path_buf);
    for m in inner.files() {
        let name = String::from_utf8_lossy(&m.name).into_owned();
        if inner.is_thin() {
            // Its own file, from the inner archive's directory.
            let path = dir.as_ref().map_or_else(
                || name.clone(),
                |d| d.join(&name).to_string_lossy().into_owned(),
            );
            out.extend(add_file(
                run,
                c,
                archive,
                &path,
                flatten,
                thin,
                deterministic,
            )?);
            continue;
        }
        let fields = if deterministic && run.flavor.is_llvm() {
            Fields::new(b"0", b"0", b"0", b"644").unwrap_or(Fields::BLANK)
        } else {
            Fields::of(&m).unwrap_or(Fields::BLANK)
        };
        let bytes = map.clone().map(|map| Bytes {
            map,
            start: usize::try_from(m.data_offset).unwrap_or(0),
            len: m.data.len(),
        });
        out.push(if thin {
            // GNU: the archive's path, and the member's header offset.
            Item {
                name: item.name.clone(),
                origin: Some(m.header_offset),
                fields,
                size: m.data.len() as u64,
                bytes,
                old: false,
            }
        } else {
            Item {
                name,
                origin: None,
                fields,
                size: m.data.len() as u64,
                bytes,
                old: false,
            }
        });
    }
    Ok(out)
}

/// The dialect `llvm-ar` writes.
fn llvm_dialect(
    run: &Run<'_>,
    c: &Command,
    old: Option<&Old>,
    members: &[Item],
    thin: bool,
) -> Result<Dialect, Fatal> {
    let detect = |m: &Item| -> Result<Dialect, Fatal> {
        let data = m.data();
        let le = |b: &[u8]| {
            b.get(..4)
                .map(|b| u32::from_le_bytes([b[0], b[1], b[2], b[3]]))
        };
        if matches!(le(data), Some(0xFEED_FACE | 0xFEED_FACF)) {
            return Ok(Dialect::Darwin);
        }
        if !turbonop_elf::ElfFile::is_elf(data) && turbonop_pe::PeFile::parse_object(data).is_ok() {
            return Err(run.fatal(
                "COFF members make llvm-ar write a Microsoft archive: use turbolib's llvm-lib personality",
            ));
        }
        Ok(Dialect::Gnu)
    };
    Ok(match c.format.as_deref() {
        Some("gnu") => Dialect::Gnu,
        Some("bsd") => Dialect::Bsd,
        Some("darwin") => Dialect::Darwin,
        Some("coff") => {
            return Err(run.fatal("--format=coff: use turbolib's llvm-lib personality"));
        }
        Some("bigarchive" | "zos") => {
            return Err(run.fatal("big archives and z/OS archives aren't written by turbolib"));
        }
        _ if thin => Dialect::Gnu,
        _ => match old {
            Some(o) => {
                let first = members.first();
                match o.dialect {
                    Dialect::Bsd | Dialect::Darwin => {
                        if first.is_some_and(|m| detect(m).ok() == Some(Dialect::Darwin)) {
                            Dialect::Darwin
                        } else {
                            Dialect::Bsd
                        }
                    }
                    Dialect::Gnu => {
                        if !o.has_symtab
                            && let Some(m) = first
                        {
                            detect(m)?;
                        }
                        Dialect::Gnu
                    }
                }
            }
            None => match members.first() {
                Some(m) => detect(m)?,
                None => Dialect::Gnu,
            },
        },
    })
}

/// The members `plan` holds, folded (`--fold`): each original member
/// (rewritten, if it gave units up), then the synthesized members.
fn fold_members(run: &mut Run<'_>, archive: &str, plan: &Plan) -> Result<Vec<Item>, Fatal> {
    let s = &run.settings;
    if s.fold == Fold::SelectAny {
        return Err(run.fatal("--fold=select-any isn't written for ELF yet"));
    }
    if s.locals != Locals::Keep {
        return Err(run.fatal("--locals=copy and merge aren't written for ELF yet"));
    }
    if s.precomp != Precomp::None {
        return Err(run.fatal("--precomp is CodeView's (COFF) only"));
    }
    if plan.thin {
        return Err(run.fatal("a thin archive's members are files of their own: not folded"));
    }
    if plan
        .members
        .iter()
        .any(|m| turbonop_macho::is_macho(m.data()))
    {
        return fold_macho(run, archive, plan);
    }
    let options = turbonop_fold::elf::Options {
        pull: match s.pull {
            Pull::Preserve => turbonop_fold::elf::Pull::Preserve,
            Pull::Relaxed => turbonop_fold::elf::Pull::Relaxed,
        },
    };
    let inputs: Vec<turbonop_fold::elf::Input<'_>> = plan
        .members
        .iter()
        .map(|m| turbonop_fold::elf::Input {
            name: m.name.as_bytes(),
            data: m.data(),
        })
        .collect();
    // A split unit's `.dwo` file: its DW_AT_dwo_name, in its DW_AT_comp_dir
    // (relative to the current directory, as the compiler wrote it).
    let dwo = |dir: &[u8], name: &[u8]| -> Option<Vec<u8>> {
        let name = std::path::PathBuf::from(String::from_utf8_lossy(name).into_owned());
        let path = if name.is_absolute() {
            name
        } else {
            std::path::Path::new(&*String::from_utf8_lossy(dir)).join(name)
        };
        std::fs::read(path).ok()
    };
    let (outputs, report) = turbonop_fold::elf::fold_with(&inputs, &options, &dwo);
    if run.settings.stats {
        let mut text = format!(
            "turbolib: {archive}: {} members, {} COMDAT units; {} with copies in more than one member, \
             {} folded ({} copies) into {} synthesized members; {} bytes of units removed, {} written; \
             {} debug relocations tombstoned; {} folded units' debug information moved ({} root DIEs, \
             {} bytes, {} addresses tombstoned), {} dropped\n",
            report.members,
            report.units,
            report.duplicated,
            report.folded,
            report.copies,
            report.synthesized,
            report.removed_bytes,
            report.synthesized_bytes,
            report.tombstoned,
            report.debug_moved,
            report.debug_roots,
            report.debug_bytes,
            report.debug_tombstoned,
            report.debug_dropped
        );
        for (why, n) in &report.debug_dropped_why {
            text.push_str(&format!(
                "turbolib:   {n} units' debug information dropped: {why}\n"
            ));
        }
        for (why, n) in &report.kept {
            text.push_str(&format!("turbolib:   {n} copies stayed: {why}\n"));
        }
        if !report.phases.is_empty() {
            let phases: Vec<String> = report
                .phases
                .iter()
                .map(|(what, d)| format!("{what} {:.3} s", d.as_secs_f64()))
                .collect();
            text.push_str(&format!("turbolib:   time: {}\n", phases.join(", ")));
        }
        for (why, n) in &report.passed {
            text.push_str(&format!("turbolib:   {n} members passed through: {why}\n"));
        }
        run.stderr.push_str(&text);
    }
    let synthesized = if plan.deterministic {
        Fields::new(b"0", b"0", b"0", b"644").unwrap_or(Fields::BLANK)
    } else {
        let now = run.env.now.to_string();
        Fields::new(now.as_bytes(), b"0", b"0", b"644").unwrap_or(Fields::BLANK)
    };
    Ok(outputs
        .into_iter()
        .map(|o| match (o.input, o.data) {
            (Some(i), None) => plan.members[i].clone(),
            (input, Some(data)) => {
                let from = input.map(|i| &plan.members[i]);
                Item {
                    name: from.map_or_else(
                        || String::from_utf8_lossy(&o.name).into_owned(),
                        |f| f.name.clone(),
                    ),
                    origin: None,
                    fields: from.map_or(synthesized, |f| f.fields),
                    size: data.len() as u64,
                    bytes: Some(Bytes::whole(Arc::new(Mmap::from(data)))),
                    old: from.is_some_and(|f| f.old),
                }
            }
            (None, None) => unreachable!("a synthesized member has its bytes"),
        })
        .collect())
}

/// [`fold_members`] for Mach-O members.
fn fold_macho(run: &mut Run<'_>, archive: &str, plan: &Plan) -> Result<Vec<Item>, Fatal> {
    let salt = Path::new(archive)
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    let options = turbonop_fold::macho::Options {
        pull: match run.settings.pull {
            Pull::Preserve => turbonop_fold::macho::Pull::Preserve,
            Pull::Relaxed => turbonop_fold::macho::Pull::Relaxed,
        },
        salt: salt.as_bytes(),
    };
    let inputs: Vec<turbonop_fold::macho::Input<'_>> = plan
        .members
        .iter()
        .map(|m| turbonop_fold::macho::Input {
            name: m.name.as_bytes(),
            data: m.data(),
        })
        .collect();
    let (outputs, report) = turbonop_fold::macho::fold(&inputs, &options);
    if run.settings.stats {
        let mut text = format!(
            "turbolib: {archive}: {} members, {} weak-definition units; {} with copies in more than one member,              {} folded ({} copies) into {} synthesized members; {} bytes of units removed, {} written;              {} debug relocations removed; {} folded units' debug information dropped (not moved for Mach-O yet)
",
            report.members,
            report.units,
            report.duplicated,
            report.folded,
            report.copies,
            report.synthesized,
            report.removed_bytes,
            report.synthesized_bytes,
            report.tombstoned,
            report.debug_dropped,
        );
        for (why, n) in &report.kept {
            text.push_str(&format!(
                "turbolib:   {n} copies stayed: {why}
"
            ));
        }
        for (why, n) in &report.passed {
            text.push_str(&format!(
                "turbolib:   {n} members passed through: {why}
"
            ));
        }
        for (why, n) in &report.kept_whole {
            text.push_str(&format!(
                "turbolib:   {n} members kept whole: {why}
"
            ));
        }
        run.stderr.push_str(&text);
    }
    let synthesized = if plan.deterministic {
        Fields::new(b"0", b"0", b"0", b"644").unwrap_or(Fields::BLANK)
    } else {
        let now = run.env.now.to_string();
        Fields::new(now.as_bytes(), b"0", b"0", b"644").unwrap_or(Fields::BLANK)
    };
    Ok(outputs
        .into_iter()
        .map(|o| match (o.input, o.data) {
            (Some(i), None) => plan.members[i].clone(),
            (input, Some(data)) => {
                let from = input.map(|i| &plan.members[i]);
                Item {
                    name: from.map_or_else(
                        || String::from_utf8_lossy(&o.name).into_owned(),
                        |f| f.name.clone(),
                    ),
                    origin: None,
                    fields: from.map_or(synthesized, |f| f.fields),
                    size: data.len() as u64,
                    bytes: Some(Bytes::whole(Arc::new(Mmap::from(data)))),
                    old: from.is_some_and(|f| f.old),
                }
            }
            (None, None) => unreachable!("a synthesized member has its bytes"),
        })
        .collect())
}

/// Writes `plan` to `archive`. The plan's members are the last holders of
/// their files' mappings (the caller drops its own): they are let go
/// before the new archive takes the old one's place, which Windows needs.
pub fn write_plan(run: &mut Run<'_>, archive: &str, plan: Plan) -> Result<(), Fatal> {
    let thin = plan.thin;
    let (layout, members) = lay_out(run, archive, plan)?;
    let datas: Vec<output::Bytes<'_>> = members
        .iter()
        .map(|m| output::Bytes {
            data: if thin { &[] } else { m.data() },
            holes: &[],
        })
        .collect();
    let path = run.resolve(archive);
    let temp = output::write_unix_beside(&path, &layout, &datas)
        .map_err(|e| run.fatal(format!("{archive}: {}", io_text(&e))))?;
    drop(datas);
    drop(members);
    output::replace(&temp, &path).map_err(|e| run.fatal(format!("{archive}: {}", io_text(&e))))?;
    Ok(())
}

/// The archive `plan` makes, in memory (for a universal file's slices).
pub fn plan_bytes(run: &mut Run<'_>, archive: &str, plan: Plan) -> Result<Vec<u8>, Fatal> {
    let thin = plan.thin;
    let (layout, members) = lay_out(run, archive, plan)?;
    let size =
        usize::try_from(layout.size()).map_err(|_| run.fatal(format!("{archive}: too large")))?;
    let mut out = vec![0u8; size];
    layout.write_into(&mut out, |i| if thin { &[] } else { members[i].data() });
    Ok(out)
}

/// Folds `plan`'s members if asked, and lays the archive out: the layout,
/// and the members in it.
fn lay_out(run: &mut Run<'_>, archive: &str, plan: Plan) -> Result<(Layout, Vec<Item>), Fatal> {
    if plan.members.iter().any(|m| m.bytes.is_none()) && !plan.thin {
        return Err(run.fatal(format!("{archive}: a member's data is missing")));
    }
    let mut plan = plan;
    if run.settings.fold != Fold::None {
        let members = fold_members(run, archive, &plan)?;
        plan.members = members;
    }
    let archiver = run.archiver();
    // Each member's symbols, on every core.
    let symbols = turbonop_par::map(&plan.members, 8, |m| match &m.bytes {
        Some(b) => member_symbols(b.get(), archiver)
            .map(|s| {
                (
                    s.object,
                    s.symbols.iter().map(|n| n.to_vec()).collect::<Vec<_>>(),
                )
            })
            .map_err(|e| format!("{}: {e}", m.name)),
        None => Ok((false, Vec::new())),
    });
    let mut index: Vec<IndexSymbol<'_>> = Vec::new();
    let mut objects = false;
    let mut per_member = Vec::with_capacity(symbols.len());
    for s in symbols {
        per_member.push(s.map_err(|e| run.fatal(e))?);
    }
    for (i, (object, names)) in per_member.iter().enumerate() {
        objects |= *object;
        let member = u32::try_from(i).map_err(|_| run.fatal("too many members"))?;
        index.extend(names.iter().map(|n| IndexSymbol {
            name: std::borrow::Cow::Borrowed(n.as_slice()),
            member,
        }));
    }
    let table = plan.symtab && (objects || plan.dialect == Dialect::Darwin);
    let mut options = Options::new(archiver, plan.dialect);
    options.thin = plan.thin;
    options.filler = objects;
    if !plan.deterministic {
        options.table_date = run.env.now;
    }
    if let Some(t) = run.env.sym64_threshold {
        options.sym64_threshold = t;
    }
    // Darwin, deterministically: each name's copies dated 1, 2, ...
    let darwin = plan.dialect == Dialect::Darwin && plan.deterministic;
    let mut members = plan.members;
    if darwin && archiver == Archiver::LlvmAr {
        let mut count: std::collections::BTreeMap<String, u32> = std::collections::BTreeMap::new();
        for m in &members {
            *count.entry(m.name.clone()).or_default() += 1;
        }
        let mut next: std::collections::BTreeMap<String, u32> = std::collections::BTreeMap::new();
        for m in &mut members {
            if count[m.name.as_str()] > 1 {
                let n = next.entry(m.name.clone()).or_insert(1);
                let date = n.to_string();
                *n += 1;
                m.fields = Fields::new(
                    date.as_bytes(),
                    m.fields.uid(),
                    m.fields.gid(),
                    m.fields.mode(),
                )
                .unwrap_or(m.fields);
            }
        }
    }
    let entries: Vec<Entry<'_>> = members
        .iter()
        .map(|m| Entry {
            name: m.name.as_bytes(),
            origin: m.origin,
            fields: m.fields,
            size: m.size,
        })
        .collect();
    let layout = Layout::new(&entries, table.then_some(index.as_slice()), &options)
        .map_err(|e| run.fatal(format!("{archive}: {e}")))?;
    drop(entries);
    Ok((layout, members))
}

/// The `t` operation's line for a member.
fn table_line(env: &Env, c: &Command, archive: &str, old: &Old, i: usize) -> String {
    let m = &old.members[i];
    let mut s = String::new();
    if c.verbose {
        let mode = std::str::from_utf8(m.fields.mode())
            .ok()
            .and_then(|t| u32::from_str_radix(t, 8).ok())
            .unwrap_or(0);
        let bits = |b: u32, set: char| if mode & b != 0 { set } else { '-' };
        let mut ms = String::new();
        for (r, w, x) in [(0o400, 0o200, 0o100), (0o40, 0o20, 0o10), (0o4, 0o2, 0o1)] {
            ms.push(bits(r, 'r'));
            ms.push(bits(w, 'w'));
            ms.push(bits(x, 'x'));
        }
        let date: i64 = std::str::from_utf8(m.fields.date())
            .ok()
            .and_then(|d| d.parse().ok())
            .unwrap_or(0);
        let uid = std::str::from_utf8(m.fields.uid()).unwrap_or("0");
        let gid = std::str::from_utf8(m.fields.gid()).unwrap_or("0");
        let num = |t: &str| t.parse::<i64>().unwrap_or(0);
        s.push_str(&format!(
            "{ms} {}/{} {:6} {} ",
            num(uid),
            num(gid),
            m.size,
            calendar(date + super::localtime::offset_at(date, env.tz.as_deref()))
        ));
    }
    let shown = if old.thin && !Path::new(&m.name).is_absolute() {
        match Path::new(archive)
            .parent()
            .map(|p| p.to_string_lossy().replace('\\', "/"))
        {
            Some(d) if !d.is_empty() => format!("{d}/{}", m.name),
            _ => m.name.clone(),
        }
    } else {
        m.name.clone()
    };
    s.push_str(&shown);
    if c.offsets && !old.thin {
        s.push_str(&format!(" 0x{:x}", old.data_offsets[i]));
    }
    s.push('\n');
    s
}

/// `Mmm dd hh:mm yyyy` for a time in seconds since 1970 (local: with the
/// zone's offset added), as `ctime` shows it in POSIX's `ar t` form.
fn calendar(t: i64) -> String {
    const MONTHS: [&str; 12] = [
        "Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec",
    ];
    let days = t.div_euclid(86_400);
    let secs = t.rem_euclid(86_400);
    // Civil from days (Howard Hinnant's algorithm).
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = if m <= 2 { y + 1 } else { y };
    format!(
        "{} {:2} {:02}:{:02} {}",
        MONTHS[usize::try_from(m - 1).unwrap_or(0)],
        d,
        secs / 3600,
        secs % 3600 / 60,
        y
    )
}

/// The members a read operation's file names select (all, without names),
/// and the names not found.
fn select(run: &Run<'_>, c: &Command, archive: &str, old: &Old) -> (Vec<usize>, Vec<String>) {
    if c.files.is_empty() {
        return ((0..old.members.len()).collect(), Vec::new());
    }
    let mut taken = vec![false; old.members.len()];
    let mut out = Vec::new();
    let mut missing = Vec::new();
    for f in &c.files {
        let mut seen = 0;
        let mut found = false;
        for (i, m) in old.members.iter().enumerate() {
            if taken[i] || !matches(run, c, archive, old.thin, f, &m.name) {
                continue;
            }
            seen += 1;
            if c.count.is_some_and(|n| n != seen) {
                continue;
            }
            taken[i] = true;
            out.push(i);
            found = true;
            break;
        }
        if !found {
            missing.push(f.clone());
        }
    }
    if run.flavor.is_llvm() {
        // In the archive's order.
        out.sort_unstable();
    }
    (out, missing)
}

fn read_op(run: &mut Run<'_>, c: &Command, archive: &str, old: &Old) -> Result<(), Fatal> {
    let op = c.op.unwrap_or(Op::Table);
    if op == Op::Extract && old.thin {
        return Err(run.fatal(if run.flavor.is_llvm() {
            "extracting from a thin archive is not supported".to_string()
        } else {
            "`x' cannot be used on thin archives.".to_string()
        }));
    }
    let (chosen, missing) = select(run, c, archive, old);
    for i in chosen {
        match op {
            Op::Table => {
                let line = table_line(run.env, c, archive, old, i);
                run.print(&line);
            }
            Op::Print => {
                let m = &old.members[i];
                if c.verbose {
                    let head = if run.flavor.is_llvm() {
                        format!("Printing {}\n", m.name)
                    } else {
                        format!("\n<{}>\n\n", m.name)
                    };
                    run.print(&head);
                }
                let data = m.data().to_vec();
                run.stdout.extend_from_slice(&data);
            }
            Op::Extract => extract(run, c, &old.members[i])?,
            _ => {}
        }
    }
    if !missing.is_empty() {
        if run.flavor.is_llvm() {
            for m in &missing {
                run.stderr
                    .push_str(&format!("{}: error: '{m}' was not found\n", run.argv0));
            }
            run.status = 1;
        } else {
            for m in &missing {
                run.stderr.push_str(&format!("no entry {m} in archive\n"));
            }
        }
    }
    Ok(())
}

fn extract(run: &mut Run<'_>, c: &Command, m: &Item) -> Result<(), Fatal> {
    let mut name = m.name.clone();
    if run.flavor.is_llvm() {
        name = basename(&name).to_string();
    } else {
        let bad = Path::new(&name).is_absolute() || name.split('/').any(|p| p == "..");
        if bad {
            let base = basename(&name).to_string();
            run.warn(&format!(
                "illegal output pathname for archive member: {name}, using '{base}' instead"
            ));
            name = base;
        }
    }
    let out = match &c.output {
        Some(dir) if run.flavor.is_llvm() => {
            std::fs::create_dir_all(run.resolve(dir))
                .map_err(|_| run.fatal(format!("'{dir}' is not a directory")))?;
            format!("{}/{name}", dir.trim_end_matches('/'))
        }
        Some(dir) if !dir.is_empty() => format!("{}/{name}", dir.trim_end_matches('/')),
        _ => name,
    };
    if c.verbose {
        run.print(&format!("x - {out}\n"));
    }
    let path = run.resolve(&out);
    let data = m.data();
    std::fs::write(&path, data).map_err(|e| run.fatal(format!("{out}: {}", io_text(&e))))?;
    let mode = std::str::from_utf8(m.fields.mode())
        .ok()
        .and_then(|t| u32::from_str_radix(t, 8).ok());
    set_mode(&path, mode);
    if c.preserve_dates
        && let Some(t) = std::str::from_utf8(m.fields.date())
            .ok()
            .and_then(|d| d.parse::<u64>().ok())
        && let Ok(f) = std::fs::File::options().write(true).open(&path)
    {
        let _ = f.set_modified(std::time::UNIX_EPOCH + std::time::Duration::from_secs(t));
    }
    Ok(())
}

#[cfg(unix)]
fn set_mode(path: &Path, mode: Option<u32>) {
    use std::os::unix::fs::PermissionsExt;
    if let Some(m) = mode {
        let _ = std::fs::set_permissions(path, std::fs::Permissions::from_mode(m & 0o7777));
    }
}

#[cfg(not(unix))]
fn set_mode(_: &Path, _: Option<u32>) {}

/// `s` or `ranlib` on one archive.
fn symbol_table(run: &mut Run<'_>, c: &Command, archive: &str) -> Result<(), Fatal> {
    let Some(old) = read_old(run, archive)? else {
        return Err(run.fatal(if run.flavor.is_llvm() {
            format!("unable to load '{archive}': No such file or directory")
        } else {
            format!("'{archive}': No such file")
        }));
    };
    if c.touch {
        // GNU ranlib -t: an ELF archive's table has no date to update.
        if !old.has_symtab {
            return Err(run.fatal(format!("{archive}: no archive map to update")));
        }
        return Ok(());
    }
    if run.flavor.is_llvm() && old.has_symtab {
        return Ok(());
    }
    let deterministic = c.deterministic.unwrap_or(true);
    let dialect = if run.flavor.is_llvm() {
        llvm_dialect(run, c, Some(&old), &old.members, old.thin)?
    } else {
        Dialect::Gnu
    };
    let thin = old.thin;
    let mut members = old.members;
    if run.flavor.is_llvm() && deterministic {
        for m in &mut members {
            m.fields = Fields::new(b"0", b"0", b"0", b"644").unwrap_or(Fields::BLANK);
        }
    }
    let plan = Plan {
        members,
        thin,
        dialect,
        symtab: true,
        deterministic,
    };
    write_plan(run, archive, plan)
}

fn write_op(run: &mut Run<'_>, c: &Command, archive: &str) -> Result<(), Fatal> {
    let op = c.op.unwrap_or(Op::Replace);
    let old = read_old(run, archive)?;
    let creating = old.is_none();
    if creating {
        if !matches!(op, Op::QuickAppend | Op::Replace) {
            return Err(run.fatal(if run.flavor.is_llvm() {
                format!("unable to load '{archive}': No such file or directory")
            } else {
                format!("{archive}: No such file or directory")
            }));
        }
        if !c.create {
            run.warn(&format!("creating {archive}"));
        }
    }
    if let Some(o) = &old {
        if c.thin && !o.thin && !o.members.is_empty() {
            return Err(run.fatal(if run.flavor.is_llvm() {
                "cannot convert a regular archive to a thin one".to_string()
            } else {
                format!("Cannot convert existing library {archive} to thin format")
            }));
        }
        if !c.thin
            && o.thin
            && !run.flavor.is_llvm()
            && matches!(op, Op::QuickAppend | Op::Replace)
            && !o.members.is_empty()
        {
            return Err(run.fatal(format!(
                "Cannot convert existing thin library {archive} to normal format"
            )));
        }
    }
    let thin = c.thin || old.as_ref().is_some_and(|o| o.thin);
    let mut log = String::new();
    let planned = plan_members(run, c, archive, old.as_ref(), &mut log);
    run.print(&log);
    let (members, changed) = planned?;
    let deterministic = c.deterministic.unwrap_or(true);
    if !run.flavor.is_llvm() && !changed {
        // GNU ar writes nothing, but the empty archive it created.
        if creating && c.files.is_empty() {
            let path = run.resolve(archive);
            let magic: &[u8] = if thin { b"!<thin>\n" } else { b"!<arch>\n" };
            std::fs::write(&path, magic)
                .map_err(|e| run.fatal(format!("{archive}: {}", io_text(&e))))?;
        }
        return Ok(());
    }
    let dialect = if run.flavor.is_llvm() {
        llvm_dialect(run, c, old.as_ref(), &members, thin)?
    } else {
        Dialect::Gnu
    };
    // (The members hold what they need of the old archive.)
    drop(old);
    let mut members = members;
    if let Some(deps) = &c.libdeps
        && !run.flavor.is_llvm()
    {
        let mut data = deps.as_bytes().to_vec();
        data.push(0);
        let item = Item {
            name: "__.LIBDEP".into(),
            origin: None,
            fields: if deterministic {
                Fields::new(b"0", b"0", b"0", b"644").unwrap_or(Fields::BLANK)
            } else {
                let now = run.env.source_date_epoch.unwrap_or(run.env.now).to_string();
                Fields::new(now.as_bytes(), b"0", b"0", b"644").unwrap_or(Fields::BLANK)
            },
            size: data.len() as u64,
            bytes: Some(Bytes::whole(Arc::new(Mmap::from(data)))),
            old: false,
        };
        let at = usize::from(!members.is_empty() && !c.files.is_empty());
        members.insert(at.min(members.len()), item);
    }
    let plan = Plan {
        members,
        thin,
        dialect,
        symtab: c.symtab != Some(false),
        deterministic,
    };
    write_plan(run, archive, plan)
}

/// Runs the personality on `args` (the words after the program's name).
pub fn run(flavor: Flavor, argv0: &str, args: &[String], env: &Env) -> Outcome {
    let mut r = Run::new(flavor, argv0, env);
    let exit = match run_inner(&mut r, args) {
        Ok(()) => r.status,
        Err(Fatal(text)) => {
            r.stderr.push_str(&text);
            r.stderr.push('\n');
            1
        }
    };
    Outcome {
        stdout: r.stdout,
        stderr: r.stderr,
        exit,
    }
}

fn run_inner(r: &mut Run<'_>, args: &[String]) -> Result<(), Fatal> {
    let cwd = r.env.cwd.clone();
    let mut read = |p: &str| std::fs::read(cwd.join(p));
    let words = args::expand(args, &mut read);
    let c = args::parse(r.flavor, words).map_err(|e| r.fatal(e))?;
    if c.help {
        r.print(&help(r.flavor, &r.argv0));
        return Ok(());
    }
    if c.version {
        r.print(&format!(
            "turbolib {} (TurboNOP), an archiver compatible with {}\n",
            env!("CARGO_PKG_VERSION"),
            match r.flavor {
                Flavor::GnuAr => "GNU ar",
                Flavor::GnuRanlib => "GNU ranlib",
                Flavor::LlvmAr => "llvm-ar",
                Flavor::LlvmRanlib => "llvm-ranlib",
            }
        ));
        return Ok(());
    }
    let mut settings = Settings::default();
    if let Some(text) = &r.env.turbolib_options {
        settings
            .apply_env(text)
            .map_err(|e| r.fatal(format!("TURBOLIB_OPTIONS: {e}")))?;
    }
    for o in &c.turbolib {
        settings.apply(o).map_err(|e| r.fatal(e))?;
    }
    settings.check_supported().map_err(|e| r.fatal(e))?;
    if let Some(n) = settings.threads {
        turbonop_par::set_threads(n);
    }
    r.settings = settings;
    if c.mri {
        return super::mri::run(r, &c);
    }
    if r.flavor.is_ranlib() {
        if c.archives.is_empty() {
            return Err(r.fatal("an archive name must be specified"));
        }
        for a in &c.archives {
            symbol_table(r, &c, a)?;
        }
        return Ok(());
    }
    let Some(archive) = c.archives.first().cloned() else {
        return Err(r.fatal(if r.flavor.is_llvm() {
            "an archive name must be specified".to_string()
        } else {
            "no archive specified".to_string()
        }));
    };
    let Some(op) = c.op else {
        return Err(r.fatal("no operation specified"));
    };
    if !r.flavor.is_llvm() && c.update && op != Op::Replace {
        return Err(r.fatal("`u' is only meaningful with the `r' option."));
    }
    match op {
        Op::SymbolTable => symbol_table(r, &c, &archive),
        Op::Table | Op::Print | Op::Extract => {
            let Some(old) = read_old(r, &archive)? else {
                return Err(r.fatal(if r.flavor.is_llvm() {
                    format!("unable to load '{archive}': No such file or directory")
                } else {
                    format!("{archive}: No such file or directory")
                }));
            };
            read_op(r, &c, &archive, &old)
        }
        Op::Delete | Op::Move | Op::QuickAppend | Op::Replace => {
            if !r.flavor.is_llvm() && matches!(op, Op::Delete | Op::Move) && c.files.is_empty() {
                // GNU: nothing to do (but the archive must be there).
                return match read_old(r, &archive)? {
                    Some(_) => Ok(()),
                    None => Err(r.fatal(format!("{archive}: No such file or directory"))),
                };
            }
            write_op(r, &c, &archive)
        }
    }
}

/// The usage text.
fn help(flavor: Flavor, argv0: &str) -> String {
    match flavor {
        Flavor::GnuRanlib | Flavor::LlvmRanlib => format!(
            "Usage: {argv0} [options] archive...\n Generate an index to speed access to archives\n \
             The options are:\n  -D                           Use zero for symbol map timestamp (default)\n  \
             -U                           Use an actual symbol map timestamp\n  -t                           \
             Update the archive's symbol map timestamp\n  -h --help                    Print this help message\n  \
             -V --version                 Print version information\n turbolib's options: --fold=, --pull=, \
             --locals=, --stats, -j N (see docs/turbolib.md)\n"
        ),
        _ => format!(
            "Usage: {argv0} [emulation options] [-]{{dmpqrstx}}[abcDfilMNoOPsSTuvV] [--plugin <name>] \
             [member-name] [count] archive-file file...\n       {argv0} -M [<mri-script]\n \
             commands:\n  d            - delete file(s) from the archive\n  m[ab]        - move file(s) in \
             the archive\n  p            - print file(s) found in the archive\n  q[f]         - quick append \
             file(s) to the archive\n  r[ab][f][u]  - replace existing or insert new file(s) into the \
             archive\n  s            - act as ranlib\n  t[O][v]      - display contents of the archive\n  \
             x[o]         - extract file(s) from the archive\n turbolib's options: --fold=, --pull=, \
             --locals=, --stats, -j N (see docs/turbolib.md)\n"
        ),
    }
}

#[cfg(test)]
mod tests;
