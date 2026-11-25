//! The integration tests, as one binary (one link, not one per file):
//! the commands on the real dumps in testdata, hermetically (only the
//! dumps' own directories are searched).

use std::path::{Path, PathBuf};

use dump_analyzer::command;
use serde_json::Value;
use turbonop_program::server::Servers;
use turbonop_target::load::Hints;

fn testdata(path: &str) -> String {
    let p: PathBuf = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../testdata")
        .join(path);
    p.display().to_string()
}

fn hints() -> Hints {
    Hints {
        skip_recorded_paths: true,
        symbols: Some((Vec::new(), Servers::default())),
        ..Hints::default()
    }
}

fn run(args: &[&str]) -> Value {
    let args: Vec<String> = args.iter().map(|s| (*s).to_string()).collect();
    command(&args, &hints()).unwrap_or_else(|e| panic!("{args:?}: {e}"))
}

#[test]
fn windows_summary_and_stack() {
    let dump = testdata("minidump/crash-x64.dmp");
    let s = run(&["summary", &dump]);
    assert_eq!(s["arch"], "x64");
    assert_eq!(s["os"], "Windows");
    let e = &s["exception"];
    assert_eq!(e["name"], "access violation");
    assert_eq!(e["address"], "0x7ff7eed9101b");
    assert_eq!(e["frame"]["function"], "leaf");
    assert_eq!(e["frame"]["offset"], "0x13");
    let st = run(&["stack", &dump]);
    let frames = st["frames"].as_array().unwrap();
    // DbgEng's offsets: leaf+0x13, middle+0x33, outer+0x22, worker+0xe.
    let shown: Vec<(String, String)> = frames[..4]
        .iter()
        .map(|f| {
            (
                f["function"].as_str().unwrap().to_string(),
                f["offset"].as_str().unwrap().to_string(),
            )
        })
        .collect();
    assert_eq!(
        shown,
        [
            ("leaf".to_string(), "0x13".to_string()),
            ("middle".into(), "0x33".into()),
            ("outer".into(), "0x22".into()),
            ("worker".into(), "0xe".into())
        ]
    );
    assert_eq!(frames[1]["confidence"], "certain");
    assert_eq!(frames[1]["method"], "leaf");
    let all = run(&["stack", &dump, "-all"]);
    assert_eq!(all.as_array().unwrap().len(), 5);
}

#[test]
fn windows_locals_registers_memory_disasm() {
    let dump = testdata("minidump/crash-x64.dmp");
    let l = run(&["locals", &dump, "-frame", "1"]);
    let n = l
        .as_array()
        .unwrap()
        .iter()
        .find(|x| x["name"] == "n")
        .unwrap();
    assert_eq!(n["value"]["pointer"], "0x848757f770");
    assert_eq!(n["location"], "rbx");
    let r = run(&["registers", &dump, "-frame", "1"]);
    let rax = r
        .as_array()
        .unwrap()
        .iter()
        .find(|x| x["register"] == "rax")
        .unwrap();
    assert_eq!(rax["clobbered"], true);
    let rsp = r
        .as_array()
        .unwrap()
        .iter()
        .find(|x| x["register"] == "rsp")
        .unwrap();
    assert_eq!(rsp["from"], "CFA");
    // The stack, from the dump.
    let m = run(&["memory", &dump, "0x848757f6d8", "16"]);
    assert_eq!(m[0]["provenance"], "dump");
    assert_eq!(m[0]["length"], 16);
    // The faulting instruction (add eax, dword ptr [rcx+8]), marked: the
    // dump has the code around it (minidumps capture it).
    let d = run(&["disasm", &dump, "-frame", "0", "-count", "2"]);
    let current = d
        .as_array()
        .unwrap()
        .iter()
        .find(|i| i["current"] == true)
        .unwrap();
    assert_eq!(current["address"], "0x7ff7eed9101b");
    assert_eq!(current["bytes"], "034108");
    assert!(
        current["text"].as_str().unwrap().starts_with("add"),
        "{current}"
    );
    assert_eq!(current["provenance"], "dump");
}

#[test]
fn windows_modules_and_reconstruct() {
    let dump = testdata("minidump/crash-x64.dmp");
    let m = run(&["modules", &dump]);
    let exe = m
        .as_array()
        .unwrap()
        .iter()
        .find(|x| x["path"].as_str().unwrap().ends_with("crash-x64.exe"))
        .unwrap();
    assert!(exe["image"].as_str().unwrap().ends_with("crash-x64.exe"));
    assert!(
        exe["debug"]["path"]
            .as_str()
            .unwrap()
            .ends_with("crash-x64.pdb")
    );
    let k32 = m
        .as_array()
        .unwrap()
        .iter()
        .find(|x| {
            x["path"]
                .as_str()
                .unwrap()
                .to_ascii_lowercase()
                .ends_with("kernel32.dll")
        })
        .unwrap();
    assert!(k32["image"].is_null());
    assert!(!k32["image_tried"].as_array().unwrap().is_empty());
    let r = run(&["reconstruct", &dump, "-thread", "9036"]);
    assert!(r["chains"].is_array());
}

#[test]
fn linux_core() {
    let core = testdata("coredump/crash-x86_64.core");
    let s = run(&["summary", &core]);
    assert_eq!(s["os"], "Linux");
    assert_eq!(s["exception"]["name"], "SIGSEGV");
    assert_eq!(s["exception"]["address"], "0x8");
    let st = run(&["stack", &core]);
    let names: Vec<&str> = st["frames"].as_array().unwrap()[..4]
        .iter()
        .map(|f| f["function"].as_str().unwrap())
        .collect();
    assert_eq!(names, ["leaf", "middle", "outer", "worker"]);
    let l = run(&["locals", &core, "-frame", "1"]);
    let depth = l
        .as_array()
        .unwrap()
        .iter()
        .find(|x| x["name"] == "depth")
        .unwrap();
    assert_eq!(depth["value"]["text"], "2");
}

#[test]
fn errors() {
    let args = |a: &[&str]| -> Vec<String> { a.iter().map(|s| (*s).to_string()).collect() };
    assert!(command(&args(&["stack"]), &hints()).is_err());
    assert!(command(&args(&["stack", "no-such.dmp"]), &hints()).is_err());
    let dump = testdata("minidump/crash-x64.dmp");
    assert!(command(&args(&["frobnicate", &dump]), &hints()).is_err());
    assert!(command(&args(&["memory", &dump, "zzz"]), &hints()).is_err());
    // A file that's neither.
    let exe = testdata("minidump/crash-x64.exe");
    assert!(command(&args(&["summary", &exe]), &hints()).is_err());
}

#[test]
fn mcp_tools() {
    use dump_analyzer::mcp::handle;
    use serde_json::json;
    let init = handle(&json!({"jsonrpc": "2.0", "id": 1, "method": "initialize", "params": {"protocolVersion": "2025-03-26"}}), &hints()).unwrap();
    assert_eq!(init["result"]["protocolVersion"], "2025-03-26");
    assert!(
        handle(
            &json!({"jsonrpc": "2.0", "method": "notifications/initialized"}),
            &hints()
        )
        .is_none()
    );
    let list = handle(
        &json!({"jsonrpc": "2.0", "id": 2, "method": "tools/list"}),
        &hints(),
    )
    .unwrap();
    let names: Vec<&str> = list["result"]["tools"]
        .as_array()
        .unwrap()
        .iter()
        .map(|t| t["name"].as_str().unwrap())
        .collect();
    assert!(
        names.contains(&"crash_summary") && names.contains(&"locals"),
        "{names:?}"
    );
    let dump = testdata("minidump/crash-x64.dmp");
    let call = handle(&json!({"jsonrpc": "2.0", "id": 3, "method": "tools/call", "params": {"name": "stack", "arguments": {"dump": dump}}}), &hints()).unwrap();
    assert_eq!(call["result"]["isError"], false);
    assert_eq!(
        call["result"]["structuredContent"]["frames"][0]["function"],
        "leaf"
    );
    let bad = handle(&json!({"jsonrpc": "2.0", "id": 4, "method": "tools/call", "params": {"name": "nope", "arguments": {}}}), &hints()).unwrap();
    assert_eq!(bad["error"]["code"], -32602);
    let missing = handle(&json!({"jsonrpc": "2.0", "id": 5, "method": "tools/call", "params": {"name": "stack", "arguments": {"dump": "no-such.dmp"}}}), &hints()).unwrap();
    assert_eq!(missing["result"]["isError"], true);
}
