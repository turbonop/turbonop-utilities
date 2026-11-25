//! This machine's processes, as the collector sees them: this test's own
//! process, and a child it starts, kills, suspends, and resumes.

use turbonop_global_alloc as _;

use std::process::{Child, Command};
use std::time::{Duration, Instant};

use turbonop_process::{Collector, Key, Monitor, Snapshot, State};

/// A child process to act on: this test binary, running `sleeper`.
fn spawn_sleeper() -> Child {
    Command::new(std::env::current_exe().unwrap())
        .args(["--exact", "sleeper", "--ignored", "--nocapture"])
        .spawn()
        .unwrap()
}

/// Run as a child only (`--ignored`).
#[test]
#[ignore = "a child process the other tests start"]
fn sleeper() {
    std::thread::sleep(Duration::from_secs(60));
}

fn key_of(s: &Snapshot, pid: u32) -> Key {
    s.processes[s
        .find_pid(pid)
        .unwrap_or_else(|| panic!("process {pid} not found"))]
    .key
}

/// Waits (up to 10 s) until a snapshot satisfies `f`.
fn until(c: &mut Collector, f: impl Fn(&Snapshot) -> bool) -> Snapshot {
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        let s = c.snapshot().unwrap();
        if f(&s) || Instant::now() > deadline {
            return s;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
}

#[test]
fn this_process() {
    let mut c = Collector::new().unwrap();
    let s = c.snapshot().unwrap();
    assert!(s.processes.len() > 2, "{} processes", s.processes.len());
    assert!(s.processes.windows(2).all(|w| w[0].key.pid < w[1].key.pid));
    let me = &s.processes[s.find_pid(std::process::id()).unwrap()];
    // Started in the last hour, as far as the clock says.
    assert!(
        me.key.start > 0 && me.key.start <= s.time + 2_000_000_000,
        "{:?} at {}",
        me.key,
        s.time
    );
    assert!(s.time - me.key.start.min(s.time) < 3600 * 1_000_000_000);
    let exe = std::env::current_exe().unwrap();
    let path = me.details.path.as_deref().unwrap();
    assert_eq!(
        std::fs::canonicalize(path).unwrap(),
        std::fs::canonicalize(&exe).unwrap()
    );
    let stem = exe.file_stem().unwrap().to_string_lossy().into_owned();
    assert!(
        me.name.starts_with(&stem[..stem.len().min(15)]),
        "{} for {stem}",
        me.name
    );
    assert!(me.details.command_line.as_deref().unwrap().contains(&stem));
    assert!(me.details.user.as_deref().is_some_and(|u| !u.is_empty()));
    assert!(me.counters.threads >= 1);
    assert!(me.counters.resident > 0, "{:?}", me.counters);
    assert!(me.counters.private > 0, "{:?}", me.counters);
    assert!(me.counters.virtual_size >= me.counters.resident);
    assert!(me.counters.handles.is_some_and(|h| h > 0));
    // Its parent is the process that ran it, and started first.
    let parent = s.parent(s.find_pid(me.key.pid).unwrap());
    if let Some(p) = parent {
        assert!(s.processes[p].key.start <= me.key.start);
    }
    let sys = s.system;
    assert!(sys.cpus >= 1);
    assert!(
        sys.memory_total > 0 && sys.memory_available <= sys.memory_total,
        "{sys:?}"
    );
    assert!(sys.cpu_total_ns >= sys.cpu_idle_ns);
    assert!(sys.uptime_ns > 0);
    // The tree holds every process, once.
    let t = s.tree();
    let mut seen: Vec<usize> = t.depth_first().into_iter().map(|(i, _)| i).collect();
    seen.sort_unstable();
    assert_eq!(seen, (0..s.processes.len()).collect::<Vec<_>>());
}

#[test]
fn details_are_read_once() {
    let mut c = Collector::new().unwrap();
    let a = c.snapshot().unwrap();
    let b = c.snapshot().unwrap();
    let pid = std::process::id();
    let (pa, pb) = (
        &a.processes[a.find_pid(pid).unwrap()],
        &b.processes[b.find_pid(pid).unwrap()],
    );
    assert_eq!(pa.key, pb.key);
    assert!(std::sync::Arc::ptr_eq(&pa.details, &pb.details));
}

#[test]
fn cpu_usage() {
    let mut m = Monitor::new().unwrap();
    m.sample().unwrap();
    // A quarter-second on a CPU, at least.
    let start = Instant::now();
    let mut x = 0u64;
    while start.elapsed() < Duration::from_millis(300) {
        x = std::hint::black_box(x.wrapping_mul(31).wrapping_add(7));
    }
    let s = m.sample().unwrap();
    let i = s.snapshot.find_pid(std::process::id()).unwrap();
    let cpus = f64::from(s.snapshot.system.cpus);
    // (Its share of every processor: one busy thread is 1/cpus of them,
    // less what the rest of this interval spent idle.)
    assert!(
        s.rates[i].cpu > 0.2 / cpus,
        "{:?} of {cpus} processors",
        s.rates[i]
    );
    assert!(s.rates[i].cpu <= 1.0);
    assert!(s.cpu > 0.0 && s.cpu <= 1.0, "{}", s.cpu);
    assert!(s.interval_ns >= 300_000_000);
}

#[test]
fn kill() {
    let mut child = spawn_sleeper();
    let mut c = Collector::new().unwrap();
    let s = until(&mut c, |s| s.find_pid(child.id()).is_some());
    let key = key_of(&s, child.id());
    // A key with another start: not this process.
    let other = Key {
        pid: key.pid,
        start: key.start + 1,
    };
    assert_eq!(
        turbonop_process::kill(other).unwrap_err().kind(),
        std::io::ErrorKind::NotFound
    );
    turbonop_process::kill(key).unwrap();
    let status = child.wait().unwrap();
    assert!(!status.success());
    // Exited: not found.
    let e = turbonop_process::kill(key).unwrap_err();
    assert_eq!(e.kind(), std::io::ErrorKind::NotFound, "{e}");
}

#[test]
fn suspend_and_resume() {
    let mut child = spawn_sleeper();
    let mut c = Collector::new().unwrap();
    let s = until(&mut c, |s| s.find_pid(child.id()).is_some());
    let key = key_of(&s, child.id());
    turbonop_process::suspend(key).unwrap();
    let pid = child.id();
    let state = |s: &Snapshot| s.find(key).map(|i| s.processes[i].state);
    let s = until(&mut c, |s| {
        matches!(state(s), Some(State::Suspended | State::Stopped))
    });
    assert!(
        matches!(state(&s), Some(State::Suspended | State::Stopped)),
        "{:?}",
        state(&s)
    );
    turbonop_process::resume(key).unwrap();
    let s = until(&mut c, |s| {
        !matches!(state(s), Some(State::Suspended | State::Stopped))
    });
    assert!(
        !matches!(state(&s), Some(State::Suspended | State::Stopped)),
        "{:?}",
        state(&s)
    );
    turbonop_process::kill(key).unwrap();
    child.wait().unwrap();
    let s = until(&mut c, |s| s.find(key).is_none());
    assert!(s.find(key).is_none() || s.processes[s.find_pid(pid).unwrap()].state == State::Zombie);
}

#[test]
fn started_and_exited() {
    let mut m = Monitor::new().unwrap();
    m.sample().unwrap();
    let mut child = spawn_sleeper();
    let pid = child.id();
    let deadline = Instant::now() + Duration::from_secs(10);
    let key = loop {
        let s = m.sample().unwrap();
        if let Some(i) = s.snapshot.find_pid(pid) {
            assert!(s.started.contains(&i));
            break s.snapshot.processes[i].key;
        }
        assert!(Instant::now() < deadline, "the child never showed");
    };
    turbonop_process::kill(key).unwrap();
    child.wait().unwrap();
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        let s = m.sample().unwrap();
        if s.exited.iter().any(|p| p.key == key) {
            break;
        }
        assert!(Instant::now() < deadline, "the child never exited");
        std::thread::sleep(Duration::from_millis(20));
    }
}
