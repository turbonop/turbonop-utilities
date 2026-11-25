//! Windows' collector: `NtQuerySystemInformation` for every process and
//! its threads at once, and each new process opened once for its details.

// Every call here is to Windows (declared below), and each result's bytes
// are viewed as bytes to parse: that is all the `unsafe` there is.
#![allow(unsafe_code)]

use std::ffi::c_void;
use std::io;
use std::os::windows::io::{AsRawHandle, FromRawHandle, OwnedHandle};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Instant;

use rustc_hash::FxHashMap;

use super::{
    Raw, filetime_ns, hundred_ns, integrity_of, machine_name, parse_processes, sid_in, sid_len,
    unicode_string_in,
};
use crate::{Counters, Details, Key, Process, Snapshot, State, System, gone, now_ns};

type Handle = *mut c_void;
type NtStatus = i32;

#[repr(C)]
#[derive(Default, Clone, Copy)]
struct FileTime {
    low: u32,
    high: u32,
}

impl FileTime {
    fn get(self) -> i64 {
        i64::try_from((u64::from(self.high) << 32) | u64::from(self.low)).unwrap_or(i64::MAX)
    }
}

#[repr(C)]
#[derive(Default)]
struct MemoryStatusEx {
    length: u32,
    memory_load: u32,
    total_phys: u64,
    avail_phys: u64,
    total_page_file: u64,
    avail_page_file: u64,
    total_virtual: u64,
    avail_virtual: u64,
    avail_extended_virtual: u64,
}

#[link(name = "ntdll", kind = "raw-dylib")]
unsafe extern "system" {
    fn NtQuerySystemInformation(class: u32, info: *mut c_void, len: u32, ret: *mut u32)
    -> NtStatus;
    fn NtQuerySystemInformationEx(
        class: u32,
        input: *const c_void,
        input_len: u32,
        info: *mut c_void,
        len: u32,
        ret: *mut u32,
    ) -> NtStatus;
    fn NtQueryInformationProcess(
        process: Handle,
        class: u32,
        info: *mut c_void,
        len: u32,
        ret: *mut u32,
    ) -> NtStatus;
    fn NtSuspendProcess(process: Handle) -> NtStatus;
    fn NtResumeProcess(process: Handle) -> NtStatus;
    fn RtlNtStatusToDosError(status: NtStatus) -> u32;
}

#[link(name = "kernel32", kind = "raw-dylib")]
unsafe extern "system" {
    fn OpenProcess(access: u32, inherit: i32, pid: u32) -> Handle;
    fn QueryFullProcessImageNameW(
        process: Handle,
        flags: u32,
        name: *mut u16,
        size: *mut u32,
    ) -> i32;
    fn GetProcessTimes(
        process: Handle,
        creation: *mut FileTime,
        exit: *mut FileTime,
        kernel: *mut FileTime,
        user: *mut FileTime,
    ) -> i32;
    fn TerminateProcess(process: Handle, code: u32) -> i32;
    fn GetSystemTimes(idle: *mut FileTime, kernel: *mut FileTime, user: *mut FileTime) -> i32;
    fn GlobalMemoryStatusEx(status: *mut MemoryStatusEx) -> i32;
    fn GetActiveProcessorCount(group: u16) -> u32;
    fn GetActiveProcessorGroupCount() -> u16;
    fn GetTickCount64() -> u64;
    fn GetProcessInformation(process: Handle, class: i32, info: *mut c_void, size: u32) -> i32;
    fn IsWow64Process2(process: Handle, machine: *mut u16, native: *mut u16) -> i32;
    fn GetSystemDirectoryW(buffer: *mut u16, size: u32) -> u32;
}

#[link(name = "advapi32", kind = "raw-dylib")]
unsafe extern "system" {
    fn OpenProcessToken(process: Handle, access: u32, token: *mut Handle) -> i32;
    fn GetTokenInformation(
        token: Handle,
        class: u32,
        info: *mut c_void,
        len: u32,
        ret: *mut u32,
    ) -> i32;
    fn LookupAccountSidW(
        system: *const u16,
        sid: *const c_void,
        name: *mut u16,
        name_len: *mut u32,
        domain: *mut u16,
        domain_len: *mut u32,
        kind: *mut i32,
    ) -> i32;
}

const SYSTEM_PROCESS_INFORMATION: u32 = 5;
const SYSTEM_PROCESSOR_IDLE_CYCLE_TIME_INFORMATION: u32 = 83;
const PROCESS_COMMAND_LINE_INFORMATION: u32 = 60;
const PROCESS_MACHINE_TYPE_INFO: i32 = 9;
const STATUS_INFO_LENGTH_MISMATCH: NtStatus = 0xC000_0004_u32.cast_signed();
const STATUS_BUFFER_TOO_SMALL: NtStatus = 0xC000_0023_u32.cast_signed();
const STATUS_BUFFER_OVERFLOW: NtStatus = 0x8000_0005_u32.cast_signed();

const PROCESS_TERMINATE: u32 = 0x0001;
const PROCESS_SUSPEND_RESUME: u32 = 0x0800;
const PROCESS_QUERY_LIMITED_INFORMATION: u32 = 0x1000;
const TOKEN_QUERY: u32 = 0x0008;
const TOKEN_USER: u32 = 1;
const TOKEN_ELEVATION: u32 = 20;
const TOKEN_INTEGRITY_LEVEL: u32 = 25;
const ALL_PROCESSOR_GROUPS: u16 = 0xffff;
const ERROR_INVALID_PARAMETER: i32 = 87;

/// The SID of the Local System account (S-1-5-18).
const SYSTEM_SID: [u8; 12] = [1, 1, 0, 0, 0, 0, 0, 5, 18, 0, 0, 0];

fn status_error(status: NtStatus) -> io::Error {
    // SAFETY: converts a status code; no pointers.
    let code = unsafe { RtlNtStatusToDosError(status) };
    io::Error::from_raw_os_error(i32::try_from(code).unwrap_or(i32::MAX))
}

/// `v`'s bytes. (`u64`s, for the alignment Windows' structures need.)
fn bytes_of(v: &[u64]) -> &[u8] {
    // SAFETY: any u64's bytes are valid u8s, and the slice covers exactly
    // `v`'s memory, borrowed for as long as `v` is.
    unsafe { std::slice::from_raw_parts(v.as_ptr().cast::<u8>(), std::mem::size_of_val(v)) }
}

fn address(v: &[u64]) -> u64 {
    v.as_ptr() as u64
}

/// The bytes an `NtQuery*`-style call fills in (`call(buffer, its length,
/// the length needed)`), the buffer grown until they fit.
fn query(
    buf: &mut Vec<u64>,
    mut call: impl FnMut(*mut c_void, u32, *mut u32) -> NtStatus,
) -> io::Result<usize> {
    loop {
        let len = u32::try_from(buf.len() * 8).unwrap_or(u32::MAX);
        let mut needed = 0u32;
        let status = call(buf.as_mut_ptr().cast(), len, &raw mut needed);
        if status >= 0 && status != STATUS_BUFFER_OVERFLOW {
            return Ok(usize::try_from(needed).map_or(len as usize, |n| n.min(len as usize)));
        }
        if status != STATUS_INFO_LENGTH_MISMATCH
            && status != STATUS_BUFFER_TOO_SMALL
            && status != STATUS_BUFFER_OVERFLOW
        {
            return Err(status_error(status));
        }
        // (With room for the processes that start before the next call.)
        let want = (needed as usize).max(buf.len() * 8 * 2) + (64 << 10);
        if want > (1 << 30) {
            return Err(io::Error::other("the process list doesn't fit in 1 GiB"));
        }
        buf.resize(want.div_ceil(8), 0);
    }
}

/// What a process's token says: its user's SID, whether it is elevated,
/// and its integrity level.
type Token = (Option<Vec<u8>>, Option<bool>, Option<&'static str>);

/// A process, opened.
struct Opened(OwnedHandle);

impl Opened {
    /// The process `pid`, opened with `access`.
    fn open(pid: u32, access: u32) -> io::Result<Self> {
        // SAFETY: OpenProcess takes no pointers and returns a new handle or
        // null.
        let h = unsafe { OpenProcess(access, 0, pid) };
        if h.is_null() {
            return Err(io::Error::last_os_error());
        }
        // SAFETY: the handle is new, and this is its only owner.
        Ok(Opened(unsafe { OwnedHandle::from_raw_handle(h) }))
    }

    /// The process `key`, opened with `access` (and to query it): an error
    /// if it has exited, even if its ID is another process's now.
    fn open_key(key: Key, access: u32) -> io::Result<Self> {
        let p = Self::open(key.pid, access | PROCESS_QUERY_LIMITED_INFORMATION).map_err(|e| {
            if e.raw_os_error() == Some(ERROR_INVALID_PARAMETER) {
                gone(key)
            } else {
                e
            }
        })?;
        if p.times()? != (key.start, false) {
            return Err(gone(key));
        }
        Ok(p)
    }

    fn raw(&self) -> Handle {
        self.0.as_raw_handle()
    }

    /// When it started, in nanoseconds since the Unix epoch.
    fn created(&self) -> io::Result<u64> {
        Ok(self.times()?.0)
    }

    /// When it started and whether it has exited (a process another
    /// handle keeps open stays, exited, until that is closed).
    fn times(&self) -> io::Result<(u64, bool)> {
        let (mut c, mut e, mut k, mut u) = Default::default();
        // SAFETY: four FILETIMEs to fill in, for an open process.
        if unsafe { GetProcessTimes(self.raw(), &raw mut c, &raw mut e, &raw mut k, &raw mut u) }
            == 0
        {
            return Err(io::Error::last_os_error());
        }
        Ok((filetime_ns(c.get()), e.get() != 0))
    }

    fn image_path(&self) -> Option<String> {
        let mut buf = vec![0u16; 32 * 1024];
        let mut len = u32::try_from(buf.len()).unwrap_or(u32::MAX);
        // SAFETY: a buffer of `len` UTF-16 units; `len` is set to the
        // units written.
        if unsafe { QueryFullProcessImageNameW(self.raw(), 0, buf.as_mut_ptr(), &raw mut len) } == 0
        {
            return None;
        }
        Some(String::from_utf16_lossy(
            &buf[..(len as usize).min(buf.len())],
        ))
    }

    fn command_line(&self) -> Option<String> {
        let mut buf = vec![0u64; 512];
        let n = query(&mut buf, |info, len, ret| {
            // SAFETY: a buffer of `len` bytes, aligned for the
            // UNICODE_STRING written at its start.
            unsafe {
                NtQueryInformationProcess(
                    self.raw(),
                    PROCESS_COMMAND_LINE_INFORMATION,
                    info,
                    len,
                    ret,
                )
            }
        })
        .ok()?;
        let b = bytes_of(&buf);
        unicode_string_in(&b[..n.max(16).min(b.len())], address(&buf))
    }

    fn architecture(&self) -> Option<&'static str> {
        let mut info = [0u16; 4];
        // SAFETY: PROCESS_MACHINE_INFORMATION is 8 bytes (a USHORT machine,
        // a USHORT, a ULONG of attributes).
        let ok = unsafe {
            GetProcessInformation(
                self.raw(),
                PROCESS_MACHINE_TYPE_INFO,
                info.as_mut_ptr().cast(),
                8,
            )
        };
        if ok != 0 {
            return machine_name(info[0]);
        }
        // (Before Windows 11: IsWow64Process2, which says only whether it is
        // emulated as x86 or 32-bit Arm.)
        let (mut process, mut native) = (0u16, 0u16);
        // SAFETY: two USHORTs to fill in.
        if unsafe { IsWow64Process2(self.raw(), &raw mut process, &raw mut native) } == 0 {
            return None;
        }
        machine_name(if process == 0 { native } else { process })
    }

    /// Its token's user SID, elevation, and integrity level.
    fn token(&self) -> Option<Token> {
        let mut token: Handle = std::ptr::null_mut();
        // SAFETY: a handle to fill in.
        if unsafe { OpenProcessToken(self.raw(), TOKEN_QUERY, &raw mut token) } == 0 {
            return None;
        }
        // SAFETY: the token handle is new, and this is its only owner.
        let token = unsafe { OwnedHandle::from_raw_handle(token) };
        let info = |class: u32| -> Option<Vec<u64>> {
            let mut buf = vec![0u64; 16];
            loop {
                let len = u32::try_from(buf.len() * 8).ok()?;
                let mut needed = 0u32;
                // SAFETY: a buffer of `len` bytes, aligned for the
                // structure written in it.
                let ok = unsafe {
                    GetTokenInformation(
                        token.as_raw_handle(),
                        class,
                        buf.as_mut_ptr().cast(),
                        len,
                        &raw mut needed,
                    )
                };
                if ok != 0 {
                    return Some(buf);
                }
                if needed <= len || needed > (1 << 20) {
                    return None;
                }
                buf.resize((needed as usize).div_ceil(8), 0);
            }
        };
        let user =
            info(TOKEN_USER).and_then(|b| sid_in(bytes_of(&b), address(&b)).map(<[u8]>::to_vec));
        let elevated = info(TOKEN_ELEVATION).map(|b| b[0] & 0xffff_ffff != 0);
        let integrity = info(TOKEN_INTEGRITY_LEVEL)
            .and_then(|b| integrity_of(sid_in(bytes_of(&b), address(&b))?));
        Some((user, elevated, integrity))
    }
}

/// Account names by SID, as `LookupAccountSidW` gives them (slowly: a
/// domain account's may ask a domain controller).
#[derive(Default)]
struct Accounts(Mutex<FxHashMap<Vec<u8>, Option<String>>>);

impl Accounts {
    fn name(&self, sid: &[u8]) -> Option<String> {
        if let Some(name) = self
            .0
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .get(sid)
        {
            return name.clone();
        }
        let name = lookup_account(sid);
        self.0
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .insert(sid.to_vec(), name.clone());
        name
    }
}

fn lookup_account(sid: &[u8]) -> Option<String> {
    sid_len(sid)?;
    let (mut name, mut domain) = (vec![0u16; 256], vec![0u16; 256]);
    let (mut name_len, mut domain_len, mut kind) = (256u32, 256u32, 0i32);
    // SAFETY: a SID (checked to be whole above), and two buffers of the
    // lengths given.
    let ok = unsafe {
        LookupAccountSidW(
            std::ptr::null(),
            sid.as_ptr().cast(),
            name.as_mut_ptr(),
            &raw mut name_len,
            domain.as_mut_ptr(),
            &raw mut domain_len,
            &raw mut kind,
        )
    };
    if ok == 0 {
        return super::sid_string(sid);
    }
    let name = String::from_utf16_lossy(&name[..(name_len as usize).min(name.len())]);
    let domain = String::from_utf16_lossy(&domain[..(domain_len as usize).min(domain.len())]);
    Some(if domain.is_empty() {
        name
    } else {
        format!("{domain}\\{name}")
    })
}

/// Windows' collector.
pub(crate) struct Collector {
    /// `SystemProcessInformation`'s buffer, kept from one snapshot to the
    /// next.
    buf: Vec<u64>,
    accounts: Accounts,
    /// The kernel's image (the System process's).
    kernel: Option<String>,
}

impl Collector {
    pub(crate) fn new() -> io::Result<Self> {
        let mut dir = vec![0u16; 1024];
        // SAFETY: a buffer of the length given.
        let n = unsafe { GetSystemDirectoryW(dir.as_mut_ptr(), 1024) } as usize;
        let kernel = (n > 0 && n < dir.len())
            .then(|| format!("{}\\ntoskrnl.exe", String::from_utf16_lossy(&dir[..n])));
        Ok(Collector {
            buf: vec![0; 128 << 10],
            accounts: Accounts::default(),
            kernel,
        })
    }

    pub(crate) fn snapshot(
        &mut self,
        details: &mut FxHashMap<Key, Arc<Details>>,
    ) -> io::Result<Snapshot> {
        let taken = Instant::now();
        let time = now_ns();
        let n = query(&mut self.buf, |info, len, ret| {
            // SAFETY: a buffer of `len` bytes, aligned for the structures
            // written in it.
            unsafe { NtQuerySystemInformation(SYSTEM_PROCESS_INFORMATION, info, len, ret) }
        })?;
        let base = address(&self.buf);
        let bytes = bytes_of(&self.buf);
        let raws = parse_processes(&bytes[..n.min(bytes.len())], base);
        let system = system()?;
        let key = |r: &Raw| Key {
            pid: r.pid,
            start: filetime_ns(r.create),
        };
        let new: Vec<&Raw> = raws
            .iter()
            .filter(|r| !details.contains_key(&key(r)))
            .collect();
        let found = turbonop_par::map(&new, 2, |r| (key(r), self.details(r)));
        for (k, d) in found {
            details.insert(k, Arc::new(d));
        }
        let processes = raws
            .iter()
            .map(|r| {
                let k = key(r);
                let name = match (r.pid, r.name.is_empty()) {
                    (0, _) => "System Idle Process".to_owned(),
                    (_, true) => "System".to_owned(),
                    _ => r.name.clone(),
                };
                Process {
                    key: k,
                    parent_pid: (r.pid != 0 && r.pid != 4).then_some(r.ppid),
                    name,
                    state: if r.suspended {
                        State::Suspended
                    } else {
                        State::Sleeping
                    },
                    counters: Counters {
                        user_ns: hundred_ns(r.user),
                        kernel_ns: hundred_ns(r.kernel),
                        cpu_ns: hundred_ns(r.user) + hundred_ns(r.kernel),
                        cycles: Some(r.cycles),
                        resident: r.working_set,
                        private: r.private,
                        virtual_size: r.virtual_size,
                        threads: r.threads,
                        handles: Some(r.handles),
                        read_bytes: Some(r.read_bytes),
                        write_bytes: Some(r.write_bytes),
                        read_ops: Some(r.read_ops),
                        write_ops: Some(r.write_ops),
                        page_faults: u64::from(r.page_faults),
                        context_switches: Some(r.context_switches),
                        priority: r.base_priority,
                        session: Some(r.session),
                    },
                    details: details.get(&k).cloned().unwrap_or_default(),
                }
            })
            .collect();
        Ok(Snapshot {
            taken,
            time,
            system,
            processes,
        })
    }

    /// What doesn't change about `r`'s process, as much as it can be opened
    /// to show.
    fn details(&self, r: &Raw) -> Details {
        let mut d = Details::default();
        if r.pid == 0 || r.pid == 4 {
            // (The Idle and System processes: the kernel.)
            d.user = self.accounts.name(&SYSTEM_SID);
            d.elevated = Some(true);
            d.integrity = Some("System");
            if r.pid == 4 {
                d.path.clone_from(&self.kernel);
            }
            return d;
        }
        let Ok(p) = Opened::open(r.pid, PROCESS_QUERY_LIMITED_INFORMATION) else {
            return d;
        };
        // (Another process, if this one exited and its ID was reused.)
        if p.created().ok() != Some(filetime_ns(r.create)) {
            return d;
        }
        d.path = p.image_path();
        d.command_line = p.command_line();
        d.architecture = p.architecture();
        if let Some((sid, elevated, integrity)) = p.token() {
            d.user = sid.and_then(|sid| self.accounts.name(&sid));
            d.elevated = elevated;
            d.integrity = integrity;
        }
        d
    }
}

/// The system's totals.
fn system() -> io::Result<System> {
    let (mut idle, mut kernel, mut user) = Default::default();
    // SAFETY: three FILETIMEs to fill in.
    if unsafe { GetSystemTimes(&raw mut idle, &raw mut kernel, &raw mut user) } == 0 {
        return Err(io::Error::last_os_error());
    }
    let mut mem = MemoryStatusEx {
        length: u32::try_from(std::mem::size_of::<MemoryStatusEx>()).unwrap_or(64),
        ..Default::default()
    };
    // SAFETY: a MEMORYSTATUSEX, its length set.
    if unsafe { GlobalMemoryStatusEx(&raw mut mem) } == 0 {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: no arguments, or none that are pointers.
    let (cpus, groups, uptime) = unsafe {
        (
            GetActiveProcessorCount(ALL_PROCESSOR_GROUPS),
            GetActiveProcessorGroupCount(),
            GetTickCount64(),
        )
    };
    Ok(System {
        cpus: cpus.max(1),
        // (Kernel time includes idle time.)
        cpu_total_ns: hundred_ns(kernel.get()) + hundred_ns(user.get()),
        cpu_idle_ns: hundred_ns(idle.get()),
        idle_cycles: idle_cycles(groups),
        memory_total: mem.total_phys,
        memory_available: mem.avail_phys,
        commit: Some(mem.total_page_file.saturating_sub(mem.avail_page_file)),
        commit_limit: Some(mem.total_page_file),
        swap_total: None,
        swap_used: None,
        uptime_ns: uptime.saturating_mul(1_000_000),
    })
}

/// Every processor's idle cycles, group by group.
fn idle_cycles(groups: u16) -> Option<u64> {
    let mut total = 0u64;
    for group in 0..groups.max(1) {
        // SAFETY: takes a group number.
        let n = unsafe { GetActiveProcessorCount(group) } as usize;
        let mut buf = vec![0u64; n.max(1)];
        let len = u32::try_from(buf.len() * 8).ok()?;
        let mut ret = 0u32;
        // SAFETY: the group number as input, and a u64 for each of its
        // processors.
        let status = unsafe {
            NtQuerySystemInformationEx(
                SYSTEM_PROCESSOR_IDLE_CYCLE_TIME_INFORMATION,
                (&raw const group).cast(),
                2,
                buf.as_mut_ptr().cast(),
                len,
                &raw mut ret,
            )
        };
        if status < 0 {
            return None;
        }
        total += buf.iter().take(ret as usize / 8).sum::<u64>();
    }
    Some(total)
}

/// Ends the process `key`.
pub(crate) fn terminate(key: Key) -> io::Result<()> {
    let p = Opened::open_key(key, PROCESS_TERMINATE)?;
    // SAFETY: an open process, and its exit code.
    if unsafe { TerminateProcess(p.raw(), 1) } == 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(())
}

/// Suspends (`true`) or resumes the process `key`.
pub(crate) fn suspend(key: Key, suspend: bool) -> io::Result<()> {
    let p = Opened::open_key(key, PROCESS_SUSPEND_RESUME)?;
    // SAFETY: an open process.
    let status = unsafe {
        if suspend {
            NtSuspendProcess(p.raw())
        } else {
            NtResumeProcess(p.raw())
        }
    };
    if status < 0 {
        return Err(status_error(status));
    }
    Ok(())
}
