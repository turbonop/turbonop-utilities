//! macOS: what `libproc` and `sysctl` say, parsed. (Built and tested on
//! every platform; the collector that asks is [`sys`], macOS's alone.)

#[cfg(target_os = "macos")]
pub(crate) mod sys;
#[cfg(target_os = "macos")]
pub(crate) use sys::Collector;

use crate::State;
use crate::bytes::{c_str, i32_at, u32_at, u64_at};

/// `struct proc_bsdinfo`'s size.
pub(crate) const BSDINFO_SIZE: usize = 136;
/// `struct proc_taskinfo`'s size.
pub(crate) const TASKINFO_SIZE: usize = 96;
/// `struct rusage_info_v2`'s size.
pub(crate) const RUSAGE_V2_SIZE: usize = 160;

/// `proc_bsdinfo`'s fields that are read.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct BsdInfo {
    pub status: u32,
    pub pid: u32,
    pub ppid: u32,
    pub uid: u32,
    pub ruid: u32,
    /// Its name (`p_comm`, at most 16 bytes).
    pub comm: String,
    /// Its longer name (`p_name`, at most 32 bytes), if set.
    pub name: String,
    pub pgid: u32,
    pub nice: i32,
    /// When it started, in microseconds since the Unix epoch.
    pub start_us: u64,
    /// `PROC_FLAG_*`.
    pub flags: u32,
}

/// A `proc_bsdinfo`.
pub(crate) fn parse_bsdinfo(b: &[u8]) -> Option<BsdInfo> {
    if b.len() < BSDINFO_SIZE {
        return None;
    }
    Some(BsdInfo {
        flags: u32_at(b, 0)?,
        status: u32_at(b, 4)?,
        pid: u32_at(b, 12)?,
        ppid: u32_at(b, 16)?,
        uid: u32_at(b, 20)?,
        ruid: u32_at(b, 28)?,
        comm: c_str(&b[48..64]),
        name: c_str(&b[64..96]),
        pgid: u32_at(b, 100)?,
        nice: i32_at(b, 116)?,
        start_us: u64_at(b, 120)?
            .saturating_mul(1_000_000)
            .saturating_add(u64_at(b, 128)?),
    })
}

/// `proc_taskinfo`'s fields that are read. Times are in Mach absolute time
/// units (see `mach_timebase_info`).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct TaskInfo {
    pub virtual_size: u64,
    pub resident: u64,
    pub user: u64,
    pub system: u64,
    pub faults: u32,
    pub pageins: u32,
    pub context_switches: u32,
    pub threads: u32,
    pub running: u32,
    pub priority: i32,
}

/// A `proc_taskinfo`.
pub(crate) fn parse_taskinfo(b: &[u8]) -> Option<TaskInfo> {
    if b.len() < TASKINFO_SIZE {
        return None;
    }
    let count = |at| u32::try_from(i32_at(b, at).unwrap_or(0)).unwrap_or(0);
    Some(TaskInfo {
        virtual_size: u64_at(b, 0)?,
        resident: u64_at(b, 8)?,
        user: u64_at(b, 16)?,
        system: u64_at(b, 24)?,
        faults: count(52),
        pageins: count(56),
        context_switches: count(80),
        threads: count(84),
        running: count(88),
        priority: i32_at(b, 92)?,
    })
}

/// `rusage_info_v2`'s fields that are read.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct Rusage {
    /// Its memory footprint, as Activity Monitor's Memory column shows.
    pub footprint: u64,
    pub disk_read: u64,
    pub disk_written: u64,
}

/// A `rusage_info_v2`.
pub(crate) fn parse_rusage(b: &[u8]) -> Option<Rusage> {
    if b.len() < RUSAGE_V2_SIZE {
        return None;
    }
    Some(Rusage {
        footprint: u64_at(b, 72)?,
        disk_read: u64_at(b, 144)?,
        disk_written: u64_at(b, 152)?,
    })
}

/// A `p_stat` value's meaning.
pub(crate) fn state_of(status: u32) -> State {
    match status {
        1 => State::Starting,
        2 => State::Running,
        4 => State::Stopped,
        5 => State::Zombie,
        _ => State::Sleeping,
    }
}

/// `KERN_PROCARGS2`'s buffer: `argc`, the executable's path, padding
/// NULs, then the arguments (then the environment). Its executable and
/// arguments.
pub(crate) fn parse_procargs(b: &[u8]) -> Option<(String, Vec<String>)> {
    let argc = usize::try_from(i32_at(b, 0)?).ok()?;
    let rest = b.get(4..)?;
    let end = rest.iter().position(|&c| c == 0)?;
    let exe = String::from_utf8_lossy(&rest[..end]).into_owned();
    let mut at = end;
    while rest.get(at) == Some(&0) {
        at += 1;
    }
    let mut args = Vec::with_capacity(argc.min(4096));
    for arg in rest[at..].split(|&c| c == 0).take(argc) {
        args.push(String::from_utf8_lossy(arg).into_owned());
    }
    Some((exe, args))
}

/// `vm_statistics64`'s counts of pages that are read.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct VmStats {
    pub free: u64,
    pub active: u64,
    pub inactive: u64,
    pub wired: u64,
    pub speculative: u64,
    pub purgeable: u64,
    pub compressor: u64,
    pub external: u64,
}

/// `host_statistics64(HOST_VM_INFO64)`'s `vm_statistics64`.
pub(crate) fn parse_vm_stats(b: &[u8]) -> Option<VmStats> {
    let count = |at| u32_at(b, at).map(u64::from);
    Some(VmStats {
        free: count(0)?,
        active: count(4)?,
        inactive: count(8)?,
        wired: count(12)?,
        purgeable: count(88)?,
        speculative: count(92)?,
        compressor: count(128)?,
        external: count(136)?,
    })
}

impl VmStats {
    /// Pages available to programs without paging: free (which counts
    /// the speculative pages), and what is cached of files (dropped when
    /// memory is needed) or may be purged.
    pub(crate) fn available(self) -> u64 {
        self.free + self.external + self.purgeable
    }
}

#[cfg(test)]
mod tests;
