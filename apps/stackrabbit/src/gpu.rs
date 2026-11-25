//! GPU work: the Microsoft-Windows-DxgKrnl provider's queue packets (work a
//! process's context submits to the GPU: command buffers, and waits for and
//! signals of synchronization objects), each from when it was queued to when
//! the GPU completed it; the processes' presents; and the vsync interrupts.
//! Then, in a time range, the packets grouped by process, engine or context:
//! how many, and how long each group had work queued or running on the GPU
//! (its packets' lifetimes, overlaps counted once); and the longest.
//!
//! A packet is queued by `QueuePacket` start (event 178, a command buffer;
//! 244, a wait; 245, a signal) and ended by `QueuePacket` stop (180), both
//! naming it by its address. A context's engine is its node ordinal, from
//! the `Context` start and rundown events, where the trace names it. (With
//! hardware scheduling, packets name a hardware queue, as its
//! `ParentDxgHwQueue`; the `HwQueue` events name its context.)
//!
//! (Not compared with WPA: its GPU tables' columns couldn't be named for
//! its exporter.)

use std::path::Path;

use rustc_hash::FxHashMap;
use serde_json::{Value, json};
use turbonop_etl::Guid;
use turbonop_etw::event_info::{EventInfo, EventInfos};
use turbonop_trace::{ProcessIx, Trace};

use crate::reading::int;

/// Microsoft-Windows-DxgKrnl.
pub const DXGKRNL: Guid = Guid::new(
    0x802e_c45a,
    0x1e99,
    0x4b83,
    [0x99, 0x20, 0x87, 0xc9, 0x82, 0x77, 0xba, 0x9d],
);

const QUEUE_COMMAND: u16 = 178;
const QUEUE_WAIT: u16 = 244;
const QUEUE_SIGNAL: u16 = 245;
const QUEUE_DONE: u16 = 180;
const CONTEXT_START: u16 = 30;
const CONTEXT_RUNDOWN: u16 = 32;
/// `HwQueue` events (a hardware queue's start, stop and rundown).
const HW_QUEUES: std::ops::RangeInclusive<u16> = 422..=425;
const PRESENT: u16 = 184;
const VSYNC: u16 = 181;

/// What a packet was.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Kind {
    /// A command buffer.
    Command,
    /// A wait for synchronization objects.
    Wait,
    /// A signal of synchronization objects.
    Signal,
}

impl Kind {
    fn name(self) -> &'static str {
        match self {
            Self::Command => "Command",
            Self::Wait => "Wait",
            Self::Signal => "Signal",
        }
    }
}

/// One queue packet.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Packet {
    /// What it was.
    pub kind: Kind,
    /// When it was queued (clock ticks).
    pub start: i64,
    /// When the GPU completed it.
    pub end: i64,
    /// The process that queued it.
    pub process: ProcessIx,
    /// Its context.
    pub context: u64,
    /// Whether it was preempted.
    pub preempted: bool,
}

/// A trace's GPU work, in packet start order.
#[derive(Clone, Debug, Default)]
pub struct GpuTable {
    /// The packets completed in the trace.
    pub packets: Vec<Packet>,
    /// Each context's engine (node ordinal), where known.
    pub engines: FxHashMap<u64, u32>,
    /// Each hardware queue's context.
    pub queues: FxHashMap<u64, u64>,
    /// The presents: when, and by which process.
    pub presents: Vec<(i64, ProcessIx)>,
    /// The vsync interrupts' times.
    pub vsyncs: Vec<i64>,
}

enum Read {
    Queued {
        kind: Kind,
        time: i64,
        offset: u64,
        packet: u64,
        context: u64,
        pid: u32,
    },
    Done {
        time: i64,
        offset: u64,
        packet: u64,
        preempted: bool,
    },
    Context {
        context: u64,
        node: u32,
    },
    HwQueue {
        queue: u64,
        context: u64,
    },
    Present {
        time: i64,
        pid: u32,
    },
    Vsync {
        time: i64,
    },
}

/// The GPU work of `trace` (read from `path`), from the buffers holding
/// DxgKrnl's events, on every core.
pub fn table(trace: &Trace, path: &Path) -> Result<GpuTable, String> {
    let mut infos = EventInfos::default();
    for b in &trace.event_infos {
        if let Ok(i) = EventInfo::parse(b) {
            infos.add(i);
        }
    }
    let wanted = |provider: &Guid, _op: u8| *provider == DXGKRNL;
    let (mut all, _) = crate::reading::events(trace, path, wanted, |_, e, mine| {
        let id = e.descriptor.id;
        if ![
            QUEUE_COMMAND,
            QUEUE_WAIT,
            QUEUE_SIGNAL,
            QUEUE_DONE,
            CONTEXT_START,
            CONTEXT_RUNDOWN,
            PRESENT,
            VSYNC,
        ]
        .contains(&id)
            && !HW_QUEUES.contains(&id)
        {
            return;
        }
        let Some(info) = infos.get(e) else { return };
        let d = info.decode_partial(e);
        let get = |name: &str| int(d.properties.iter().find(|p| p.name == name));
        let has = |name: &str| d.properties.iter().any(|p| p.name == name);
        let time = e.timestamp;
        let offset = e.file_offset;
        match id {
            QUEUE_COMMAND | QUEUE_WAIT | QUEUE_SIGNAL if has("pQueuePacket") => {
                mine.push(Read::Queued {
                    kind: match id {
                        QUEUE_COMMAND => Kind::Command,
                        QUEUE_WAIT => Kind::Wait,
                        _ => Kind::Signal,
                    },
                    time,
                    offset,
                    packet: get("pQueuePacket"),
                    context: get("hContext"),
                    pid: e.process_id,
                });
            }
            QUEUE_DONE if has("pQueuePacket") => mine.push(Read::Done {
                time,
                offset,
                packet: get("pQueuePacket"),
                preempted: get("bPreempted") != 0,
            }),
            CONTEXT_START | CONTEXT_RUNDOWN if has("NodeOrdinal") => mine.push(Read::Context {
                context: get("hContext"),
                node: u32::try_from(get("NodeOrdinal")).unwrap_or(u32::MAX),
            }),
            _ if HW_QUEUES.contains(&id) && has("ParentDxgHwQueue") => mine.push(Read::HwQueue {
                queue: get("ParentDxgHwQueue"),
                context: get("hContext"),
            }),
            PRESENT => mine.push(Read::Present {
                time,
                pid: e.process_id,
            }),
            VSYNC => mine.push(Read::Vsync { time }),
            _ => {}
        }
    })?;
    let key = |r: &Read| match r {
        Read::Queued { time, offset, .. } | Read::Done { time, offset, .. } => (*time, *offset),
        Read::Present { time, .. } | Read::Vsync { time } => (*time, 0),
        Read::Context { .. } | Read::HwQueue { .. } => (i64::MIN, 0),
    };
    all.sort_by_key(key);
    let mut out = GpuTable::default();
    let mut open: FxHashMap<u64, (Kind, i64, u64, u32)> = FxHashMap::default();
    let mut process_of: FxHashMap<u32, ProcessIx> = FxHashMap::default();
    let mut process = |pid: u32, time: i64| {
        *process_of
            .entry(pid)
            .or_insert_with(|| crate::reading::process_at(trace, pid, time))
    };
    for r in all {
        match r {
            Read::Queued {
                kind,
                time,
                packet,
                context,
                pid,
                ..
            } => {
                open.insert(packet, (kind, time, context, pid));
            }
            Read::Done {
                time,
                packet,
                preempted,
                ..
            } => {
                if let Some((kind, start, context, pid)) = open.remove(&packet) {
                    out.packets.push(Packet {
                        kind,
                        start,
                        end: time,
                        process: process(pid, start),
                        context,
                        preempted,
                    });
                }
            }
            Read::Context { context, node } => {
                out.engines.insert(context, node);
            }
            Read::HwQueue { queue, context } => {
                out.queues.insert(queue, context);
            }
            Read::Present { time, pid } => out.presents.push((time, process(pid, time))),
            Read::Vsync { time } => out.vsyncs.push(time),
        }
    }
    out.packets.sort_by_key(|p| (p.start, p.end));
    Ok(out)
}

/// What packets are grouped by.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum GroupBy {
    /// The process that queued them.
    Process,
    /// Their context's engine.
    Engine,
    /// Their context.
    Context,
    /// Their kind.
    Kind,
}

/// The total of `intervals` (start, end), overlaps counted once.
fn union(mut intervals: Vec<(i64, i64)>) -> i64 {
    intervals.sort_unstable();
    let (mut total, mut until) = (0i64, i64::MIN);
    for (a, b) in intervals {
        let a = a.max(until);
        if b > a {
            total += b - a;
            until = b;
        }
    }
    total
}

/// A group's packets, preempted ones, packets' lifetimes, and longest.
type Grouped = (u64, u64, Vec<(i64, i64)>, u64);

/// `table`'s packets that started in `time` (clock ticks; all, if none):
/// by `group`, how many and how long the group had work queued or running
/// (overlaps counted once), and the longest wait from queued to completed;
/// the presents by process; the vsync rate; and the `limit` longest
/// packets.
pub fn summary(
    trace: &Trace,
    table: &GpuTable,
    group: GroupBy,
    time: Option<(i64, i64)>,
    limit: usize,
) -> Value {
    let (start, end) = time.unwrap_or((i64::MIN, i64::MAX));
    let from = table.packets.partition_point(|p| p.start < start);
    let to = table.packets.partition_point(|p| p.start < end);
    let packets = &table.packets[from..to];
    let ticks_ns = |t: i64| {
        u64::try_from(
            i128::from(t) * 1_000_000_000 / i128::from(trace.clock.ticks_per_second.max(1)),
        )
        .unwrap_or(0)
    };
    let process_label = |p: ProcessIx| {
        if p.is_some() {
            let pr = &trace.processes[p.index()];
            format!("{} ({})", pr.name, pr.pid)
        } else {
            "Unknown process".to_string()
        }
    };
    let engine = |context: u64| match table.engines.get(&context).or_else(|| {
        table
            .queues
            .get(&context)
            .and_then(|c| table.engines.get(c))
    }) {
        Some(n) => format!("Engine {n}"),
        None => "Unknown engine".to_string(),
    };
    let label = |p: &Packet| match group {
        GroupBy::Process => process_label(p.process),
        GroupBy::Engine => engine(p.context),
        GroupBy::Context => format!("{:#x} ({})", p.context, engine(p.context)),
        GroupBy::Kind => p.kind.name().to_string(),
    };
    let mut groups: FxHashMap<String, Grouped> = FxHashMap::default();
    for p in packets {
        let g = groups.entry(label(p)).or_default();
        g.0 += 1;
        g.1 += u64::from(p.preempted);
        g.2.push((p.start, p.end));
        g.3 = g.3.max(ticks_ns(p.end - p.start));
    }
    let mut groups: Vec<(String, u64, u64, u64, u64)> = groups
        .into_iter()
        .map(|(l, (n, pre, iv, max))| (l, n, pre, ticks_ns(union(iv)), max))
        .collect();
    groups.sort_by(|a, b| {
        b.3.cmp(&a.3)
            .then(b.1.cmp(&a.1))
            .then_with(|| a.0.cmp(&b.0))
    });
    let in_range = |t: i64| t >= start && t < end;
    let mut presents: FxHashMap<ProcessIx, u64> = FxHashMap::default();
    for &(t, p) in &table.presents {
        if in_range(t) {
            *presents.entry(p).or_default() += 1;
        }
    }
    let mut presents: Vec<(String, u64)> = presents
        .into_iter()
        .map(|(p, n)| (process_label(p), n))
        .collect();
    presents.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
    let vsyncs: Vec<i64> = table
        .vsyncs
        .iter()
        .copied()
        .filter(|&t| in_range(t))
        .collect();
    let vsync_hz = match (vsyncs.first(), vsyncs.last()) {
        (Some(&a), Some(&b)) if vsyncs.len() > 1 && b > a => {
            (vsyncs.len() - 1) as f64 * 1e9 / ticks_ns(b - a) as f64
        }
        _ => 0.0,
    };
    let mut longest: Vec<&Packet> = packets.iter().collect();
    longest.sort_by(|a, b| {
        (b.end - b.start)
            .cmp(&(a.end - a.start))
            .then(a.start.cmp(&b.start))
    });
    longest.truncate(limit);
    json!({
        "packets": packets.len(),
        "busyNs": ticks_ns(union(packets.iter().map(|p| (p.start, p.end)).collect())),
        "presents": presents.iter().map(|p| p.1).sum::<u64>(),
        "vsyncHz": (vsync_hz * 100.0).round() / 100.0,
        "groupsTotal": groups.len(),
        "groups": groups.iter().map(|(l, n, pre, busy, max)| json!({
            "label": l, "count": n, "preempted": pre, "busyNs": busy, "maxNs": max,
        })).collect::<Vec<_>>(),
        "presenters": presents.iter().map(|(l, n)| json!({"label": l, "count": n})).collect::<Vec<_>>(),
        "longest": longest.iter().map(|p| json!({
            "kind": p.kind.name(),
            "process": process_label(p.process),
            "engine": engine(p.context),
            "startNs": trace.clock.nanos(p.start),
            "durationNs": ticks_ns(p.end - p.start),
            "preempted": p.preempted,
        })).collect::<Vec<_>>(),
    })
}
