//! Disk usage: the trace's disk I/Os (DiskIo read, write and flush
//! completions), each with its issuing thread and process, its file (by
//! the FileIo name and create events), its I/O time and its disk service
//! time, read
//! from the trace file and kept; then totals grouped as asked, and the
//! heaviest I/Os.
//!
//! - I/O time: from the I/O's initiation (its `ReadInit`, `WriteInit` or
//!   `FlushInit` event, paired by its IRP) to its completion, as WPA
//!   gives it; without its init event (a trace without DiskIo's init
//!   events), the partition manager's response time
//!   (`HighResResponseTime`, in the performance counter's ticks), which
//!   starts a little later.
//! - Disk service time: the time the disk spent on the I/O alone: from
//!   the later of its initiation by the response time and the disk's
//!   previous completion, to its completion (as WPA computes it).

use std::path::Path;

use rustc_hash::FxHashMap;
use turbonop_etw::guids;
use turbonop_etw::schema::Properties;
use turbonop_trace::{ProcessIx, ThreadIx, Trace};

use crate::reading::int;
use serde_json::{Value, json};

/// What an I/O did.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum IoKind {
    /// A read.
    Read,
    /// A write.
    Write,
    /// A flush.
    Flush,
}

impl IoKind {
    fn name(self) -> &'static str {
        match self {
            Self::Read => "Read",
            Self::Write => "Write",
            Self::Flush => "Flush",
        }
    }
}

/// One disk I/O.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DiskIo {
    /// What it did.
    pub kind: IoKind,
    /// The disk.
    pub disk: u32,
    /// Its offset on the disk.
    pub offset: u64,
    /// Its size in bytes.
    pub size: u32,
    /// When it completed (clock ticks).
    pub complete: i64,
    /// Its I/O time, in nanoseconds.
    pub io_ns: u64,
    /// The partition manager's response time, in nanoseconds.
    pub response_ns: u64,
    /// Its disk service time, in nanoseconds.
    pub service_ns: u64,
    /// The thread that issued it.
    pub thread: ThreadIx,
    /// That thread's process.
    pub process: ProcessIx,
    /// Its file's path, if known.
    pub file: Option<u32>,
}

/// A trace's disk I/Os, in completion order, and their files' paths.
#[derive(Clone, Debug, Default)]
pub struct DiskTable {
    /// The I/Os.
    pub ios: Vec<DiskIo>,
    /// The paths I/Os name (by index).
    pub files: Vec<String>,
}

/// One event read: a disk I/O's fields, or a file object's name.
enum Read {
    Io {
        kind: IoKind,
        complete: i64,
        disk: u32,
        offset: u64,
        size: u32,
        response: u64,
        file_object: u64,
        irp: u64,
        thread_id: u32,
    },
    Init {
        time: i64,
        irp: u64,
    },
    Name {
        time: i64,
        file_object: u64,
        name: String,
    },
}

/// The disk I/Os of `trace` (read from `path`): its DiskIo and FileIo name
/// events, from the buffers holding them, on every core.
pub fn table(trace: &Trace, path: &Path) -> Result<DiskTable, String> {
    let wanted = |provider: &turbonop_etl::Guid, opcode: u8| {
        (*provider == guids::DISK_IO && matches!(opcode, 10..=15))
            || (*provider == guids::FILE_IO && matches!(opcode, 0 | 32 | 35 | 36 | 64))
    };
    let (all, info) = crate::reading::events(trace, path, wanted, |_, e, mine| {
        let op = e.descriptor.opcode;
        if !wanted(&e.provider, op) {
            return;
        }
        let Some(Ok(p)) = Properties::of(e) else {
            return;
        };
        if e.provider == guids::FILE_IO {
            // (Names, rundowns and creates: the path
            // opened.)
            let name = p
                .get("FileName")
                .or_else(|| p.get("OpenPath"))
                .map_or_else(String::new, |n| {
                    turbonop_etw::values::format(n, turbonop_etw::schema::pointer_size(e))
                });
            mine.push(Read::Name {
                time: e.timestamp,
                file_object: int(p.get("FileObject")),
                name,
            });
            return;
        }
        if matches!(op, 12 | 13 | 15) {
            mine.push(Read::Init {
                time: e.timestamp,
                irp: int(p.get("Irp")),
            });
            return;
        }
        let kind = match op {
            10 => IoKind::Read,
            11 => IoKind::Write,
            _ => IoKind::Flush,
        };
        mine.push(Read::Io {
            kind,
            complete: e.timestamp,
            disk: u32::try_from(int(p.get("DiskNumber"))).unwrap_or(u32::MAX),
            offset: int(p.get("ByteOffset")),
            size: u32::try_from(int(p.get("TransferSize"))).unwrap_or(u32::MAX),
            response: int(p.get("HighResResponseTime")),
            file_object: int(p.get("FileObject")),
            irp: int(p.get("Irp")),
            thread_id: u32::try_from(int(p.get("IssuingThreadId"))).unwrap_or(u32::MAX),
        });
    })?;
    let perf_freq = info.perf_freq;
    let mut table = assemble(trace, all, perf_freq);
    // (Paths as DOS paths, as WPA shows them.)
    let volumes = crate::reading::Volumes::of(trace, path);
    for f in &mut table.files {
        *f = volumes.dos(f);
    }
    Ok(table)
}

/// The I/Os, in completion order, with their files, threads and times.
fn assemble(trace: &Trace, all: Vec<Read>, perf_freq: u64) -> DiskTable {
    // Each file object's names over time.
    let mut names = crate::reading::Names::default();
    let mut ios = Vec::new();
    // Each IRP's inits, by time (IRPs are reused).
    let mut inits: Vec<(u64, i64)> = Vec::new();
    for r in all {
        match r {
            Read::Init { time, irp } => inits.push((irp, time)),
            Read::Name {
                time,
                file_object,
                name,
            } => {
                if !name.is_empty() {
                    names.add(file_object, time, name);
                }
            }
            io @ Read::Io { .. } => ios.push(io),
        }
    }
    names.finish();
    inits.sort_unstable();
    // Each IRP's completions, by time.
    let mut dones: Vec<(u64, i64)> = ios
        .iter()
        .filter_map(|r| match r {
            Read::Io { irp, complete, .. } => Some((*irp, *complete)),
            _ => None,
        })
        .collect();
    dones.sort_unstable();
    // The I/O's init: its IRP's last at or before its completion, and
    // after the IRP's previous completion (else the I/O's own init isn't
    // in the trace, and that one was an earlier I/O's).
    let last_before = |v: &[(u64, i64)], irp: u64, t: i64, inclusive: bool| {
        let i =
            v.partition_point(|&(r, x)| r < irp || (r == irp && (x < t || (inclusive && x == t))));
        i.checked_sub(1)
            .map(|i| v[i])
            .filter(|&(r, _)| r == irp)
            .map(|(_, x)| x)
    };
    let init_of = |irp: u64, complete: i64| {
        let init = last_before(&inits, irp, complete, true)?;
        match last_before(&dones, irp, complete, false) {
            Some(previous) if previous >= init => None,
            _ => Some(init),
        }
    };
    let mut files = crate::reading::Interner::default();
    let threads = crate::reading::Threads::new(trace);
    let mut out: Vec<DiskIo> = ios
        .into_iter()
        .filter_map(|r| {
            let Read::Io {
                kind,
                complete,
                disk,
                offset,
                size,
                response,
                file_object,
                irp,
                thread_id,
            } = r
            else {
                return None;
            };
            let response_ns =
                u64::try_from(u128::from(response) * 1_000_000_000 / u128::from(perf_freq))
                    .unwrap_or(u64::MAX);
            let io_ns = init_of(irp, complete).map_or(response_ns, |init| {
                u64::try_from(trace.clock.nanos(complete) - trace.clock.nanos(init)).unwrap_or(0)
            });
            // The file: its object's latest name by then, else its first
            // after (rundowns come at the end).
            let file = names.at(file_object, complete).map(|n| files.intern(n));
            let thread = threads.at(thread_id, complete);
            Some(DiskIo {
                kind,
                disk,
                offset,
                size,
                complete,
                io_ns,
                response_ns,
                service_ns: 0,
                thread,
                process: trace.thread_process(thread),
                file,
            })
        })
        .collect();
    out.sort_by_key(|io| (io.complete, io.disk, io.offset));
    // Service time: from the later of the I/O's initiation and the disk's
    // previous completion.
    let tick_ns = |ticks: i64| trace.clock.nanos(trace.clock.origin + ticks);
    let mut last_done: FxHashMap<u32, i64> = FxHashMap::default();
    for io in &mut out {
        let done_ns = tick_ns(io.complete - trace.clock.origin);
        let init_ns = done_ns.saturating_sub(i64::try_from(io.response_ns).unwrap_or(i64::MAX));
        let from = last_done
            .get(&io.disk)
            .map_or(init_ns, |&prev| init_ns.max(prev));
        io.service_ns = u64::try_from(done_ns - from.min(done_ns)).unwrap_or(0);
        last_done.insert(io.disk, done_ns);
    }
    DiskTable {
        ios: out,
        files: files.strings,
    }
}

/// The columns a filter may name (WPA's names).
pub const COLUMNS: &[&str] = &[
    "Process Name",
    "Process",
    "Process ID",
    "Thread ID",
    "Thread Name",
    "IO Type",
    "Path Name",
    "File Name",
    "Disk",
    "Size",
];

/// A row's cell of `column`.
fn cell(
    trace: &Trace,
    table: &DiskTable,
    row: &DiskIo,
    column: &str,
) -> Option<crate::reading::Cell> {
    use crate::reading::{number, process_cell, text, thread_cell};
    if let Some(c) =
        process_cell(trace, row.process, column).or_else(|| thread_cell(trace, row.thread, column))
    {
        return c;
    }
    match column.to_ascii_lowercase().as_str() {
        "io type" => text(row.kind.name()),
        "path name" | "file name" => row.file.and_then(|f| text(&table.files[f as usize])),
        "disk" => number(u64::from(row.disk)),
        "size" => number(u64::from(row.size)),
        _ => None,
    }
}

/// What disk I/Os are grouped by.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum GroupBy {
    /// Process.
    Process,
    /// File.
    File,
    /// Disk.
    Disk,
    /// Read, write or flush.
    Kind,
}

/// `table`'s I/Os completed in `time` (clock ticks; all, if none):
/// totals by `group` (count, bytes, I/O time, service time; heaviest
/// service time first), and the `limit` I/Os with the most service time.
pub fn summary(
    trace: &Trace,
    table: &DiskTable,
    group: GroupBy,
    time: Option<(i64, i64)>,
    ask: &crate::reading::Ask,
) -> Value {
    let (limit, group_limit) = (ask.items, ask.groups);
    let (start, end) = time.unwrap_or((i64::MIN, i64::MAX));
    let from = table.ios.partition_point(|io| io.complete < start);
    let to = table.ios.partition_point(|io| io.complete < end);
    // (Only the rows the filter keeps, if there is one.)
    let kept: Vec<DiskIo>;
    let ios: &[DiskIo] = if ask.filter.is_some() {
        kept = table.ios[from..to]
            .iter()
            .filter(|r| ask.keeps(*r, |r, c| cell(trace, table, r, c)))
            .cloned()
            .collect();
        &kept
    } else {
        &table.ios[from..to]
    };
    let process_label = |p: ProcessIx| {
        if p.is_some() {
            let pr = &trace.processes[p.index()];
            format!("{} ({})", pr.name, pr.pid)
        } else {
            "Unknown process".to_string()
        }
    };
    let file_label = |f: Option<u32>| {
        f.map_or_else(
            || "Unknown file".to_string(),
            |f| table.files[f as usize].clone(),
        )
    };
    let key = |io: &DiskIo| -> String {
        match group {
            GroupBy::Process => process_label(io.process),
            GroupBy::File => file_label(io.file),
            GroupBy::Disk => format!("Disk {}", io.disk),
            GroupBy::Kind => io.kind.name().to_string(),
        }
    };
    let mut groups: FxHashMap<String, (u64, u64, u64, u64)> = FxHashMap::default();
    for io in ios {
        let g = groups.entry(key(io)).or_default();
        g.0 += 1;
        g.1 += u64::from(io.size);
        g.2 += io.io_ns;
        g.3 += io.service_ns;
    }
    let mut groups: Vec<(String, (u64, u64, u64, u64))> = groups.into_iter().collect();
    groups.sort_by(|a, b| b.1.3.cmp(&a.1.3).then_with(|| a.0.cmp(&b.0)));
    let groups_total = groups.len();
    groups.truncate(group_limit);
    let mut heaviest: Vec<&DiskIo> = ios.iter().collect();
    heaviest.sort_by(|a, b| {
        b.service_ns
            .cmp(&a.service_ns)
            .then(a.complete.cmp(&b.complete))
    });
    heaviest.truncate(limit);
    json!({
        "count": ios.len(),
        "bytes": ios.iter().map(|io| u64::from(io.size)).sum::<u64>(),
        "groupsTotal": groups_total,
        "groups": groups.iter().map(|(label, (n, bytes, io_ns, service_ns))| json!({
            "label": label,
            "count": n,
            "bytes": bytes,
            "ioNs": io_ns,
            "serviceNs": service_ns,
        })).collect::<Vec<_>>(),
        "ios": heaviest.iter().map(|io| json!({
            "kind": io.kind.name(),
            "disk": io.disk,
            "offset": io.offset,
            "size": io.size,
            "completeNs": trace.clock.nanos(io.complete),
            "ioNs": io.io_ns,
            "serviceNs": io.service_ns,
            "process": process_label(io.process),
            "tid": io.thread.is_some().then(|| trace.threads[io.thread.index()].tid),
            "file": io.file.map(|f| table.files[f as usize].clone()),
        })).collect::<Vec<_>>(),
    })
}
