//! `procupine serve`: samples of this machine's processes, for an editor.
//!
//! Requests come on standard input and responses and events go to standard
//! output, one JSON object per line. A request names a method:
//!
//! ```text
//! {"id": 1, "method": "watch", "interval": 1000}
//! ```
//!
//! and is answered with its result, `{"id": 1, "result": ...}`, or
//! `{"id": 1, "error": "..."}`. The methods:
//!
//! - `watch` (`interval`, in milliseconds; 1000 if not given): samples
//!   are sent as events, one each interval, until `pause`. Given again,
//!   it changes the interval.
//! - `pause`: no more samples until `watch`.
//! - `refresh`: a sample now (as an event), with every process's details
//!   and row (as a client that has lost track would need).
//! - `kill`, `terminate`, `suspend`, `resume` (`processes`: session IDs):
//!   each process acted on; the result has each one's `ok` or `error`.
//! - `columns`: the rows' columns.
//!
//! A sample event is `{"event": "sample", "time", "interval", "system",
//! "added", "removed", "rows"}` (see `wire`): the processes that started,
//! with their details; the IDs of those that exited; and the rows that
//! changed. The first is everything. At the end of input, it exits.

use std::io::{self, BufRead, Write};
use std::process::ExitCode;
use std::sync::{Arc, Condvar, Mutex, PoisonError};
use std::time::{Duration, Instant};

use serde_json::{Value, json};
use turbonop_process::{Key, Monitor};

use crate::wire::{COLUMNS, Session};

pub const USAGE: &str = "\
usage: procupine serve

Samples this machine's processes for an editor. Requests on standard input, one JSON object per
line: {\"id\": 1, \"method\": \"watch\", \"interval\": 1000}. Responses ({\"id\": 1, \"result\": ...}) and
sample events ({\"event\": \"sample\", ...}) on standard output, one per line. Methods: watch, pause,
refresh, kill, terminate, suspend, resume, columns.";

/// What the sampler is asked to do.
struct Control {
    interval: Option<Duration>,
    /// A sample now, with everything.
    refresh: bool,
    done: bool,
}

struct Shared {
    control: Mutex<Control>,
    wake: Condvar,
    session: Mutex<Session>,
}

type Out = Arc<Mutex<dyn Write + Send>>;

fn send(out: &Out, v: &Value) {
    let mut line = serde_json::to_vec(v).unwrap_or_default();
    line.push(b'\n');
    let mut out = out.lock().unwrap_or_else(PoisonError::into_inner);
    // (The editor gone: the end of input ends this too.)
    let _ = out.write_all(&line).and_then(|()| out.flush());
}

pub fn run(args: &[String]) -> ExitCode {
    if !args.is_empty() {
        eprintln!("{USAGE}");
        return ExitCode::from(2);
    }
    let out: Out = Arc::new(Mutex::new(io::stdout()));
    match serve(io::stdin().lock(), &out) {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("procupine serve: {e}");
            ExitCode::FAILURE
        }
    }
}

/// Answers the requests read from `input` on `out`, sampling as asked.
pub fn serve(input: impl BufRead, out: &Out) -> io::Result<()> {
    let monitor = Monitor::new()?;
    let shared = Arc::new(Shared {
        control: Mutex::new(Control {
            interval: None,
            refresh: false,
            done: false,
        }),
        wake: Condvar::new(),
        session: Mutex::new(Session::default()),
    });
    let sampler = {
        let shared = Arc::clone(&shared);
        let out = Arc::clone(out);
        std::thread::spawn(move || sample(monitor, &shared, &out))
    };
    for line in input.lines() {
        let line = line?;
        if line.trim().is_empty() {
            continue;
        }
        let response = match serde_json::from_str::<Value>(&line) {
            Ok(request) => {
                let id = request.get("id").cloned().unwrap_or(Value::Null);
                match answer(&request, &shared) {
                    Ok(result) => json!({ "id": id, "result": result }),
                    Err(e) => json!({ "id": id, "error": e }),
                }
            }
            Err(e) => json!({ "id": null, "error": format!("not JSON: {e}") }),
        };
        send(out, &response);
    }
    shared
        .control
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .done = true;
    shared.wake.notify_all();
    sampler
        .join()
        .map_err(|_| io::Error::other("the sampler panicked"))?
}

fn answer(request: &Value, shared: &Shared) -> Result<Value, String> {
    let method = request
        .get("method")
        .and_then(Value::as_str)
        .ok_or("no method")?;
    let control = || {
        shared
            .control
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
    };
    match method {
        "watch" => {
            let ms = request
                .get("interval")
                .and_then(Value::as_u64)
                .unwrap_or(1000)
                .clamp(50, 3_600_000);
            control().interval = Some(Duration::from_millis(ms));
            shared.wake.notify_all();
            Ok(json!({ "interval": ms }))
        }
        "pause" => {
            control().interval = None;
            shared.wake.notify_all();
            Ok(json!(true))
        }
        "refresh" => {
            control().refresh = true;
            shared.wake.notify_all();
            Ok(json!(true))
        }
        "columns" => Ok(json!(COLUMNS)),
        "kill" | "terminate" | "suspend" | "resume" => {
            let ids = request
                .get("processes")
                .and_then(Value::as_array)
                .ok_or("no processes")?;
            let act: fn(Key) -> io::Result<()> = match method {
                "kill" => turbonop_process::kill,
                "terminate" => turbonop_process::terminate,
                "suspend" => turbonop_process::suspend,
                _ => turbonop_process::resume,
            };
            let results: Vec<Value> = ids
                .iter()
                .map(|id| {
                    let key = id.as_u64().and_then(|id| {
                        shared
                            .session
                            .lock()
                            .unwrap_or_else(PoisonError::into_inner)
                            .key(id)
                    });
                    match key.map(act) {
                        Some(Ok(())) => json!({ "process": id, "ok": true }),
                        Some(Err(e)) => json!({ "process": id, "error": e.to_string() }),
                        None => {
                            json!({ "process": id, "error": "no such process (it has exited)" })
                        }
                    }
                })
                .collect();
            // (Its new state, soon.)
            control().refresh = true;
            shared.wake.notify_all();
            Ok(json!(results))
        }
        _ => Err(format!("no method {method}")),
    }
}

/// Samples, as `shared.control` says, until it says it's done.
fn sample(mut monitor: Monitor, shared: &Shared, out: &Out) -> io::Result<()> {
    let mut next = Instant::now();
    loop {
        let all = {
            let mut c = shared
                .control
                .lock()
                .unwrap_or_else(PoisonError::into_inner);
            loop {
                if c.done {
                    return Ok(());
                }
                if c.refresh {
                    c.refresh = false;
                    break true;
                }
                match c.interval {
                    None => c = shared.wake.wait(c).unwrap_or_else(PoisonError::into_inner),
                    Some(interval) => {
                        let now = Instant::now();
                        // (Changed to a shorter interval: from now.)
                        next = next.min(now + interval);
                        if now >= next {
                            break false;
                        }
                        c = shared
                            .wake
                            .wait_timeout(c, next - now)
                            .unwrap_or_else(PoisonError::into_inner)
                            .0;
                    }
                }
            }
        };
        let sample = match monitor.sample() {
            Ok(s) => s,
            Err(e) => {
                send(out, &json!({ "event": "error", "error": e.to_string() }));
                std::thread::sleep(Duration::from_secs(1));
                continue;
            }
        };
        let event = shared
            .session
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .event(&sample, all);
        send(out, &event);
        let interval = shared
            .control
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .interval;
        if let Some(interval) = interval {
            // (Every interval from the last, not from when this one ended.)
            next = (next + interval).max(Instant::now());
        }
    }
}
