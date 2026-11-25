//! What each CPU sample weighs: the time it stands for, as WPA's CPU Usage
//! (Sampled) table gives it in its Weight column.
//!
//! A sample stands for the time since the previous sample on its
//! processor, if that is at most 1.1 sampling intervals; if longer (the
//! processor stopped its timer, idle), or if it is the processor's first,
//! for one interval. On the trace corpus this gives every process's
//! weight as WPA does, to the 0.1 µs it prints, except the idle process's:
//! WPA counts time no sample covers as idle too, over a span not
//! recovered here.

use turbonop_trace::Trace;

/// The gap, in intervals, beyond which a sample stands for one interval
/// only.
pub const GAP_LIMIT: f64 = 1.1;

/// Samples a worker takes at a time.
const CHUNK: usize = 1 << 16;

/// Each of `trace`'s samples' weight, in nanoseconds (in the samples'
/// order), worked out on every core.
pub fn sample_weights_ns(trace: &Trace) -> Vec<u64> {
    let s = &trace.samples;
    let n = s.len();
    let interval = trace.sample_interval_ns();
    // (1.1 intervals, rounded down to the nanosecond: what WPA keeps.)
    #[allow(
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss,
        clippy::cast_precision_loss
    )]
    let limit = (interval as f64 * GAP_LIMIT) as i64;
    let processors = s.cpu.iter().map(|&c| usize::from(c) + 1).max().unwrap_or(0);
    let chunks = n.div_ceil(CHUNK);
    // Each chunk's last sample's time on each processor (or none)...
    let lasts: Vec<Vec<Option<i64>>> = turbonop_par::map_range(chunks, 1, |c| {
        let mut last = vec![None; processors];
        for i in c * CHUNK..((c + 1) * CHUNK).min(n) {
            last[usize::from(s.cpu[i])] = Some(s.time[i]);
        }
        last
    });
    // ...so each chunk knows the last before it.
    let mut before = Vec::with_capacity(chunks);
    let mut running = vec![None; processors];
    for last in &lasts {
        before.push(running.clone());
        for (r, l) in running.iter_mut().zip(last) {
            if l.is_some() {
                *r = *l;
            }
        }
    }
    let parts: Vec<Vec<u64>> = turbonop_par::map_range(chunks, 1, |c| {
        let mut last = before[c].clone();
        (c * CHUNK..((c + 1) * CHUNK).min(n))
            .map(|i| {
                let cpu = usize::from(s.cpu[i]);
                let weight = match last[cpu] {
                    Some(t) => {
                        let gap = trace.clock.nanos(s.time[i]) - trace.clock.nanos(t);
                        if (0..=limit).contains(&gap) {
                            u64::try_from(gap).unwrap_or(interval)
                        } else {
                            interval
                        }
                    }
                    None => interval,
                };
                last[cpu] = Some(s.time[i]);
                weight
            })
            .collect()
    });
    parts.concat()
}
