//! CPU frequency: each processor's frequency over time, from the
//! Microsoft-Windows-Kernel-Processor-Power provider's
//! `ProcessorPerfStateChange` events (a processor's new speed, in MHz, and
//! its performance, a percentage of its nominal), as WPA's CPU Frequency
//! table has it: a processor runs at a speed from one change to its next
//! (the last until the trace ends). Then, in a time range, each processor's
//! average (by time), lowest and highest frequency, and the time spent at
//! each frequency.

use std::path::Path;

use rustc_hash::FxHashMap;
use serde_json::{Value, json};
use turbonop_etl::Guid;
use turbonop_etw::event_info::{EventInfo, EventInfos};
use turbonop_trace::Trace;

use crate::reading::int;

/// Microsoft-Windows-Kernel-Processor-Power.
pub const PROCESSOR_POWER: Guid = Guid::new(
    0x0f67_e49f,
    0xfe51,
    0x4e9f,
    [0xb4, 0x90, 0x6f, 0x29, 0x48, 0xcc, 0x60, 0x27],
);

/// `ProcessorPerfStateChange`.
const PERF_STATE_CHANGE: u16 = 10;

/// A processor's speed from a time on.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Change {
    /// When (clock ticks).
    pub time: i64,
    /// The processor (its group's 64 and its number).
    pub processor: u32,
    /// Its speed, in MHz.
    pub mhz: u32,
    /// Its performance, a percentage of its nominal.
    pub percent: u32,
}

/// A trace's frequency changes, by processor then time, and when the
/// trace ends.
#[derive(Clone, Debug, Default)]
pub struct FrequencyTable {
    /// The changes.
    pub changes: Vec<Change>,
    /// The trace's last event (clock ticks): the last speeds last until
    /// then.
    pub end: i64,
}

/// The frequency changes of `trace` (read from `path`), from the buffers
/// holding them, on every core.
pub fn table(trace: &Trace, path: &Path) -> Result<FrequencyTable, String> {
    let mut infos = EventInfos::default();
    for b in &trace.event_infos {
        if let Ok(i) = EventInfo::parse(b) {
            infos.add(i);
        }
    }
    let wanted = |provider: &Guid, _op: u8| *provider == PROCESSOR_POWER;
    let (mut changes, _) = crate::reading::events(trace, path, wanted, |_, e, mine| {
        if e.descriptor.id != PERF_STATE_CHANGE {
            return;
        }
        let Some(info) = infos.get(e) else { return };
        let d = info.decode_partial(e);
        let get = |name: &str| d.properties.iter().find(|p| p.name == name);
        let (Some(number), Some(speed)) = (get("Number"), get("Speed")) else {
            return;
        };
        let group = u32::try_from(int(get("Group"))).unwrap_or(0);
        mine.push(Change {
            time: e.timestamp,
            processor: group * 64 + u32::try_from(int(Some(number))).unwrap_or(u32::MAX),
            mhz: u32::try_from(int(Some(speed))).unwrap_or(0),
            percent: u32::try_from(int(get("Performance"))).unwrap_or(0),
        });
    })?;
    changes.sort_by_key(|c| (c.processor, c.time));
    let end = trace.events.iter().map(|k| k.last).max().unwrap_or(0);
    Ok(FrequencyTable { changes, end })
}

/// `table`'s frequencies in `time` (clock ticks; all of the trace, if
/// none): each processor's average (weighted by time), lowest and highest
/// frequency and changes (`groups`, as the summary tabs have them), the
/// time spent at each frequency (all processors), and the average over
/// them all.
pub fn summary(trace: &Trace, table: &FrequencyTable, time: Option<(i64, i64)>) -> Value {
    let (start, end) = time.unwrap_or((i64::MIN, i64::MAX));
    let ns =
        |a: i64, b: i64| u64::try_from(trace.clock.nanos(b) - trace.clock.nanos(a)).unwrap_or(0);
    // Per processor: time, MHz x time, lowest, highest, changes; and
    // the time at each MHz.
    let mut per: Vec<(u32, u64, u128, u32, u32, u64)> = Vec::new();
    let mut at: FxHashMap<u32, u64> = FxHashMap::default();
    for run in table.changes.chunk_by(|a, b| a.processor == b.processor) {
        let processor = run[0].processor;
        let (mut total, mut weighted, mut low, mut high, mut changes) =
            (0u64, 0u128, u32::MAX, 0u32, 0u64);
        for (i, c) in run.iter().enumerate() {
            let until = run.get(i + 1).map_or(table.end, |n| n.time);
            let (a, b) = (c.time.max(start), until.min(end));
            if c.time >= start && c.time < end {
                changes += 1;
            }
            if a >= b {
                continue;
            }
            let d = ns(a, b);
            total += d;
            weighted += u128::from(d) * u128::from(c.mhz);
            low = low.min(c.mhz);
            high = high.max(c.mhz);
            *at.entry(c.mhz).or_default() += d;
        }
        if total > 0 {
            per.push((processor, total, weighted, low, high, changes));
        }
    }
    let all_time: u64 = per.iter().map(|p| p.1).sum();
    let all_weighted: u128 = per.iter().map(|p| p.2).sum();
    let mean = |w: u128, t: u64| if t == 0 { 0.0 } else { w as f64 / t as f64 };
    let mut frequencies: Vec<(u32, u64)> = at.into_iter().collect();
    frequencies.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(&b.0)));
    json!({
        "changes": per.iter().map(|p| p.5).sum::<u64>(),
        "averageMhz": (mean(all_weighted, all_time) * 10.0).round() / 10.0,
        "groupsTotal": per.len(),
        "groups": per.iter().map(|&(cpu, t, w, low, high, n)| json!({
            "label": format!("CPU {cpu}"),
            "cpu": cpu,
            "timeNs": t,
            "averageMhz": (mean(w, t) * 10.0).round() / 10.0,
            "minMhz": low,
            "maxMhz": high,
            "changes": n,
        })).collect::<Vec<_>>(),
        "levels": frequencies.iter().map(|&(mhz, t)| json!({
            "mhz": mhz,
            "timeNs": t,
            "share": if all_time == 0 { 0.0 } else { ((t as f64 / all_time as f64) * 1e4).round() / 1e4 },
        })).collect::<Vec<_>>(),
    })
}
