//! .NET: the Microsoft-Windows-DotNETRuntime provider's garbage
//! collections, allocations, exceptions, JIT compilations and lock
//! contention, decoded by the descriptions WPR records; then, in a time
//! range, each process's totals, its garbage collections (with how long the
//! runtime was paused for each), exceptions by type, the methods that took
//! longest to compile, and allocations by type.
//!
//! - A garbage collection is from its `GCStart` (event 1) to its `GCEnd`
//!   (2); its pause, from the runtime's suspension (`GCSuspendEEBegin`, 9)
//!   to its restart (`GCRestartEEEnd`, 3) around it. Both are paired in
//!   order within a process's runtime instance (`ClrInstanceID`).
//! - Allocations are the runtime's samples (`GCAllocationTick`, 10): one
//!   about every 100 KB a process allocates, naming the type allocated
//!   then and the bytes since the last sample.
//! - A JIT compilation is from `MethodJittingStarted` (145) to the
//!   method's `MethodLoadVerbose` (143), by method ID.
//! - Lock contention is from `ContentionStart` (81) to `ContentionStop`
//!   (91), by thread.

use std::path::Path;

use rustc_hash::FxHashMap;
use serde_json::{Value, json};
use turbonop_etl::Guid;
use turbonop_etw::event_info::{EventInfo, EventInfos};
use turbonop_trace::{ProcessIx, Trace};

use crate::reading::int;

/// Microsoft-Windows-DotNETRuntime.
pub const DOTNET_RUNTIME: Guid = Guid::new(
    0xe13c_0d23,
    0xccbc,
    0x4e12,
    [0x93, 0x1b, 0xd9, 0xcc, 0x2e, 0xee, 0x27, 0xe4],
);

const GC_START: u16 = 1;
const GC_END: u16 = 2;
const RESTART_END: u16 = 3;
const SUSPEND_BEGIN: u16 = 9;
const ALLOCATION_TICK: u16 = 10;
const EXCEPTION: u16 = 80;
const CONTENTION_START: u16 = 81;
const CONTENTION_STOP: u16 = 91;
const METHOD_LOAD: u16 = 143;
const JITTING_STARTED: u16 = 145;

/// A garbage collection.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Gc {
    /// The process.
    pub process: ProcessIx,
    /// Its number in its runtime.
    pub number: u64,
    /// The generation collected.
    pub generation: u64,
    /// Why (the runtime's reason).
    pub reason: u64,
    /// Its type: blocking, background or foreground.
    pub kind: u64,
    /// When it started and ended (clock ticks).
    pub start: i64,
    /// When it ended.
    pub end: i64,
    /// How long the runtime was paused for it, in clock ticks, if its
    /// suspension and restart are in the trace.
    pub pause: Option<i64>,
}

/// Something timed, by process: a JIT compilation or a lock's contention.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Timed {
    /// The process.
    pub process: ProcessIx,
    /// When it started (clock ticks).
    pub start: i64,
    /// When it ended.
    pub end: i64,
    /// What (a method's name, for a compilation).
    pub what: Option<u32>,
}

/// A trace's .NET events, each list in start order, and the strings they
/// name.
#[derive(Clone, Debug, Default)]
pub struct DotnetTable {
    /// The garbage collections.
    pub gcs: Vec<Gc>,
    /// The allocation samples: when, which process, the type (a string),
    /// and the bytes since the last.
    pub allocations: Vec<(i64, ProcessIx, u32, u64)>,
    /// The exceptions: when, which process, their type (a string).
    pub exceptions: Vec<(i64, ProcessIx, u32)>,
    /// The JIT compilations.
    pub jits: Vec<Timed>,
    /// The lock contentions.
    pub contentions: Vec<Timed>,
    /// The strings (types, methods), by index.
    pub strings: Vec<String>,
}

/// One event read: when, where in the file, its process and thread, its
/// ID, and its fields that matter.
struct Read {
    time: i64,
    offset: u64,
    pid: u32,
    tid: u32,
    id: u16,
    clr: u64,
    a: u64,
    b: u64,
    c: u64,
    text: Option<String>,
}

/// A garbage collection begun: when, its number, generation, reason and
/// type (high 32 bits), and when its runtime was suspended.
type OpenGc = (i64, u64, u64, u64, Option<i64>);

/// An exception type's count, and by process.
type Thrown = (u64, FxHashMap<ProcessIx, u64>);

/// The .NET events of `trace` (read from `path`), from the buffers
/// holding them, on every core.
pub fn table(trace: &Trace, path: &Path) -> Result<DotnetTable, String> {
    let mut infos = EventInfos::default();
    for b in &trace.event_infos {
        if let Ok(i) = EventInfo::parse(b) {
            infos.add(i);
        }
    }
    let wanted = |provider: &Guid, _op: u8| *provider == DOTNET_RUNTIME;
    let (mut all, _) = crate::reading::events(trace, path, wanted, |_, e, mine| {
        let id = e.descriptor.id;
        if ![
            GC_START,
            GC_END,
            RESTART_END,
            SUSPEND_BEGIN,
            ALLOCATION_TICK,
            EXCEPTION,
            CONTENTION_START,
            CONTENTION_STOP,
            METHOD_LOAD,
            JITTING_STARTED,
        ]
        .contains(&id)
        {
            return;
        }
        let Some(info) = infos.get(e) else { return };
        let d = info.decode_partial(e);
        let prop = |name: &str| d.properties.iter().find(|p| p.name == name);
        let get = |name: &str| int(prop(name));
        let text = |name: &str| {
            prop(name)
                .map(|p| turbonop_etw::values::format(p, turbonop_etw::schema::pointer_size(e)))
                .filter(|s| !s.is_empty())
        };
        let (a, b, c, t) = match id {
            GC_START => (
                get("Count"),
                get("Depth"),
                get("Reason") | get("Type") << 32,
                None,
            ),
            GC_END => (get("Count"), get("Depth"), 0, None),
            ALLOCATION_TICK => {
                let bytes = if prop("AllocationAmount64").is_some() {
                    get("AllocationAmount64")
                } else {
                    get("AllocationAmount")
                };
                (bytes, 0, 0, text("TypeName"))
            }
            EXCEPTION => (0, 0, 0, text("ExceptionType")),
            JITTING_STARTED | METHOD_LOAD => {
                let name = match (text("MethodNamespace"), text("MethodName")) {
                    (Some(ns), Some(n)) => Some(format!("{ns}.{n}")),
                    (None, Some(n)) => Some(n),
                    _ => None,
                };
                (get("MethodID"), 0, 0, name)
            }
            _ => (0, 0, 0, None),
        };
        mine.push(Read {
            time: e.timestamp,
            offset: e.file_offset,
            pid: e.process_id,
            tid: e.thread_id,
            id,
            clr: get("ClrInstanceID"),
            a,
            b,
            c,
            text: t,
        });
    })?;
    all.sort_by_key(|r| (r.time, r.offset));
    let mut out = DotnetTable::default();
    let mut strings = crate::reading::Interner::default();
    let mut process_of: FxHashMap<u32, ProcessIx> = FxHashMap::default();
    let mut process = |pid: u32, time: i64| {
        *process_of
            .entry(pid)
            .or_insert_with(|| crate::reading::process_at(trace, pid, time))
    };
    // Open: GCs and suspensions by (process, runtime); compilations by
    // (process, method); contentions by (process, thread).
    let mut gcs: FxHashMap<(u32, u64), OpenGc> = FxHashMap::default();
    let mut suspended: FxHashMap<(u32, u64), i64> = FxHashMap::default();
    // A GC's pause, once its runtime restarts: the GC's index.
    let mut pausing: FxHashMap<(u32, u64), usize> = FxHashMap::default();
    let mut jitting: FxHashMap<(u32, u64), (i64, Option<u32>)> = FxHashMap::default();
    let mut contending: FxHashMap<(u32, u32), i64> = FxHashMap::default();
    for r in all {
        let key = (r.pid, r.clr);
        match r.id {
            SUSPEND_BEGIN => {
                suspended.insert(key, r.time);
            }
            GC_START => {
                let suspend = suspended.get(&key).copied();
                gcs.insert(key, (r.time, r.a, r.b, r.c, suspend));
            }
            GC_END => {
                if let Some((start, number, generation, rk, suspend)) = gcs.remove(&key) {
                    out.gcs.push(Gc {
                        process: process(r.pid, start),
                        number,
                        generation,
                        reason: rk & 0xffff_ffff,
                        kind: rk >> 32,
                        start,
                        end: r.time,
                        pause: None,
                    });
                    if let Some(s) = suspend {
                        // (Its pause ends when the runtime restarts.)
                        pausing.insert(key, out.gcs.len() - 1);
                        suspended.insert(key, s);
                    }
                }
            }
            RESTART_END => {
                if let (Some(i), Some(s)) = (pausing.remove(&key), suspended.remove(&key)) {
                    out.gcs[i].pause = Some(r.time - s);
                } else {
                    suspended.remove(&key);
                }
            }
            ALLOCATION_TICK => {
                let t = strings.intern(r.text.as_deref().unwrap_or("Unknown type"));
                out.allocations
                    .push((r.time, process(r.pid, r.time), t, r.a));
            }
            EXCEPTION => {
                let t = strings.intern(r.text.as_deref().unwrap_or("Unknown exception"));
                out.exceptions.push((r.time, process(r.pid, r.time), t));
            }
            JITTING_STARTED => {
                let name = r.text.as_deref().map(|n| strings.intern(n));
                jitting.insert((r.pid, r.a), (r.time, name));
            }
            METHOD_LOAD => {
                if let Some((start, name)) = jitting.remove(&(r.pid, r.a)) {
                    let name = name.or_else(|| r.text.as_deref().map(|n| strings.intern(n)));
                    out.jits.push(Timed {
                        process: process(r.pid, start),
                        start,
                        end: r.time,
                        what: name,
                    });
                }
            }
            CONTENTION_START => {
                contending.insert((r.pid, r.tid), r.time);
            }
            CONTENTION_STOP => {
                if let Some(start) = contending.remove(&(r.pid, r.tid)) {
                    out.contentions.push(Timed {
                        process: process(r.pid, start),
                        start,
                        end: r.time,
                        what: None,
                    });
                }
            }
            _ => {}
        }
    }
    out.gcs.sort_by_key(|g| (g.start, g.number));
    out.jits.sort_by_key(|j| j.start);
    out.contentions.sort_by_key(|c| c.start);
    out.strings = strings.strings;
    Ok(out)
}

/// The runtime's name for a garbage collection's reason.
pub fn reason_name(reason: u64) -> String {
    const NAMES: [&str; 14] = [
        "AllocSmall",
        "Induced",
        "LowMemory",
        "Empty",
        "AllocLarge",
        "OutOfSpaceSOH",
        "OutOfSpaceLOH",
        "InducedNotForced",
        "Internal",
        "InducedLowMemory",
        "InducedCompacting",
        "LowMemoryHost",
        "PMFullGC",
        "LowMemoryHostBlocking",
    ];
    usize::try_from(reason)
        .ok()
        .and_then(|r| NAMES.get(r))
        .map_or_else(|| format!("Reason {reason}"), |n| (*n).to_string())
}

/// The runtime's name for a garbage collection's type.
pub fn kind_name(kind: u64) -> &'static str {
    match kind {
        0 => "Blocking",
        1 => "Background",
        2 => "Foreground",
        _ => "Other",
    }
}

/// `table`'s events in `time` (clock ticks; all, if none): each process's
/// totals (as `groups`), its garbage collections (the `limit` longest
/// pauses), exceptions by type, the `limit` longest compilations, and
/// allocations by type (the `limit` most).
pub fn summary(
    trace: &Trace,
    table: &DotnetTable,
    time: Option<(i64, i64)>,
    limit: usize,
) -> Value {
    let (start, end) = time.unwrap_or((i64::MIN, i64::MAX));
    let inside = |t: i64| t >= start && t < end;
    let ns = |ticks: i64| {
        u64::try_from(
            i128::from(ticks) * 1_000_000_000 / i128::from(trace.clock.ticks_per_second.max(1)),
        )
        .unwrap_or(0)
    };
    let label = |p: ProcessIx| {
        if p.is_some() {
            let pr = &trace.processes[p.index()];
            format!("{} ({})", pr.name, pr.pid)
        } else {
            "Unknown process".to_string()
        }
    };
    #[derive(Default)]
    struct Totals {
        gcs: u64,
        gens: [u64; 3],
        gc_ns: u64,
        pause_ns: u64,
        max_pause_ns: u64,
        allocated: u64,
        exceptions: u64,
        jits: u64,
        jit_ns: u64,
        contentions: u64,
        contention_ns: u64,
    }
    let mut per: FxHashMap<ProcessIx, Totals> = FxHashMap::default();
    let gcs: Vec<&Gc> = table.gcs.iter().filter(|g| inside(g.start)).collect();
    for g in &gcs {
        let t = per.entry(g.process).or_default();
        t.gcs += 1;
        if let Some(n) = t
            .gens
            .get_mut(usize::try_from(g.generation).unwrap_or(usize::MAX))
        {
            *n += 1;
        }
        t.gc_ns += ns(g.end - g.start);
        let pause = g.pause.map_or(0, ns);
        t.pause_ns += pause;
        t.max_pause_ns = t.max_pause_ns.max(pause);
    }
    let mut by_type: FxHashMap<u32, u64> = FxHashMap::default();
    for &(t, p, ty, bytes) in &table.allocations {
        if inside(t) {
            per.entry(p).or_default().allocated += bytes;
            *by_type.entry(ty).or_default() += bytes;
        }
    }
    let mut exceptions: FxHashMap<u32, (u64, FxHashMap<ProcessIx, u64>)> = FxHashMap::default();
    for &(t, p, ty) in &table.exceptions {
        if inside(t) {
            per.entry(p).or_default().exceptions += 1;
            let e = exceptions.entry(ty).or_default();
            e.0 += 1;
            *e.1.entry(p).or_default() += 1;
        }
    }
    let jits: Vec<&Timed> = table.jits.iter().filter(|j| inside(j.start)).collect();
    for j in &jits {
        let t = per.entry(j.process).or_default();
        t.jits += 1;
        t.jit_ns += ns(j.end - j.start);
    }
    for c in table.contentions.iter().filter(|c| inside(c.start)) {
        let t = per.entry(c.process).or_default();
        t.contentions += 1;
        t.contention_ns += ns(c.end - c.start);
    }
    let mut groups: Vec<(String, Totals)> = per.into_iter().map(|(p, t)| (label(p), t)).collect();
    groups.sort_by(|a, b| {
        b.1.pause_ns
            .cmp(&a.1.pause_ns)
            .then(b.1.allocated.cmp(&a.1.allocated))
            .then_with(|| a.0.cmp(&b.0))
    });
    let mut longest: Vec<&Gc> = gcs.clone();
    longest.sort_by(|a, b| {
        b.pause
            .unwrap_or(0)
            .cmp(&a.pause.unwrap_or(0))
            .then((b.end - b.start).cmp(&(a.end - a.start)))
            .then(a.start.cmp(&b.start))
    });
    longest.truncate(limit);
    let mut exceptions: Vec<(u32, Thrown)> = exceptions.into_iter().collect();
    exceptions.sort_by(|a, b| b.1.0.cmp(&a.1.0).then(a.0.cmp(&b.0)));
    let mut slow: Vec<&Timed> = jits.clone();
    slow.sort_by(|a, b| {
        (b.end - b.start)
            .cmp(&(a.end - a.start))
            .then(a.start.cmp(&b.start))
    });
    slow.truncate(limit);
    let mut by_type: Vec<(u32, u64)> = by_type.into_iter().collect();
    by_type.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(&b.0)));
    by_type.truncate(limit);
    let s = |i: u32| table.strings.get(i as usize).map_or("", String::as_str);
    json!({
        "gcs": gcs.len(),
        "pauseNs": groups.iter().map(|g| g.1.pause_ns).sum::<u64>(),
        "allocated": groups.iter().map(|g| g.1.allocated).sum::<u64>(),
        "exceptions": groups.iter().map(|g| g.1.exceptions).sum::<u64>(),
        "jits": jits.len(),
        "groupsTotal": groups.len(),
        "groups": groups.iter().map(|(l, t)| json!({
            "label": l,
            "gcs": t.gcs,
            "gen0": t.gens[0],
            "gen1": t.gens[1],
            "gen2": t.gens[2],
            "gcNs": t.gc_ns,
            "pauseNs": t.pause_ns,
            "maxPauseNs": t.max_pause_ns,
            "allocated": t.allocated,
            "exceptions": t.exceptions,
            "jits": t.jits,
            "jitNs": t.jit_ns,
            "contentions": t.contentions,
            "contentionNs": t.contention_ns,
        })).collect::<Vec<_>>(),
        "collections": longest.iter().map(|g| json!({
            "process": label(g.process),
            "number": g.number,
            "generation": g.generation,
            "reason": reason_name(g.reason),
            "kind": kind_name(g.kind),
            "startNs": trace.clock.nanos(g.start),
            "durationNs": ns(g.end - g.start),
            "pauseNs": g.pause.map(ns),
        })).collect::<Vec<_>>(),
        "exceptionTypes": exceptions.iter().map(|(ty, (n, by))| {
            let mut by: Vec<(String, u64)> = by.iter().map(|(p, n)| (label(*p), *n)).collect();
            by.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
            json!({
                "type": s(*ty),
                "count": n,
                "processes": by.iter().map(|(l, _)| l.clone()).collect::<Vec<_>>().join(", "),
            })
        }).collect::<Vec<_>>(),
        "compilations": slow.iter().map(|j| json!({
            "method": j.what.map_or("Unknown method", s),
            "process": label(j.process),
            "startNs": trace.clock.nanos(j.start),
            "durationNs": ns(j.end - j.start),
        })).collect::<Vec<_>>(),
        "allocationTypes": by_type.iter().map(|(ty, bytes)| json!({
            "type": s(*ty),
            "bytes": bytes,
        })).collect::<Vec<_>>(),
    })
}
