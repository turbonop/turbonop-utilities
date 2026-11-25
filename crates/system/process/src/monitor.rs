//! Rates between snapshots: CPU usage, I/O per second, and the processes
//! that started and exited in between.

use std::io;

use crate::{Collector, Process, Snapshot};

/// A process's rates since the snapshot before.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Rates {
    /// Its share of every processor's time (0 to 1: 1 is every processor,
    /// busy).
    pub cpu: f64,
    /// Bytes read per second, where counted.
    pub read_bytes: Option<f64>,
    /// Bytes written per second, where counted.
    pub write_bytes: Option<f64>,
    /// Read and write operations per second, where counted.
    pub io_ops: Option<f64>,
    /// Context switches per second, where counted.
    pub context_switches: Option<f64>,
    /// Page faults per second.
    pub page_faults: f64,
}

/// A snapshot, with each process's rates since the one before.
#[derive(Clone, Debug)]
pub struct Sample {
    /// The snapshot.
    pub snapshot: Snapshot,
    /// Each process's rates, in the order of `snapshot.processes`.
    pub rates: Vec<Rates>,
    /// The share of every processor's time that wasn't idle (0 to 1).
    pub cpu: f64,
    /// The time since the snapshot before, in nanoseconds (0 for the
    /// first).
    pub interval_ns: u64,
    /// The processes that weren't in the snapshot before, by index (none
    /// in the first).
    pub started: Vec<usize>,
    /// The processes in the snapshot before that have exited.
    pub exited: Vec<Process>,
}

/// Takes snapshots and compares each with the one before.
pub struct Monitor {
    collector: Collector,
    last: Option<Snapshot>,
}

impl Monitor {
    /// A monitor of this machine's processes.
    pub fn new() -> io::Result<Self> {
        Ok(Monitor {
            collector: Collector::new()?,
            last: None,
        })
    }

    /// A snapshot now, and its rates since the last.
    pub fn sample(&mut self) -> io::Result<Sample> {
        let next = self.collector.snapshot()?;
        let sample = compare(self.last.as_ref(), next);
        self.last = Some(sample.snapshot.clone());
        Ok(sample)
    }

    /// The last snapshot taken.
    pub fn last(&self) -> Option<&Snapshot> {
        self.last.as_ref()
    }
}

/// `next`'s rates since `prev`.
pub fn compare(prev: Option<&Snapshot>, next: Snapshot) -> Sample {
    let Some(prev) = prev else {
        let rates = vec![Rates::default(); next.processes.len()];
        return Sample {
            snapshot: next,
            rates,
            cpu: 0.0,
            interval_ns: 0,
            started: Vec::new(),
            exited: Vec::new(),
        };
    };
    let interval_ns = u64::try_from(next.taken.saturating_duration_since(prev.taken).as_nanos())
        .unwrap_or(u64::MAX);
    let secs = interval_ns as f64 / 1e9;
    let per_s = |d: u64| if secs > 0.0 { d as f64 / secs } else { 0.0 };
    // Each process's index in the snapshot before (both are sorted by ID).
    let before: Vec<Option<&Process>> = next
        .processes
        .iter()
        .map(|p| prev.find(p.key).map(|i| &prev.processes[i]))
        .collect();
    // What a counter grew by: since the snapshot before, or for a process
    // that started since, all of it. (A process seen before whose counter
    // went down, as one a debugger reset: 0.)
    let grew = |now: u64, then: Option<u64>, started_since: bool| match then {
        Some(then) => now.saturating_sub(then),
        None if started_since => now,
        None => 0,
    };
    let started_since = |p: &Process| p.key.start >= prev.time.saturating_sub(interval_ns);
    // CPU by cycles, where every process and the processors' idle time
    // count them (Windows): a share of all the cycles counted.
    let by_cycles = match (prev.system.idle_cycles, next.system.idle_cycles) {
        (Some(a), Some(b)) if next.processes.iter().all(|p| p.counters.cycles.is_some()) => {
            let idle = b.saturating_sub(a);
            let busy: Vec<u64> = next
                .processes
                .iter()
                .zip(&before)
                .map(|(p, then)| {
                    if p.key.pid == 0 {
                        return 0;
                    }
                    grew(
                        p.counters.cycles.unwrap_or(0),
                        then.and_then(|t| t.counters.cycles),
                        started_since(p),
                    )
                })
                .collect();
            let total = idle + busy.iter().sum::<u64>();
            (total > 0).then_some((busy, idle, total))
        }
        _ => None,
    };
    let capacity = interval_ns as f64 * f64::from(next.system.cpus.max(1));
    let mut rates = Vec::with_capacity(next.processes.len());
    for (i, (p, then)) in next.processes.iter().zip(&before).enumerate() {
        let new = started_since(p);
        let c = &p.counters;
        let delta = |now: Option<u64>, then_v: Option<Option<u64>>| -> Option<u64> {
            let now = now?;
            Some(grew(now, then_v.flatten(), new))
        };
        let tc = then.map(|t| &t.counters);
        let cpu = match &by_cycles {
            Some((busy, idle, total)) => {
                let mine = if p.key.pid == 0 { *idle } else { busy[i] };
                mine as f64 / *total as f64
            }
            None if capacity > 0.0 => grew(c.cpu_ns, tc.map(|t| t.cpu_ns), new) as f64 / capacity,
            None => 0.0,
        };
        let io_ops = match (
            delta(c.read_ops, tc.map(|t| t.read_ops)),
            delta(c.write_ops, tc.map(|t| t.write_ops)),
        ) {
            (Some(r), Some(w)) => Some(per_s(r + w)),
            (r, w) => r.or(w).map(per_s),
        };
        rates.push(Rates {
            cpu: cpu.clamp(0.0, 1.0),
            read_bytes: delta(c.read_bytes, tc.map(|t| t.read_bytes)).map(per_s),
            write_bytes: delta(c.write_bytes, tc.map(|t| t.write_bytes)).map(per_s),
            io_ops,
            context_switches: delta(c.context_switches, tc.map(|t| t.context_switches)).map(per_s),
            page_faults: per_s(grew(c.page_faults, tc.map(|t| t.page_faults), new)),
        });
    }
    let cpu = match &by_cycles {
        Some((_, idle, total)) => 1.0 - *idle as f64 / *total as f64,
        None => {
            let total = next
                .system
                .cpu_total_ns
                .saturating_sub(prev.system.cpu_total_ns);
            let idle = next
                .system
                .cpu_idle_ns
                .saturating_sub(prev.system.cpu_idle_ns);
            if total > 0 {
                1.0 - idle as f64 / total as f64
            } else {
                0.0
            }
        }
    };
    let started = before
        .iter()
        .enumerate()
        .filter(|(_, b)| b.is_none())
        .map(|(i, _)| i)
        .collect();
    let exited = prev
        .processes
        .iter()
        .filter(|p| next.find(p.key).is_none())
        .cloned()
        .collect();
    Sample {
        snapshot: next,
        rates,
        cpu: cpu.clamp(0.0, 1.0),
        interval_ns,
        started,
        exited,
    }
}
