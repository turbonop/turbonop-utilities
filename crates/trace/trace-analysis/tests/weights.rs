//! Samples' weights, as WPA gives them.

use turbonop_trace::{Clock, Process, ProcessIx, StackId, Thread, ThreadIx, Trace};
use turbonop_trace_analysis::weights::sample_weights_ns;

/// A trace with a clock of 1 ns ticks, a 1 ms interval, and samples at
/// `samples` (processor, time in ns).
fn trace(samples: &[(u16, i64)]) -> Trace {
    let mut t = Trace::new(
        Clock {
            ticks_per_second: 1_000_000_000,
            origin: 0,
        },
        4,
    );
    t.sample_interval = Some(1_000_000);
    t.processes.push(Process {
        pid: 10,
        name: "app.exe".into(),
        ..Process::default()
    });
    t.threads.push(Thread {
        tid: 1,
        process: ProcessIx(0),
        name: None,
        start_address: 0,
        start: None,
        end: None,
    });
    for &(cpu, time) in samples {
        let s = &mut t.samples;
        s.time.push(time);
        s.cpu.push(cpu);
        s.thread.push(ThreadIx(0));
        s.ip.push(0);
        s.stack.push(StackId::EMPTY);
        s.weight.push(1);
    }
    t
}

#[test]
fn a_sample_weighs_the_gap_back_to_its_processors_last() {
    // Processor 0: first (an interval), then gaps of 0.9 ms, 1.1 ms (the
    // limit: still the gap), 1.2 ms and 5 ms (too long: an interval).
    // Processor 1, interleaved: first, then 0.5 ms.
    let t = trace(&[
        (0, 0),
        (1, 100_000),
        (0, 900_000),
        (1, 600_000),
        (0, 2_000_000),
        (0, 3_200_000),
        (0, 8_200_000),
    ]);
    assert_eq!(
        sample_weights_ns(&t),
        [
            1_000_000, 1_000_000, 900_000, 500_000, 1_100_000, 1_000_000, 1_000_000
        ]
    );
}

#[test]
fn weights_are_the_same_however_the_work_is_split() {
    // More samples than a worker takes at a time, on processors in turn,
    // with gaps that cross the limit now and then.
    let mut samples = Vec::new();
    let mut at = [0i64; 4];
    for i in 0..200_000u32 {
        let cpu = (i % 4) as u16;
        let gap = [950_000, 1_050_000, 1_150_000, 30_000_000][(i as usize / 4) % 4];
        at[usize::from(cpu)] += gap;
        samples.push((cpu, at[usize::from(cpu)]));
    }
    samples.sort_by_key(|s| s.1);
    let t = trace(&samples);
    let w = sample_weights_ns(&t);
    // Worked out one by one, in order.
    let mut last = [None; 4];
    let expected: Vec<u64> = samples
        .iter()
        .map(|&(cpu, time)| {
            let gap = last[usize::from(cpu)].map(|l| time - l);
            last[usize::from(cpu)] = Some(time);
            match gap {
                Some(g) if g <= 1_100_000 => g as u64,
                _ => 1_000_000,
            }
        })
        .collect();
    assert_eq!(w, expected);
    assert!(w.contains(&950_000) && w.contains(&1_050_000));
}

#[test]
fn no_samples_no_weights() {
    assert!(sample_weights_ns(&trace(&[])).is_empty());
}

// (Tests allocate with mimalloc, as the programs do.)
#[cfg(not(target_family = "wasm"))]
#[global_allocator]
static TEST_ALLOCATOR: mimalloc::MiMalloc = mimalloc::MiMalloc;
