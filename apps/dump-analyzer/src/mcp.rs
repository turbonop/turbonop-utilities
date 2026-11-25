//! `dump-analyzer mcp`: a Model Context Protocol server (JSON-RPC 2.0 over
//! standard input and output, one message per line), so that an agent can
//! triage a crash dump: its summary, stacks, locals, registers, memory,
//! code, modules, and rebuilt stacks. Each tool is a `dump-analyzer`
//! command; its answer is the command's JSON.

use std::io::{self, BufRead, Write};
use std::process::ExitCode;

use serde_json::{Value, json};
use turbonop_target::load::Hints;

const PROTOCOL_VERSIONS: &[&str] = &["2025-06-18", "2025-03-26", "2024-11-05"];

const INSTRUCTIONS: &str = "Dump Analyzer reads crash dumps (Windows, Linux, and macOS minidumps; Linux core files) without a debugger. Start with crash_summary (the exception and the frame it happened in), then stack (the exception's thread, or any thread's), then locals and registers of the frames that matter. Every value says where it came from; a frame found by a heuristic (the frame pointer chain, a stack scan) is 'likely', not certain, and a variable that can't be read says why. reconstruct rebuilds probable stacks from a corrupt stack's memory.";

/// The tools: (name, description, the command, its arguments' schema).
fn tools() -> Value {
    let dump = json!({ "type": "string", "description": "The dump's path." });
    let thread = json!({ "type": "integer", "description": "The thread's ID (else the exception's thread)." });
    let frame =
        json!({ "type": "integer", "description": "The frame's index (0 is the innermost)." });
    let tool = |name: &str, description: &str, props: Value, required: &[&str]| {
        json!({
            "name": name,
            "description": description,
            "inputSchema": { "type": "object", "properties": props, "required": required },
        })
    };
    json!({ "tools": [
        tool("crash_summary", "The dump's exception, the frame it happened in, and how many of its modules' files were found.", json!({ "dump": dump }), &["dump"]),
        tool("threads", "Every thread, and the frame it's in.", json!({ "dump": dump }), &["dump"]),
        tool("stack", "A thread's frames: each one's function, source line, inlined calls, and how it was found (certain or likely).", json!({ "dump": dump, "thread": thread }), &["dump"]),
        tool("locals", "A frame's parameters and locals, as their types say, or why each can't be read.", json!({ "dump": dump, "thread": thread, "frame": frame }), &["dump"]),
        tool("registers", "A frame's registers, each with where its value came from (or clobbered by a call).", json!({ "dump": dump, "thread": thread, "frame": frame }), &["dump"]),
        tool("memory", "Memory at an address, and where each byte came from (the dump, or a module's file).", json!({ "dump": dump, "address": { "type": "string" }, "length": { "type": "integer" } }), &["dump", "address"]),
        tool("disassembly", "A frame's code around its PC, the PC marked.", json!({ "dump": dump, "thread": thread, "frame": frame, "count": { "type": "integer" } }), &["dump"]),
        tool("modules", "Every module, and where its image and debug information were found or looked for.", json!({ "dump": dump }), &["dump"]),
        tool("reconstruct", "Probable stacks rebuilt from a thread's stack memory, for a stack the walk couldn't finish.", json!({ "dump": dump, "thread": thread }), &["dump"]),
    ]})
}

/// A tool call as a command line.
fn command_line(name: &str, a: &Value) -> Option<Vec<String>> {
    let dump = a.get("dump")?.as_str()?.to_string();
    let command = match name {
        "crash_summary" => "summary",
        "threads" | "stack" | "locals" | "registers" | "memory" | "modules" | "reconstruct" => name,
        "disassembly" => "disasm",
        _ => return None,
    };
    let mut args = vec![command.to_string(), dump];
    if command == "memory" {
        args.push(a.get("address")?.as_str()?.to_string());
        args.push(
            a.get("length")
                .and_then(Value::as_u64)
                .unwrap_or(64)
                .to_string(),
        );
    }
    for (key, flag) in [
        ("thread", "-thread"),
        ("frame", "-frame"),
        ("count", "-count"),
    ] {
        if let Some(v) = a.get(key).and_then(Value::as_u64) {
            args.push(flag.into());
            args.push(v.to_string());
        }
    }
    Some(args)
}

fn error(id: Value, code: i64, message: &str) -> Value {
    json!({ "jsonrpc": "2.0", "id": id, "error": { "code": code, "message": message } })
}

/// The response to one message, if it needs one.
pub fn handle(message: &Value, hints: &Hints) -> Option<Value> {
    let id = message.get("id").cloned();
    let Some(method) = message.get("method").and_then(Value::as_str) else {
        return id.map(|id| error(id, -32600, "invalid request"));
    };
    let id = id?;
    let params = message.get("params").cloned().unwrap_or(Value::Null);
    let result = match method {
        "initialize" => {
            let asked = params.get("protocolVersion").and_then(Value::as_str);
            let version = asked
                .filter(|v| PROTOCOL_VERSIONS.contains(v))
                .unwrap_or(PROTOCOL_VERSIONS[0]);
            json!({
                "protocolVersion": version,
                "capabilities": { "tools": { "listChanged": false } },
                "serverInfo": { "name": "dump-analyzer", "version": env!("CARGO_PKG_VERSION") },
                "instructions": INSTRUCTIONS,
            })
        }
        "ping" => json!({}),
        "tools/list" => tools(),
        "tools/call" => {
            let name = params.get("name").and_then(Value::as_str).unwrap_or("");
            let args = params.get("arguments").cloned().unwrap_or(json!({}));
            let Some(line) = command_line(name, &args) else {
                return Some(error(
                    id,
                    -32602,
                    &format!("unknown tool '{name}', or its arguments are missing"),
                ));
            };
            match crate::command(&line, hints) {
                Ok(j) => json!({
                    "content": [{ "type": "text", "text": serde_json::to_string_pretty(&j).unwrap_or_default() }],
                    "structuredContent": if j.is_object() { j } else { json!({ "result": j }) },
                    "isError": false,
                }),
                Err(e) => json!({ "content": [{ "type": "text", "text": e }], "isError": true }),
            }
        }
        other => return Some(error(id, -32601, &format!("method not found: {other}"))),
    };
    Some(json!({ "jsonrpc": "2.0", "id": id, "result": result }))
}

pub fn run(hints: &Hints) -> ExitCode {
    let mut out = io::stdout().lock();
    for line in io::stdin().lock().lines() {
        let Ok(line) = line else { break };
        if line.trim().is_empty() {
            continue;
        }
        let response = match serde_json::from_str::<Value>(&line) {
            Ok(m) => handle(&m, hints),
            Err(e) => Some(error(Value::Null, -32700, &format!("parse error: {e}"))),
        };
        if let Some(r) = response {
            let _ = writeln!(out, "{r}");
            let _ = out.flush();
        }
    }
    ExitCode::SUCCESS
}
