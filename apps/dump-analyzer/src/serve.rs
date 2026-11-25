//! `dump-analyzer serve`: commands for an editor, from one process that
//! keeps each dump it opens.
//!
//! Requests come on standard input and answers go to standard output, one
//! JSON object per line. A request is a command line:
//!
//! ```text
//! {"id": 1, "args": ["stack", "crash.dmp", "-thread", "4"]}
//! ```
//!
//! and its answer is the command's JSON, `{"id": 1, "json": ...}`, or
//! `{"id": 1, "error": "..."}`. Requests run at once, each on its own
//! thread, and are answered as they finish (in any order). A dump is
//! opened once, by the first request on it; the rest share it.

use std::io::{self, BufRead, Write};
use std::process::ExitCode;
use std::sync::{Arc, Mutex};

use serde_json::{Value, json};
use turbonop_target::load::Hints;

/// The stack each request runs on.
const STACK: usize = 16 << 20;

pub fn run(hints: &Hints) -> ExitCode {
    let out = Arc::new(Mutex::new(io::stdout()));
    let mut running = Vec::new();
    for line in io::stdin().lock().lines() {
        let Ok(line) = line else { break };
        if line.trim().is_empty() {
            continue;
        }
        let out = Arc::clone(&out);
        let hints = hints.clone();
        let t = std::thread::Builder::new()
            .name("dump-analyzer serve".into())
            .stack_size(STACK)
            .spawn(move || {
                let answer = answer(&line, &hints);
                let mut o = out
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner);
                let _ = writeln!(o, "{answer}");
                let _ = o.flush();
            });
        if let Ok(t) = t {
            running.push(t);
        }
        running.retain(|t| !t.is_finished());
    }
    for t in running {
        let _ = t.join();
    }
    ExitCode::SUCCESS
}

/// The answer to a request line.
pub fn answer(line: &str, hints: &Hints) -> Value {
    let v: Value = match serde_json::from_str(line) {
        Ok(v) => v,
        Err(e) => return json!({ "id": null, "error": format!("not JSON: {e}") }),
    };
    let id = v.get("id").cloned().unwrap_or(Value::Null);
    let Some(args) = v.get("args").and_then(Value::as_array).and_then(|a| {
        a.iter()
            .map(|x| x.as_str().map(str::to_owned))
            .collect::<Option<Vec<_>>>()
    }) else {
        return json!({ "id": id, "error": "`args` must be an array of strings" });
    };
    if args.first().map(String::as_str) == Some("serve") {
        return json!({ "id": id, "error": "'serve' is not a command to serve" });
    }
    match std::panic::catch_unwind(|| crate::command(&args, hints)) {
        Ok(Ok(j)) => json!({ "id": id, "json": j }),
        Ok(Err(e)) => json!({ "id": id, "error": e }),
        Err(_) => json!({ "id": id, "error": "internal error (a bug: please report it)" }),
    }
}
