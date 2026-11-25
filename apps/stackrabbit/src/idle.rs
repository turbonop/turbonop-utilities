//! CPU idle states: each processor's idle periods (the kernel Power
//! provider's events 57, entering an idle state, and 58, leaving it), as
//! WPA's CPU Idle States table has them; then, in a time range, each
//! processor's time idle (in all and by state) and each state's periods.
//!
//! No description of the two events is on the machine (TDH has none), so
//! their layout is what the corpus shows: event 57's first 32 bits are the
//! idle state's index (0, 1, 2, ...), then 32 bits of flags and 64 of a
//! value (often a time, or all ones); event 58's first 32 bits are a state
//! too. On every processor they alternate, 57 then 58; and each one's
//! time is a boundary of a period in WPA's table (on s09, 69,459 of the
//! 69,464 entries and all 69,462 exits).

use std::path::Path;

use rustc_hash::FxHashMap;
use serde_json::{Value, json};
use turbonop_etw::guids;
use turbonop_trace::Trace;

/// The kernel Power provider's idle entry and exit.
const ENTER: u8 = 57;
const EXIT: u8 = 58;

/// One idle period.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Period {
    /// The processor.
    pub processor: u16,
    /// The idle state's index.
    pub state: u32,
    /// When it entered the state (clock ticks).
    pub start: i64,
    /// When it left (clock ticks).
    pub end: i64,
}

/// A trace's idle periods, in start order.
#[derive(Clone, Debug, Default)]
pub struct IdleTable {
    /// The periods.
    pub periods: Vec<Period>,
}

/// The idle periods of `trace` (read from `path`), from the buffers
/// holding its idle events, on every core.
pub fn table(trace: &Trace, path: &Path) -> Result<IdleTable, String> {
    let wanted = |provider: &turbonop_etl::Guid, op: u8| {
        *provider == guids::POWER && (op == ENTER || op == EXIT)
    };
    let (mut all, _) = crate::reading::events(trace, path, wanted, |processor, e, mine| {
        let d = e.user_data;
        let Some(state) = d
            .get(..4)
            .map(|b| u32::from_le_bytes([b[0], b[1], b[2], b[3]]))
        else {
            return;
        };
        mine.push((
            processor,
            e.timestamp,
            e.file_offset,
            e.descriptor.opcode == ENTER,
            state,
        ));
    })?;
    all.sort_unstable();
    let mut periods = Vec::new();
    // Each processor's entry not yet left: when, and its state.
    let mut open: FxHashMap<u16, (i64, u32)> = FxHashMap::default();
    for (processor, time, _, enter, state) in all {
        if enter {
            open.insert(processor, (time, state));
        } else if let Some((start, state)) = open.remove(&processor) {
            periods.push(Period {
                processor,
                state,
                start,
                end: time,
            });
        }
    }
    periods.sort_by_key(|p| (p.start, p.processor));
    Ok(IdleTable { periods })
}

/// A processor's time idle, periods, and time by state.
type Idled = (u64, u64, FxHashMap<u32, u64>);

/// The upper bounds (nanoseconds) of the duration histogram's buckets;
/// the last is unbounded.
const BUCKETS: [u64; 6] = [
    10_000,
    100_000,
    1_000_000,
    10_000_000,
    100_000_000,
    u64::MAX,
];

/// `table`'s idle periods in `time` (clock ticks; all, if none), clipped
/// to it: each processor's time idle and periods (in all and by state,
/// as `groups`), each state's periods, total, average and longest, and how
/// long the periods were.
pub fn summary(trace: &Trace, table: &IdleTable, time: Option<(i64, i64)>) -> Value {
    let (start, end) = time.unwrap_or((i64::MIN, i64::MAX));
    let ns =
        |a: i64, b: i64| u64::try_from(trace.clock.nanos(b) - trace.clock.nanos(a)).unwrap_or(0);
    // Per processor: idle time, periods, and time by state; per state:
    // periods, time, longest; and the histogram by state.
    let mut per: FxHashMap<u16, Idled> = FxHashMap::default();
    let mut states: FxHashMap<u32, (u64, u64, u64)> = FxHashMap::default();
    let mut histogram: FxHashMap<u32, [u64; BUCKETS.len()]> = FxHashMap::default();
    // (Periods start in order, and none is longer than the longest.)
    for p in &table.periods {
        if p.start >= end {
            break;
        }
        let (a, b) = (p.start.max(start), p.end.min(end));
        if a >= b {
            continue;
        }
        let d = ns(a, b);
        let g = per.entry(p.processor).or_default();
        g.0 += d;
        g.1 += 1;
        *g.2.entry(p.state).or_default() += d;
        let s = states.entry(p.state).or_default();
        s.0 += 1;
        s.1 += d;
        s.2 = s.2.max(d);
        let bucket = BUCKETS
            .iter()
            .position(|&x| d < x)
            .unwrap_or(BUCKETS.len() - 1);
        histogram.entry(p.state).or_default()[bucket] += 1;
    }
    // The span asked about, to say what share of it each processor idled:
    // the range, or the trace's periods' span.
    let first = table.periods.first().map_or(0, |p| p.start);
    let last = table.periods.iter().map(|p| p.end).max().unwrap_or(0);
    let span = ns(start.max(first), end.min(last)).max(1);
    let mut groups: Vec<(u16, Idled)> = per.into_iter().collect();
    groups.sort_by_key(|g| g.0);
    let mut states: Vec<(u32, (u64, u64, u64))> = states.into_iter().collect();
    states.sort_by_key(|s| s.0);
    let mut histogram: Vec<(u32, [u64; BUCKETS.len()])> = histogram.into_iter().collect();
    histogram.sort_by_key(|h| h.0);
    let name = |s: u32| format!("State {s}");
    json!({
        "periods": groups.iter().map(|g| g.1.1).sum::<u64>(),
        "idleNs": groups.iter().map(|g| g.1.0).sum::<u64>(),
        "spanNs": span,
        "groupsTotal": groups.len(),
        "groups": groups.iter().map(|(cpu, (idle, n, by))| {
            let mut by: Vec<(&u32, &u64)> = by.iter().collect();
            by.sort();
            json!({
                "label": format!("CPU {cpu}"),
                "cpu": cpu,
                "idleNs": idle,
                "periods": n,
                "share": ((*idle as f64 / span as f64) * 1e4).round() / 1e4,
                "byState": by.iter().map(|(s, t)| json!({"state": name(**s), "ns": t})).collect::<Vec<_>>(),
            })
        }).collect::<Vec<_>>(),
        "states": states.iter().map(|(s, (n, t, max))| json!({
            "state": name(*s),
            "periods": n,
            "ns": t,
            "averageNs": t / (*n).max(1),
            "maxNs": max,
        })).collect::<Vec<_>>(),
        "bucketsNs": BUCKETS[..BUCKETS.len() - 1],
        "histogram": histogram.iter().map(|(s, counts)| json!({
            "type": name(*s), "counts": counts,
        })).collect::<Vec<_>>(),
    })
}
