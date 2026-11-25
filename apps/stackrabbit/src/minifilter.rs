//! Minifilter delays: the trace's file system minifilter callbacks (FileIo
//! `PreOpCompletion` and `PostOpCompletion`, and the failures), each with
//! its filter (the module its routine is in), whether it ran before or
//! after the operation, the operation's major function, its thread and
//! process, its file, and how long it took; read from the trace file and
//! kept; then totals grouped as asked, a histogram of their durations, and
//! the longest.
//!
//! A completion is logged when its callback returns: its timestamp is its
//! end, and its `InitialTime` its start. A failure has no start of its own:
//! it's paired with its callback's `PreOpInit` or `PostOpInit` (by its
//! routine and callback data), if the trace has it (xperf's `FLT_IO_INIT`).
//! Fast I/O callbacks are logged as completions only.

use std::path::Path;

use rustc_hash::FxHashMap;
use serde_json::{Value, json};
use turbonop_etw::guids;
use turbonop_etw::schema::{Properties, pointer_size};
use turbonop_trace::{ProcessIx, ThreadIx, Trace};
use turbonop_trace_analysis::{Resolver, Symbolizer};

use crate::reading::int;

/// The callback events (FileIo opcodes, version 3).
const PRE_INIT: u8 = 96;
const POST_INIT: u8 = 97;
const PRE_DONE: u8 = 98;
const POST_DONE: u8 = 99;
const PRE_FAIL: u8 = 100;
const POST_FAIL: u8 = 101;
/// The name events: Name, FileCreate, FileDelete, FileRundown; and
/// Create, whose `OpenPath` names its file object.
const NAMES: [u8; 4] = [0, 32, 35, 36];
const CREATE: u8 = 64;

/// When a callback ran.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Callback {
    /// Before the operation went on down the stack.
    Pre,
    /// After it completed.
    Post,
}

impl Callback {
    fn name(self) -> &'static str {
        match self {
            Self::Pre => "Pre-operation",
            Self::Post => "Post-operation",
        }
    }
}

/// One callback's run.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Call {
    /// Before or after the operation.
    pub callback: Callback,
    /// When it was called (clock ticks; for a failure not paired with its
    /// call, when it failed).
    pub start: i64,
    /// When it returned (or failed).
    pub end: i64,
    /// Its routine's address.
    pub routine: u64,
    /// The operation's major function (`IRP_MJ_...`; the filter
    /// manager's own, as fast I/O's and section synchronization's, below
    /// zero as a byte).
    pub major: u8,
    /// Its status, if it failed.
    pub failed: Option<u32>,
    /// Its thread.
    pub thread: ThreadIx,
    /// That thread's process.
    pub process: ProcessIx,
    /// Its file's path, if known.
    pub file: Option<u32>,
}

/// A trace's minifilter callbacks, in start order, and their files' paths.
#[derive(Clone, Debug, Default)]
pub struct FilterTable {
    /// The callbacks.
    pub calls: Vec<Call>,
    /// The paths callbacks name (by index).
    pub files: Vec<String>,
}

/// One event read.
enum Read {
    Init {
        time: i64,
        callback: Callback,
        routine: u64,
        data: u64,
        thread_id: u32,
    },
    Done {
        time: i64,
        start: i64,
        callback: Callback,
        routine: u64,
        data: u64,
        file_object: u64,
        file_context: u64,
        major: u8,
        thread_id: u32,
        failed: Option<u32>,
    },
    Name {
        time: i64,
        file: u64,
        name: String,
    },
}

fn time_of(r: &Read) -> i64 {
    match r {
        Read::Init { time, .. } | Read::Done { time, .. } | Read::Name { time, .. } => *time,
    }
}

/// The minifilter callbacks of `trace` (read from `path`): its FileIo
/// minifilter and name events, from the buffers holding them, on every
/// core.
pub fn table(trace: &Trace, path: &Path) -> Result<FilterTable, String> {
    let wanted = |provider: &turbonop_etl::Guid, op: u8| {
        *provider == guids::FILE_IO
            && ((PRE_INIT..=POST_FAIL).contains(&op) || NAMES.contains(&op) || op == CREATE)
    };
    let (all, _) = crate::reading::events(trace, path, wanted, |_, e, mine| {
        let op = e.descriptor.opcode;
        let Some(Ok(p)) = Properties::of(e) else {
            return;
        };
        let text = |name: &str| {
            p.get(name)
                .map(|n| turbonop_etw::values::format(n, pointer_size(e)))
                .filter(|s| !s.is_empty())
        };
        let callback = if matches!(op, PRE_INIT | PRE_DONE | PRE_FAIL) {
            Callback::Pre
        } else {
            Callback::Post
        };
        match op {
            PRE_INIT | POST_INIT => mine.push(Read::Init {
                time: e.timestamp,
                callback,
                routine: int(p.get("RoutineAddr")),
                data: int(p.get("CallbackDataPtr")),
                thread_id: e.thread_id,
            }),
            PRE_DONE | POST_DONE | PRE_FAIL | POST_FAIL => {
                let failed = matches!(op, PRE_FAIL | POST_FAIL)
                    .then(|| u32::try_from(int(p.get("Status"))).unwrap_or(u32::MAX));
                let start = match p.get("InitialTime") {
                    // (Never after its end.)
                    Some(t) => i64::try_from(int(Some(t)))
                        .unwrap_or(e.timestamp)
                        .min(e.timestamp),
                    None => e.timestamp,
                };
                mine.push(Read::Done {
                    time: e.timestamp,
                    start,
                    callback,
                    routine: int(p.get("RoutineAddr")),
                    data: int(p.get("CallbackDataPtr")),
                    file_object: int(p.get("FileObject")),
                    file_context: int(p.get("FileContext")),
                    // (The major function is a byte, widened.)
                    major: (int(p.get("MajorFunction")) & 0xff) as u8,
                    thread_id: e.thread_id,
                    failed,
                });
            }
            CREATE => {
                if let Some(name) = text("OpenPath") {
                    mine.push(Read::Name {
                        time: e.timestamp,
                        file: int(p.get("FileObject")),
                        name,
                    });
                }
            }
            _ => {
                if let Some(name) = text("FileName") {
                    mine.push(Read::Name {
                        time: e.timestamp,
                        file: int(p.get("FileObject")),
                        name,
                    });
                }
            }
        }
    })?;
    Ok(assemble(trace, all))
}

/// The callbacks, in start order, each failure paired with its call,
/// named by its file.
fn assemble(trace: &Trace, mut all: Vec<Read>) -> FilterTable {
    all.sort_by_key(time_of);
    let mut names = crate::reading::Names::default();
    for r in &all {
        if let Read::Name { time, file, name } = r {
            names.add(*file, *time, name.clone());
        }
    }
    names.finish();
    let threads = crate::reading::Threads::new(trace);
    let mut files = crate::reading::Interner::default();
    let mut calls = Vec::new();
    // Callbacks called and not yet returned: when, and on which thread,
    // by routine, callback data, and whether before or after.
    let mut open: FxHashMap<(u64, u64, Callback), (i64, u32)> = FxHashMap::default();
    for r in all {
        match r {
            Read::Init {
                time,
                callback,
                routine,
                data,
                thread_id,
            } => {
                open.insert((routine, data, callback), (time, thread_id));
            }
            Read::Done {
                time,
                start,
                callback,
                routine,
                data,
                file_object,
                file_context,
                major,
                thread_id,
                failed,
            } => {
                let called = open.remove(&(routine, data, callback));
                // (A failure's start is its call's.)
                let start = match (failed, called) {
                    (Some(_), Some((t, _))) => t,
                    _ => start,
                };
                // (A completion's header names no thread; its call's does.)
                let thread_id = match called {
                    Some((_, tid)) if thread_id == u32::MAX || thread_id == 0 => tid,
                    _ => thread_id,
                };
                let thread = threads.at(thread_id, time);
                calls.push(Call {
                    callback,
                    start,
                    end: time,
                    routine,
                    major,
                    failed,
                    thread,
                    process: trace.thread_process(thread),
                    // (Name events key a file by its file system context,
                    // as other FileIo events do by their FileKey; a create
                    // names its file object.)
                    file: names
                        .at(file_context, time)
                        .or_else(|| names.at(file_object, time))
                        .map(|n| files.intern(n)),
                });
            }
            Read::Name { .. } => {}
        }
    }
    calls.sort_by_key(|c| (c.start, c.end, c.routine));
    FilterTable {
        calls,
        files: files.strings,
    }
}

/// The name of major function `major` (`IRP_MJ_...`, without the prefix:
/// the I/O manager's, and the filter manager's own below zero).
pub fn major_name(major: u8) -> String {
    const IRP: [&str; 28] = [
        "CREATE",
        "CREATE_NAMED_PIPE",
        "CLOSE",
        "READ",
        "WRITE",
        "QUERY_INFORMATION",
        "SET_INFORMATION",
        "QUERY_EA",
        "SET_EA",
        "FLUSH_BUFFERS",
        "QUERY_VOLUME_INFORMATION",
        "SET_VOLUME_INFORMATION",
        "DIRECTORY_CONTROL",
        "FILE_SYSTEM_CONTROL",
        "DEVICE_CONTROL",
        "INTERNAL_DEVICE_CONTROL",
        "SHUTDOWN",
        "LOCK_CONTROL",
        "CLEANUP",
        "CREATE_MAILSLOT",
        "QUERY_SECURITY",
        "SET_SECURITY",
        "POWER",
        "SYSTEM_CONTROL",
        "DEVICE_CHANGE",
        "QUERY_QUOTA",
        "SET_QUOTA",
        "PNP",
    ];
    // fltKernel.h's, from -1 down.
    const FLT: [&str; 20] = [
        "ACQUIRE_FOR_SECTION_SYNCHRONIZATION",
        "RELEASE_FOR_SECTION_SYNCHRONIZATION",
        "ACQUIRE_FOR_MOD_WRITE",
        "RELEASE_FOR_MOD_WRITE",
        "ACQUIRE_FOR_CC_FLUSH",
        "RELEASE_FOR_CC_FLUSH",
        "QUERY_OPEN",
        "",
        "",
        "",
        "",
        "",
        "FAST_IO_CHECK_IF_POSSIBLE",
        "NETWORK_QUERY_OPEN",
        "MDL_READ",
        "MDL_READ_COMPLETE",
        "PREPARE_MDL_WRITE",
        "MDL_WRITE_COMPLETE",
        "VOLUME_MOUNT",
        "VOLUME_DISMOUNT",
    ];
    let name = match IRP.get(usize::from(major)) {
        Some(n) => Some(*n),
        None => FLT
            .get(usize::from(255 - major))
            .copied()
            .filter(|n| !n.is_empty()),
    };
    match name {
        Some(n) => format!("IRP_MJ_{n}"),
        None => format!("Major function {major}"),
    }
}

/// The columns a filter may name (WPA's names, as its other tables').
pub const COLUMNS: &[&str] = &[
    "Process Name",
    "Process",
    "Process ID",
    "Thread ID",
    "Thread Name",
    "Minifilter",
    "Function",
    "Major Function",
    "Callback",
    "File Name",
    "Failed",
];

/// What callbacks are grouped by.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum GroupBy {
    /// Their routine's module: the minifilter.
    Filter,
    /// Their routine.
    Function,
    /// The operation's major function.
    Major,
    /// Their process.
    Process,
    /// Their file.
    File,
}

/// The upper bounds (nanoseconds) of the duration histogram's buckets;
/// the last is unbounded.
const BUCKETS: [u64; 6] = [1_000, 10_000, 100_000, 1_000_000, 10_000_000, u64::MAX];

/// `table`'s callbacks started in `time` (clock ticks; all, if none):
/// totals by `group` (count, failures, total and longest duration; the most
/// time first), how many of each callback took how long, and the `limit`
/// longest; their routines named by `symbols` where it can.
pub fn summary(
    trace: &Trace,
    table: &FilterTable,
    resolver: &Resolver,
    symbols: &dyn Symbolizer,
    group: GroupBy,
    time: Option<(i64, i64)>,
    ask: &crate::reading::Ask,
) -> Value {
    let (limit, group_limit) = (ask.items, ask.groups);
    let (start, end) = time.unwrap_or((i64::MIN, i64::MAX));
    let from = table.calls.partition_point(|c| c.start < start);
    let to = table.calls.partition_point(|c| c.start < end);
    let calls = &table.calls[from..to];
    let ns = |c: &Call| {
        u64::try_from(trace.clock.nanos(c.end) - trace.clock.nanos(c.start)).unwrap_or(0)
    };
    // Each routine's module and function, once.
    let mut names: FxHashMap<u64, (String, String)> = FxHashMap::default();
    let mut name = |routine: u64| -> (String, String) {
        names
            .entry(routine)
            .or_insert_with(|| match resolver.image(ProcessIx::NONE, routine) {
                None => ("Unknown module".to_string(), format!("{routine:#x}")),
                Some(image) => {
                    let i = &trace.images[image.index()];
                    let file = i.file_name().to_string();
                    let offset = routine - i.base;
                    let function = match symbols.function(trace, image, offset) {
                        Some((start, f)) if start == offset => format!("{file}!{f}"),
                        Some((start, f)) => format!("{file}!{f}+{:#x}", offset - start),
                        None => format!("{file}+{offset:#x}"),
                    };
                    (file, function)
                }
            })
            .clone()
    };
    let file_of = |c: &Call| c.file.map(|f| table.files[f as usize].as_str());
    let process_label = |c: &Call| {
        if c.process.is_some() {
            let p = &trace.processes[c.process.index()];
            format!("{} ({})", p.name, p.pid)
        } else {
            "Unknown process".to_string()
        }
    };
    // (Only the callbacks the filter keeps, if there is one.)
    let kept: Vec<Call>;
    let calls: &[Call] = match &ask.filter {
        None => calls,
        Some(q) => {
            use crate::reading::{number, process_cell, text, thread_cell};
            kept = calls
                .iter()
                .filter(|c| {
                    let (module, function) = name(c.routine);
                    q.holds(&|col: &str| {
                        if let Some(v) = process_cell(trace, c.process, col) {
                            return v;
                        }
                        if let Some(v) = thread_cell(trace, c.thread, col) {
                            return v;
                        }
                        match col.to_ascii_lowercase().as_str() {
                            "minifilter" => text(&module),
                            "function" => text(&function),
                            "major function" => text(&major_name(c.major)),
                            "callback" => text(c.callback.name()),
                            "file name" => file_of(c).and_then(text),
                            "failed" => number(u64::from(c.failed.is_some())),
                            _ => None,
                        }
                    })
                })
                .copied()
                .collect();
            &kept
        }
    };
    let mut groups: FxHashMap<String, (u64, u64, u64, u64)> = FxHashMap::default();
    let mut histogram: FxHashMap<Callback, [u64; BUCKETS.len()]> = FxHashMap::default();
    let mut total = 0u64;
    let mut failed = 0u64;
    for c in calls {
        let d = ns(c);
        total += d;
        failed += u64::from(c.failed.is_some());
        let label = match group {
            GroupBy::Filter => name(c.routine).0,
            GroupBy::Function => name(c.routine).1,
            GroupBy::Major => major_name(c.major),
            GroupBy::Process => process_label(c),
            GroupBy::File => file_of(c).unwrap_or("Unknown file").to_string(),
        };
        let g = groups.entry(label).or_default();
        g.0 += 1;
        g.1 += u64::from(c.failed.is_some());
        g.2 += d;
        g.3 = g.3.max(d);
        let b = BUCKETS
            .iter()
            .position(|&b| d < b)
            .unwrap_or(BUCKETS.len() - 1);
        histogram.entry(c.callback).or_default()[b] += 1;
    }
    let mut groups: Vec<(String, (u64, u64, u64, u64))> = groups.into_iter().collect();
    groups.sort_by(|a, b| {
        b.1.2
            .cmp(&a.1.2)
            .then(b.1.0.cmp(&a.1.0))
            .then_with(|| a.0.cmp(&b.0))
    });
    let mut histogram: Vec<(Callback, [u64; BUCKETS.len()])> = histogram.into_iter().collect();
    histogram.sort_by_key(|(t, _)| *t);
    let groups_total = groups.len();
    groups.truncate(group_limit);
    let mut longest: Vec<&Call> = calls.iter().collect();
    longest.sort_by(|a, b| ns(b).cmp(&ns(a)).then(a.start.cmp(&b.start)));
    longest.truncate(limit);
    json!({
        "count": calls.len(),
        "failed": failed,
        "durationNs": total,
        "groupsTotal": groups_total,
        "groups": groups.iter().map(|(label, (n, f, d, max))| json!({
            "label": label, "count": n, "failed": f, "durationNs": d, "maxNs": max,
        })).collect::<Vec<_>>(),
        "bucketsNs": BUCKETS[..BUCKETS.len() - 1],
        "histogram": histogram.iter().map(|(t, counts)| json!({
            "type": t.name(), "counts": counts,
        })).collect::<Vec<_>>(),
        "calls": longest.iter().map(|c| {
            let (module, function) = name(c.routine);
            json!({
                "callback": c.callback.name(),
                "filter": module,
                "function": function,
                "major": major_name(c.major),
                "process": c.process.is_some().then(|| trace.processes[c.process.index()].name.clone()),
                "tid": c.thread.is_some().then(|| trace.threads[c.thread.index()].tid),
                "file": file_of(c),
                "startNs": trace.clock.nanos(c.start),
                "durationNs": ns(c),
                "status": c.failed.map(|s| format!("{s:#010x}")),
            })
        }).collect::<Vec<_>>(),
    })
}
