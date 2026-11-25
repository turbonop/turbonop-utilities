//! macOS's collector: `libproc` and `sysctl`, every process read on every
//! core.

// Every call here is to libSystem (`libproc`, Mach, `sysctl`, `getpwuid_r`,
// `kill`), with buffers of the sizes given: that is all the `unsafe` there
// is.
#![allow(unsafe_code)]

use std::ffi::{CStr, c_int, c_void};
use std::io;
use std::sync::{Arc, Mutex, OnceLock, PoisonError};
use std::time::Instant;

use rustc_hash::FxHashMap;

use super::{
    BSDINFO_SIZE, BsdInfo, RUSAGE_V2_SIZE, Rusage, TASKINFO_SIZE, TaskInfo, parse_bsdinfo,
    parse_procargs, parse_rusage, parse_taskinfo, parse_vm_stats, state_of,
};
use crate::{
    Counters, Details, Key, Process, Snapshot, System, file_name, gone, join_args, now_ns,
};

unsafe extern "C" {
    fn proc_listallpids(buffer: *mut c_void, size: c_int) -> c_int;
    fn proc_pidinfo(pid: c_int, flavor: c_int, arg: u64, buffer: *mut c_void, size: c_int)
    -> c_int;
    fn proc_pidpath(pid: c_int, buffer: *mut c_void, size: u32) -> c_int;
    fn proc_pid_rusage(pid: c_int, flavor: c_int, buffer: *mut *mut c_void) -> c_int;
    fn mach_timebase_info(info: *mut u32) -> c_int;
    fn mach_host_self() -> u32;
    fn host_statistics(host: u32, flavor: c_int, info: *mut c_int, count: *mut u32) -> c_int;
    fn host_statistics64(host: u32, flavor: c_int, info: *mut c_int, count: *mut u32) -> c_int;
}

const PROC_PIDLISTFDS: c_int = 1;
const PROC_PIDTASKALLINFO: c_int = 2;
const PROC_PIDTBSDINFO: c_int = 3;
const RUSAGE_INFO_V2: c_int = 2;
const HOST_CPU_LOAD_INFO: c_int = 3;
const HOST_VM_INFO64: c_int = 4;
const HOST_VM_INFO64_COUNT: u32 = 38;
const PROC_FDINFO_SIZE: usize = 8;

/// Mach absolute time's units, as a fraction of a nanosecond.
fn timebase() -> (u64, u64) {
    static TIMEBASE: OnceLock<(u64, u64)> = OnceLock::new();
    *TIMEBASE.get_or_init(|| {
        let mut info = [0u32; 2];
        // SAFETY: mach_timebase_info_data_t: two u32s to fill in.
        if unsafe { mach_timebase_info(info.as_mut_ptr()) } != 0 || info[1] == 0 {
            return (1, 1);
        }
        (u64::from(info[0]), u64::from(info[1]))
    })
}

fn mach_ns(t: u64) -> u64 {
    let (numer, denom) = timebase();
    u64::try_from(u128::from(t) * u128::from(numer) / u128::from(denom)).unwrap_or(u64::MAX)
}

/// `sysctlbyname`'s bytes for `name`, up to `max`.
fn sysctl_bytes(name: &CStr, max: usize) -> Option<Vec<u8>> {
    let mut buf = vec![0u8; max];
    let mut len = buf.len();
    // SAFETY: a NUL-terminated name, a buffer of `len` bytes, and no new
    // value.
    let r = unsafe {
        libc::sysctlbyname(
            name.as_ptr(),
            buf.as_mut_ptr().cast(),
            &raw mut len,
            std::ptr::null_mut(),
            0,
        )
    };
    if r != 0 {
        return None;
    }
    buf.truncate(len);
    Some(buf)
}

/// A number `sysctlbyname` gives, 4 or 8 bytes.
fn sysctl_number(name: &CStr) -> Option<u64> {
    let b = sysctl_bytes(name, 8)?;
    match b.len() {
        4 => Some(u64::from(u32::from_ne_bytes(b.try_into().ok()?))),
        8 => Some(u64::from_ne_bytes(b.try_into().ok()?)),
        _ => None,
    }
}

/// The `proc_pidinfo` of `flavor`, `size` bytes, if all of it was given.
fn pidinfo(pid: c_int, flavor: c_int, size: usize) -> Option<Vec<u8>> {
    let mut buf = vec![0u8; size];
    // SAFETY: a buffer of the size given.
    let n = unsafe {
        proc_pidinfo(
            pid,
            flavor,
            0,
            buf.as_mut_ptr().cast(),
            c_int::try_from(size).ok()?,
        )
    };
    (usize::try_from(n).ok()? == size).then_some(buf)
}

fn bsdinfo(pid: c_int) -> Option<BsdInfo> {
    parse_bsdinfo(&pidinfo(pid, PROC_PIDTBSDINFO, BSDINFO_SIZE)?)
}

/// A process's counters' structures.
struct Raw {
    bsd: BsdInfo,
    task: Option<TaskInfo>,
    rusage: Option<Rusage>,
    fds: Option<u32>,
}

fn read(pid: c_int) -> Option<Raw> {
    // Its BSD and task information at once; only the BSD information, for
    // another user's process without root.
    let (bsd, task) = match pidinfo(pid, PROC_PIDTASKALLINFO, BSDINFO_SIZE + TASKINFO_SIZE) {
        Some(b) => (
            parse_bsdinfo(&b[..BSDINFO_SIZE])?,
            parse_taskinfo(&b[BSDINFO_SIZE..]),
        ),
        None => (bsdinfo(pid)?, None),
    };
    let mut usage = [0u8; RUSAGE_V2_SIZE];
    let mut ptr: *mut c_void = usage.as_mut_ptr().cast();
    // SAFETY: rusage_info_t is a pointer to the structure to fill in, here
    // `usage`, which is as large as RUSAGE_INFO_V2's.
    let rusage = (unsafe { proc_pid_rusage(pid, RUSAGE_INFO_V2, &raw mut ptr) } == 0)
        .then(|| parse_rusage(&usage))
        .flatten();
    Some(Raw {
        bsd,
        task,
        rusage,
        fds: fds(pid),
    })
}

/// Its open file descriptors.
fn fds(pid: c_int) -> Option<u32> {
    // SAFETY: no buffer: the size one would need.
    let need = unsafe { proc_pidinfo(pid, PROC_PIDLISTFDS, 0, std::ptr::null_mut(), 0) };
    let need = usize::try_from(need).ok().filter(|&n| n > 0)?;
    let mut buf = vec![0u8; need + 32 * PROC_FDINFO_SIZE];
    // SAFETY: a buffer of the size given.
    let n = unsafe {
        proc_pidinfo(
            pid,
            PROC_PIDLISTFDS,
            0,
            buf.as_mut_ptr().cast(),
            c_int::try_from(buf.len()).ok()?,
        )
    };
    u32::try_from(usize::try_from(n).ok()? / PROC_FDINFO_SIZE).ok()
}

/// User names by ID.
#[derive(Default)]
struct Users(Mutex<FxHashMap<u32, Option<String>>>);

impl Users {
    fn name(&self, uid: u32) -> Option<String> {
        if let Some(name) = self
            .0
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .get(&uid)
        {
            return name.clone();
        }
        let name = user_name(uid);
        self.0
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .insert(uid, name.clone());
        name
    }
}

fn user_name(uid: u32) -> Option<String> {
    let mut buf = vec![0u8; 16 << 10];
    // SAFETY: a passwd is plain data (integers and pointers), for which all
    // zeros is a valid value.
    let mut pw: libc::passwd = unsafe { std::mem::zeroed() };
    let mut result: *mut libc::passwd = std::ptr::null_mut();
    // SAFETY: a passwd and a buffer of the size given for its strings.
    let r = unsafe {
        libc::getpwuid_r(
            uid,
            &raw mut pw,
            buf.as_mut_ptr().cast(),
            buf.len(),
            &raw mut result,
        )
    };
    if r != 0 || result.is_null() || pw.pw_name.is_null() {
        return None;
    }
    // SAFETY: getpwuid_r succeeded: pw_name is a NUL-terminated string in
    // `buf`, which is still alive.
    Some(
        unsafe { CStr::from_ptr(pw.pw_name) }
            .to_string_lossy()
            .into_owned(),
    )
}

/// macOS's collector.
pub(crate) struct Collector {
    host: u32,
    page: u64,
    argmax: usize,
    users: Users,
}

impl Collector {
    pub(crate) fn new() -> io::Result<Self> {
        // SAFETY: takes nothing; a send right to the host port.
        let host = unsafe { mach_host_self() };
        let page = sysctl_number(c"hw.pagesize").unwrap_or(16384);
        let argmax = sysctl_number(c"kern.argmax")
            .map_or(1 << 20, |n| usize::try_from(n).unwrap_or(1 << 20));
        Ok(Collector {
            host,
            page,
            argmax: argmax.min(16 << 20),
            users: Users::default(),
        })
    }

    pub(crate) fn snapshot(
        &mut self,
        details: &mut FxHashMap<Key, Arc<Details>>,
    ) -> io::Result<Snapshot> {
        let taken = Instant::now();
        let time = now_ns();
        let pids = all_pids()?;
        let (raws, system) = turbonop_par::join(
            || turbonop_par::map(&pids, 8, |&pid| read(pid)),
            || self.system(),
        );
        let raws: Vec<Raw> = raws.into_iter().flatten().collect();
        let key = |r: &Raw| Key {
            pid: r.bsd.pid,
            start: r.bsd.start_us.saturating_mul(1000),
        };
        let new: Vec<&Raw> = raws
            .iter()
            .filter(|r| !details.contains_key(&key(r)))
            .collect();
        let found = turbonop_par::map(&new, 4, |r| (key(r), self.details(&r.bsd)));
        for (k, d) in found {
            details.insert(k, Arc::new(d));
        }
        let processes = raws
            .iter()
            .map(|r| {
                let k = key(r);
                let d = details.get(&k).cloned().unwrap_or_default();
                let b = &r.bsd;
                let name = d
                    .path
                    .as_deref()
                    .map(file_name)
                    .filter(|n| !n.is_empty())
                    .map_or_else(
                        || {
                            if b.name.is_empty() {
                                b.comm.clone()
                            } else {
                                b.name.clone()
                            }
                        },
                        str::to_owned,
                    );
                let t = r.task.unwrap_or_default();
                let (user_ns, kernel_ns) = (mach_ns(t.user), mach_ns(t.system));
                Process {
                    key: k,
                    parent_pid: (b.pid != 0).then_some(b.ppid),
                    name,
                    state: state_of(b.status),
                    counters: Counters {
                        user_ns,
                        kernel_ns,
                        cpu_ns: user_ns + kernel_ns,
                        cycles: None,
                        resident: t.resident,
                        private: r.rusage.map_or(t.resident, |u| u.footprint),
                        virtual_size: t.virtual_size,
                        threads: t.threads,
                        handles: r.fds,
                        read_bytes: r.rusage.map(|u| u.disk_read),
                        write_bytes: r.rusage.map(|u| u.disk_written),
                        read_ops: None,
                        write_ops: None,
                        page_faults: u64::from(t.faults),
                        context_switches: r.task.map(|t| u64::from(t.context_switches)),
                        priority: b.nice,
                        session: Some(b.pgid),
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

    fn details(&self, b: &BsdInfo) -> Details {
        let Ok(pid) = c_int::try_from(b.pid) else {
            return Details::default();
        };
        let mut d = Details {
            uid: Some(b.uid),
            user: self.users.name(b.uid),
            elevated: Some(b.uid == 0),
            ..Details::default()
        };
        let mut path = vec![0u8; 4096];
        // SAFETY: a buffer of the size given.
        let n = unsafe { proc_pidpath(pid, path.as_mut_ptr().cast(), 4096) };
        if let Ok(n) = usize::try_from(n)
            && n > 0
        {
            d.path = Some(String::from_utf8_lossy(&path[..n.min(path.len())]).into_owned());
        }
        if let Some((exe, args)) = self.procargs(pid) {
            if d.path.is_none() && !exe.is_empty() {
                d.path = Some(exe);
            }
            if !args.is_empty() {
                d.command_line = Some(join_args(&args));
                d.args = Some(args);
            }
        }
        d
    }

    fn procargs(&self, pid: c_int) -> Option<(String, Vec<String>)> {
        let mut mib = [libc::CTL_KERN, libc::KERN_PROCARGS2, pid];
        let mut buf = vec![0u8; self.argmax];
        let mut len = buf.len();
        // SAFETY: a MIB of three, and a buffer of `len` bytes.
        let r = unsafe {
            libc::sysctl(
                mib.as_mut_ptr(),
                3,
                buf.as_mut_ptr().cast(),
                &raw mut len,
                std::ptr::null_mut(),
                0,
            )
        };
        if r != 0 {
            return None;
        }
        parse_procargs(&buf[..len.min(buf.len())])
    }

    fn system(&self) -> io::Result<System> {
        let mut ticks = [0 as c_int; 4];
        let mut count = 4u32;
        // SAFETY: host_cpu_load_info: four natural_t, `count` of them.
        let r = unsafe {
            host_statistics(
                self.host,
                HOST_CPU_LOAD_INFO,
                ticks.as_mut_ptr(),
                &raw mut count,
            )
        };
        if r != 0 {
            return Err(io::Error::other(format!("host_statistics: {r}")));
        }
        // SAFETY: takes a name; no pointers.
        let hz = u64::try_from(unsafe { libc::sysconf(libc::_SC_CLK_TCK) })
            .ok()
            .filter(|&h| h > 0)
            .unwrap_or(100);
        let tick_ns = |t: c_int| u64::from(t.cast_unsigned()) * 1_000_000_000 / hz;
        // (user, system, idle, nice)
        let total = ticks.iter().map(|&t| tick_ns(t)).sum();
        let mut vm = [0 as c_int; HOST_VM_INFO64_COUNT as usize];
        let mut count = HOST_VM_INFO64_COUNT;
        // SAFETY: vm_statistics64: `count` integer_t.
        let r = unsafe {
            host_statistics64(self.host, HOST_VM_INFO64, vm.as_mut_ptr(), &raw mut count)
        };
        let vm: Vec<u8> = vm.iter().flat_map(|v| v.to_ne_bytes()).collect();
        let available = if r == 0 {
            parse_vm_stats(&vm).map_or(0, |s| s.available() * self.page)
        } else {
            0
        };
        let swap = sysctl_bytes(c"vm.swapusage", 32).filter(|b| b.len() >= 24);
        let word =
            |b: &[u8], at: usize| u64::from_ne_bytes(b[at..at + 8].try_into().unwrap_or_default());
        let boot = sysctl_bytes(c"kern.boottime", 16)
            .filter(|b| b.len() >= 12)
            .map_or(0, |b| {
                let sec = u64::from_ne_bytes(b[..8].try_into().unwrap_or_default());
                let usec = u64::from(u32::from_ne_bytes(b[8..12].try_into().unwrap_or_default()));
                sec * 1_000_000_000 + usec * 1000
            });
        Ok(System {
            cpus: u32::try_from(sysctl_number(c"hw.logicalcpu").unwrap_or(1))
                .unwrap_or(1)
                .max(1),
            cpu_total_ns: total,
            cpu_idle_ns: tick_ns(ticks[2]),
            idle_cycles: None,
            memory_total: sysctl_number(c"hw.memsize").unwrap_or(0),
            memory_available: available,
            commit: None,
            commit_limit: None,
            swap_total: swap.as_ref().map(|b| word(b, 0)),
            swap_used: swap.as_ref().map(|b| word(b, 16)),
            uptime_ns: now_ns().saturating_sub(boot),
        })
    }
}

/// Every process's ID.
fn all_pids() -> io::Result<Vec<c_int>> {
    // SAFETY: no buffer: how many there are.
    let n = unsafe { proc_listallpids(std::ptr::null_mut(), 0) };
    let n = usize::try_from(n).map_err(|_| io::Error::last_os_error())?;
    let mut pids = vec![0 as c_int; n + 64];
    let size =
        c_int::try_from(pids.len() * 4).map_err(|_| io::Error::other("too many processes"))?;
    // SAFETY: a buffer of the size given.
    let got = unsafe { proc_listallpids(pids.as_mut_ptr().cast(), size) };
    let got = usize::try_from(got).map_err(|_| io::Error::last_os_error())?;
    pids.truncate(got.min(pids.len()));
    Ok(pids)
}

/// Sends `signal` to the process `key`, once it is checked to be the
/// process with its ID.
pub(crate) fn signal(key: Key, signal: c_int) -> io::Result<()> {
    let pid = c_int::try_from(key.pid).map_err(|_| gone(key))?;
    if bsdinfo(pid).map(|b| b.start_us.saturating_mul(1000)) != Some(key.start) {
        return Err(gone(key));
    }
    // SAFETY: kill takes a process ID and a signal number.
    if unsafe { libc::kill(pid, signal) } == 0 {
        Ok(())
    } else {
        Err(io::Error::last_os_error())
    }
}
