//! Linux's collector: `/proc`, every process's files read on every core.

// The `unsafe` calls: `pidfd_open` and `pidfd_send_signal` (system calls
// libc has no functions for), `kill`, and taking ownership of the
// descriptor `pidfd_open` returns.
#![allow(unsafe_code)]

use std::fs;
use std::io;
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd};
use std::sync::{Arc, OnceLock};
use std::time::Instant;

use rustc_hash::FxHashMap;

use super::{
    Io, PF_KTHREAD, Stat, display_name, exe_path, parse_auxv, parse_cmdline, parse_io,
    parse_meminfo, parse_passwd, parse_proc_stat, parse_stat, parse_statm, parse_status_uids,
    parse_uptime, state_of,
};
use crate::{Counters, Details, Key, Process, Snapshot, System, gone, join_args, now_ns};

/// What converts `/proc`'s ticks and pages: read once, so that a process's
/// start (its key) is the same in every snapshot, though the kernel's
/// `btime` may move by a second as the clock is adjusted.
#[derive(Clone, Copy, Debug)]
struct Clock {
    tick_hz: u64,
    page: u64,
    btime: u64,
}

fn clock() -> Clock {
    static CLOCK: OnceLock<Clock> = OnceLock::new();
    *CLOCK.get_or_init(|| {
        let (tick, page) = fs::read("/proc/self/auxv").map_or((None, None), |b| {
            parse_auxv(&b, std::mem::size_of::<usize>())
        });
        let btime = fs::read_to_string("/proc/stat").map_or(0, |t| parse_proc_stat(&t).btime);
        Clock {
            tick_hz: tick.unwrap_or(100),
            page: page.unwrap_or(4096),
            btime,
        }
    })
}

impl Clock {
    fn ticks_ns(self, ticks: u64) -> u64 {
        // (As u128: ticks since boot, times 10^9, outgrow u64 in a year.)
        u64::try_from(u128::from(ticks) * 1_000_000_000 / u128::from(self.tick_hz.max(1)))
            .unwrap_or(u64::MAX)
    }

    fn start_ns(self, starttime: u64) -> u64 {
        self.btime
            .saturating_mul(1_000_000_000)
            .saturating_add(self.ticks_ns(starttime))
    }
}

/// One process's files, as read for a snapshot.
struct Raw {
    stat: Stat,
    statm: Option<(u64, u64, u64)>,
    io: Option<Io>,
    fds: Option<u32>,
}

/// Linux's collector.
pub(crate) struct Collector {
    clock: Clock,
    /// User names by ID, from `/etc/passwd` (read again when a process has
    /// an ID it doesn't name).
    users: FxHashMap<u32, String>,
    users_read: Option<Instant>,
}

impl Collector {
    pub(crate) fn new() -> io::Result<Self> {
        fs::metadata("/proc/self/stat")?;
        Ok(Collector {
            clock: clock(),
            users: FxHashMap::default(),
            users_read: None,
        })
    }

    pub(crate) fn snapshot(
        &mut self,
        details: &mut FxHashMap<Key, Arc<Details>>,
    ) -> io::Result<Snapshot> {
        let taken = Instant::now();
        let time = now_ns();
        let pids: Vec<u32> = fs::read_dir("/proc")?
            .filter_map(|e| e.ok()?.file_name().to_str()?.parse().ok())
            .collect();
        let (raws, system) = turbonop_par::join(
            || turbonop_par::map(&pids, 8, |&pid| read(pid)),
            || self.system(),
        );
        let raws: Vec<Raw> = raws.into_iter().flatten().collect();
        let clock = self.clock;
        let key = |r: &Raw| Key {
            pid: r.stat.pid,
            start: clock.start_ns(r.stat.starttime),
        };
        // Details of the processes not seen before, read on every core.
        let new: Vec<(Key, &Raw)> = raws
            .iter()
            .map(|r| (key(r), r))
            .filter(|(k, _)| !details.contains_key(k))
            .collect();
        let read_details = turbonop_par::map(&new, 4, |&(k, r)| (k, read_details(k.pid, &r.stat)));
        if read_details
            .iter()
            .any(|(_, d)| d.uid.is_some_and(|uid| !self.users.contains_key(&uid)))
        {
            self.read_users();
        }
        for (k, mut d) in read_details {
            d.user = d
                .uid
                .and_then(|uid| self.users.get(&uid).cloned())
                .or_else(|| d.uid.map(|u| u.to_string()));
            details.insert(k, Arc::new(d));
        }
        let page = clock.page;
        let processes = raws
            .iter()
            .map(|r| {
                let k = key(r);
                let d = details.get(&k).cloned().unwrap_or_default();
                let s = &r.stat;
                let (vsize, resident, shared) = r.statm.unwrap_or((s.vsize / page.max(1), 0, 0));
                let user_ns = clock.ticks_ns(s.utime);
                let kernel_ns = clock.ticks_ns(s.stime);
                let name = display_name(
                    &s.comm,
                    d.path.as_deref(),
                    d.args.as_ref().and_then(|a| a.first()).map(String::as_str),
                );
                Process {
                    key: k,
                    parent_pid: (s.ppid != 0).then_some(s.ppid),
                    name,
                    state: state_of(s.state),
                    counters: Counters {
                        user_ns,
                        kernel_ns,
                        cpu_ns: user_ns + kernel_ns,
                        cycles: None,
                        resident: resident * page,
                        private: resident.saturating_sub(shared) * page,
                        virtual_size: vsize * page,
                        threads: s.threads,
                        handles: r.fds,
                        read_bytes: r.io.map(|io| io.read_bytes),
                        write_bytes: r
                            .io
                            .map(|io| io.write_bytes.saturating_sub(io.cancelled_write_bytes)),
                        read_ops: r.io.map(|io| io.syscr),
                        write_ops: r.io.map(|io| io.syscw),
                        page_faults: s.minflt + s.majflt,
                        // (`status` and `schedstat` count only its first
                        // thread's.)
                        context_switches: None,
                        priority: s.nice,
                        session: Some(s.session),
                    },
                    details: d,
                }
            })
            .collect();
        Ok(Snapshot {
            taken,
            time,
            system: system?,
            processes,
        })
    }

    fn read_users(&mut self) {
        // (At most once a second: an ID no one names would read it on
        // every snapshot.)
        if self.users_read.is_some_and(|t| t.elapsed().as_secs() < 1) {
            return;
        }
        self.users_read = Some(Instant::now());
        if let Ok(text) = fs::read_to_string("/etc/passwd") {
            self.users = parse_passwd(&text);
        }
    }

    fn system(&self) -> io::Result<System> {
        let stat = parse_proc_stat(&fs::read_to_string("/proc/stat")?);
        let mem = parse_meminfo(&fs::read_to_string("/proc/meminfo")?);
        let uptime = fs::read_to_string("/proc/uptime")
            .ok()
            .and_then(|t| parse_uptime(&t))
            .unwrap_or(0);
        Ok(System {
            cpus: stat.cpus.max(1),
            cpu_total_ns: self.clock.ticks_ns(stat.total),
            cpu_idle_ns: self.clock.ticks_ns(stat.idle),
            idle_cycles: None,
            memory_total: mem.total,
            memory_available: mem.available,
            commit: mem.committed,
            commit_limit: mem.commit_limit,
            swap_total: Some(mem.swap_total),
            swap_used: Some(mem.swap_total.saturating_sub(mem.swap_free)),
            uptime_ns: uptime,
        })
    }
}

/// A process's counters' files (`None` if it has exited).
fn read(pid: u32) -> Option<Raw> {
    let dir = format!("/proc/{pid}");
    let stat = parse_stat(&fs::read_to_string(format!("{dir}/stat")).ok()?)?;
    let statm = fs::read_to_string(format!("{dir}/statm"))
        .ok()
        .and_then(|t| parse_statm(&t));
    // (Another user's, without root: not readable.)
    let io = fs::read_to_string(format!("{dir}/io"))
        .ok()
        .map(|t| parse_io(&t));
    let fds = fs::read_dir(format!("{dir}/fd"))
        .ok()
        .map(|d| u32::try_from(d.count()).unwrap_or(u32::MAX));
    Some(Raw {
        stat,
        statm,
        io,
        fds,
    })
}

/// What doesn't change about a process: its executable, arguments, and
/// user.
fn read_details(pid: u32, stat: &Stat) -> Details {
    let dir = format!("/proc/{pid}");
    let kernel = stat.flags & PF_KTHREAD != 0;
    let mut d = Details {
        kernel,
        ..Details::default()
    };
    if kernel {
        d.uid = Some(0);
        d.elevated = Some(true);
        return d;
    }
    d.path = fs::read_link(format!("{dir}/exe"))
        .ok()
        .map(|p| exe_path(&p.to_string_lossy()).to_owned());
    if let Ok(bytes) = fs::read(format!("{dir}/cmdline")) {
        let args = parse_cmdline(&bytes);
        if !args.is_empty() {
            d.command_line = Some(join_args(&args));
            d.args = Some(args);
        }
    }
    if let Some((_, euid)) = fs::read_to_string(format!("{dir}/status"))
        .ok()
        .and_then(|t| parse_status_uids(&t))
    {
        d.uid = Some(euid);
        d.elevated = Some(euid == 0);
    }
    d
}

/// The signals sent.
#[derive(Clone, Copy, Debug)]
pub(crate) enum Signal {
    Kill,
    Term,
    Stop,
    Cont,
}

impl Signal {
    fn number(self) -> libc::c_int {
        match self {
            Signal::Kill => libc::SIGKILL,
            Signal::Term => libc::SIGTERM,
            Signal::Stop => libc::SIGSTOP,
            Signal::Cont => libc::SIGCONT,
        }
    }
}

/// Whether the process with `key`'s ID is the one `key` names.
fn is(key: Key) -> bool {
    let clock = clock();
    fs::read_to_string(format!("/proc/{}/stat", key.pid))
        .ok()
        .and_then(|t| parse_stat(&t))
        .is_some_and(|s| clock.start_ns(s.starttime) == key.start)
}

/// Sends `signal` to the process `key`. Through a pidfd where the kernel
/// has them (5.3 on): opened, then checked to be `key`'s process, it can
/// reach no other.
pub(crate) fn signal(key: Key, signal: Signal) -> io::Result<()> {
    let pid = libc::pid_t::try_from(key.pid).map_err(|_| gone(key))?;
    // SAFETY: pidfd_open takes a process ID and flags, and returns a new
    // descriptor or -1.
    let fd = unsafe { libc::syscall(libc::SYS_pidfd_open, pid, 0) };
    if fd >= 0 {
        let raw = libc::c_int::try_from(fd)
            .map_err(|_| io::Error::other("pidfd_open: bad descriptor"))?;
        // SAFETY: pidfd_open returned this descriptor, which nothing else
        // owns.
        let fd = unsafe { OwnedFd::from_raw_fd(raw) };
        if !is(key) {
            return Err(gone(key));
        }
        // SAFETY: a pidfd, a signal number, no siginfo, no flags.
        let r = unsafe {
            libc::syscall(
                libc::SYS_pidfd_send_signal,
                fd.as_raw_fd(),
                signal.number(),
                std::ptr::null::<u8>(),
                0,
            )
        };
        return if r == 0 {
            Ok(())
        } else {
            Err(io::Error::last_os_error())
        };
    }
    let e = io::Error::last_os_error();
    if e.raw_os_error() == Some(libc::ESRCH) {
        return Err(gone(key));
    }
    if e.raw_os_error() != Some(libc::ENOSYS) {
        return Err(e);
    }
    // (No pidfds: checked, then signaled, by ID.)
    if !is(key) {
        return Err(gone(key));
    }
    // SAFETY: kill takes a process ID and a signal number.
    if unsafe { libc::kill(pid, signal.number()) } == 0 {
        Ok(())
    } else {
        Err(io::Error::last_os_error())
    }
}
