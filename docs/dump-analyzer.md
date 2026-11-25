# Dump Analyzer: plan

Dump Analyzer (`dump-analyzer`; the VS Code extension is "Dump Analyzer") is a
post-mortem debugger for crash dumps from Windows (PE), Linux and Android
(ELF), and macOS and iOS (Mach-O). It aims to do everything WinDbg does for
dumps, with a better interface, and to add analyses no debugger has. The
first such analysis is **"how did we get here"**: which branches each frame's
function provably took, with the evidence for each.

It **replaces** WinDbg. It does not drive it. Nothing at run time needs
WinDbg, DbgEng, dbghelp, lldb, or gdb. Those tools appear only as test
oracles, in `hexcavator-private`.

This is a plan, not a specification. Each phase becomes TODO.md items, and
items leave TODO.md as they ship.

## Status (2026-11-25)

| Phase | State |
|---|---|
| 0 Groundwork | Done: `turbonop-abi`, DWARF register numbers everywhere, CodeView registers mapped to them, minidump contexts read by their flags. |
| 1 Targets | Done: the target model (snapshots, capabilities, provenance), minidumps, Linux core files, the address space (images relocated as the loader would have), images found by identity locally or from servers, batched parallel fetches, all modules loaded at once. Not yet: the corpus run against oracles in `hexcavator-private`; measuring fetches. |
| 2 Frames | Done: the walk with each register's provenance, ABI clobbers, PAC stripping (by the target's address width where it says), Linux signal frames, frame-pointer and scan fallbacks checked for calls, symbols with inlined calls; corrupt stacks rebuilt. Checked against DbgEng and gdb on real dumps. Not yet: x86's PDB frame data in the walk (its unwinder exists), macOS `_sigtramp`, Windows ARM64 dumps. |
| 3 Values | Done: locals by PDB def-ranges (as DbgEng reads them) and DWARF location expressions (a complete evaluator, entry values through call sites). Checked against DbgEng's `dv` and gdb. Not yet: x86's virtual frame (`$T0`), TLS, implicit pointers, typed DWARF operations, NatVis. |
| 4 App and extension | Done: `dump-analyzer` (commands, `serve`, `mcp`), the VS Code extension (an inline DAP adapter: threads, stacks, locals and members, registers, disassembly, memory, the debug console). Not yet: the web host's debug-UI shim, the release workflow, a shared disassembly view with the dump's values. |
| 5 Access data | In progress (on branches `access-x86` and `access-arm64`). |
| 6 IR and lifters | Not yet. Phase 7 uses x86-64 write sets of its own meanwhile (an instruction not described writes everything: sound, imprecise). |
| 7 How did we get here | First version: branches decided by control flow and by flag operands whose values survive to the PC; blocks that ran; the ways left. Not yet: values computed from others (the IR's symbolic execution and solver), the dead stack's evidence, outer frames' stale returns, ARM64, TTD ground truth. |
| 8 macOS and iOS | Begun: Mach-O core files (threads, exception states, notes, images), tested on cores made in tests. Not yet: the dyld shared cache, `_sigtramp`, real cores (the Mac VM). |
| 9, 10 SLEIGH, decompiler | Not yet. |

## Decisions

| | Decision |
|---|---|
| Runtime | Our own code from end to end. Windows debuggers are only test oracles. |
| Targets now | Minidumps (`MiniDumpWriteDump`, Breakpad, Crashpad) and core files (ELF `ET_CORE`, Mach-O `MH_CORE`). |
| Targets later | Live processes. The target model is designed for them now (see [The target model](#the-target-model)) but none are built. |
| Order | Windows and Linux first, and made great. macOS and iOS get best-effort support next, finished on a rented Mac VM. |
| Architectures | x64 and ARM64 for everything. x86 and ARM32 get stacks and locals at first, and path analysis later. |
| Front end | A DAP adapter written inline in the extension, over the tool's `serve` protocol, plus custom webviews. The web build gets the debug-UI shim it needs. |
| Name | `dump-analyzer` (app, crate, and command), "Dump Analyzer" (the extension), released on its own `dump-analyzer-v*` tags, like each TurboNOP tool. |
| Crates | `crates/debug/abi` beside the unwinders. The engine is a new layer, `crates/debugger` (`target`, `frames`, `eval`, `paths`), at rank 4, because it builds on `crates/analysis` (programs, code, the IR), which is `crates/debug`'s peer at rank 3. Apps move to rank 5. Plus `crates/analysis/ir`. |
| Private data | Every dump, trace, and oracle result collected on this PC stays in `hexcavator-private`. Fixtures committed here are synthetic (dumps written by our own test programs or a dump writer), and their only PII is paths under `c:\jm`. |
| Confidence | Every claim carries a confidence that we have actually proved: certain, one of *n*, or likely (see [Confidence](#confidence)). |
| Symbol fetches | Parallel and saturating. A real HTTP crate is allowed if measurements call for one (see [Fetching](#fetching)). |

## What exists, and what's missing

The **existing** pieces:

- **Dumps:** `turbonop-minidump` reads threads, modules (with `code_view()`:
  PDB 7.0, NB10, and ELF build IDs; Mach-O UUIDs arrive as RSDS records with
  age 0), the memory map (`MemoryMap::read` spans adjacent ranges), the
  exception, system info, and Breakpad and Crashpad streams. Register
  contexts are decoded for x86, x64, ARM, and ARM64.
- **Unwinding:** `turbonop-unwind` has Windows x64, x86 (FrameData, FPO, EBP,
  `.raSearch`), ARM64 (with PAC stripping), and ARM32 unwinders, plus DWARF
  CFI, Apple compact unwind, and ARM EHABI. `any::unwind` dispatches by image
  and uses DWARF register numbers.
- **Debug info:** `turbonop-debuginfo` has PDB and DWARF support, frames at
  an address with inline frames (`Locator::frames`), variable places
  (`variables`), types (`TypeShape`, `layout`), and value trees
  (`value::read`). `turbonop-dwarf` has a CFI parser and a value-expression
  evaluator.
- **Program loading:** `turbonop-program` opens a program and fetches debug
  files from symbol servers (SSQP keys for PDBs, ELF build IDs, and Mach-O
  UUIDs; debuginfod `debuginfo`).
- **Code:** decoders for 9 instruction sets, sweeps, basic blocks with
  Taken and NotTaken edges (`turbonop-code`), and cross-references.
- **Front ends:** `serve` (JSON lines over stdio), the VS Code extension
  with its disassembly view (raw, blocks, source, and graph), and the web
  build (wasm with a VS Code API shim).

The **missing** pieces, each covered by a phase below:

- **Wiring:**
  - Nothing joins the minidump crate to the unwinders. There is no
    register conversion and no address space.
  - No program can be opened from a dump's module record, and no
    executable can be fetched (only debug files).
- **Stack walking:**
  - There is no frame model and no walk loop.
  - There is no ABI model. `dwarf_cfi` keeps a register that has no rule,
    so volatile registers survive into callers.
  - There are no signal frames.
  - Frame-pointer and stack-scan fallbacks exist only for x86.
- **Variables:**
  - There is no location evaluator.
  - DWARF places are pure places only. Stack values, implicit values,
    entry values, `deref`, arithmetic, and all but the first memory piece
    are dropped.
  - Three PDB def-range kinds aren't read.
- **Instruction data:**
  - No decoder exposes per-operand reads and writes, implicit operands, or
    flags.
  - The ARM64 decoder has no operand API.
- **Formats:** There is no dyld shared cache parser, and no core-file
  parsing beyond the file-type constants. ELF has a note iterator.

## The target model

A **target** is anything with threads, registers, memory, and modules. Today
that means a minidump or a core file. A live process could be one later.
Live targets aren't built, but this design keeps them possible:

- **Snapshots.** Every query is made against a `Snapshot`: the state at one
  instant, with a generation number. A dump has exactly one snapshot,
  forever. A live target would make a new snapshot at each stop, and caches
  are keyed by generation, so nothing computed for one stop is reused for
  another.
- **Capabilities.** A target says what it can do: read memory (all targets),
  write memory, run, set breakpoints, and get events. A dump offers reads
  only. The engine never assumes it has more than a target says.
- **Provenance.** Every read returns bytes and where they came from: `Dump`,
  `ImageFile` (a module's file on disk or from a server, standing in for
  memory the dump didn't capture), or `Live`. The UI marks bytes that came
  from an image file, because that code may not be the code that ran (it
  may have been hot-patched or relocated differently).
- **Threads and registers** come from the target. The frame model above them
  doesn't care where they came from.
- **The DAP adapter** already speaks in stops and sessions, so a live target
  would add requests (`continue`, `setBreakpoints`, `next`) without changing
  the ones a dump uses.

Dumps never change, so for them every query is a pure function of the file
and its arguments. The `serve` cache makes repeated requests instant, and the
server needs no session state.

## Confidence

Every conclusion the analyzer draws, about a stack frame or a branch or a
path, carries one of these confidences. **It never claims more than it has
proved.**

| Confidence | Meaning | Where it can come from |
|---|---|---|
| **Certain** | It was this. | Only sound reasoning over exact facts: register and memory values from the dump, complete write sets, and solver results that are proofs. |
| **One of *n*** | It was one of these *n*, and each is still possible. | The same sound reasoning, when it narrows the choices but doesn't decide them. |
| **Likely** | Probably this, for a stated reason. | Heuristics: stale return addresses below SP, a stack-scan frame, a branch that would be odd on the other path. A likely claim names its heuristic. |
| **Unknown** | Nothing can be said. | Not enough evidence, or a budget ran out. |

These rules follow:

- A heuristic can never raise a claim to certain or one of *n*. It can only
  rank the remaining choices of a one-of-*n* claim, or make an unknown
  likely.
- A certain claim always comes with its proof (the facts and instructions
  that decided it), and the UI shows that proof.
- Correctness is measured against ground truth (TTD traces, single-step
  traces). A wrong certain or one-of-*n* claim is a bug to fix before
  release, not a quality score.
- Stack frames carry the same confidences: exact unwind data is certain, a
  frame-pointer chain is likely, and a stack scan is likely only when the
  candidate follows a call instruction (otherwise it isn't offered).

## Fetching

Symbol and image fetches decide how fast a dump first opens. A dump with 300
modules can mean 600 or more requests to servers. Today `server::fetch` runs
one `curl` process per file, and the program loader calls it once per
candidate, in order.

1. **Parallel.** Fetch every module's image and debug files at once, as soon
   as the module list is read, without waiting for each other. Then parse
   each one on its own core as soon as it arrives. The network is kept busy
   and every core parses in parallel.
2. **Fewer round trips.** Group requests per server and reuse connections.
   `curl --parallel --parallel-max N` with a config file of URLs gives HTTP/2
   multiplexing over shared connections from one process.
3. **Measure.** Time the first open of corpus dumps with a cold cache. If one
   `curl` per batch can't fill the link, or process start-up shows in the
   profile, change to an HTTP crate. TLS is the hard part of that choice:
   rustls brings C and assembly (ring or aws-lc), and the alternative is the
   platform's own TLS (SChannel on Windows, Security.framework on macOS,
   OpenSSL on Linux). Whichever we choose goes into THIRD_PARTY_NOTICES and
   the licenses test.
4. **Progress.** DAP `progressStart` and `progressUpdate` show what is being
   fetched, and the UI is usable while fetches continue. The faulting
   thread's modules come first.
5. **The web build.** The browser's `fetch` is used through the host, and
   symbol servers must allow CORS (msdl does not; this needs a proxy or the
   user's own cache).

## Phases

The phases run in this order. Phases 5 and 6 depend on nothing in 0
through 4, and can start at any time.

```
0 groundwork ─▶ 1 targets ─▶ 2 frames ─▶ 3 values ─▶ 4 app and extension (MVP: Windows, Linux)
                                                               │
5 access data ─▶ 6 IR and lifters ─────────────────────────────┴─▶ 7 how did we get here
                                                                       │
                                     8 macOS and iOS ◀──────────────────┘
                                     9 SLEIGH and p-code ─▶ 10 decompiler
```

### Phase 0: groundwork

- **0.1 One register numbering.** DWARF numbers, as `any.rs` already uses,
  everywhere, with tables from CodeView register numbers and from minidump
  and core-file context fields.
- **0.2 `turbonop-abi`** (`crates/debug/abi`). For each (platform, arch)
  pair, it records:
  - volatile and non-volatile registers
  - argument and return registers
  - red zone (SysV x64 and Darwin: 128 bytes; Windows: none)
  - home space (Windows x64: 32 bytes)
  - reserved registers (`x18` on Apple and Windows ARM64)
  - PAC rules
  - stack alignment

  It covers Windows x64, ARM64, x86, and ARM32; SysV x64; AAPCS64 and
  Android; Apple ARM64; and ARM EABI. `xref/scan.rs`'s ad hoc rule that
  calls clobber volatile registers moves here.
- **0.3 Context conversion.** Convert minidump contexts (and, later,
  core-file thread states) to `dwarf_cfi::Registers` and to the Windows
  unwinders' contexts. Each conversion follows the platform in the dump's
  system info.

**Done when:** every context in `testdata` round-trips, and each ABI's tables
match its specification (golden tests).

### Phase 1: targets

- **1.1 `turbonop-target`** (`crates/debugger/target`). The model above
  (snapshots, capabilities, provenance), with these parts:
  - **Modules** from the minidump module list. On Linux, `/proc/<pid>/maps`
    from Breakpad's Linux streams finds modules the list misses.
  - **An address space** that layers dump memory over module images mapped
    at their load bases. Dump bytes win. Image bytes need:
    - PE base relocations, applied when a page wasn't captured.
    - ELF `R_*_RELATIVE` relocations and Mach-O chained fixups, applied to
      data pointers.
  - **`unwind::Memory`, and an `unwind::Image` view for each module**, so
    the unwinders run on a target unchanged.
- **1.2 ELF core files.** Linux comes first, so these come now:
  - `NT_PRSTATUS` for the threads
  - `NT_FILE` for the modules
  - `NT_SIGINFO`, `NT_AUXV`, and `NT_PRPSINFO`
  - `PT_LOAD` segments for the memory

  The existing note iterator reads them. They are also how gdb and lldb
  check our stacks and locals (phases 2 and 3).
- **1.3 Opening a program from an identity.** `Opened::open_identity` opens
  a program from a module's identity, without a file at hand:
  1. Look locally: the symbol path, `_NT_SYMBOL_PATH`, beside the dump, and
     directory mappings from settings.
  2. Otherwise fetch it.
- **1.4 Fetching executables** in `server.rs`:

  | Format | Server key |
  |---|---|
  | PE | `name.dll/<TIMESTAMP><SIZEOFIMAGE>/name.dll` |
  | NB10 PDBs | `name.pdb/<SIG><AGE>/name.pdb` |
  | ELF | `elf-buildid-<id>/<name>`, and debuginfod's `/executable` |
  | Mach-O | `mach-uuid-<uuid>/<name>` |

  Breakpad modules without a build ID (identified by the hash of their first
  page of `.text`) are matched locally only.
- **1.5 Parallel loading,** as in [Fetching](#fetching).

**Done when:** every Windows and Linux dump and core in the corpus opens with
its modules' images and debug info found, and anything not found comes with
the reason.

### Phase 2: frames

- **2.1 `turbonop-frames`** (`crates/debugger/frames`):

  ```
  Frame {
    pc, cfa, module, function, inline frames (virtual, from Locator::frames),
    method: Context | Pdata | Cfi | Compact | Ehabi | FrameData
          | FramePointer | Scan | SignalFrame | ContextRecord,
    confidence: Certain | Likely,       // see Confidence
    registers: RegState for each DWARF register number,
  }
  RegState = Known { value, from: Context | SavedAt(address) | Cfa | Computed }
           | Clobbered        // volatile under the ABI: the caller's value is lost
           | Unknown
  ```

  Every register carries its provenance. The variables pane and "how did we
  get here" both depend on it.
- **2.2 The walk:**
  - **The first frame** comes from the exception's context for the faulting
    thread, and from each thread's own context otherwise.
  - **Each step** unwinds with `any::unwind`, or on x86 with FrameData and
    FPO and a check that each candidate return address follows a call.
    (`any::unwind`'s PE adapters take an unknown register as 0 and report
    every register known afterward. They are fixed to keep unknown
    registers unknown.)
  - **After each step:**
    - The ABI's volatile registers become `Clobbered`.
    - PAC bits are stripped.
    - The caller is looked up at its return address minus one.
  - **Fallbacks,** each lowering the frame's confidence:
    1. The frame-pointer chain (`rbp` on x64; `fp` and `lr` on ARM64).
    2. A stack scan, offering only addresses that follow a `call` or `bl`.
  - **Signal and trap frames:** `__restore_rt` on Linux (the x64 and ARM64
    `rt_sigframe` layouts), `_sigtramp` on macOS, Windows
    `KiUserExceptionDispatcher` on x64 and ARM64, and machine frames.
  - **The walk stops** at an SP that doesn't increase, an unknown PC, a loop,
    or a depth limit.
  - **Threads** are walked in parallel.
- **2.3 Reconstructing a corrupt stack.** When the walk fails (an
  overwritten return address, a smashed frame, a PC in nothing), the stack
  memory still holds the older frames' return addresses. The analyzer
  rebuilds the probable stacks from them, which is slow and manual work in
  WinDbg:
  1. **Candidates.** Every pointer-sized word in the thread's stack (from
     the SP up, and below it as far as the dump has memory, skipping the
     red zone) that points into a module's code. Slots are scanned in
     parallel.
  2. **Return addresses.** A candidate is a return address only if the
     instruction before it is a call. A direct call also names its
     target, and the frame below it on the stack should be in that target
     (or in a thunk or import stub that jumps there). Indirect calls are
     kept, with less weight.
  3. **Frame sizes.** For a return address R found at slot A, the function
     containing R has unwind data (`.pdata`, CFI, compact unwind) or a
     prolog that the decoder can read. Unwinding a virtual frame (PC = R,
     SP just above A) says where the caller's return address must be. If
     a candidate is there, the two are linked. On ARM64 and on x64 code
     with frame pointers, frame records (a saved FP and LR pair) give a
     second, independent kind of link.
  4. **Chains.** The links form a graph. Its longest consistent chains are
     the probable stacks, and each chain reaches the stack's base or a
     thread entry point such as `RtlUserThreadStart`, `__libc_start_main`,
     or `thread_start`. The part of the stack that walked correctly
     anchors the chains that continue it.
  5. **Report** with [confidence](#confidence). A chain whose every link
     is proved by unwind data is certain from where it starts. Several
     chains that fit the evidence are reported as one of *n*. Gaps
     bridged by a scan are likely. The UI shows each chain beside the raw
     stack slots, with the reason each slot was or wasn't taken as a
     frame, and marks where the control flow probably went between them.
     Phase 7's path analysis then runs on the reconstructed frames like
     on any others.
- **2.4 Oracles,** in `hexcavator-private`, each driven by a tool under
  `tools/`:
  - DbgEng's `k` for Windows dumps.
  - rust-minidump's `minidump-stackwalk` and Breakpad's `minidump_stackwalk`
    for Breakpad and Crashpad dumps.
  - gdb and lldb for ELF cores.
  - True return addresses, recorded by test programs that crash on purpose.

**Done when:** stacks match the oracles on every Windows and Linux dump in the
corpus, and every disagreement is explained. All the threads of a dump with
2,000 threads walk in well under a second, warm.

### Phase 3: values

- **3.1 DWARF places, completely:**
  - `DW_OP_stack_value`, `implicit_value`, `implicit_pointer`, and `deref`
  - arithmetic
  - every piece, not just the first
  - TLS, through the TEB or the TCB
  - `DW_OP_entry_value`, resolved through the caller's
    `DW_TAG_call_site_parameter`. That recovers clobbered arguments, and
    it's evidence for phase 7.
- **3.2 PDB:** `S_DEFRANGE_REGISTER_REL_INDIR`, `S_DEFRANGE_SUBFIELD`, and plain
  `S_DEFRANGE`.
- **3.3 `turbonop-eval`** (`crates/debugger/eval`). It takes a frame, a
  variable, and a PC, and gives either:
  - a `Value` (from `value::read`), with the provenance of its bytes, or
  - `Unavailable` with a reason, such as "rbx clobbered by the call at
    +0x42", "page not in dump", or "optimized out here".

**Done when:** locals match DbgEng's `dv` and `dx` on Windows dumps, and gdb
and lldb on ELF cores of the same test programs.

### Phase 4: the app and the extension (MVP)

- **4.1 `apps/dump-analyzer`.** It's a separate tool from hexcavator and
  shares the libraries. Its subcommands:

  | Subcommand | Shows |
  |---|---|
  | `summary` | the exception, and the faulting frame and module |
  | `threads` | all threads |
  | `stack` | a thread's frames |
  | `locals` | a frame's variables |
  | `memory` | bytes, with their provenance |
  | `disasm` | a frame's code, with values |
  | `modules` | each module, and why its symbols were or weren't found |

  It also has `serve` and `mcp`, like hexcavator's. Every subcommand has
  JSON output.
- **4.2 `editors/vscode/dump-analyzer`** ("Dump Analyzer"):
  - **An inline DAP adapter** (`DebugAdapterInlineImplementation`) that
    turns DAP requests into `serve` requests. It gives VS Code's own
    threads, call-stack, variables, and watch panes, and the debug console.
    - `variablesReference` stands for (thread, frame, path), so it stays
      stable.
    - `readMemory` opens hexcavator's hex view on dump memory.
    - `evaluate` runs subcommands from the debug console.
  - **A disassembly view** with every instruction annotated with the values
    the dump knows at that point in the frame. It marks the PC, inline
    frames, and bytes from image files.
  - **Shared views.** The disassembly and graph views (`media/disasm-view.js`,
    `media/graph.js`) move to a shared place both extensions build from,
    not copies.
- **4.3 The web build.** The host (`editors/web/host`) serves both tools and
  gains a debug-UI shim:
  - `vscode.debug`, and adapter registration
  - threads, call-stack, and variables panes
  - a debug console
  - progress
- **4.4 MCP tools** (`crash_summary`, `stack`, `locals`), so an agent can
  triage a dump.
- **4.5 Releases:** the `dump-analyzer-release.yml` workflow on
  `dump-analyzer-v*` tags (the TODO item "A release workflow for each tool").

**Done when:** opening a Windows `.dmp` or a Linux core or Crashpad dump in
VS Code (and on the web) shows the crash, every stack, locals, and the
annotated disassembly.

### Phase 5: instruction access data

Phase 7 needs this. So do the disassembly view (highlighting where a
register is written and read) and the cross-reference scanner.

- **x86** (`tables/*.txt`, `build.rs`):
  - whether each operand is read, written, or both
  - implicit operands: `rDX` for `mul` and `div`; `rCX`, `rSI`, and `rDI`
    for string instructions; `rSP` and stack memory for `push`, `pop`,
    `call`, and `ret`
  - flags read, written, and left undefined
- **ARM64:**
  - first, an operand API, which the decoder doesn't have yet
  - then reads, writes, implicit operands, and NZCV, generated by
    `tools/arm64/extract.py` from LLVM's `(outs)` and `(ins)` lists and its
    `Defs` and `Uses`, and checked against AARCHMRS
- **Oracles:** Intel XED (complete read, write, and flags data for x86) and
  LLVM MC's `MCInstrDesc` (implicit defs and uses), plus fuzzing.

**Done when:** every x64 and ARM64 encoding has a complete write set that
agrees with XED and LLVM. **A missing write leads to wrong answers, not just
unknown ones.**

### Phase 6: the IR and its lifters

- **6.1 `turbonop-ir`** (`crates/analysis/ir`). Values are `Const`,
  `Reg@def`, `Load(address, size)`, `Op(op, args)` at a fixed width, and
  `Unknown`.
  - Ops: wrapping arithmetic, extends, shifts, logic, and compares.
  - Flags are lazy: an instruction that sets them records its operands, and
    a branch's condition is computed from them. That keeps the compared
    values visible, which is what lets phase 7 decide branches.
- **6.2 x64 and ARM64 lifters** for the instructions compilers emit:
  - **Moves:** `mov`, `movzx` and `movsx`, `lea`
  - **Arithmetic:** `add`, `sub`, `inc`, `dec`, `neg`
  - **Logic:** `and`, `or`, `xor`, `not`, and shifts
  - **Compares and conditionals:** `cmp`, `test`, `setcc`, `cmov`
  - **Stack:** `push`, `pop`
  - **ARM64:** `cbz`, `cbnz`, `tbz`, `csel`, `csinc`, `ccmp`, `ldr` and
    `str` and their pairs

  Any other instruction lifts to its write set with `Unknown` values. How
  much of each corpus lifts exactly is measured and reported.
- **6.3 Testing.** A native harness in `tools/` (outside the workspace, so
  outside its rule against `unsafe`) runs random instructions on random
  inputs on real CPUs, and checks the lifters against them: this PC for
  x64, and ARM64 Windows CI or the Mac VM for ARM64.

### Phase 7: how did we get here

- **7.1 `turbonop-paths`** (`crates/debugger/paths`). It works on one frame,
  given its registers and their provenance, its function's CFG, the target,
  and the ABI:
  1. **Enumerate paths** backward from the PC (or, in an outer frame, its
     call site) to the function's entry. Each loop is summarized as zero
     iterations or more, with an exact count when the induction variable
     survives.
  2. **Execute each path** in SSA, from symbolic values at entry (the
     arguments and non-volatile registers).
  3. **Pin the end** to the dump: every `Known` register and every captured
     stack word.
  4. **Solve,** using equalities, inverted operations, intervals, and known
     bits, all modulo 2^n. A contradiction rules a path out. Each path has a
     budget, and when the budget runs out, the path is undecided.
  5. **Report** with the confidence of [Confidence](#confidence):
     - Each branch is taken, not taken, one of the two, or unknown.
     - The paths together are: certain (exactly one path is feasible), one
       of *n* (these are the feasible paths), or likely (a heuristic ranks
       the feasible paths).
     - Every certain and one-of-*n* claim carries its proof.
- **7.2 More evidence:**
  - spilled locals, saved non-volatile registers, and the x64 home space
  - memory captured behind register pointers
  - `DW_OP_entry_value` values at call sites
  - **the dead stack below SP,** skipping the red zone the ABI defines:
    - A stale return address shows a call site probably ran. That is
      likely, never certain: it could be left from an earlier call.
    - Their depth below SP suggests their order.
    - A callee's stale locals are further likely evidence.
- **7.3 Outer frames.** An outer frame is solved with its own registers and
  memory, from its call site. Frames, and the paths within each frame, are
  solved in parallel.
- **7.4 UI:**
  - In the disassembly and graph views, blocks are colored by confidence,
    and each decision expands to its proof.
  - Through line info, decisions read at source level: "`if (p ==
    nullptr)` at line 120 was false (certain)."
  - Each frame has a summary of its path.
- **7.5 Ground truth:**
  - On Windows, test programs run under TTD and crash, and the trace's real
    path is compared with every claim.
  - On Linux, a ptrace single-step tracer in `tools/` (or `rr`) records the
    real path.
  - Wrong certain and one-of-*n* claims must be zero, and the share of
    branches decided is tracked.

### Phase 8: macOS and iOS (best effort, then the Mac VM)

- **The dyld shared cache** (in `turbonop-macho`, or `crates/formats/dyld-cache`
  if it grows large):
  - the header and mappings
  - images, each read as a Mach-O
  - split sub-caches (macOS 12 and later)
  - the `.symbols` file
  - slide info, versions 2 through 5
  - UUIDs
- **Mach-O core files:**
  - `LC_THREAD` state
  - `LC_NOTE` (`addrable bits`, `all image infos`)
  - segments
- **Fixes for Apple:** arm64e PAC, `_sigtramp`, and Crashpad's Mach-O module
  identities.
- **Tests, at first:** built from the specifications and from dumps of
  synthetic programs where we can make them.
- **On the rented Mac VM:**
  - caches from several OS versions
  - lldb as the oracle
  - crashing test programs, for real dumps
  - fixes for what they show

### Phases 9, 10, and later

- **9:** a port of SLEIGH and the p-code runtime (from the C++ in Ghidra's
  decompiler tree). A p-code-to-IR translator then gives the other
  architectures to phase 7, and each side checks the other: the hand-written
  lifters against SLEIGH, and our decoders against SLEIGH's.
- **10:** the decompiler (Ghidra's actions and rules, and its control-flow
  structuring), with values from the dump shown in its output.
- **Later:**
  - x86 and ARM32 path analysis
  - live targets
  - NatVis
  - a triage summary like `!analyze`
  - heap walkers
  - managed code (through the DAC)

## Risks

- **Wrong claims.** A missing write, a lifter bug, or a solver bug produces
  a confident claim that is false. The defenses:
  - complete write sets, checked against XED and LLVM
  - lifters tested on hardware
  - proofs shown with every claim
  - TTD and single-step ground truth
  - zero tolerance for wrong claims
- **Fetching.** Fetches decide how fast a dump first opens. The defenses are
  parallel fetches, measuring them, and being ready to change transports.
- **Breakpad's quirks:**
  - its old ARM64 context, which the context decoder doesn't read yet
  - modules without build IDs
  - the `/proc` streams
- **Apple.** The dyld cache changes often, and nothing on macOS or iOS can
  be validated until the Mac VM.
