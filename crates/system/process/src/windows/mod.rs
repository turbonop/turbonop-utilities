//! Windows: what `NtQuerySystemInformation` and process tokens say,
//! parsed. (Built and tested on every platform; the collector that asks
//! is [`sys`], Windows' alone.)
//!
//! The structures are a 64-bit process's: Windows on x64 and arm64.
//! (Windows on 32-bit processors isn't supported.)

#[cfg(windows)]
pub(crate) mod sys;
#[cfg(windows)]
pub(crate) use sys::Collector;

use crate::bytes::{i32_at, i64_at, u16_at, u32_at, u64_at, utf16};

/// `SYSTEM_PROCESS_INFORMATION`'s size, before its threads.
pub(crate) const PROCESS_SIZE: usize = 256;
/// `SYSTEM_THREAD_INFORMATION`'s size.
pub(crate) const THREAD_SIZE: usize = 80;

/// A process, as `SystemProcessInformation` describes it.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct Raw {
    pub pid: u32,
    pub ppid: u32,
    /// Its image's file name (empty for the Idle process).
    pub name: String,
    /// When it started (a `FILETIME`: 100 ns since 1601).
    pub create: i64,
    /// CPU time, in 100 ns.
    pub user: i64,
    pub kernel: i64,
    pub cycles: u64,
    pub threads: u32,
    pub handles: u32,
    pub session: u32,
    pub base_priority: i32,
    pub working_set: u64,
    /// Private bytes (`PrivatePageCount`, which counts bytes).
    pub private: u64,
    pub virtual_size: u64,
    pub page_faults: u32,
    pub read_ops: u64,
    pub write_ops: u64,
    pub read_bytes: u64,
    pub write_bytes: u64,
    /// Every thread's context switches.
    pub context_switches: u64,
    /// Every thread is suspended.
    pub suspended: bool,
}

/// `KTHREAD_STATE`'s `Waiting`, and `KWAIT_REASON`'s `Suspended`.
const WAITING: u32 = 5;
const SUSPENDED: u32 = 5;

/// The processes `SystemProcessInformation` returned in `buf`, which was at
/// address `base` (its names point into it). Entries past the buffer's end,
/// or that loop back, end the list.
pub(crate) fn parse_processes(buf: &[u8], base: u64) -> Vec<Raw> {
    let mut out = Vec::with_capacity(buf.len() / 1024);
    let mut at = 0usize;
    while let Some(e) = buf.get(at..).filter(|e| e.len() >= PROCESS_SIZE) {
        let next = u32_at(e, 0).unwrap_or(0) as usize;
        let threads = u32_at(e, 4).unwrap_or(0);
        let name = (|| {
            let len = usize::from(u16_at(e, 56)?);
            let ptr = u64_at(e, 64)?;
            let start = usize::try_from(ptr.checked_sub(base)?).ok()?;
            buf.get(start..start.checked_add(len & !1)?).map(utf16)
        })()
        .unwrap_or_default();
        let ptr = |off| u64_at(e, off).unwrap_or(0);
        let pid = u32::try_from(ptr(80)).unwrap_or(u32::MAX);
        let ppid = u32::try_from(ptr(88)).unwrap_or(0);
        let mut context_switches = 0u64;
        let mut suspended = threads > 0;
        for t in 0..threads as usize {
            let Some(th) =
                e.get(PROCESS_SIZE + t * THREAD_SIZE..PROCESS_SIZE + (t + 1) * THREAD_SIZE)
            else {
                suspended = false;
                break;
            };
            context_switches += u64::from(u32_at(th, 64).unwrap_or(0));
            if u32_at(th, 68) != Some(WAITING) || u32_at(th, 72) != Some(SUSPENDED) {
                suspended = false;
            }
        }
        out.push(Raw {
            pid,
            ppid,
            name,
            create: i64_at(e, 32).unwrap_or(0),
            user: i64_at(e, 40).unwrap_or(0),
            kernel: i64_at(e, 48).unwrap_or(0),
            cycles: u64_at(e, 24).unwrap_or(0),
            threads,
            handles: u32_at(e, 96).unwrap_or(0),
            session: u32_at(e, 100).unwrap_or(0),
            base_priority: i32_at(e, 72).unwrap_or(0),
            working_set: ptr(144),
            private: ptr(200),
            virtual_size: ptr(120),
            page_faults: u32_at(e, 128).unwrap_or(0),
            read_ops: ptr(208),
            write_ops: ptr(216),
            read_bytes: ptr(232),
            write_bytes: ptr(240),
            context_switches,
            suspended,
        });
        if next == 0 {
            break;
        }
        match at.checked_add(next) {
            Some(n) if n > at => at = n,
            _ => break,
        }
    }
    out
}

/// 100 ns intervals between 1601 and the Unix epoch.
const EPOCH_DIFFERENCE: i64 = 116_444_736_000_000_000;

/// A `FILETIME` (100 ns since 1601) as nanoseconds since the Unix epoch;
/// 0 for 0 (the Idle and System processes have no start), or one before
/// 1970.
pub(crate) fn filetime_ns(ft: i64) -> u64 {
    if ft <= EPOCH_DIFFERENCE {
        return 0;
    }
    u64::try_from(ft - EPOCH_DIFFERENCE).map_or(0, |t| t.saturating_mul(100))
}

/// An interval in 100 ns as nanoseconds.
pub(crate) fn hundred_ns(t: i64) -> u64 {
    u64::try_from(t).unwrap_or(0).saturating_mul(100)
}

/// The length of the SID at the start of `b`, if it is one.
pub(crate) fn sid_len(b: &[u8]) -> Option<usize> {
    let revision = *b.first()?;
    let count = usize::from(*b.get(1)?);
    let len = 8 + 4 * count;
    (revision == 1 && b.len() >= len).then_some(len)
}

/// A SID as text (`S-1-5-18`).
pub(crate) fn sid_string(sid: &[u8]) -> Option<String> {
    let len = sid_len(sid)?;
    let authority = sid[2..8].iter().fold(0u64, |a, &b| (a << 8) | u64::from(b));
    let mut s = format!("S-1-{authority}");
    for i in 0..usize::from(sid[1]) {
        s.push('-');
        s.push_str(&u32_at(&sid[..len], 8 + 4 * i)?.to_string());
    }
    Some(s)
}

/// The integrity level a mandatory label's SID names: its last
/// sub-authority.
pub(crate) fn integrity_of(sid: &[u8]) -> Option<&'static str> {
    let len = sid_len(sid)?;
    let rid = u32_at(sid, len.checked_sub(4).filter(|&at| at >= 8)?)?;
    Some(match rid {
        0..0x1000 => "Untrusted",
        0x1000..0x2000 => "Low",
        0x2000..0x2100 => "Medium",
        0x2100..0x3000 => "Medium Plus",
        0x3000..0x4000 => "High",
        0x4000..0x5000 => "System",
        _ => "Protected",
    })
}

/// An `IMAGE_FILE_MACHINE_*` constant's architecture.
pub(crate) fn machine_name(machine: u16) -> Option<&'static str> {
    Some(match machine {
        0x8664 => "x64",
        0x014c => "x86",
        0xaa64 => "arm64",
        0xa641 => "arm64ec",
        0x01c4 | 0x01c2 | 0x01c0 => "arm",
        _ => return None,
    })
}

/// The SID in a structure that starts with a pointer to it (`TOKEN_USER`,
/// `TOKEN_MANDATORY_LABEL`), returned in `buf` at address `base`.
pub(crate) fn sid_in(buf: &[u8], base: u64) -> Option<&[u8]> {
    let ptr = u64_at(buf, 0)?;
    let start = usize::try_from(ptr.checked_sub(base)?).ok()?;
    let sid = buf.get(start..)?;
    Some(&sid[..sid_len(sid)?])
}

/// The string a `UNICODE_STRING` at the start of `buf` (at address `base`)
/// holds, its characters in `buf` too.
pub(crate) fn unicode_string_in(buf: &[u8], base: u64) -> Option<String> {
    let len = usize::from(u16_at(buf, 0)?);
    let ptr = u64_at(buf, 8)?;
    if len == 0 {
        return Some(String::new());
    }
    let start = usize::try_from(ptr.checked_sub(base)?).ok()?;
    buf.get(start..start.checked_add(len & !1)?).map(utf16)
}

#[cfg(test)]
mod tests;
