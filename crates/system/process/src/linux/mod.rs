//! Linux: what `/proc` says, parsed. (Built and tested on every platform;
//! the collector that reads `/proc` is [`sys`], Linux's alone.)

#[cfg(target_os = "linux")]
pub(crate) mod sys;
#[cfg(target_os = "linux")]
pub(crate) use sys::Collector;

use rustc_hash::FxHashMap;

use crate::State;

/// `/proc/<pid>/stat`'s fields that are read.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct Stat {
    pub pid: u32,
    /// The name it calls itself (at most 15 bytes: the kernel's `comm`).
    pub comm: String,
    pub state: u8,
    pub ppid: u32,
    pub session: u32,
    pub flags: u64,
    pub minflt: u64,
    pub majflt: u64,
    /// CPU time, in clock ticks.
    pub utime: u64,
    pub stime: u64,
    pub nice: i32,
    pub threads: u32,
    /// When it started, in clock ticks since boot.
    pub starttime: u64,
    pub vsize: u64,
}

/// The kernel's flag for a kernel thread (`PF_KTHREAD`).
pub(crate) const PF_KTHREAD: u64 = 0x0020_0000;

/// `/proc/<pid>/stat`. Its name is in parentheses and may hold anything,
/// parentheses and spaces included: it ends at the last `)`.
pub(crate) fn parse_stat(text: &str) -> Option<Stat> {
    let open = text.find('(')?;
    let close = text.rfind(')')?;
    let pid = text.get(..open)?.trim().parse().ok()?;
    let comm = text.get(open + 1..close)?.to_owned();
    let mut f = text.get(close + 1..)?.split_ascii_whitespace();
    // (Fields 3 on, as proc(5) numbers them.)
    let mut next = || f.next();
    let state = *next()?.as_bytes().first()?;
    let ppid = next()?.parse().ok()?;
    let _pgrp = next()?;
    let session = next()?.parse().ok()?;
    let _tty = next()?;
    let _tpgid = next()?;
    let flags = next()?.parse().ok()?;
    let minflt = next()?.parse().ok()?;
    let _cminflt = next()?;
    let majflt = next()?.parse().ok()?;
    let _cmajflt = next()?;
    let utime = next()?.parse().ok()?;
    let stime = next()?.parse().ok()?;
    let _cutime = next()?;
    let _cstime = next()?;
    let _priority = next()?;
    let nice = next()?.parse().ok()?;
    let threads = next()?.parse().ok()?;
    let _itrealvalue = next()?;
    let starttime = next()?.parse().ok()?;
    let vsize = next()?.parse().ok()?;
    Some(Stat {
        pid,
        comm,
        state,
        ppid,
        session,
        flags,
        minflt,
        majflt,
        utime,
        stime,
        nice,
        threads,
        starttime,
        vsize,
    })
}

/// A `stat` state's meaning.
pub(crate) fn state_of(c: u8) -> State {
    match c {
        b'R' => State::Running,
        b'D' => State::DiskSleep,
        b'T' | b't' => State::Stopped,
        b'Z' => State::Zombie,
        b'X' | b'x' => State::Dead,
        // (S sleeping, I idle, W waking or paging, P parked, K wakekill.)
        _ => State::Sleeping,
    }
}

/// `/proc/<pid>/statm`: its size, resident set, and the resident pages
/// that are shared (file-backed or shared memory), in pages.
pub(crate) fn parse_statm(text: &str) -> Option<(u64, u64, u64)> {
    let mut f = text.split_ascii_whitespace().map(str::parse::<u64>);
    Some((f.next()?.ok()?, f.next()?.ok()?, f.next()?.ok()?))
}

/// `/proc/<pid>/io`'s counters.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct Io {
    pub rchar: u64,
    pub wchar: u64,
    pub syscr: u64,
    pub syscw: u64,
    pub read_bytes: u64,
    pub write_bytes: u64,
    pub cancelled_write_bytes: u64,
}

/// `/proc/<pid>/io`: `name: value` lines.
pub(crate) fn parse_io(text: &str) -> Io {
    let mut io = Io::default();
    for line in text.lines() {
        let Some((name, value)) = line.split_once(':') else {
            continue;
        };
        let Ok(v) = value.trim().parse() else {
            continue;
        };
        match name {
            "rchar" => io.rchar = v,
            "wchar" => io.wchar = v,
            "syscr" => io.syscr = v,
            "syscw" => io.syscw = v,
            "read_bytes" => io.read_bytes = v,
            "write_bytes" => io.write_bytes = v,
            "cancelled_write_bytes" => io.cancelled_write_bytes = v,
            _ => {}
        }
    }
    io
}

/// `/proc/<pid>/cmdline`: its arguments, each ended by a NUL. (A process
/// that rewrote its arguments, as `setproctitle` does, may have one
/// string with spaces and no NULs after it, or padding NULs.)
pub(crate) fn parse_cmdline(bytes: &[u8]) -> Vec<String> {
    let bytes = bytes.strip_suffix(b"\0").unwrap_or(bytes);
    if bytes.is_empty() {
        return Vec::new();
    }
    let mut args: Vec<String> = bytes
        .split(|&b| b == 0)
        .map(|a| String::from_utf8_lossy(a).into_owned())
        .collect();
    while args.len() > 1 && args.last().is_some_and(String::is_empty) {
        args.pop();
    }
    args
}

/// `/proc/<pid>/status`'s user IDs: real, effective.
pub(crate) fn parse_status_uids(text: &str) -> Option<(u32, u32)> {
    let line = text.lines().find_map(|l| l.strip_prefix("Uid:"))?;
    let mut f = line.split_ascii_whitespace().map(str::parse::<u32>);
    Some((f.next()?.ok()?, f.next()?.ok()?))
}

/// `/etc/passwd`: each user ID's name (the first, of an ID with several).
pub(crate) fn parse_passwd(text: &str) -> FxHashMap<u32, String> {
    let mut users = FxHashMap::default();
    for line in text.lines() {
        if line.starts_with('#') {
            continue;
        }
        let mut f = line.split(':');
        let (Some(name), Some(_), Some(uid)) = (f.next(), f.next(), f.next()) else {
            continue;
        };
        if let Ok(uid) = uid.parse() {
            users.entry(uid).or_insert_with(|| name.to_owned());
        }
    }
    users
}

/// `/proc/stat`'s totals.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct ProcStat {
    /// Processors (`cpuN` lines).
    pub cpus: u32,
    /// Every processor's time, in clock ticks (`steal` included; `guest`,
    /// which `user` already counts, not).
    pub total: u64,
    /// Idle and waiting for I/O, in clock ticks.
    pub idle: u64,
    /// When the system booted, in seconds since the Unix epoch.
    pub btime: u64,
}

/// `/proc/stat`.
pub(crate) fn parse_proc_stat(text: &str) -> ProcStat {
    let mut s = ProcStat::default();
    for line in text.lines() {
        if let Some(rest) = line.strip_prefix("cpu ") {
            let v: Vec<u64> = rest
                .split_ascii_whitespace()
                .filter_map(|f| f.parse().ok())
                .collect();
            // user nice system idle iowait irq softirq steal guest guest_nice
            s.total = v.iter().take(8).sum();
            s.idle = v.get(3).copied().unwrap_or(0) + v.get(4).copied().unwrap_or(0);
        } else if line.starts_with("cpu") && line.as_bytes().get(3).is_some_and(u8::is_ascii_digit)
        {
            s.cpus += 1;
        } else if let Some(rest) = line.strip_prefix("btime ") {
            s.btime = rest.trim().parse().unwrap_or(0);
        }
    }
    s
}

/// `/proc/meminfo`'s totals, in bytes.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct MemInfo {
    pub total: u64,
    pub available: u64,
    pub swap_total: u64,
    pub swap_free: u64,
    pub committed: Option<u64>,
    pub commit_limit: Option<u64>,
}

/// `/proc/meminfo`: `Name:   value kB` lines. (A kernel before 3.14 has no
/// `MemAvailable`: free, buffers, and cache.)
pub(crate) fn parse_meminfo(text: &str) -> MemInfo {
    let mut m = MemInfo::default();
    let (mut free, mut buffers, mut cached, mut available) = (0, 0, 0, None);
    for line in text.lines() {
        let Some((name, value)) = line.split_once(':') else {
            continue;
        };
        let mut f = value.split_ascii_whitespace();
        let Some(Ok(n)) = f.next().map(str::parse::<u64>) else {
            continue;
        };
        let bytes = if f.next() == Some("kB") {
            n.saturating_mul(1024)
        } else {
            n
        };
        match name {
            "MemTotal" => m.total = bytes,
            "MemFree" => free = bytes,
            "MemAvailable" => available = Some(bytes),
            "Buffers" => buffers = bytes,
            "Cached" => cached = bytes,
            "SwapTotal" => m.swap_total = bytes,
            "SwapFree" => m.swap_free = bytes,
            "Committed_AS" => m.committed = Some(bytes),
            "CommitLimit" => m.commit_limit = Some(bytes),
            _ => {}
        }
    }
    m.available = available.unwrap_or(free + buffers + cached);
    m
}

/// `/proc/uptime`: seconds since boot, as nanoseconds.
pub(crate) fn parse_uptime(text: &str) -> Option<u64> {
    let secs = text.split_ascii_whitespace().next()?;
    let (whole, frac) = secs.split_once('.').unwrap_or((secs, ""));
    let whole: u64 = whole.parse().ok()?;
    let mut ns = 0u64;
    let mut scale = 100_000_000;
    for d in frac.bytes().take(9) {
        if !d.is_ascii_digit() {
            return None;
        }
        ns += u64::from(d - b'0') * scale;
        scale /= 10;
    }
    whole.checked_mul(1_000_000_000)?.checked_add(ns)
}

/// The auxiliary vector (`/proc/self/auxv`) of a process whose words are
/// `word` bytes: its clock tick rate (`AT_CLKTCK`) and page size
/// (`AT_PAGESZ`), where it has them.
pub(crate) fn parse_auxv(bytes: &[u8], word: usize) -> (Option<u64>, Option<u64>) {
    const AT_PAGESZ: u64 = 6;
    const AT_CLKTCK: u64 = 17;
    let read = |c: &[u8]| -> u64 {
        match word {
            4 => u64::from(u32::from_ne_bytes([c[0], c[1], c[2], c[3]])),
            _ => u64::from_ne_bytes([c[0], c[1], c[2], c[3], c[4], c[5], c[6], c[7]]),
        }
    };
    let word = if word == 4 { 4 } else { 8 };
    let (mut tick, mut page) = (None, None);
    for pair in bytes.chunks_exact(word * 2) {
        let (kind, value) = (read(&pair[..word]), read(&pair[word..]));
        match kind {
            0 => break,
            AT_CLKTCK if value > 0 => tick = Some(value),
            AT_PAGESZ if value > 0 => page = Some(value),
            _ => {}
        }
    }
    (tick, page)
}

/// `/proc/<pid>/exe`'s target: the executable, without the ` (deleted)`
/// the kernel adds for one that has since been deleted or replaced.
pub(crate) fn exe_path(link: &str) -> &str {
    link.strip_suffix(" (deleted)").unwrap_or(link)
}

/// The name to show for a process: its `comm`, unless that is cut short
/// (15 bytes), and its executable's or first argument's file name begins
/// with it, which is then the whole name.
pub(crate) fn display_name(comm: &str, exe: Option<&str>, argv0: Option<&str>) -> String {
    if comm.len() >= 15 {
        for whole in [exe, argv0].into_iter().flatten() {
            let name = crate::file_name(whole);
            if name.len() > comm.len() && name.starts_with(comm) {
                return name.to_owned();
            }
        }
    }
    comm.to_owned()
}

#[cfg(test)]
mod tests;
