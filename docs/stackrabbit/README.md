# Stackrabbit

Stackrabbit is a fast trace viewer for VS Code. It aims to cover what
Windows Performance Analyzer (WPA) does and to respond instantly, on traces
of any size. It reads ETW traces first; the name and the model aren't tied
to ETW, so other formats can come later.

- `apps/stackrabbit` is the engine, a Rust program. It provides the session
  protocol the extension talks to, the command line (`tree`, `query`,
  `bench`, `synth`, `symbols`, `profile`) and an MCP server (`mcp`).
- `editors/vscode/stackrabbit` is the extension. Its webview is in
  `media/`, the host side in `extension.js` and `lib/`, and its tests in
  `test/`.
- The shared crates are in `crates/`: `turbonop-etl`, `-etw`, `-trace`,
  `-trace-analysis`, `-trace-synth`, `-symbolize`, and Hexcavator's
  disassemblers (`turbonop-size`, `-code`) for annotated assembly.
- `tools/stackrabbit-corpus` holds the scripts that record the corpus and
  compare Stackrabbit with WPA.

This folder holds:

- [plan.md](plan.md): the plan, approved on 2026-11-17, with its goals,
  budgets, architecture, data catalog, milestones M0–M7 and risks.
- [report-2026-11-18.md](report-2026-11-18.md): the status report after
  the first night's work. It covers how speed and correctness were
  verified, and the judgment calls made.
- This file: the decisions made since, where the work stands, and how to
  resume it.

The backlog is the Stackrabbit section of [TODO.md](../../TODO.md).

## Where it stands (2026-11-25)

All work is on main. The extension version is 0.0.1, publisher
`turbonop`. Nothing has been published.

- **M0, M1:** done, apart from one budget. The first stack tree on the big
  build traces takes 0.29 s against the 50 ms target.
- **M2:** done. Generic events decode from schemas captured at record
  time. Regions of interest and marks are supported.
- **M3:** done: Disk, File I/O, Mini-Filters, Registry, DPC/ISR, Hard
  Faults, Virtual Memory, Heap, Pool, Memory.
- **M4:** mostly done:
  - `.wpaProfile` import, WPA filter queries, CSV and Markdown copy,
    `stackrabbit query`, the MCP server.
  - Left: some WPA filters and region features (see TODO.md).
- **M5:** CPU Frequency, Idle States, GPU and .NET are done. Hardware
  counters, TCP/IP, UI delays, boot, working sets and energy are left;
  each needs a new recording.
- **M6:** started:
  - Done: annotated assembly, with time per instruction, basic blocks and
    loops, the sampled targets of each call, and source lines over the
    instructions. Also done: time by inlined function, and a CodeLens with
    each loop's time in the source editor.
  - Left: trace comparison, wait chains, source servers and SourceLink,
    and annotations by call path or thread.
- **M7:** not started.

Agreement with WPA, checked on the whole corpus by
`tools/stackrabbit-corpus/compare-wpa.py` and `compare-wpa-tables.py`:

- Every process's sampled count and precise CPU time matches on all 16
  traces with CPU data. The one exception: WPA drops 7 samples of
  short-lived processes in s06.
- Sample weights match WPA's to 0.1 µs, except Idle's.
- Disk I/O pairs one for one with WPA's.
- VirtualAlloc, Pool, Memory, Heap and Marks match.
- DPC/ISR runs and durations match.
- The open differences are listed in TODO.md.

### The overnight list of 2026-11-24

This was the working list, in order:

| # | Item | State |
|---|---|---|
| 1–2 | Compare CPU and every other exported table with WPA | Done; differences explained in TODO.md |
| 3 | Mini-filters | Done |
| 4 | Samples without stacks in l03 and l04 | Explained: neither recording contains a build; re-record them |
| 5 | Context switch v5's last 4 bytes | Open: we know what they are not |
| 6 | Marks | Done |
| 7 | Trees in 50 ms on the big traces | Open: 0.42 → 0.29 s on l01 so far |
| 8 | Decoding that scales past 4 threads | Done: workers were waiting on locks (s04 import 19.6 → 14.0 s) |
| 9 | s04's import and reopen budgets | Open |
| 10–11 | Speed of names from image files; saving them in `.hxsyms` | Open |
| 12 | mimalloc | Done on main, by `turbonop-global-alloc` |
| 13 | One set of parallel helpers (`turbonop-par`) | Open |
| 14–15 | The WPA filters and region features left | Open |
| 16–18 | Idle states and frequency, GPU, .NET | Done |
| 19–20 | Hardware counters; TCP/IP, UI delays, boot, energy | Open: need recordings |
| 21–24 | Annotated assembly, blocks and loops, inlinees, call targets, source lines, loop CodeLens | Done |
| 25 | Source servers and SourceLink; annotations by call path or thread | Next |
| 26 | Fuzz the ETL reader, index, saved names, WPA profile reader | Open |
| 27 | Peak memory on the largest traces | Open |
| 28 | 32-bit, ARM64 and unmerged traces end to end | Open |
| 29 | The Edge harness on corpus traces | Open |
| 30 | Status report | Superseded by this file |

## Decisions

- **Separate from Hexcavator.** Stackrabbit is its own program and its own
  extension. Neither is integrated into `hexc` or the Hexcavator extension.
  It stays in this repository for good, with all crates in `crates/`.
- **Name.** "Stackrabbit" is the working name. A rename was discussed and
  nothing was decided; the leading alternative was "Traceblazer". Don't
  rename until it's decided.
- **Publishing.** The Marketplace publisher is `turbonop`. Nothing is
  published, to the Marketplace or anywhere else, until the extension is
  mostly done. Packaging a VSIX locally is fine.
- **License and telemetry.** Apache for everything. No telemetry, ever.
- **No flame graphs.** Use WPA-style stack trees, tables and timeline
  graphs instead.
- **Event schemas.** Schemas are captured at record time (WPR's merge
  writes them into the trace), plus local lookup and folders the user
  names. Schemas extracted from Windows binaries are never bundled, for
  licensing reasons.
- **Symbols.** The cache is in WinDbg's symbol-store layout, shared with
  WinDbg. Its location comes from `stackrabbit.symbolCache`, else
  `_NT_SYMBOL_PATH`'s cache, else WinDbg's default. Downloading asks
  first (`stackrabbit.downloadSymbols`), so tests and scripts don't
  contact Microsoft by accident. Downloads run 16 at a time. Names are
  saved beside the trace (`.hxsyms`), so a reopened trace needs no PDBs.
- **Recording.** Wanted: an easy setup in the side panel, with checkboxes
  for what to record. It hasn't been designed yet.
- **Live ETW.** Deferred to the plan's Future list; it isn't scheduled
  into milestones.
- **Profiles.** Stackrabbit's own profile format is fine alongside
  `.wpaProfile` import.
- **Budgets.** They are soft targets, approached step by step. The
  reference machine is an 8-core Intel Core Ultra 5 335 laptop with 32 GB
  and an NVMe drive.
- **Competitors.** WPA, and Microsoft's Profile Explorer, which claims a
  near-instant UI on ETLs over 10 GB but covers CPU sampling only.

## Rules for the work

- **Traces never leave the machine they were recorded on.** They capture
  everything that ran. This applies to the corpus and anything derived
  from it (`.hxtrace` indexes, `.hxsyms`, WPA exports):
  - Never commit, upload or paste them anywhere.
  - Report aggregates only: timings, counts, pass or fail. Never report
    process names, paths or command lines.
  - Committed fixtures are synthetic, made with the ETL writer
    (`turbonop-trace-synth`), never cut from real traces.
  - Results from real data go in the private repository
    (`hexcavator-private`), not this one.
- **Speed.** Every data-processing step uses every core, or saturates the
  I/O. Check every speed change against something exact: the corpus
  digests, synthetic truth, or WPA.
- **Benchmarks on a busy machine.** Before trusting a timing, check for
  WSL builds (`vmmemWSL` doesn't show per process in Windows) and
  Defender. Record the load beside each timing. Compare before and after
  in the same sitting, with repeated runs.
- **No `unsafe`** unless there's no other way. Every program and test
  allocates with mimalloc (`use turbonop_global_alloc as _;` in each
  root).
- **Test the whole thing.** A parser is implemented and tested completely,
  even when only part of it is needed. Cover every request in the engine's
  integration tests and every tab in the Edge harness.
- **Confidence.** Report a WPA match only when it was compared. Say
  "likely" when that's all it is.

## Resuming on a new machine

1. **Build and test:** `cargo xtask test` for the workspace (it includes
   the repository's policy tests).
2. **Extension tests.** These need Node 22 or newer, and Edge for the
   harness. From `editors/vscode/stackrabbit`:
   - `node test/engine.test.js <stackrabbit>`
   - `node test/{line-heat,protocol,tree-model}.test.js`
   - `python test/harness.py <stackrabbit>`

   Here `<stackrabbit>` is `target/release/stackrabbit.exe`. CI runs these
   in the `vscode-stackrabbit` job.
3. **Run it.** Start VS Code with
   `--extensionDevelopmentPath=editors/vscode/stackrabbit`, then open an
   `.etl` file. For symbols, turn on `stackrabbit.downloadSymbols`, and
   set `stackrabbit.symbolCache` or `_NT_SYMBOL_PATH`.
4. **The corpus is not in the repository.** Record a new one on the new
   machine with `tools/stackrabbit-corpus/record-corpus.cmd` (as
   administrator). It writes to a private folder outside every repository
   (`c:\jm\dev-private\stackrabbit\corpus` on the old machine).
   `collect-data.bat` gathers the measurements that need an administrator.
   The corpus traces are named `s01`–`s14` (small, one subsystem each) and
   `l01`–`l04` (large builds). Re-record l03 and l04, whose builds failed.
5. **Compare with WPA.** With WPA installed, run
   `tools/stackrabbit-corpus/compare-wpa.py` and `compare-wpa-tables.py`
   against the corpus. They use `wpaexporter` and WPA's built-in profiles,
   and need no administrator rights. Their output stays in the private
   folder.
6. **Pick up the work** from TODO.md's Stackrabbit section. The next item
   is source servers and SourceLink.
