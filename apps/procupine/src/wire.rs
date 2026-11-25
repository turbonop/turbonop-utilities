//! Samples as JSON, for `procupine serve` and `procupine list -json`.
//!
//! Each process is given an ID for the session (`id`), a small number that
//! no other process in the session has (process IDs are reused; IDs here
//! aren't), which requests name it by. What doesn't change (its details) is
//! sent once, when it is first seen; its counters and rates are rows of
//! numbers, in the order of [`COLUMNS`], sent when they change.

use std::collections::HashMap;

use serde_json::{Value, json};
use turbonop_process::{Key, Process, Rates, Sample, State};

/// A row's columns.
pub const COLUMNS: &[&str] = &[
    "id",
    "parent",
    "cpu",
    "private",
    "resident",
    "virtual",
    "threads",
    "handles",
    "readRate",
    "writeRate",
    "ioOps",
    "contextSwitches",
    "pageFaults",
    "priority",
    "state",
    "userTime",
    "kernelTime",
    "readBytes",
    "writeBytes",
];

/// The session's processes: their IDs, and what was last sent of each.
#[derive(Default)]
pub struct Session {
    next: u64,
    ids: HashMap<Key, u64>,
    keys: HashMap<u64, Key>,
    sent: HashMap<u64, Vec<Value>>,
}

impl Session {
    /// The process `key`'s ID, given it if new.
    fn id(&mut self, key: Key) -> (u64, bool) {
        if let Some(&id) = self.ids.get(&key) {
            return (id, false);
        }
        self.next += 1;
        self.ids.insert(key, self.next);
        self.keys.insert(self.next, key);
        (self.next, true)
    }

    /// The process with ID `id`, if it was in the last sample.
    pub fn key(&self, id: u64) -> Option<Key> {
        self.keys.get(&id).copied()
    }

    /// `sample` as an event: the processes started (with their details),
    /// those exited, and the rows that changed (all of them, if `all`).
    pub fn event(&mut self, sample: &Sample, all: bool) -> Value {
        let s = &sample.snapshot;
        let mut added = Vec::new();
        let mut rows = Vec::new();
        let ids: Vec<(u64, bool)> = s.processes.iter().map(|p| self.id(p.key)).collect();
        let mut live = HashMap::with_capacity(ids.len());
        for (i, p) in s.processes.iter().enumerate() {
            let (id, new) = ids[i];
            live.insert(id, ());
            let parent = s.parent(i).map(|j| ids[j].0);
            if new || all {
                added.push(details(p, id, parent));
            }
            let row = row(p, &sample.rates[i], id, parent);
            if all || self.sent.get(&id) != Some(&row) {
                rows.push(Value::Array(row.clone()));
                self.sent.insert(id, row);
            }
        }
        let removed: Vec<u64> = self
            .keys
            .keys()
            .copied()
            .filter(|id| !live.contains_key(id))
            .collect();
        for id in &removed {
            if let Some(key) = self.keys.remove(id) {
                self.ids.remove(&key);
            }
            self.sent.remove(id);
        }
        let threads: u64 = s
            .processes
            .iter()
            .map(|p| u64::from(p.counters.threads))
            .sum();
        let handles: u64 = s
            .processes
            .iter()
            .filter_map(|p| p.counters.handles)
            .map(u64::from)
            .sum();
        let sys = &s.system;
        json!({
            "event": "sample",
            "time": s.time / 1_000_000,
            "interval": sample.interval_ns / 1_000_000,
            "system": {
                "cpus": sys.cpus,
                "cpu": round(sample.cpu),
                "memoryTotal": sys.memory_total,
                "memoryAvailable": sys.memory_available,
                "commit": sys.commit,
                "commitLimit": sys.commit_limit,
                "swapTotal": sys.swap_total,
                "swapUsed": sys.swap_used,
                "uptime": sys.uptime_ns / 1_000_000,
                "processes": s.processes.len(),
                "threads": threads,
                "handles": handles,
            },
            "added": added,
            "removed": removed,
            "rows": rows,
        })
    }
}

/// What doesn't change about a process.
pub fn details(p: &Process, id: u64, parent: Option<u64>) -> Value {
    let d = &p.details;
    json!({
        "id": id,
        "pid": p.key.pid,
        "parent": parent,
        "parentPid": p.parent_pid,
        "name": p.name,
        // (Milliseconds since the Unix epoch, as JavaScript's dates count.)
        "start": (p.key.start > 0).then(|| p.key.start as f64 / 1e6),
        "path": d.path,
        "commandLine": d.command_line,
        "user": d.user,
        "architecture": d.architecture,
        "elevated": d.elevated,
        "integrity": d.integrity,
        "kernel": d.kernel,
        "session": p.counters.session,
    })
}

/// A process's row: its numbers, in the order of [`COLUMNS`].
pub fn row(p: &Process, r: &Rates, id: u64, parent: Option<u64>) -> Vec<Value> {
    let c = &p.counters;
    let rate = |v: Option<f64>| v.map_or(Value::Null, |v| json!(v.round()));
    vec![
        json!(id),
        json!(parent),
        json!(round(r.cpu)),
        json!(c.private),
        json!(c.resident),
        json!(c.virtual_size),
        json!(c.threads),
        json!(c.handles),
        rate(r.read_bytes),
        rate(r.write_bytes),
        rate(r.io_ops),
        rate(r.context_switches),
        json!(r.page_faults.round()),
        json!(c.priority),
        json!(state_name(p.state)),
        json!(c.user_ns / 1_000_000),
        json!(c.kernel_ns / 1_000_000),
        json!(c.read_bytes),
        json!(c.write_bytes),
    ]
}

/// A share to 5 decimals: rows that differ by less are the same.
fn round(v: f64) -> f64 {
    (v * 100_000.0).round() / 100_000.0
}

fn state_name(s: State) -> &'static str {
    s.name()
}
