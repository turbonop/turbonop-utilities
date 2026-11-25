use std::time::{Duration, Instant};

use super::*;

pub(crate) fn process(pid: u32, start: u64, parent: Option<u32>) -> Process {
    Process {
        key: Key { pid, start },
        parent_pid: parent,
        name: format!("p{pid}"),
        state: State::Sleeping,
        counters: Counters::default(),
        details: Arc::default(),
    }
}

pub(crate) fn snapshot(taken: Instant, time: u64, mut processes: Vec<Process>) -> Snapshot {
    processes.sort_by_key(|p| p.key.pid);
    Snapshot {
        taken,
        time,
        system: System {
            cpus: 4,
            ..System::default()
        },
        processes,
    }
}

#[test]
fn finds_processes_by_key() {
    let s = snapshot(
        Instant::now(),
        1000,
        vec![
            process(10, 5, None),
            process(3, 1, None),
            process(7, 2, Some(3)),
        ],
    );
    assert_eq!(s.find(Key { pid: 7, start: 2 }), Some(1));
    // (The same ID, another start: another process.)
    assert_eq!(s.find(Key { pid: 7, start: 3 }), None);
    assert_eq!(s.find_pid(10), Some(2));
    assert_eq!(s.find_pid(11), None);
}

#[test]
fn parents_started_first() {
    let s = snapshot(
        Instant::now(),
        1000,
        vec![
            process(1, 10, None),
            process(2, 20, Some(1)),
            // Its parent exited, and its ID is a later process's.
            process(3, 30, Some(4)),
            process(4, 40, Some(1)),
            // Its own parent.
            process(5, 50, Some(5)),
            // Unknown starts: believed.
            process(6, 0, Some(1)),
        ],
    );
    assert_eq!(s.parent(0), None);
    assert_eq!(s.parent(1), Some(0));
    assert_eq!(s.parent(2), None);
    assert_eq!(s.parent(3), Some(0));
    assert_eq!(s.parent(4), None);
    assert_eq!(s.parent(5), Some(0));
}

#[test]
fn tree() {
    let s = snapshot(
        Instant::now(),
        1000,
        vec![
            process(1, 10, None),
            process(9, 15, Some(1)),
            process(2, 20, Some(1)),
            process(3, 30, Some(2)),
            process(50, 5, None),
        ],
    );
    let t = s.tree();
    let pid = |i: usize| s.processes[i].key.pid;
    assert_eq!(t.roots.iter().map(|&i| pid(i)).collect::<Vec<_>>(), [50, 1]);
    let order: Vec<(u32, usize)> = t
        .depth_first()
        .into_iter()
        .map(|(i, d)| (pid(i), d))
        .collect();
    assert_eq!(order, [(50, 0), (1, 0), (9, 1), (2, 1), (3, 2)]);
    let one = s.find_pid(1).unwrap();
    assert_eq!(
        t.descendants(one).into_iter().map(pid).collect::<Vec<_>>(),
        [1, 9, 2, 3]
    );
    assert_eq!(t.parent[s.find_pid(3).unwrap()], s.find_pid(2));
}

#[test]
fn tree_of_a_loop() {
    // Two processes each the other's parent (unknown starts): both roots.
    let s = snapshot(
        Instant::now(),
        0,
        vec![
            process(1, 0, Some(2)),
            process(2, 0, Some(1)),
            process(3, 0, Some(1)),
        ],
    );
    let t = s.tree();
    assert_eq!(t.depth_first().len(), 3);
    assert!(t.roots.contains(&0) || t.roots.contains(&1));
    assert_eq!(t.parent[2], Some(0));
}

fn with_counters(mut p: Process, f: impl FnOnce(&mut Counters)) -> Process {
    f(&mut p.counters);
    p
}

#[test]
fn rates_by_time() {
    let t0 = Instant::now();
    let t1 = t0 + Duration::from_secs(2);
    let base = 1_000_000_000_000;
    let a0 = with_counters(process(1, 10, None), |c| {
        c.cpu_ns = 1_000_000_000;
        c.read_bytes = Some(100);
        c.write_bytes = Some(0);
        c.read_ops = Some(1);
        c.write_ops = Some(1);
        c.page_faults = 10;
    });
    let gone = process(2, 20, None);
    let mut s0 = snapshot(t0, base, vec![a0, gone]);
    s0.system.cpu_total_ns = 0;
    s0.system.cpu_idle_ns = 0;
    let a1 = with_counters(process(1, 10, None), |c| {
        c.cpu_ns = 3_000_000_000;
        c.read_bytes = Some(2100);
        c.write_bytes = Some(400);
        c.read_ops = Some(5);
        c.write_ops = Some(3);
        c.page_faults = 30;
    });
    // Started since: all of its time counts.
    let new = with_counters(process(3, base + 1, Some(1)), |c| c.cpu_ns = 800_000_000);
    // Not seen before, but older: none of it (it wasn't read last time).
    let missed = with_counters(process(4, 1, None), |c| c.cpu_ns = 5_000_000_000);
    let mut s1 = snapshot(t1, base + 2_000_000_000, vec![a1, new, missed]);
    s1.system.cpu_total_ns = 8_000_000_000;
    s1.system.cpu_idle_ns = 6_000_000_000;
    let sample = compare(Some(&s0), s1);
    assert_eq!(sample.interval_ns, 2_000_000_000);
    // 2 s of CPU in 2 s of 4 processors: a quarter.
    let r = sample.rates[0];
    assert!((r.cpu - 0.25).abs() < 1e-9, "{r:?}");
    assert_eq!(r.read_bytes, Some(1000.0));
    assert_eq!(r.write_bytes, Some(200.0));
    assert_eq!(r.io_ops, Some(3.0));
    assert_eq!(r.page_faults, 10.0);
    assert_eq!(r.context_switches, None);
    assert!((sample.rates[1].cpu - 0.1).abs() < 1e-9);
    assert_eq!(sample.rates[2].cpu, 0.0);
    assert!((sample.cpu - 0.25).abs() < 1e-9);
    assert_eq!(sample.started, [1, 2]);
    assert_eq!(
        sample.exited.iter().map(|p| p.key.pid).collect::<Vec<_>>(),
        [2]
    );
}

#[test]
fn rates_by_cycles() {
    let t0 = Instant::now();
    let t1 = t0 + Duration::from_secs(1);
    let idle = |cycles| with_counters(process(0, 0, None), |c| c.cycles = Some(cycles));
    let busy = |cycles| with_counters(process(8, 5, None), |c| c.cycles = Some(cycles));
    let mut s0 = snapshot(t0, 100, vec![idle(999), busy(1000)]);
    s0.system.idle_cycles = Some(10_000);
    let mut s1 = snapshot(t1, 1_000_000_100, vec![idle(5555), busy(4000)]);
    s1.system.idle_cycles = Some(19_000);
    let sample = compare(Some(&s0), s1);
    // 3000 busy cycles and 9000 idle: a quarter busy. (The Idle process's
    // own counter isn't used: the processors' idle cycles are.)
    assert!((sample.rates[1].cpu - 0.25).abs() < 1e-9);
    assert!((sample.rates[0].cpu - 0.75).abs() < 1e-9);
    assert!((sample.cpu - 0.25).abs() < 1e-9);
}

#[test]
fn first_sample() {
    let s = snapshot(Instant::now(), 0, vec![process(1, 0, None)]);
    let sample = compare(None, s);
    assert_eq!(sample.rates, [Rates::default()]);
    assert!(sample.started.is_empty() && sample.exited.is_empty());
    assert_eq!(sample.interval_ns, 0);
}

#[test]
fn command_lines() {
    let a = |v: &[&str]| join_args(&v.iter().map(|s| (*s).to_owned()).collect::<Vec<_>>());
    assert_eq!(a(&["/bin/ls", "-la", "/tmp"]), "/bin/ls -la /tmp");
    assert_eq!(a(&["echo", "a b", ""]), "echo 'a b' ''");
    assert_eq!(a(&["sh", "-c", "it's"]), r"sh -c 'it'\''s'");
    assert_eq!(a(&["k=v", "x$y"]), "k=v 'x$y'");
    assert_eq!(a(&[]), "");
    assert_eq!(file_name("/usr/bin/bash"), "bash");
    assert_eq!(file_name(r"C:\Windows\explorer.exe"), "explorer.exe");
    assert_eq!(file_name("init"), "init");
}

#[test]
fn keys_and_states() {
    assert_eq!(Key { pid: 4, start: 99 }.to_string(), "4@99");
    assert_eq!(State::DiskSleep.name(), "disk sleep");
    assert_eq!(State::default(), State::Sleeping);
}
