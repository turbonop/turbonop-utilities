//! The processes running on this machine, as a process explorer shows
//! them.
//!
//! A [`Collector`] takes [`Snapshot`]s: every process with its counters
//! (CPU time, memory, I/O, threads, handles) and its details (executable,
//! command line, user), and the system's totals. Details don't change over
//! a process's life, so they are read once per process and shared by every
//! later snapshot. A [`Monitor`] compares each snapshot with the one before
//! it, for rates (CPU usage, I/O per second) and the processes that
//! started and exited between them.
//!
//! A process is known by its [`Key`], its ID and when it started: IDs are
//! reused, and a key isn't. [`kill`], [`suspend`], and [`resume`] act on a
//! key, so they never reach a later process that was given the same ID.
//!
//! Each platform's collector reads what that platform has:
//!
//! - Windows: one `NtQuerySystemInformation` call for every process and
//!   its threads; each new process opened once for its details. CPU usage
//!   is by cycles, as Process Explorer counts it (time is counted only at
//!   clock ticks).
//! - Linux: `/proc`, every process's files read on every core. (CPU time
//!   is `stat`'s, in clock ticks: `schedstat`'s nanoseconds count only a
//!   process's first thread.)
//! - macOS: `libproc` (`proc_pidinfo`, `proc_pid_rusage`) and `sysctl`.
//!   Without root, another user's processes show only what macOS allows.
//!
//! The parsing of what each platform returns (`/proc`'s text, Windows'
//! and macOS's structures) is built on every platform, and tested on every
//! platform; only the calls that get them are each platform's.

#![warn(missing_docs)]

#[cfg(test)]
use turbonop_global_alloc as _;

// (Each platform's parsing is built on every platform, for its tests, but
// used only on its own.)
#[cfg_attr(not(any(windows, target_os = "macos")), allow(dead_code))]
mod bytes;
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
mod linux;
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
mod macos;
mod monitor;
mod tree;
#[cfg_attr(not(windows), allow(dead_code))]
mod windows;

use std::fmt;
use std::io;
use std::sync::Arc;
use std::time::Instant;

use rustc_hash::FxHashMap;

pub use monitor::{Monitor, Rates, Sample, compare};
pub use tree::Tree;

/// A process: its ID, and when it started (nanoseconds since the Unix
/// epoch, 0 if unknown), which together no other process has had.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct Key {
    /// The process ID.
    pub pid: u32,
    /// When it started, in nanoseconds since the Unix epoch (0 if unknown,
    /// as for Windows' Idle and System processes).
    pub start: u64,
}

impl fmt::Display for Key {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}@{}", self.pid, self.start)
    }
}

/// What a process is doing, where the platform says.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum State {
    /// Running or able to run.
    Running,
    /// Waiting for something (most processes, most of the time).
    #[default]
    Sleeping,
    /// Waiting in the kernel, uninterruptibly (Linux's `D`).
    DiskSleep,
    /// Stopped by a signal (`SIGSTOP`) or a debugger.
    Stopped,
    /// Suspended: every thread (Windows).
    Suspended,
    /// Exited, not yet waited for by its parent.
    Zombie,
    /// Being created.
    Starting,
    /// Exiting.
    Dead,
}

impl State {
    /// Its name, for display.
    pub fn name(self) -> &'static str {
        match self {
            State::Running => "running",
            State::Sleeping => "sleeping",
            State::DiskSleep => "disk sleep",
            State::Stopped => "stopped",
            State::Suspended => "suspended",
            State::Zombie => "zombie",
            State::Starting => "starting",
            State::Dead => "dead",
        }
    }
}

/// What a process says about itself that doesn't change while it runs:
/// read once, when it is first seen.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Details {
    /// Its executable's path.
    pub path: Option<String>,
    /// Its command line, as one string (on Unix, its arguments joined with
    /// spaces, quoted where they need it).
    pub command_line: Option<String>,
    /// Its arguments, where the platform keeps them apart (Unix).
    pub args: Option<Vec<String>>,
    /// The user it runs as (`DOMAIN\user` on Windows).
    pub user: Option<String>,
    /// Its user ID (Unix).
    pub uid: Option<u32>,
    /// Its architecture, where it can differ from the machine's: "x64",
    /// "x86", "arm64", "arm" (Windows).
    pub architecture: Option<&'static str>,
    /// Whether it runs elevated (Windows' UAC; on Unix, as root).
    pub elevated: Option<bool>,
    /// Its integrity level (Windows): "Untrusted", "Low", "Medium",
    /// "Medium Plus", "High", "System", "Protected".
    pub integrity: Option<&'static str>,
    /// A kernel thread, not a program (Linux).
    pub kernel: bool,
}

/// What a process has used so far: counters, read on every snapshot.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Counters {
    /// CPU time in user mode, in nanoseconds.
    pub user_ns: u64,
    /// CPU time in the kernel, in nanoseconds.
    pub kernel_ns: u64,
    /// CPU time in all, in nanoseconds.
    pub cpu_ns: u64,
    /// CPU cycles, where the platform counts them (Windows).
    pub cycles: Option<u64>,
    /// Physical memory in use (working set, resident set), in bytes.
    pub resident: u64,
    /// Memory that is the process's alone (private bytes, anonymous
    /// memory, macOS's footprint), in bytes.
    pub private: u64,
    /// Address space reserved, in bytes.
    pub virtual_size: u64,
    /// Its threads.
    pub threads: u32,
    /// Its handles (Windows) or open file descriptors (Unix), where they
    /// can be counted.
    pub handles: Option<u32>,
    /// Bytes read, where counted (on Linux, from storage; on Windows, all
    /// I/O; on macOS, from disk).
    pub read_bytes: Option<u64>,
    /// Bytes written, where counted.
    pub write_bytes: Option<u64>,
    /// Read operations, where counted.
    pub read_ops: Option<u64>,
    /// Write operations, where counted.
    pub write_ops: Option<u64>,
    /// Page faults (hard and soft).
    pub page_faults: u64,
    /// Context switches, where counted.
    pub context_switches: Option<u64>,
    /// Its priority: Windows' base priority; on Unix, its nice value.
    pub priority: i32,
    /// Its session (Windows) or session leader (Unix).
    pub session: Option<u32>,
}

/// A process in a snapshot.
#[derive(Clone, Debug)]
pub struct Process {
    /// Which process it is.
    pub key: Key,
    /// Its parent's ID, as the system recorded it. (On Windows the parent
    /// may have exited, and its ID been reused: see [`Snapshot::parent`].)
    pub parent_pid: Option<u32>,
    /// Its name: its executable's file name, or what it calls itself.
    pub name: String,
    /// What it is doing.
    pub state: State,
    /// What it has used so far.
    pub counters: Counters,
    /// What doesn't change.
    pub details: Arc<Details>,
}

/// The system's totals in a snapshot.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct System {
    /// Logical processors.
    pub cpus: u32,
    /// Every processor's time in all, in nanoseconds (idle included).
    pub cpu_total_ns: u64,
    /// Every processor's idle time, in nanoseconds.
    pub cpu_idle_ns: u64,
    /// Every processor's idle cycles, where counted (Windows).
    pub idle_cycles: Option<u64>,
    /// Physical memory, in bytes.
    pub memory_total: u64,
    /// Physical memory available to programs without paging, in bytes.
    pub memory_available: u64,
    /// Memory committed (Windows' commit charge; Linux's `Committed_AS`).
    pub commit: Option<u64>,
    /// The commit limit (Windows), or Linux's `CommitLimit`.
    pub commit_limit: Option<u64>,
    /// Swap or page file, in bytes.
    pub swap_total: Option<u64>,
    /// Swap in use, in bytes.
    pub swap_used: Option<u64>,
    /// Time since boot, in nanoseconds.
    pub uptime_ns: u64,
}

/// Every process, and the system's totals, at one moment.
#[derive(Clone, Debug)]
pub struct Snapshot {
    /// When it was taken.
    pub taken: Instant,
    /// When it was taken, in nanoseconds since the Unix epoch.
    pub time: u64,
    /// The system's totals.
    pub system: System,
    /// Every process, by ID.
    pub processes: Vec<Process>,
}

impl Snapshot {
    /// The process with `key`, by index.
    pub fn find(&self, key: Key) -> Option<usize> {
        let i = self
            .processes
            .binary_search_by_key(&key.pid, |p| p.key.pid)
            .ok()?;
        (self.processes[i].key == key).then_some(i)
    }

    /// The process with ID `pid`, by index.
    pub fn find_pid(&self, pid: u32) -> Option<usize> {
        self.processes
            .binary_search_by_key(&pid, |p| p.key.pid)
            .ok()
    }

    /// The parent of the process at `i`, by index: the process with its
    /// parent's ID, if that started before it (else that ID is another
    /// process's now, and its parent has exited).
    pub fn parent(&self, i: usize) -> Option<usize> {
        let p = &self.processes[i];
        let ppid = p.parent_pid?;
        if ppid == p.key.pid {
            return None;
        }
        let j = self.find_pid(ppid)?;
        let parent = &self.processes[j];
        (parent.key.start <= p.key.start || p.key.start == 0 || parent.key.start == 0).then_some(j)
    }

    /// The processes as a tree, by parent.
    pub fn tree(&self) -> Tree {
        Tree::of(self)
    }
}

/// Takes snapshots of this machine's processes.
pub struct Collector {
    platform: Platform,
    /// Each process's details, from when it was first seen.
    details: FxHashMap<Key, Arc<Details>>,
}

#[cfg(target_os = "linux")]
type Platform = linux::Collector;
#[cfg(windows)]
type Platform = windows::Collector;
#[cfg(target_os = "macos")]
type Platform = macos::Collector;
#[cfg(not(any(target_os = "linux", windows, target_os = "macos")))]
type Platform = Unsupported;

/// The collector of a platform without one.
#[cfg(not(any(target_os = "linux", windows, target_os = "macos")))]
pub struct Unsupported;

#[cfg(not(any(target_os = "linux", windows, target_os = "macos")))]
impl Unsupported {
    fn new() -> io::Result<Self> {
        Err(unsupported())
    }
    fn snapshot(&mut self, _: &mut FxHashMap<Key, Arc<Details>>) -> io::Result<Snapshot> {
        Err(unsupported())
    }
}

/// The error of an operation this platform doesn't have.
#[cfg(not(any(target_os = "linux", windows, target_os = "macos")))]
pub(crate) fn unsupported() -> io::Error {
    io::Error::new(
        io::ErrorKind::Unsupported,
        "processes can't be listed on this platform",
    )
}

impl Collector {
    /// A collector for this machine.
    pub fn new() -> io::Result<Self> {
        Ok(Collector {
            platform: Platform::new()?,
            details: FxHashMap::default(),
        })
    }

    /// Every process, and the system's totals, now. Details are read for
    /// the processes not seen before; those that have exited are
    /// forgotten.
    pub fn snapshot(&mut self) -> io::Result<Snapshot> {
        let mut snapshot = self.platform.snapshot(&mut self.details)?;
        snapshot.processes.sort_unstable_by_key(|p| p.key.pid);
        self.details.retain(|key, _| snapshot.find(*key).is_some());
        Ok(snapshot)
    }
}

/// Ends the process `key` at once (Windows' `TerminateProcess`; Unix's
/// `SIGKILL`). It fails if no process has that key now.
pub fn kill(key: Key) -> io::Result<()> {
    #[cfg(target_os = "linux")]
    return linux::sys::signal(key, linux::sys::Signal::Kill);
    #[cfg(windows)]
    return windows::sys::terminate(key);
    #[cfg(target_os = "macos")]
    return macos::sys::signal(key, libc::SIGKILL);
    #[cfg(not(any(target_os = "linux", windows, target_os = "macos")))]
    {
        let _ = key;
        Err(unsupported())
    }
}

/// Asks the process `key` to end (Unix's `SIGTERM`). Windows has no way to
/// ask any process: there it is [`kill`].
pub fn terminate(key: Key) -> io::Result<()> {
    #[cfg(target_os = "linux")]
    return linux::sys::signal(key, linux::sys::Signal::Term);
    #[cfg(windows)]
    return windows::sys::terminate(key);
    #[cfg(target_os = "macos")]
    return macos::sys::signal(key, libc::SIGTERM);
    #[cfg(not(any(target_os = "linux", windows, target_os = "macos")))]
    {
        let _ = key;
        Err(unsupported())
    }
}

/// Suspends the process `key`: every thread (Windows' `NtSuspendProcess`;
/// Unix's `SIGSTOP`).
pub fn suspend(key: Key) -> io::Result<()> {
    #[cfg(target_os = "linux")]
    return linux::sys::signal(key, linux::sys::Signal::Stop);
    #[cfg(windows)]
    return windows::sys::suspend(key, true);
    #[cfg(target_os = "macos")]
    return macos::sys::signal(key, libc::SIGSTOP);
    #[cfg(not(any(target_os = "linux", windows, target_os = "macos")))]
    {
        let _ = key;
        Err(unsupported())
    }
}

/// Resumes the process `key` (Windows' `NtResumeProcess`; Unix's
/// `SIGCONT`).
pub fn resume(key: Key) -> io::Result<()> {
    #[cfg(target_os = "linux")]
    return linux::sys::signal(key, linux::sys::Signal::Cont);
    #[cfg(windows)]
    return windows::sys::suspend(key, false);
    #[cfg(target_os = "macos")]
    return macos::sys::signal(key, libc::SIGCONT);
    #[cfg(not(any(target_os = "linux", windows, target_os = "macos")))]
    {
        let _ = key;
        Err(unsupported())
    }
}

/// The error for a key whose process has exited (or whose ID is another
/// process's now).
#[cfg(any(target_os = "linux", windows, target_os = "macos"))]
pub(crate) fn gone(key: Key) -> io::Error {
    io::Error::new(
        io::ErrorKind::NotFound,
        format!("process {} has exited", key.pid),
    )
}

/// The time now, in nanoseconds since the Unix epoch.
#[cfg(any(target_os = "linux", windows, target_os = "macos"))]
pub(crate) fn now_ns() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| u64::try_from(d.as_nanos()).unwrap_or(u64::MAX))
}

/// `args` as one command line, each quoted as a POSIX shell would need
/// (single quotes) where it has a space or a character the shell treats
/// specially.
pub fn join_args(args: &[String]) -> String {
    let mut out = String::new();
    for (i, a) in args.iter().enumerate() {
        if i > 0 {
            out.push(' ');
        }
        let plain = !a.is_empty()
            && a.bytes()
                .all(|b| b.is_ascii_alphanumeric() || b"-_./:=,+@%^".contains(&b) || b >= 0x80);
        if plain {
            out.push_str(a);
        } else {
            out.push('\'');
            out.push_str(&a.replace('\'', r"'\''"));
            out.push('\'');
        }
    }
    out
}

/// The file name of `path` (after its last `/`, or on Windows `\`).
pub(crate) fn file_name(path: &str) -> &str {
    path.rsplit(['/', '\\']).next().unwrap_or(path)
}

#[cfg(test)]
mod tests;
