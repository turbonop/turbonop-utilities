//! `dump-analyzer`: post-mortem debugging of crash dumps from the command
//! line, or served to an editor (`dump-analyzer serve`, which the VS Code
//! extension "Dump Analyzer" runs).
//!
//! Every command reads a dump (a minidump: Windows', Breakpad's,
//! Crashpad's; or a Linux core file), finds its modules' images and debug
//! information (beside the dump, in the directories given, in the symbol
//! directories, and from the symbol servers given), and answers from
//! there. With `-json`, a command's answer is JSON (what `serve` gives).

pub mod mcp;
mod serve;
pub mod session;

use std::path::PathBuf;
use std::process::ExitCode;

use serde_json::{Value as Json, json};
use turbonop_debuginfo::value::Value;
use turbonop_frames::reconstruct::{ChainConfidence, LinkKind, reconstruct};
use turbonop_frames::{Confidence, End, Frame, Method, RegState, Source};
use turbonop_program::server::{self as servers, Servers};
use turbonop_target::load::Hints;
use turbonop_target::{DebugId, ImageId, Provenance};

use crate::session::Session;

pub const USAGE: &str = "\
usage: dump-analyzer [options] <command> <dump> [args]

commands:
  summary <dump>                    the exception, the faulting frame, the modules
  threads <dump>                    every thread, and where it is
  stack <dump> [-thread id] [-all]  a thread's frames (the exception's thread,
                                    or every thread's with -all)
  locals <dump> -thread id -frame n a frame's parameters and locals
  registers <dump> -thread id -frame n
                                    a frame's registers, and where each came from
  memory <dump> <address> <length>  memory, and where each byte came from
  disasm <dump> -thread id -frame n [-count n]
                                    a frame's code around its PC
  disasm <dump> -address a [-count n]
                                    code from an address
  modules <dump>                    every module, and where its files were found
  paths <dump> -thread id -frame n  how the frame's function got to its PC: which way
                                    each branch went, and how sure that is
  reconstruct <dump> -thread id     probable stacks, rebuilt from what's in the
                                    thread's stack memory
  serve                             commands for an editor, as JSON lines
  mcp                               a Model Context Protocol server, for agents

options:
  -json                 answer as JSON
  -symbols <dir>        look for modules' files here too (any number)
  -sysroot <dir>        a directory that stands for the dump's root (any number)
  -symbol-server <url>  a symbol server (Microsoft's layout; any number)
  -debuginfod <url>     a debuginfod server (any number)
  -symbol-cache <dir>   where to keep what servers send";

/// Runs a command line.
pub fn main(args: &[String]) -> ExitCode {
    let mut hints = Hints::default();
    let mut servers_given = Servers::from_env();
    let mut json_out = false;
    let mut rest = Vec::new();
    let mut i = 0;
    while i < args.len() {
        let a = args[i].as_str();
        let value = || args.get(i + 1).cloned();
        match a {
            "-json" => json_out = true,
            "-symbols" | "-sysroot" | "-symbol-server" | "-debuginfod" | "-symbol-cache" => {
                let Some(v) = value() else {
                    eprintln!("dump-analyzer: {a} needs a value\n\n{USAGE}");
                    return ExitCode::from(2);
                };
                match a {
                    "-symbols" => hints.dirs.push(PathBuf::from(v)),
                    "-sysroot" => hints.sysroots.push(PathBuf::from(v)),
                    "-symbol-server" => servers_given.symbol_servers.push(v),
                    "-debuginfod" => servers_given.debuginfod.push(v),
                    _ => servers_given.cache = Some(PathBuf::from(v)),
                }
                i += 1;
            }
            _ => rest.push(args[i].clone()),
        }
        i += 1;
    }
    servers::set_servers(servers_given);
    match rest.first().map(String::as_str) {
        Some("serve") => serve::run(&hints),
        Some("mcp") => mcp::run(&hints),
        Some("-h" | "-help" | "--help" | "help") | None => {
            println!("{USAGE}");
            ExitCode::SUCCESS
        }
        Some(_) => match command(&rest, &hints) {
            Ok(v) => {
                if json_out {
                    println!("{}", serde_json::to_string_pretty(&v).unwrap_or_default());
                } else {
                    print!("{}", text(&rest[0], &v));
                }
                ExitCode::SUCCESS
            }
            Err(e) => {
                eprintln!("dump-analyzer: {e}");
                ExitCode::FAILURE
            }
        },
    }
}

/// A command's answer, as JSON.
///
/// # Errors
///
/// If the command line is wrong, or the dump can't be read.
pub fn command(args: &[String], hints: &Hints) -> Result<Json, String> {
    let name = args.first().ok_or("no command")?;
    let dump = args
        .get(1)
        .ok_or_else(|| format!("{name}: no dump given\n\n{USAGE}"))?;
    let s = session::open(std::path::Path::new(dump), hints)?;
    let opt = |flag: &str| -> Option<&str> {
        args.iter()
            .position(|a| a == flag)
            .and_then(|i| args.get(i + 1))
            .map(String::as_str)
    };
    let number =
        |flag: &str| -> Result<Option<u64>, String> { opt(flag).map(parse_number).transpose() };
    let thread = |s: &Session| -> Result<u64, String> {
        match number("-thread")? {
            Some(t) => Ok(t),
            None => s
                .target
                .exception()
                .map(|e| e.thread)
                .or_else(|| s.target.threads().first().map(|t| t.id))
                .ok_or_else(|| "the dump has no threads".to_string()),
        }
    };
    match name.as_str() {
        "summary" => Ok(summary(s)),
        "threads" => Ok(threads(s)),
        "stack" => {
            if args.iter().any(|a| a == "-all") {
                Ok(Json::Array(
                    s.stacks().iter().map(|st| stack_json(s, st)).collect(),
                ))
            } else {
                let t = thread(s)?;
                let st = s.stack(t).ok_or_else(|| format!("no thread {t}"))?;
                Ok(stack_json(s, st))
            }
        }
        "locals" => {
            let t = thread(s)?;
            let f = usize::try_from(number("-frame")?.unwrap_or(0)).map_err(|e| e.to_string())?;
            let st = s.stack(t).ok_or_else(|| format!("no thread {t}"))?;
            Ok(locals(s, st, f))
        }
        "registers" => {
            let t = thread(s)?;
            let f = usize::try_from(number("-frame")?.unwrap_or(0)).map_err(|e| e.to_string())?;
            let st = s.stack(t).ok_or_else(|| format!("no thread {t}"))?;
            let frame = st.frames.get(f).ok_or_else(|| format!("no frame {f}"))?;
            Ok(registers(s, frame))
        }
        "memory" => {
            let at = parse_number(args.get(2).ok_or("memory: no address")?)?;
            let len = parse_number(args.get(3).map_or("64", String::as_str))?.min(1 << 20);
            Ok(memory(s, at, len))
        }
        "disasm" if opt("-address").is_some() => {
            let at = parse_number(opt("-address").unwrap_or("0"))?;
            let count = number("-count")?.unwrap_or(64).min(4096);
            Ok(disasm_at(s, at, count))
        }
        "disasm" => {
            let t = thread(s)?;
            let f = usize::try_from(number("-frame")?.unwrap_or(0)).map_err(|e| e.to_string())?;
            let count = number("-count")?.unwrap_or(24).min(4096);
            let st = s.stack(t).ok_or_else(|| format!("no thread {t}"))?;
            let frame = st.frames.get(f).ok_or_else(|| format!("no frame {f}"))?;
            Ok(disasm(s, frame, count))
        }
        "modules" => Ok(modules(s)),
        "paths" => {
            let t = thread(s)?;
            let f = usize::try_from(number("-frame")?.unwrap_or(0)).map_err(|e| e.to_string())?;
            let st = s.stack(t).ok_or_else(|| format!("no thread {t}"))?;
            let frame = st.frames.get(f).ok_or_else(|| format!("no frame {f}"))?;
            paths(s, frame)
        }
        "reconstruct" => {
            let t = thread(s)?;
            Ok(reconstructed(s, t))
        }
        other => Err(format!("unknown command '{other}'\n\n{USAGE}")),
    }
}

fn parse_number(s: &str) -> Result<u64, String> {
    let t = s.trim().replace(['`', '_'], "");
    let r = match t.strip_prefix("0x").or_else(|| t.strip_prefix("0X")) {
        Some(h) => u64::from_str_radix(h, 16),
        None => t.parse(),
    };
    r.map_err(|_| format!("not a number: {s}"))
}

fn hex(v: u64) -> String {
    format!("{v:#x}")
}

fn exception_name(code: u32, windows: bool) -> String {
    if windows {
        match code {
            0xC000_0005 => "access violation".into(),
            0xC000_001D => "illegal instruction".into(),
            0xC000_0094 => "integer divide by zero".into(),
            0xC000_00FD => "stack overflow".into(),
            0xC000_0409 => "stack buffer overrun (fail fast)".into(),
            0x8000_0003 => "breakpoint".into(),
            0xE06D_7363 => "C++ exception".into(),
            c => format!("{c:#010x}"),
        }
    } else {
        match code {
            4 => "SIGILL".into(),
            5 => "SIGTRAP".into(),
            6 => "SIGABRT".into(),
            7 => "SIGBUS".into(),
            8 => "SIGFPE".into(),
            11 => "SIGSEGV".into(),
            c => format!("signal {c}"),
        }
    }
}

fn summary(s: &Session) -> Json {
    let t = s.target;
    let windows = t.os() == Some(turbonop_abi::Os::Windows);
    let (images, debug) = s.loaded.counts();
    let exception = t.exception().map(|e| {
        let frame = s.stack(e.thread).and_then(|st| st.frames.first());
        json!({
            "thread": e.thread,
            "code": e.code,
            "name": exception_name(e.code, windows),
            "address": hex(e.address),
            "parameters": e.parameters.iter().map(|p| hex(*p)).collect::<Vec<_>>(),
            "frame": frame.map(|f| frame_json(s, 0, f)),
        })
    });
    json!({
        "dump": s.path.display().to_string(),
        "arch": t.arch().name(),
        "os": t.os().map(|o| format!("{o:?}")),
        "threads": t.threads().len(),
        "modules": t.modules().len(),
        "images_found": images,
        "debug_found": debug,
        "exception": exception,
    })
}

fn threads(s: &Session) -> Json {
    let exc = s.target.exception().map(|e| e.thread);
    Json::Array(
        s.target
            .threads()
            .iter()
            .map(|t| {
                let top = s.stack(t.id).and_then(|st| st.frames.first());
                json!({
                    "id": t.id,
                    "name": t.name,
                    "exception": exc == Some(t.id),
                    "frame": top.map(|f| frame_json(s, 0, f)),
                })
            })
            .collect(),
    )
}

fn method(m: Method) -> &'static str {
    match m {
        Method::Context => "context",
        Method::Pdata => "pdata",
        Method::Leaf => "leaf",
        Method::Compact => "compact unwind",
        Method::Ehabi => "EHABI",
        Method::Cfi => "CFI",
        Method::SignalFrame => "signal frame",
        Method::FramePointer => "frame pointer",
        Method::Scan => "stack scan",
    }
}

fn frame_json(s: &Session, index: usize, f: &Frame) -> Json {
    let module = f.module.and_then(|m| s.target.modules().get(m));
    let sym = f.symbol.as_ref();
    json!({
        "index": index,
        "pc": hex(f.pc),
        "sp": hex(f.sp),
        "return_address": f.return_address,
        "method": method(f.method),
        "confidence": match f.confidence { Confidence::Certain => "certain", Confidence::Likely => "likely" },
        "module": module.map(turbonop_target::Module::file_name),
        "module_index": f.module,
        "function": sym.map(|s| s.function.clone()),
        // (A return address's own offset: the lookup is the byte before.)
        "offset": sym.map(|s| hex(s.offset + (f.pc - f.lookup()))),
        "file": sym.and_then(|s| s.file.clone()),
        "line": sym.map(|s| s.line).filter(|&l| l != 0),
        "inlines": sym.map(|s| s.inlines.iter().map(|i| json!({
            "function": i.function, "file": i.file, "line": i.line,
        })).collect::<Vec<_>>()).unwrap_or_default(),
    })
}

fn end(e: &End) -> &'static str {
    match e {
        End::Outermost => "outermost",
        End::NoCaller => "no caller found",
        End::StackNotGrowing => "the stack pointer didn't grow",
        End::Loop => "a loop",
        End::TooDeep => "too deep",
        End::NoRegisters => "no registers",
    }
}

fn stack_json(s: &Session, st: &turbonop_frames::Stack) -> Json {
    let t = s.target.threads().iter().find(|t| t.id == st.thread);
    json!({
        "thread": st.thread,
        "name": t.and_then(|t| t.name.clone()),
        "end": end(&st.end),
        "frames": st.frames.iter().enumerate().map(|(i, f)| frame_json(s, i, f)).collect::<Vec<_>>(),
    })
}

fn provenance(p: Provenance) -> &'static str {
    match p {
        Provenance::Dump => "dump",
        Provenance::Live => "live",
        Provenance::Image => "image",
    }
}

fn value_json(v: &Value) -> Json {
    json!({
        "name": v.name,
        "type": v.type_name,
        "kind": v.kind.label(),
        "text": v.text,
        "pointer": v.pointer.map(hex),
        "offset": v.offset,
        "size": v.size,
        "children": v.children.iter().map(value_json).collect::<Vec<_>>(),
        "elided": v.elided,
    })
}

fn locals(s: &Session, st: &turbonop_frames::Stack, f: usize) -> Json {
    match turbonop_eval::locals(st, f, &s.modules, &s.space, s.target.arch()) {
        Ok(l) => Json::Array(
            l.iter()
                .map(|x| {
                    json!({
                        "name": x.name,
                        "type": x.type_name,
                        "param": x.param,
                        "inlined": x.inlined,
                        "location": x.location,
                        "provenance": x.provenance.map(provenance),
                        "value": x.value.as_ref().ok().map(value_json),
                        "unavailable": x.value.as_ref().err().map(ToString::to_string),
                    })
                })
                .collect(),
        ),
        Err(e) => json!({ "unavailable": e.to_string() }),
    }
}

fn source(src: Source) -> String {
    match src {
        Source::Context => "context".into(),
        Source::Saved(a) => format!("saved at {a:#x}"),
        Source::Cfa => "CFA".into(),
        Source::ReturnAddress => "return address".into(),
        Source::SignalContext(a) => format!("signal context at {a:#x}"),
        Source::FrameRecord(a) => format!("frame record at {a:#x}"),
        Source::Scanned(a) => format!("stack slot at {a:#x}"),
        Source::FrameData => "frame data".into(),
    }
}

fn registers(s: &Session, f: &Frame) -> Json {
    let arch = s.target.arch();
    Json::Array(
        f.registers
            .iter()
            .map(|(n, st)| {
                let name = turbonop_dwarf_name(arch, n);
                match st {
                    RegState::Known { value, bits, from } => json!({
                        "register": name, "number": n,
                        "value": format!("{value:#x}"), "bits": bits, "from": source(from),
                    }),
                    RegState::Clobbered => json!({
                        "register": name, "number": n, "clobbered": true,
                    }),
                }
            })
            .collect(),
    )
}

fn turbonop_dwarf_name(arch: turbonop_core::Arch, n: u16) -> String {
    turbonop_dwarf::registers::name(arch, n).unwrap_or_else(|| format!("r{n}"))
}

fn memory(s: &Session, at: u64, len: u64) -> Json {
    let runs = s.space.runs(at, len);
    Json::Array(
        runs.iter()
            .map(|r| {
                let n = usize::try_from(r.len).unwrap_or(0);
                let mut b = vec![0u8; n];
                let _ = s.space.read(r.start, &mut b);
                json!({
                    "address": hex(r.start),
                    "length": r.len,
                    "provenance": provenance(r.provenance),
                    "bytes": b.iter().map(|x| format!("{x:02x}")).collect::<String>(),
                })
            })
            .collect(),
    )
}

fn disasm(s: &Session, f: &Frame, count: u64) -> Json {
    let arch = s.target.arch();
    // From the frame's function's start (else a little before the PC), on
    // past the PC.
    let at = f.lookup();
    let start = f
        .symbol
        .as_ref()
        .map_or(at.saturating_sub(32), |sym| at.wrapping_sub(sym.offset));
    let start = if at - start > 4096 {
        at.saturating_sub(64)
    } else {
        start
    };
    let mut out = Vec::new();
    let mut pc = start;
    let mut after = 0;
    while out.len() < 4096 && after < count {
        let mut code = [0u8; 16];
        let n = s
            .space
            .runs(pc, 16)
            .first()
            .filter(|r| r.start == pc)
            .map_or(0, |r| r.len);
        if n == 0
            || s.space
                .read(pc, &mut code[..usize::try_from(n.min(16)).unwrap_or(0)])
                .is_none()
        {
            break;
        }
        let provenance_of = s
            .space
            .runs(pc, 1)
            .first()
            .map(|r| provenance(r.provenance));
        let (len, text) = match arch {
            turbonop_core::Arch::X64 | turbonop_core::Arch::X86 => {
                let mode = if arch == turbonop_core::Arch::X64 {
                    turbonop_x86::Mode::Bits64
                } else {
                    turbonop_x86::Mode::Bits32
                };
                match turbonop_x86::decode(&code, mode, pc) {
                    Ok(i) => (i.len() as u64, turbonop_x86::format::to_string(&i)),
                    Err(_) => (1, format!("db {:02x}", code[0])),
                }
            }
            turbonop_core::Arch::Arm64 => {
                let w = u32::from_le_bytes([code[0], code[1], code[2], code[3]]);
                match turbonop_arm64::decode(w) {
                    Ok(i) => (4, turbonop_arm64::format(&i, pc)),
                    Err(_) => (4, format!(".inst {w:#010x}")),
                }
            }
            _ => break,
        };
        let len = len.max(1);
        out.push(json!({
            "address": hex(pc),
            "bytes": code[..usize::try_from(len.min(16)).unwrap_or(1)].iter().map(|x| format!("{x:02x}")).collect::<String>(),
            "text": text,
            "current": pc <= at && at < pc + len,
            "provenance": provenance_of,
        }));
        if pc > at {
            after += 1;
        }
        pc += len;
    }
    Json::Array(out)
}

/// `count` instructions from `at` (an address, not a frame's).
fn disasm_at(s: &Session, at: u64, count: u64) -> Json {
    let frame = Frame {
        pc: at,
        sp: 0,
        return_address: false,
        method: Method::Context,
        confidence: Confidence::Likely,
        registers: turbonop_frames::FrameRegisters::new(),
        module: None,
        symbol: None,
        callee_arguments: 0,
    };
    // (From the address itself: no function's start to begin at.)
    let mut v = disasm(s, &frame, count);
    if let Json::Array(a) = &mut v {
        a.retain(|i| parse_number(i["address"].as_str().unwrap_or("0")).is_ok_and(|x| x >= at));
        a.truncate(usize::try_from(count).unwrap_or(64));
    }
    v
}

fn modules(s: &Session) -> Json {
    Json::Array(
        s.target
            .modules()
            .iter()
            .zip(&s.loaded.modules)
            .map(|(m, files)| {
                let image_id = m.image_id.as_ref().map(|i| match i {
                    ImageId::Pe {
                        time_date_stamp,
                        size_of_image,
                    } => format!("{time_date_stamp:08X}{size_of_image:x}"),
                    ImageId::BuildId(b) => hexbytes(b),
                    ImageId::Uuid(u) => hexbytes(u),
                });
                let debug_id = m.debug_id.as_ref().map(|d| match d {
                    DebugId::Pdb { guid, age, path } => format!("{path} {}{age:x}", hexbytes(guid)),
                    DebugId::Pdb20 {
                        signature,
                        age,
                        path,
                    } => format!("{path} {signature:08X}{age:x}"),
                    DebugId::BuildId(b) => hexbytes(b),
                    DebugId::Uuid(u) => hexbytes(u),
                });
                let (image, tried) = match &files.image {
                    Ok(f) => (Some(f.path.display().to_string()), Vec::new()),
                    Err(nf) => (None, nf.tried.clone()),
                };
                let debug = match &files.program {
                    Some(Ok(o)) => json!({ "path": o.debug_path().display().to_string() }),
                    Some(Err(e)) => json!({ "error": e.to_string() }),
                    None => Json::Null,
                };
                json!({
                    "path": m.path,
                    "base": hex(m.base),
                    "size": hex(m.size),
                    "image_id": image_id,
                    "debug_id": debug_id,
                    "image": image,
                    "image_tried": tried,
                    "debug": debug,
                })
            })
            .collect(),
    )
}

/// How the frame's function got to its PC: its branches' ways, its
/// blocks, and the ways left.
fn paths(s: &Session, f: &Frame) -> Result<Json, String> {
    let sym = f.symbol.as_ref().ok_or("the frame has no function")?;
    let at = f.lookup();
    let start = at.wrapping_sub(sym.offset);
    // (The function's extent, from its debug information; else 64 KiB.)
    let len = f
        .module
        .and_then(|m| s.modules.debug(m))
        .and_then(|(d, bias)| d.variables(None, start.wrapping_sub(bias)).ok().flatten())
        .map_or(64 * 1024, |v| v.end.saturating_sub(v.start).max(1));
    let abi = s
        .target
        .abi()
        .ok_or("no calling convention for this target")?;
    let r = turbonop_paths::analyze(&s.space, s.target.arch(), abi, start, len, at, &f.registers)
        .map_err(|e| format!("{e:?}"))?;
    Ok(json!({
        "function": sym.function,
        "start": hex(r.function),
        "pc": hex(r.pc),
        "paths": match r.paths {
            turbonop_paths::Paths::One => "certain: one way".to_string(),
            turbonop_paths::Paths::OneOf(n) => format!("one of {n} ways"),
            turbonop_paths::Paths::Unbounded => "one of an open number of ways (a loop)".to_string(),
        },
        "branches": r.branches.iter().map(|b| json!({
            "address": hex(b.address),
            "way": match b.way { turbonop_paths::Way::Taken => "taken", turbonop_paths::Way::NotTaken => "not taken", turbonop_paths::Way::Unknown => "unknown" },
            "why": b.why,
            "in_loop": b.in_loop,
        })).collect::<Vec<_>>(),
        "blocks": r.blocks.iter().map(|b| json!({
            "start": hex(b.start), "end": hex(b.end),
            "ran": match b.ran { turbonop_paths::Ran::Yes => "yes", turbonop_paths::Ran::No => "no", turbonop_paths::Ran::Maybe => "maybe" },
        })).collect::<Vec<_>>(),
    }))
}

fn hexbytes(b: &[u8]) -> String {
    b.iter().map(|x| format!("{x:02x}")).collect()
}

fn reconstructed(s: &Session, thread: u64) -> Json {
    let Some(t) = s.target.threads().iter().find(|t| t.id == thread) else {
        return json!({ "error": format!("no thread {thread}") });
    };
    let st = s.stack(thread);
    // From where the walk stopped (its last frame's stack pointer), else
    // the thread's stack pointer, to the stack's end.
    let from = st.and_then(|st| st.frames.last()).map(|f| f.sp);
    let Some(range) = t.stack.clone() else {
        return json!({ "error": "the thread's stack wasn't captured" });
    };
    let from = from.filter(|&f| range.contains(&f)).unwrap_or(range.start);
    let r = reconstruct(s.target, &s.space, &s.modules, from..range.end);
    json!({
        "thread": thread,
        "from": hex(from),
        "to": hex(range.end),
        "chains": r.chains.iter().map(|c| json!({
            "confidence": match c.confidence {
                ChainConfidence::Certain => "certain".to_string(),
                ChainConfidence::OneOf(n) => format!("one of {n}"),
                ChainConfidence::Likely => "likely".to_string(),
            },
            "reaches_entry": c.reaches_entry,
            "frames": c.frames.iter().enumerate().map(|(i, f)| {
                let mut j = frame_json(s, i, &f.frame);
                j["slot"] = json!(hex(f.slot));
                j["link"] = json!(f.link.map(|l| match l { LinkKind::UnwindData => "unwind data", LinkKind::FrameRecord => "frame record" }));
                j
            }).collect::<Vec<_>>(),
        })).collect::<Vec<_>>(),
        "slots": r.slots.iter().map(|sl| json!({
            "at": hex(sl.at), "value": hex(sl.value),
            "after_call": sl.after_call,
        })).collect::<Vec<_>>(),
    })
}

/// A command's answer, as text.
fn text(command: &str, v: &Json) -> String {
    use std::fmt::Write as _;
    let mut out = String::new();
    let frame_line = |f: &Json| -> String {
        let func = f["function"].as_str().map_or_else(
            || {
                f["module"]
                    .as_str()
                    .map_or_else(|| "?".to_string(), |m| format!("{m}!?"))
            },
            |n| {
                let m = f["module"].as_str().unwrap_or("?");
                format!("{m}!{n}+{}", f["offset"].as_str().unwrap_or("0"))
            },
        );
        let at = match (f["file"].as_str(), f["line"].as_u64()) {
            (Some(file), Some(line)) => format!("  [{file}:{line}]"),
            _ => String::new(),
        };
        let likely = if f["confidence"] == "likely" {
            " (likely)"
        } else {
            ""
        };
        format!(
            "{:>2} {:>18} {:>18} {func}{at}{likely}",
            f["index"].as_u64().unwrap_or(0),
            f["sp"].as_str().unwrap_or(""),
            f["pc"].as_str().unwrap_or("")
        )
    };
    match command {
        "stack" => {
            let stacks = if v.is_array() {
                v.as_array().cloned().unwrap_or_default()
            } else {
                vec![v.clone()]
            };
            for st in stacks {
                let _ = writeln!(
                    out,
                    "thread {}{}",
                    st["thread"],
                    st["name"]
                        .as_str()
                        .map(|n| format!(" ({n})"))
                        .unwrap_or_default()
                );
                let _ = writeln!(out, " # {:>18} {:>18} call site", "sp", "pc");
                for f in st["frames"].as_array().into_iter().flatten() {
                    let _ = writeln!(out, "{}", frame_line(f));
                    for i in f["inlines"].as_array().into_iter().flatten() {
                        let _ = writeln!(
                            out,
                            "      (inlined) {}",
                            i["function"].as_str().unwrap_or("?")
                        );
                    }
                }
                let _ = writeln!(out, "   ({})\n", st["end"].as_str().unwrap_or(""));
            }
        }
        "summary" => {
            let _ = writeln!(
                out,
                "{} {} dump: {} threads, {} modules ({} images and {} with debug information found)",
                v["os"].as_str().unwrap_or("?"),
                v["arch"].as_str().unwrap_or("?"),
                v["threads"],
                v["modules"],
                v["images_found"],
                v["debug_found"]
            );
            if let Some(e) = v["exception"].as_object() {
                let _ = writeln!(
                    out,
                    "{} at {} on thread {}",
                    e["name"].as_str().unwrap_or("?"),
                    e["address"].as_str().unwrap_or("?"),
                    e["thread"]
                );
                if e["frame"].is_object() {
                    let _ = writeln!(out, "{}", frame_line(&e["frame"]));
                }
            }
        }
        "locals" => {
            for l in v.as_array().into_iter().flatten() {
                let val = l["value"]["text"]
                    .as_str()
                    .map(String::from)
                    .or_else(|| l["value"]["pointer"].as_str().map(String::from));
                let shown = match (val, l["unavailable"].as_str()) {
                    (Some(t), _) if !t.is_empty() => t,
                    (_, Some(why)) => format!("<{why}>"),
                    _ => "{...}".to_string(),
                };
                let _ = writeln!(
                    out,
                    "{:<10} {} {} = {shown}",
                    l["location"].as_str().unwrap_or(""),
                    l["type"].as_str().unwrap_or(""),
                    l["name"].as_str().unwrap_or("")
                );
            }
            if let Some(why) = v["unavailable"].as_str() {
                let _ = writeln!(out, "<{why}>");
            }
        }
        "disasm" => {
            for i in v.as_array().into_iter().flatten() {
                let mark = if i["current"] == true { ">" } else { " " };
                let _ = writeln!(
                    out,
                    "{mark} {} {:<20} {}",
                    i["address"].as_str().unwrap_or(""),
                    i["bytes"].as_str().unwrap_or(""),
                    i["text"].as_str().unwrap_or("")
                );
            }
        }
        _ => {
            let _ = writeln!(
                out,
                "{}",
                serde_json::to_string_pretty(v).unwrap_or_default()
            );
        }
    }
    out
}
