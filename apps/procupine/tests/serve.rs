//! `procupine serve`, as an editor runs it: samples watched, a process
//! killed by its session ID, the end of input.

use turbonop_global_alloc as _;

use std::io::{BufRead, BufReader, Write};
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::mpsc::{Receiver, channel};
use std::time::Duration;

use serde_json::{Value, json};

struct Server {
    child: Child,
    input: Option<ChildStdin>,
    lines: Receiver<Value>,
}

impl Server {
    fn start() -> Server {
        let mut child = Command::new(env!("CARGO_BIN_EXE_procupine"))
            .arg("serve")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .spawn()
            .unwrap();
        let out = child.stdout.take().unwrap();
        let (tx, lines) = channel();
        std::thread::spawn(move || {
            for line in BufReader::new(out).lines() {
                let Ok(line) = line else { break };
                if tx.send(serde_json::from_str(&line).unwrap()).is_err() {
                    break;
                }
            }
        });
        Server {
            input: child.stdin.take(),
            child,
            lines,
        }
    }

    fn send(&mut self, v: &Value) {
        let input = self.input.as_mut().unwrap();
        writeln!(input, "{v}").unwrap();
        input.flush().unwrap();
    }

    /// The next line that satisfies `f` (others skipped), within 20 s.
    fn next(&self, f: impl Fn(&Value) -> bool) -> Value {
        loop {
            let v = self
                .lines
                .recv_timeout(Duration::from_secs(20))
                .expect("no answer");
            if f(&v) {
                return v;
            }
        }
    }

    fn answer(&self, id: u64) -> Value {
        self.next(|v| v["id"] == json!(id))
    }
}

/// The session ID of the process `pid` in a sample's `added`.
fn added(sample: &Value, pid: u32) -> Option<u64> {
    sample["added"]
        .as_array()?
        .iter()
        .find(|p| p["pid"] == json!(pid))?["id"]
        .as_u64()
}

#[test]
fn watch_kill_and_end() {
    let mut s = Server::start();
    s.send(&json!({"id": 1, "method": "columns"}));
    let columns = s.answer(1)["result"].clone();
    assert_eq!(columns[0], "id");
    s.send(&json!({"id": 2, "method": "watch", "interval": 100}));
    assert_eq!(s.answer(2)["result"]["interval"], 100);
    let first = s.next(|v| v["event"] == "sample");
    // The first sample is everything: this test among the processes, a row
    // for each.
    let me = added(&first, std::process::id()).expect("this process in the first sample");
    let rows = first["rows"].as_array().unwrap();
    assert_eq!(rows.len(), first["added"].as_array().unwrap().len());
    assert!(rows.iter().any(|r| r[0] == json!(me)));
    assert!(
        rows.iter()
            .all(|r| r.as_array().unwrap().len() == columns.as_array().unwrap().len())
    );
    assert!(first["system"]["cpus"].as_u64().unwrap() >= 1);

    // A child, seen as it starts, then killed by its ID.
    let mut child = Command::new(env!("CARGO_BIN_EXE_procupine"))
        .arg("serve")
        .stdin(Stdio::piped())
        .spawn()
        .unwrap();
    let pid = child.id();
    let started = s.next(|v| v["event"] == "sample" && added(v, pid).is_some());
    let id = added(&started, pid).unwrap();
    s.send(&json!({"id": 3, "method": "kill", "processes": [id, 999_999]}));
    let results = s.answer(3)["result"].clone();
    assert_eq!(results[0], json!({"process": id, "ok": true}));
    assert!(results[1]["error"].is_string());
    child.wait().unwrap();
    s.next(|v| v["event"] == "sample" && v["removed"].as_array().unwrap().contains(&json!(id)));

    // Refreshed: everything again.
    s.send(&json!({"id": 4, "method": "pause"}));
    s.answer(4);
    s.send(&json!({"id": 5, "method": "refresh"}));
    let again = s.next(|v| v["event"] == "sample" && added(v, std::process::id()).is_some());
    assert_eq!(added(&again, std::process::id()), Some(me));

    s.send(&json!({"id": 6, "method": "nope"}));
    assert!(s.answer(6)["error"].as_str().unwrap().contains("nope"));
    s.input = None;
    assert!(s.child.wait().unwrap().success());
}
