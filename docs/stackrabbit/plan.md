<!-- The plan as approved on 2026-11-17, moved here from its Claude doc
(https://claude.ai/artifact/F4XmEu7SUdMkiHdi9Le1Ea) on 2026-11-25. Where
the work stands, and the decisions made since, are in README.md; this is
the plan as written then. -->

# Stackrabbit: a plan for a complete WPA replacement

2026-11-17

## Summary

Stackrabbit is a VS Code extension, backed by a native Rust engine, that does everything Windows Performance Analyzer (WPA) does, with every click answered within one frame. It starts with ETW traces and is built to open traces from other tools later.

"Complete" means three things:

- **Every WPA analysis people actually use.** Graphs and tables for CPU, waits, disk, file, memory, power, GPU, networking, generic events and .NET.
- **WPA's interaction model.** Configurable tables with grouping and aggregation columns, linked graphs, time selection shared by every view, presets, stack tags, and import of existing `.wpaProfile` files.
- **WPA's reach.** Any ETL that WPA opens, opens here, with numbers that match WPA's.

The plan is eight milestones. The first makes one thing excellent: sampled CPU usage in a stack tree, on real multi-GB traces, measured against WPA and Microsoft Profile Explorer. Each later milestone adds a family of analyses on the same engine and protocol, so speed is designed in once, not retrofitted.

We already have about 10,000 lines of the engine: an ETL reader, kernel event decoders, a column-based trace model with an on-disk index, and a lazy stack tree. None of it is connected to a UI yet.

## Goals, non-goals and principles

**Goals**

1. Replace WPA for day-to-day investigations: a WPA user should never need to switch back for any analysis listed in the data catalog below.
2. Respond instantly. Every click, expand, sort, scroll and selection updates what is on screen within the budgets below, on traces of 10 GB and more.
3. Live in the editor. Open an `.etl` from the Explorer, jump from a frame to its source line, and keep investigations next to the code.
4. Run anywhere VS Code runs. The engine reads ETL files directly, with no Windows APIs, so traces can be analyzed on Windows, macOS and Linux, and over Remote SSH or WSL where the trace lives.
5. Match WPA's numbers. Where WPA and Stackrabbit show the same table, the numbers agree, and the test suite proves it.

**Non-goals**

- Flame graphs. Stack trees, tables and timeline graphs carry the analysis.
- A pixel-for-pixel copy of WPA. We keep its model (tables, columns, graphs, selection) and improve its rough edges.
- Live, real-time ETW sessions in the first release. Files only; live sessions are on the Future list.
- Integration with hexc or the Hexcavator extension. Stackrabbit is its own tool, sharing only library crates.

**Principles**

- **Never block the UI.** Work that can't finish within budget runs in the background, shows progress, and can be cancelled by the next click.
- **Compute only what is visible.** The UI asks for the rows and pixels on screen. The engine never builds a whole table to show 40 rows.
- **Pay once, at import.** Indexes, sorted columns, time summaries and symbol tables are built once and saved, so reopening a trace is nearly free.
- **One engine, one protocol.** Every table and graph is a query over the same model through the same viewport protocol. A new analysis is new data plus a new query, not new plumbing.
- **Measure everything.** Every budget has a benchmark on real traces, run in CI, compared against WPA and Profile Explorer.

## Performance budgets

Interactive operations finish within one frame (16 ms) as the target and 50 ms as the limit, measured at the 95th percentile on a 10 GB reference trace. All budgets are soft targets: we work toward them step by step and adjust them as measurements come in.

Reference machine: the development laptop, an Intel Core Ultra 5 335 (8 cores), 32 GB RAM, Samsung 990 PRO NVMe disk. Time is from the input event to the new pixels.

| Operation | Target | Limit | Notes |
|---|---|---|---|
| Scroll a table or tree | 8 ms | 16 ms | Rows on screen only |
| Expand or collapse a tree node | 16 ms | 50 ms | Children computed on demand |
| Sort by a column | 16 ms | 100 ms | Over the visible level of a tree; full tables may show progress |
| Change grouping columns (drag past the gold bar) | 50 ms | 200 ms | New query, top level only |
| Select a time range: every open view refreshes | 50 ms | 150 ms | Uses time summaries; edges scanned exactly |
| Zoom or pan a graph | 8 ms | 16 ms | Drawn from precomputed summaries |
| Hover tooltip | 4 ms | 16 ms | Local to the webview |
| Search a table (Ctrl+F) | 50 ms | 300 ms | First match shown first; rest streams |
| Reopen an indexed trace: first graph drawn | 300 ms | 1 s | Memory-maps the saved index |
| First import, 1 GB ETL | 2 s | 5 s | All cores |
| First import, 10 GB ETL | 15 s | 40 s | CPU graph and process list appear within 2 s; the rest streams in |
| Symbols for a trace's hot modules, cached | 1 s | 3 s | Only functions the trace's frames hit |
| Engine memory, indexed 10 GB trace | 2 GB | 4 GB resident | The index is memory-mapped, not loaded |

For context, WPA routinely takes minutes to load a large trace and seconds per interaction. Profile Explorer claims "near-instant UI interaction" on ETLs over 10 GB, but publishes no numbers. We will publish ours.

## Where we start

The engine's lower layers exist and are tested; the query layer covers CPU only; nothing is connected to a UI. All the tests pass today, but the test traces are under 3 KB, so speed at scale is unproven.

| Crate | Has | Missing for Stackrabbit |
|---|---|---|
| `hexcavator-etl` | ETL buffers, logfile header, per-processor merge in timestamp order, LZ77 compressed buffers, a writer for tests; checked against `ProcessTrace` | Parallel buffer parsing; multi-file merge (separate user and kernel ETLs); lost-event and buffer-loss reporting |
| `hexcavator-etw` | Fast kernel decoders (process, thread, image, CPU sample, context switch, ready thread, stack walk, image identity); classic kernel event layouts by name; manifest parsing from `WEVT_TEMPLATE`; checked against TDH | TraceLogging, WPP, EventSource, non-kernel MOF classes; disk, file, memory, registry, network, power and GPU events |
| `hexcavator-trace` | Platform-neutral model: processes, threads, images with lifetimes; samples, switches and ready threads as columns; interned stacks; a validated `.hxtrace` index | Every other event family; generic events; time summaries; memory-mapped index instead of reading it whole |
| `hexcavator-trace-analysis` | Lazy stack tree with inclusive and exclusive weights, inversion, callers and callees, hot path; CPU sampled and precise sources; stack tags (WPA XML); folding rules; frames by function, module or address | A general table engine (grouping, aggregation, sorting over any columns); time-range filtering; graph series; every non-CPU analysis |
| `hexcavator-symbolize`, `-pdb`, `-debuginfo` (shared) | PDB reading, functions, lines and inline frames at addresses | Symbol server download and cache; per-trace symbol tables; .NET and JIT frames |

The shared crates stay in this workspace. Stackrabbit adds its own engine binary and extension and changes the shared crates only through their public interfaces.

## Architecture

A native Rust engine process holds the trace and answers queries. The VS Code webview only draws, and asks only for what is on screen.

Requests flow right through the layers and answers flow back as small binary row batches. The query engine is the new core; the layers under it extend existing crates.

The layers, from the screen down to the disk:

| Part | Role |
|---|---|
| Webview | Canvas tables and graphs; asks for visible rows and pixels only |
| Extension host | Starts the engine; relays binary messages |
| Engine: session protocol | Viewport requests, cancellation |
| Engine: query engine (the new core) | Tables, stack trees, graph series, time-block summaries |
| Engine: trace model and index | Columns, stack trie, memory-mapped |
| Engine: importers | ETL reader, ETW decoders; other formats later |
| Engine: symbols | PDBs, symbol servers, cache |
| On disk | The trace (read once), its index beside it, the symbol cache |

- **Processes.** The extension host starts one `stackrabbit` engine per VS Code window, which can hold several traces. A crash in the engine never takes down VS Code, and the extension restarts it.
- **Why native, not WebAssembly.** The engine needs memory-mapped files, every core, and network access for symbol servers. It also runs remotely, next to the trace.
- **Protocol.** Length-prefixed messages over stdin and stdout: a small JSON header plus a binary payload. Requests name a view (table, grouping, columns, filters, time range, sort, expanded nodes) and a viewport. A view is a plain value, so results cache by view, and undo is just the previous view.
- **State.** Each open trace has its model, index and symbol table. Each analysis view has its query cache. Closing a tab frees its cache.
- **Index.** Written beside the trace (or in a cache folder if the trace's folder is read-only), versioned, and validated against the trace's size and hash before use, as `.hxtrace` already is.

## How the speed is achieved

Speed comes from doing the expensive work once at import, storing it in shapes that queries can slice, and answering only for what is on screen.

| Operation | Technique |
|---|---|
| Import | ETL buffers are independent, so they are decoded in parallel across all cores and merged by timestamp afterwards. Strings (process names, file paths, event names) are interned once. Stacks go into the existing deduplicated trie. |
| Reopen | The index is a set of flat, aligned arrays, memory-mapped and validated lazily per section. Reopening reads a header, not the trace. |
| Time-range selection | Events are stored in time order in blocks of about 64K. Each block keeps a pre-aggregated summary (for example, sample count per stack and thread). A range query merges whole-block summaries and scans only the two partial blocks at its edges, so cost depends on the number of blocks, not events. |
| Graphs | Each graph series keeps a pyramid of per-interval totals (1 µs, 10 µs, … up to 1 s buckets). Drawing picks the level closest to one bucket per pixel, so zooming never touches raw events. |
| Stack trees | The existing lazy tree: expanding a node partitions only that node's instances. Results are cached by query, so collapsing and re-expanding is free. |
| Tables | A table is a query (filter, group, aggregate, sort) over columns. Grouped tables only materialize the expanded levels. Flat tables of millions of rows sort with a parallel radix sort over row indices; the visible page is returned first. |
| Cancellation | Every query carries a token. A new click cancels work the old view no longer needs, so nothing queues up behind a slow query. |
| Transport | The webview asks for a viewport (rows 0 to 60, or 1,600 pixels of a graph). Answers are compact binary row batches, not JSON text, sent as transferable buffers. A typical answer is a few kilobytes. |
| Rendering | Tables and graphs draw on canvas, not DOM nodes per row, so cost is constant in table size. |
| Symbols | After import we know every address the trace's stacks hit. We resolve only those, in parallel, and save the result with the index, so frames have names on every later open. |

The hard cases, honestly: sorting a 100-million-row flat event table, or regex search across all of it, won't fit in 50 ms. Those show the first page as soon as it is ready, show progress for the rest, and never freeze.

## WPA's interaction model, kept

WPA's strength is one general model: every analysis is a table of columns plus a graph of the same data, and every view shares one time selection. Stackrabbit keeps that model whole, so WPA users already know how to drive it.

| Concept | What it does | Stackrabbit |
|---|---|---|
| Graph Explorer | A tree of every graph the trace has data for, by category (Computation, Storage, Memory, …) | A sidebar listing only graphs with data, with live thumbnails |
| Analysis view | Graphs and tables stacked on one shared time axis | One editor tab per analysis view; several per trace |
| Columns | Each table has many columns; users choose, reorder and resize them | Same, with a column chooser and search |
| Gold bar | Columns left of it group rows (a tree); columns right of it are aggregated | Same, dragged the same way |
| Blue bar | Columns that become the graph's series | Same |
| Aggregation per column | Sum, count, average, min, max, unique count, peak | Same set |
| Time selection | Selecting in any graph or table selects that time range everywhere | Same, plus a history to step back and forward |
| Zoom to selection | Narrows the whole view to the selected range | Same, instant |
| Filter to selection, or out | Keeps or removes the selected rows | Same; filters show as removable chips |
| Highlight | Colors selected rows' time ranges in every graph | Same |
| Presets | Named column layouts per table | Same, plus import of WPA's built-in presets by name |
| Profiles (`.wpaProfile`) | Saved set of views, presets and stack tags | Import of existing WPA profiles; our own profile format alongside |
| Stack tags | XML rules that name stacks by their frames | Already parsed; becomes a column |
| Regions of interest | XML that defines named intervals from event pairs | Same format, as a table and a graph |
| Symbols | Load symbols for some or all modules | Automatic and background, with a per-module override |
| Trace properties | System information, providers, lost events | A summary page when a trace opens |
| Export | Copy rows, export a table to CSV | Same, plus a command line export for automation |

## Data catalog

These are the WPA analysis families Stackrabbit must cover, the events behind each, and the milestone that delivers it. Only the first three rows are partly built today. Provider and event names are from memory of WPA and the kernel's event classes, and each milestone starts by confirming them on real traces.

| Category | Graph or table | ETW sources | Milestone |
|---|---|---|---|
| Computation | CPU Usage (Sampled) | PerfInfo SampledProfile, StackWalk | M1 |
| Computation | CPU Usage (Precise), with waits and ready time | Thread CSwitch, ReadyThread, stacks on both | M1 |
| System Activity | Processes, Threads, Images (lifetimes) | Process, Thread, Image events and their rundown | M1 |
| System Activity | Generic Events (any provider, decoded fields) | Every provider: manifest, TraceLogging, MOF, WPP | M2 |
| System Activity | Stacks (any event with a stack) | StackWalk joined to its event | M2 |
| System Activity | Marks, Regions of Interest | Mark events; ROI definition files over any events | M2 |
| Computation | DPC and ISR | PerfInfo DPC, ISR | M3 |
| Storage | Disk Usage (service time, queue, I/O time) | DiskIo read, write, flush and their init events | M3 |
| Storage | File I/O (create, read, write, close, with paths) | FileIo events, file name rundown | M3 |
| Storage | Mini-filter delays | File system filter manager events | M3 |
| Storage | Registry | Registry events | M3 |
| Memory | Hard Faults | PageFault hard faults | M3 |
| Memory | Virtual Memory (commit lifetimes) | VirtualAlloc, VirtualFree | M3 |
| Memory | Heap Allocations (outstanding, leaked) | Heap tracing provider | M3 |
| Memory | Pool | Pool allocation and free | M3 |
| Memory | Memory Utilization, Total Commit | Kernel memory information events | M3 |
| Memory | Resident and Reference Set | Working set and page access events | M5 |
| Power | CPU Idle States, CPU Frequency | Kernel processor power events | M5 |
| Power | Battery, Energy Estimation | Kernel power and energy estimation providers | M5 |
| Video | GPU Utilization, GPU Hardware Queue | DxgKrnl | M5 |
| Video | Frames and presents | DXGI, Desktop Window Manager | M5 |
| System Activity | UI delays, window in focus | Win32k | M5 |
| Networking | TCP/IP | Kernel TcpIp events, TCPIP provider | M5 |
| .NET | GC pauses and heaps, JIT, exceptions | .NET runtime provider and its rundown | M5 |
| System Activity | Boot and resume phases | Kernel boot and power transition events | M5 |
| Computation | Hardware counters (PMC samples, cache misses) | PerfInfo PMC samples | M5 |

Any family WPA has that isn't listed here is out of scope until someone asks for it. Generic Events (M2) is the safety net: any event, from any provider, can always be viewed as a table.

## Event decoding coverage

To show every event as WPA does, Stackrabbit must decode five kinds of event schema. It decodes two of them today, and the hardest part is finding the schema, not reading the payload.

| Schema kind | Where the schema comes from | Today | Plan |
|---|---|---|---|
| Classic kernel (MOF) | Fixed layouts per event class and version | Decoded, checked against TDH | Extend to every class the catalog needs |
| Manifest-based providers | The `WEVT_TEMPLATE` resource in the provider's binary | Parser done, checked against TDH | Find the binary: on the analysis machine, a bundled schema set, or the trace's own metadata |
| TraceLogging | Inside each event (self-describing) | Not started | Implement; no outside schema needed. Covers much of modern Windows and many apps |
| EventSource (.NET) | Self-describing events, or a manifest the provider writes into the trace | Not started | Implement both forms |
| WPP (driver tracing) | TMF files, or PDBs that contain them | Not started | Read TMFs from PDBs through our PDB crate; show undecoded events as raw bytes otherwise |
| Non-kernel MOF classes | WMI class definitions on the machine | Not started | Low priority; show as raw bytes with sizes |

**Where manifests come from.** WPA decodes manifest-based events with schemas installed on the analysis machine, which is why traces from other machines often show unknown events. Stackrabbit will look in four places, in order:

1. Schemas Stackrabbit captured when it recorded the trace. Recording reads the manifests of every provider in the session from the local machine and saves them beside the trace, so traces we record are always fully decodable, on any OS.
2. The trace itself. Some recorders write provider metadata into the ETL; what WPR's merge embeds is to be checked on the corpus.
3. Provider binaries named in the trace, if present locally, read through our PE crate.
4. Folders the user adds in settings.

We do not bundle schemas extracted from Windows: redistributing Microsoft's manifests may not be allowed by the Windows license, and they change between Windows versions.

**Formats inside a trace file.** The reader must also handle: multiple ETLs recorded together (unmerged user and kernel files), 32-bit and ARM64 traces, every clock type, circular-buffer traces, and lost events, which must be reported where they make numbers wrong.

## Symbols

Symbols load automatically in the background, and the trace is usable at once: frames show module and offset until their names arrive, then the visible rows update in place. In WPA, symbol loading is a manual step that can take minutes.

- **Finding PDBs.** From the image identity events in the trace (PDB name, GUID, age), through `_NT_SYMBOL_PATH` syntax: local folders, symbol servers, and a local cache in the standard downstream layout. Downloads run in parallel, compressed (`.pd_`) files included.
- **Resolving only what is needed.** After import, the set of distinct addresses in the trace's stacks is known. Only those are resolved, through the shared symbolize crate, and the results are saved with the trace's index. Reopening needs no PDBs.
- **Inline frames.** Our symbolizer already reports inline frames at an address. Stackrabbit shows them as their own tree levels, marked as inlined.
- **Kernel and drivers.** The same path; kernel image addresses come from the trace's image events.
- **.NET JIT code.** Method names and ranges come from the .NET runtime's method load and rundown events. ReadyToRun and NGen images resolve through their own PDBs.
- **Other JIT code.** JavaScript engines and others that emit ETW method events, after .NET.
- **Source lines.** Each frame keeps its file and line, which is what "go to source" needs.
- **Missing symbols.** Each module reports why it has no names (not found, mismatched, stripped), with a button to retry or point at a file.

**Cache and download speed.** Downloaded symbols go into `c:\jm\symbols`, in the same symbol-store layout WinDbg uses, so WinDbg and Stackrabbit share one cache (a configurable path for other users). Downloads aim to saturate the network connection: they start during import, as soon as the trace's image identity events are read; they run with high concurrency over reused HTTP/2 connections; and modules are fetched in order of their sample counts, so the frames people see first get names first.

## The VS Code extension

Opening an `.etl` file opens Stackrabbit as a custom editor, and everything else follows WPA's layout inside VS Code's own frame.

- **Trace summary page** on open: duration, machine, processors, providers, lost events, and the graphs the trace has data for.
- **Graph Explorer** in the side bar: every available graph by category, each with a live thumbnail. Click to add it to the current analysis view.
- **Analysis views** as editor tabs. Each holds a stack of panes (graph above, table below, or either alone) on one shared time axis. Panes can be split side by side to compare.
- **Tables**: column chooser, gold and blue bars, drag-to-group, per-column aggregation, sort, filters as chips, Ctrl+F search, expand hot path, copy, export.
- **Graphs**: line, stacked area and bar, with time selection, zoom, pan and highlight. No flame graphs.
- **Stack frames**: go to source (opens the file at the line), view disassembly at the address, callers and callees of a function, and focus on one function.
- **Command palette** for every action, and keyboard shortcuts close to WPA's where they don't clash with VS Code's.
- **Themes**: VS Code's light, dark and high-contrast colors, with a color-blind-safe series palette.
- **Several traces at once**, with a side-by-side comparison view (see Beyond WPA).
- **Remote**: the engine runs where the trace is (Remote SSH, WSL, Codespaces), and only viewport answers cross the network.

**Recording.** The Stackrabbit side panel records traces, as WPR does, with checkboxes for what to capture (CPU, waits, disk, file, memory, heap for a chosen process, and so on), a start and stop button, and the new trace opening as soon as it stops. The detailed design comes with the milestone that builds it.

All drawing is in the webview on canvas. The extension host only starts the engine and relays messages.

## Beyond WPA

Parity makes Stackrabbit a replacement; these make it the better tool. Most come cheaply from the engine design or from crates we already have.

- **Trace comparison.** Two traces, or two time ranges of one trace, in one tree with before, after and difference columns. Regressions sort to the top.
- **Wait chains and critical path.** From any slow interval, follow who readied each waiting thread back to the root cause, as a table you can drill into. Precise CPU data already holds the links.
- **Source and disassembly.** Go from a hot frame to its source line in the editor, or to its disassembly with sample counts per instruction, using our x86, ARM64 and ARM32 disassemblers.
- **Inline frames** as real tree levels.
- **Shareable views.** A view (trace, query, selection) as a short link or file another Stackrabbit user can open on the same trace.
- **Command line.** `stackrabbit` answers the same queries without the UI, as tables or JSON, for CI checks such as "fail if this function's CPU time grew more than 10%".
- **Assistant access.** An MCP server over the same queries, so an AI assistant in the editor can investigate a trace with the user.
- **Any OS.** Analyze Windows traces on macOS and Linux.

## Ideas from Profile Explorer

Profile Explorer's best ideas are about going down into one function: its instructions, source lines and loops, annotated with time. WPA stops at the function. We should take most of that depth, and VS Code makes several of them better, because the source opens in the real editor.

| Feature in Profile Explorer | Take it? | In Stackrabbit | Milestone |
|---|---|---|---|
| Annotated assembly: time per instruction, the slowest colored red to yellow, the top three flagged, jump to next slowest | Yes | Our disassemblers, with sample counts per instruction | M6 |
| Basic blocks and loops recovered, each block labeled with its total time, plus a control-flow graph | Yes | Hexcavator already draws basic-block graphs; reuse that code | M6 |
| Inlinees per instruction, and "time by inlinee" within a function | Yes | Our symbolizer already reports inline frames | M6 |
| Call targets, including which functions an indirect or virtual call actually reached, from the samples | Yes | Derived from stacks: return address to callee | M6 |
| Source view with time per line, assembly expandable under each line | Yes, in the real editor | Heat in the gutter and line backgrounds of the actual source file in VS Code, not a copy | M1 (line time), M6 (assembly under lines) |
| Time per statement (a whole loop, an if branch, a switch case) by parsing the source | Yes | CodeLens above each loop and branch with its total | M6 |
| Source files from source servers and SourceLink, with remembered path mappings | Yes | Same, with mappings stored in workspace settings | M6 |
| Filter a function's annotations to one call path (instance) or to some threads | Yes | Instance and thread pickers on the annotated views | M6 |
| Hot functions list: every function by self time, with a modules panel | Yes | A built-in preset of the CPU table; nearly free | M1 |
| Hover a function for its hottest call path; hover the timeline for the hottest stack at that moment; pin popups | Yes | Hover cards in tables and graphs | M1 |
| Per-thread CPU timeline, sorted by time, with include, exclude and same-name thread filters | Yes | A preset of the CPU graph, plus thread filter chips | M1 |
| Mark a function's samples in color on the timeline | Yes | Highlight by function, in any graph | M1 |
| Copy rows as an HTML and Markdown table | Yes | Copy as Markdown, for pull requests and issues | M1 |
| Symbol options: skip modules with few samples, don't retry symbols that failed before, cache processed symbols, a report of why each module has no symbols | Yes | Folded into Symbols | M1 |
| Kernel and user stacks joined into one stack | Yes | Already how ETW stacks are modeled; verify | M1 |
| CPU performance counter columns in function, assembly and source views | Yes | With the hardware counters work | M5 |
| Back and forward history per tab; recent traces reopen where you left off | Yes | VS Code's navigation history; state saved per trace | M1 |
| Flame graph | No | Out of scope |  |
| Pick one process at load time, then load only that process | No | We load the whole trace once; filters are instant |  |
| Excel export | No | CSV and Markdown are enough |  |

One difference to watch: Profile Explorer asks for a single process when a trace is loaded and processes only that. That is likely part of why it feels fast. We must be fast on the whole trace, which is a harder bar, and our benchmarks should compare both ways.

Source: the [Profile Explorer documentation](https://github.com/microsoft/profile-explorer/tree/main/docs/docs) in its repository (assembly, source, summary, timeline, caller and callee, and trace loading pages).

## Correctness and testing

A fast tool with wrong numbers is worse than a slow one, so every table is checked against WPA's own numbers on real traces, and every budget is a benchmark that can fail the build.

- **WPA as the oracle.** `wpaexporter` (part of the Windows Performance Toolkit) exports any WPA table to CSV with a given profile. For each table we support, a test exports the same table from WPA and from Stackrabbit and compares them row by row. Differences are either fixed or documented as deliberate.
- **TDH and ****ProcessTrace**** as oracles** for decoding, as the existing `tools/etwdump` already does.
- **Trace corpus.** Real traces recorded on the reference laptop by tools/stackrabbit-corpus/record-corpus.cmd into a private folder. They never leave the laptop, and nothing derived from them is committed. Small traces (seconds each) cover each WPR profile, compression, an unmerged xperf log and 8 kHz sampling; large ones trace full builds of this repository, several GB each. ARM64 and other Windows versions come later.
- **Small committed fixtures** made with the existing ETL writer, for unit tests that run anywhere.
- **Fuzzing** of every parser and of the index reader, extending `apps/fuzz`. A hostile trace or index is an error, never a crash.
- **Benchmarks** for each row of the budgets table, run in CI on fixed hardware and compared against the previous release. A regression beyond 10% fails the build.
- **Head-to-head runs** against WPA and Profile Explorer on the corpus each milestone, published with the release notes.
- **UI tests** for the extension, extending the harness the Hexcavator extension already uses.

## Other trace formats, later

The trace model is already independent of ETW, so each new format is an importer into the same model, and every view works on it unchanged. The rule while building ETW support: nothing above the importer may depend on ETW.

Likely candidates after 1.0, in a suggested order:

1. **.NET EventPipe** (`.nettrace`): the same events as .NET's ETW provider, recorded on any OS.
2. **Linux ****perf.data**: samples, context switches, tracepoints.
3. **Perfetto and Chrome traces**: Android, ChromeOS, Chrome.
4. **macOS Instruments** (`.trace`) and **simpleperf** (Android).

Views that need data a format lacks are hidden for that trace, just as WPA hides graphs with no data.

## Repository, packaging and release

Stackrabbit is built in the Hexcavator workspace on the `stackrabbit` branch (worktree `c:\jm\dev\stackrabbit`) and ships as its own VS Code extension. It stays in this repository for good, and all crates, shared or not, live in its one crates folder.

| Path | What it holds |
|---|---|
| `apps/stackrabbit` | The engine binary: the session protocol, the command line and the MCP server |
| `crates/trace/*`, `crates/formats/etl` | The existing trace crates, extended in place |
| `crates/trace/trace-query` (new) | The table engine: filters, grouping, aggregation, sorting, time summaries |
| `crates/trace/symbols-server` (new, name to be decided) | Symbol server download and cache |
| `editors/vscode/stackrabbit` | The extension and its webview |
| `tools/stackrabbit-bench` | Benchmarks and the WPA comparison harness |

**Release.**

- One extension per platform with the engine binary inside it, on the same platforms Hexcavator's extension ships for today (Windows, Linux, macOS). The release pipeline, signing and provenance steps are reused.
- Published to the VS Code Marketplace under its own extension name, `stackrabbit`. The publisher is still to be decided. Everything is under the Apache license.
- Previews go out from M1 onward, marked as preview on the Marketplace, so real users drive priorities early.

## Milestones

Eight milestones, each ending in a working tool. M1 is the gate that matters most: it proves the speed claims on CPU analysis before any breadth is added.

| Milestone | Delivers | Exit criteria |
|---|---|---|
| M0 · Foundation | Engine binary, session protocol, .etl custom editor, sampled CPU tree, benchmarks, corpus | The CPU tree opens on the 10 GB corpus trace; budgets measured, reviewed and fixed |
| **M1 · CPU, processes and threads (first preview)** | CPU sampled and precise, processes and threads, graphs, time selection, symbols, stack tags | Budgets met on 10 GB; CPU tables match WPA; faster than Profile Explorer on its own views |
| M2 · System activity, generic events and recording | Any event from any provider (manifest, TraceLogging, EventSource, WPP), stacks, marks, ROIs | Every corpus event decodes or is shown raw; Generic Events matches WPA |
| M3 · Storage and memory | Disk, file I/O, mini-filter, registry, DPC/ISR, hard faults, virtual memory, heap, pool, commit | Each table matches WPA on the corpus, within budget |
| M4 · WPA compatibility and automation | .wpaProfile import, WPA presets by name, CSV export, command-line queries, MCP server | WPA's built-in profiles open with nothing skipped that M1 to M3 cover |
| M5 · Power, GPU, network and .NET | CPU idle and frequency, energy, GPU, frames, UI delays, TCP/IP, .NET GC and JIT, boot, PMC | Every row of the data catalog is covered |
| M6 · Beyond WPA | Trace comparison, wait chains and critical path, disassembly view, shareable views | A regression between two traces is found from the comparison view alone |
| M7 · 1.0 release | Performance and polish pass, documentation, Marketplace release out of preview | All budgets met on the full corpus on every platform; no open correctness bugs |

Each milestone is done only when its exit criteria pass on the trace corpus. The plan has no dates yet: M0's measurements will size the rest, and we'll add estimates then.

- **Order.** M2 comes before storage and memory because Generic Events makes every trace useful, even before its specialized views exist.
- **Previews.** Each milestone from M1 on ships to the Marketplace as a preview, so real use shapes the next one.
- **Flexible middle.** M3 to M6 can be reordered by what users ask for most; M0, M1 and M7 can't.

## Future

Features we want but have deliberately left out of the milestones above.

| Feature | Notes |
|---|---|
| Live (real-time) ETW sessions | Events delivered as they happen, about once a second. First step: a live CPU-by-process view in the side panel while a recording runs. Windows-only, needs admin for kernel events, and the reader must keep up or events are lost. Needs a trace model that grows while it is viewed |
| Triggers | Start or stop keeping events when something happens (a hang, a process start), built on live sessions |
| Other trace formats | See Other trace formats, later |

## Risks

The largest risks are the undocumented parts of ETW and proving the speed claims at scale. Each has a fallback.

| Risk | Effect | Mitigation |
|---|---|---|
| Undocumented event layouts and versions | Wrong or missing numbers for some events | TDH and WPA oracles on the corpus; show raw bytes rather than guess |
| Manifests not available on the analysis machine | Generic events show as unknown | Four schema sources (Event decoding coverage); TraceLogging needs none |
| Budgets not met on huge traces | The core promise fails | Benchmarks from M0; block summaries designed in before features; budgets revised in review if the data says so |
| WPA's numbers depend on undocumented rules (attribution, rounding, interval edges) | Tables disagree with WPA | Compare row by row; document deliberate differences |
| `.wpaProfile` format is undocumented XML | Imported profiles lose settings | Import what maps cleanly, report what was skipped |
| Webview limits (memory, message size) | Stutter on big views | Viewport protocol keeps answers small; canvas rendering |
| Symbol servers are slow or unreachable | Frames stay unnamed | Background loading, cache, per-trace saved symbol tables |
| Microsoft speeds up WPA or extends Profile Explorer | Less reason to switch | Editor integration, any-OS and comparison features hold regardless |
| Scope: WPA is large | 1.0 slips | Milestones ship value in order; Generic Events covers the long tail |

## Open questions

All questions but one were settled on 2026-11-17.

| Question | Decision |
|---|---|
| Budgets | Soft targets, reached incrementally; this laptop is the reference machine |
| Trace corpus | Recorded on this laptop with tools/stackrabbit-corpus/record-corpus.cmd into a private folder; the traces never leave it |
| Recording | Yes: easy setup from the side panel, with checkboxes for what to record |
| Live sessions | Later: on the Future list |
| Event schemas | Captured when Stackrabbit records, plus local lookup; no bundled Windows schemas |
| Symbol cache | `c:\jm\symbols`, in WinDbg's layout, shared with WinDbg; downloads saturate the network |
| Repository | This one, for good (it may be renamed): `apps/stackrabbit`, `editors/vscode/stackrabbit`, crates in `crates/` |
| License | Apache, for everything |
| Profile format | Our own, alongside `.wpaProfile` import |
| Telemetry | None |

**Still open**

- **Marketplace publisher.** To be decided before the first preview; nothing waits on it.
