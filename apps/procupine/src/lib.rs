//! `procupine`: the processes running on this machine, as a process
//! explorer shows them, from the command line or served to an editor (the
//! VS Code extension runs `procupine serve`).

#[cfg(test)]
use turbonop_global_alloc as _;

pub mod serve;
pub mod wire;

use std::process::ExitCode;
use std::time::Duration;

use serde_json::{Value, json};
use turbonop_process::{Key, Monitor, Sample, Snapshot};

pub const USAGE: &str = "\
usage: procupine <command> [options]

Commands:
  list [-tree] [-sort cpu|memory|pid|name|io] [-n N] [-json] [-interval MS] [-filter TEXT]
                 every process, with its CPU usage over an interval (250 ms)
  kill PID[@START]...       end processes at once
  terminate PID[@START]...  ask processes to end (on Windows, as kill)
  suspend PID[@START]...    suspend processes
  resume PID[@START]...     resume processes
  serve          sample processes for an editor (JSON lines on standard input and output)
  version        print the version

A process is named by its ID, or by its ID and start (as list -json gives it), which no later
process with the same ID has.";

pub fn run(args: &[String]) -> ExitCode {
    let Some((command, rest)) = args.split_first() else {
        eprintln!("{USAGE}");
        return ExitCode::from(2);
    };
    match command.as_str() {
        "list" | "ls" => list(rest),
        "kill" => act(rest, turbonop_process::kill, "kill"),
        "terminate" => act(rest, turbonop_process::terminate, "terminate"),
        "suspend" => act(rest, turbonop_process::suspend, "suspend"),
        "resume" => act(rest, turbonop_process::resume, "resume"),
        "serve" => serve::run(rest),
        "version" | "-version" | "--version" => {
            println!("procupine {}", env!("CARGO_PKG_VERSION"));
            ExitCode::SUCCESS
        }
        "help" | "-h" | "-help" | "--help" => {
            println!("{USAGE}");
            ExitCode::SUCCESS
        }
        other => {
            eprintln!("procupine: no command {other}\n\n{USAGE}");
            ExitCode::from(2)
        }
    }
}

/// How `list` sorts.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Sort {
    Cpu,
    Memory,
    Pid,
    Name,
    Io,
}

struct ListOptions {
    tree: bool,
    sort: Option<Sort>,
    limit: Option<usize>,
    json: bool,
    interval: Duration,
    filter: Option<String>,
}

fn list_options(args: &[String]) -> Result<ListOptions, String> {
    let mut o = ListOptions {
        tree: false,
        sort: None,
        limit: None,
        json: false,
        interval: Duration::from_millis(250),
        filter: None,
    };
    let mut it = args.iter();
    while let Some(a) = it.next() {
        let mut value = |name: &str| it.next().cloned().ok_or(format!("{name} needs a value"));
        match a.as_str() {
            "-tree" => o.tree = true,
            "-json" => o.json = true,
            "-sort" => {
                o.sort = Some(match value("-sort")?.as_str() {
                    "cpu" => Sort::Cpu,
                    "memory" | "mem" | "private" => Sort::Memory,
                    "pid" => Sort::Pid,
                    "name" => Sort::Name,
                    "io" => Sort::Io,
                    s => return Err(format!("-sort: no column {s} (cpu, memory, pid, name, io)")),
                });
            }
            "-n" => o.limit = Some(value("-n")?.parse().map_err(|_| "-n: not a number")?),
            "-interval" => {
                o.interval = Duration::from_millis(
                    value("-interval")?
                        .parse()
                        .map_err(|_| "-interval: not a number")?,
                );
            }
            "-filter" => o.filter = Some(value("-filter")?.to_lowercase()),
            other => return Err(format!("no option {other}")),
        }
    }
    if o.tree && o.sort.is_some() {
        return Err("-tree and -sort: one or the other".into());
    }
    Ok(o)
}

fn list(args: &[String]) -> ExitCode {
    let o = match list_options(args) {
        Ok(o) => o,
        Err(e) => {
            eprintln!("procupine list: {e}\n\n{USAGE}");
            return ExitCode::from(2);
        }
    };
    let sample = (|| -> std::io::Result<Sample> {
        let mut m = Monitor::new()?;
        if o.interval.is_zero() {
            return m.sample();
        }
        m.sample()?;
        std::thread::sleep(o.interval);
        m.sample()
    })();
    let sample = match sample {
        Ok(s) => s,
        Err(e) => {
            eprintln!("procupine list: {e}");
            return ExitCode::FAILURE;
        }
    };
    let rows = ordered(&sample, &o);
    if o.json {
        println!(
            "{}",
            serde_json::to_string_pretty(&list_json(&sample, &rows)).unwrap_or_default()
        );
    } else {
        print!("{}", list_text(&sample, &rows));
    }
    ExitCode::SUCCESS
}

/// The processes to list, by index, with their depth in the tree (0 unless
/// `-tree`): filtered, sorted, and cut to `-n`.
fn ordered(sample: &Sample, o: &ListOptions) -> Vec<(usize, usize)> {
    let s = &sample.snapshot;
    let matches = |i: usize| {
        let Some(f) = &o.filter else { return true };
        let p = &s.processes[i];
        p.name.to_lowercase().contains(f)
            || p.key.pid.to_string() == *f
            || p.details
                .command_line
                .as_deref()
                .is_some_and(|c| c.to_lowercase().contains(f))
            || p.details
                .user
                .as_deref()
                .is_some_and(|u| u.to_lowercase().contains(f))
    };
    let mut rows: Vec<(usize, usize)> = if o.tree {
        s.tree().depth_first()
    } else {
        (0..s.processes.len()).map(|i| (i, 0)).collect()
    };
    rows.retain(|&(i, _)| matches(i));
    let io = |i: usize| {
        let r = &sample.rates[i];
        r.read_bytes.unwrap_or(0.0) + r.write_bytes.unwrap_or(0.0)
    };
    match o.sort {
        Some(Sort::Cpu) => {
            rows.sort_by(|a, b| sample.rates[b.0].cpu.total_cmp(&sample.rates[a.0].cpu));
        }
        Some(Sort::Memory) => {
            rows.sort_by_key(|r| std::cmp::Reverse(s.processes[r.0].counters.private));
        }
        Some(Sort::Name) => rows.sort_by_cached_key(|r| s.processes[r.0].name.to_lowercase()),
        Some(Sort::Io) => rows.sort_by(|a, b| io(b.0).total_cmp(&io(a.0))),
        Some(Sort::Pid) | None => {}
    }
    if let Some(n) = o.limit {
        rows.truncate(n);
    }
    rows
}

fn list_json(sample: &Sample, rows: &[(usize, usize)]) -> Value {
    let s = &sample.snapshot;
    let processes: Vec<Value> = rows
        .iter()
        .map(|&(i, depth)| {
            let p = &s.processes[i];
            let r = &sample.rates[i];
            let c = &p.counters;
            json!({
                "pid": p.key.pid,
                "start": p.key.start,
                "key": p.key.to_string(),
                "parentPid": p.parent_pid,
                "parent": s.parent(i).map(|j| s.processes[j].key.to_string()),
                "depth": depth,
                "name": p.name,
                "state": p.state.name(),
                "cpu": r.cpu,
                "private": c.private,
                "resident": c.resident,
                "virtual": c.virtual_size,
                "threads": c.threads,
                "handles": c.handles,
                "readRate": r.read_bytes,
                "writeRate": r.write_bytes,
                "userTime": c.user_ns,
                "kernelTime": c.kernel_ns,
                "priority": c.priority,
                "path": p.details.path,
                "commandLine": p.details.command_line,
                "user": p.details.user,
                "architecture": p.details.architecture,
                "elevated": p.details.elevated,
                "integrity": p.details.integrity,
                "kernel": p.details.kernel,
            })
        })
        .collect();
    json!({
        "cpus": s.system.cpus,
        "cpu": sample.cpu,
        "memoryTotal": s.system.memory_total,
        "memoryAvailable": s.system.memory_available,
        "processes": processes,
    })
}

fn list_text(sample: &Sample, rows: &[(usize, usize)]) -> String {
    let s = &sample.snapshot;
    let mut out = format!(
        "CPU {:.1}% of {} processors; memory {} of {} in use; {} processes\n\n",
        sample.cpu * 100.0,
        s.system.cpus,
        bytes(
            s.system
                .memory_total
                .saturating_sub(s.system.memory_available)
        ),
        bytes(s.system.memory_total),
        s.processes.len()
    );
    out.push_str(&format!(
        "{:>7} {:>7} {:>6} {:>10} {:>10} {:>5} {:>6}  {:<24} {}\n",
        "PID", "PPID", "CPU%", "PRIVATE", "WORKING", "THR", "HNDL", "USER", "NAME"
    ));
    for &(i, depth) in rows {
        let p = &s.processes[i];
        let c = &p.counters;
        let user = p.details.user.as_deref().unwrap_or("");
        let user = user.rsplit('\\').next().unwrap_or(user);
        out.push_str(&format!(
            "{:>7} {:>7} {:>6.2} {:>10} {:>10} {:>5} {:>6}  {:<24} {}{}\n",
            p.key.pid,
            p.parent_pid.map_or_else(String::new, |p| p.to_string()),
            sample.rates[i].cpu * 100.0,
            bytes(c.private),
            bytes(c.resident),
            c.threads,
            c.handles.map_or_else(String::new, |h| h.to_string()),
            truncate(user, 24),
            "  ".repeat(depth),
            p.name,
        ));
    }
    out
}

fn truncate(s: &str, n: usize) -> String {
    if s.chars().count() <= n {
        return s.to_owned();
    }
    let mut t: String = s.chars().take(n - 1).collect();
    t.push('…');
    t
}

/// A size, in binary units.
pub fn bytes(n: u64) -> String {
    const UNITS: [&str; 7] = ["B", "KiB", "MiB", "GiB", "TiB", "PiB", "EiB"];
    let mut v = n as f64;
    let mut unit = 0;
    while v >= 1024.0 && unit + 1 < UNITS.len() {
        v /= 1024.0;
        unit += 1;
    }
    if unit == 0 {
        format!("{n} B")
    } else {
        format!("{v:.1} {}", UNITS[unit])
    }
}

/// The processes `args` name: `PID` (the process with that ID now) or
/// `PID@START`.
fn keys(
    args: &[String],
    snapshot: impl FnOnce() -> std::io::Result<Snapshot>,
) -> Result<Vec<Key>, String> {
    if args.is_empty() {
        return Err("no processes".into());
    }
    let mut snap = Some(snapshot);
    let mut taken: Option<Snapshot> = None;
    let mut keys = Vec::new();
    for a in args {
        let (pid, start) = match a.split_once('@') {
            Some((p, s)) => (
                p,
                Some(
                    s.parse::<u64>()
                        .map_err(|_| format!("{a}: not PID@START"))?,
                ),
            ),
            None => (a.as_str(), None),
        };
        let pid: u32 = pid.parse().map_err(|_| format!("{a}: not a process ID"))?;
        if let Some(start) = start {
            keys.push(Key { pid, start });
            continue;
        }
        if taken.is_none() {
            let f = snap.take().ok_or("no snapshot")?;
            taken = Some(f().map_err(|e| e.to_string())?);
        }
        let s = taken.as_ref().ok_or("no snapshot")?;
        let i = s.find_pid(pid).ok_or(format!("no process {pid}"))?;
        keys.push(s.processes[i].key);
    }
    Ok(keys)
}

fn act(args: &[String], f: fn(Key) -> std::io::Result<()>, verb: &str) -> ExitCode {
    let keys = match keys(args, || turbonop_process::Collector::new()?.snapshot()) {
        Ok(k) => k,
        Err(e) => {
            eprintln!("procupine {verb}: {e}");
            return ExitCode::from(2);
        }
    };
    let mut failed = false;
    for key in keys {
        if let Err(e) = f(key) {
            eprintln!("procupine {verb}: {}: {e}", key.pid);
            failed = true;
        }
    }
    if failed {
        ExitCode::FAILURE
    } else {
        ExitCode::SUCCESS
    }
}

#[cfg(test)]
mod tests;
