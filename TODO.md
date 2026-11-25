# To do

What's planned for TurboNOP's tools (Hexcavator so far) and their
libraries, and what's waiting on something. Items
leave this list when they ship.

## TurboNOP's tools

- **One site for every tool's web version.** GitHub Pages has one site per
  repository, and a deploy replaces all of it: hexcavator's release
  deploys `/hexcavator/` (and a root that sends visitors there). Once a
  second tool has a web version, assemble the site from each tool's
  latest release (its site as a release asset) in one workflow, so that
  no tool's release removes another's.
- **A release workflow for each tool.** `<tool>-release.yml` on
  `<tool>-v*` tags, signing through the shared actions
  (`.github/actions/sign-windows`, `sign-macos`), with the release
  environment's tag rule allowing its tags.
- **macOS Intel until its runner goes.** `x86_64-apple-darwin` is built on
  `macos-15-intel`, GitHub's last Intel macOS image (said to go around
  August 2027); macOS Tahoe 26 is the last macOS for Intel Macs, and Rust
  made the target Tier 2 in 1.90. When the runner goes, drop the build
  (or cross-compile it from Apple silicon, untested).
- **Release notes.** A release's notes are GitHub's list of the pull
  requests since the last release; write them from the changes instead
  (what a user would notice, by command).
- **hexcavator on crates.io.** `cargo binstall hexcavator` finds the crate
  on crates.io (its metadata names the release's programs); until it is
  published there (and with it every `turbonop-*` crate it depends on),
  `cargo binstall --git https://github.com/turbonop/turbonop-utilities
  hexcavator` reads it from the repository.
- **Packages where they fit.** GitHub Packages holds container images and
  npm, NuGet, and Maven packages, not programs: a container image of
  hexcavator; the web version as an npm package.
- **winget's token.** Microsoft's enterprise, which owns
  `microsoft/winget-pkgs`, refuses classic tokens that live more than 8
  days, and a fine-grained token can't open a pull request on another
  owner's repository: each release needs a fresh classic token
  (`public_repo`, 7 days) in `WINGET_TOKEN`, or its winget job fails
  (rerun it with `hexcavator-winget.yml` by hand). Find a way that doesn't
  need a person each release: a GitHub App's token, if the enterprise
  takes one.
- **musl's speed.** With mimalloc in musl's allocator's place, the test
  suite still runs about four times as long under musl as under glibc
  (35 s against 9 s, in WSL): musl's `memcpy`, `memchr`, and string
  functions are simple loops. Measure where the time goes; what's fast
  enough to ship in the static Linux build matters more than the tests.

## Dump Analyzer

A post-mortem debugger for Windows, Linux, and macOS crash dumps
(minidumps and core files) that replaces WinDbg for dumps: `apps/dump-analyzer`
(the command line, `serve`, `mcp`), `editors/vscode/dump-analyzer` (the
"Dump Analyzer" extension), and `crates/debugger/*`. The plan, its
decisions, and its status table are in `docs/dump-analyzer.md`. Neither
the app nor the extension is released: publishing is a step of its own,
when they're ready. What's left:

- **Where to pick up.** Everything through the first "how did we get
  here" (x86-64) is merged; the next big step is the IR and its
  lifters, then the corpus run below. Fixtures are rebuilt by
  `tools/minidump/make.cmd x64|x86` (Windows, in the repository's
  `target` directory so no other path lands in them) and
  `tools/coredump/make.sh` (Linux, in WSL); their oracles' output
  (`cdb.txt`, `dv.txt`, `gdb.txt`) sits beside them. Not yet run: the
  nightly exhaustive ARM64 access test on the merged code.
- **Release.** `dump-analyzer-release.yml` on `dump-analyzer-v*` tags (its
  builds, signing, and VSIX packaging, as hexcavator's), and the
  Marketplace's listing for `turbonop.dump-analyzer`.
- **Checked against the corpus.** Stacks and locals on every Windows and
  Linux dump in `hexcavator-private`, against DbgEng, minidump-stackwalk,
  and gdb. Fetching measured there: cold, the first open of the test
  dumps took 43 s (x64) and 168 s (x86) once, warm 0.4 s and 2 s; find
  where the cold time goes (first reads of big system PDBs?) and make it
  fast.
- **Frames.** Windows ARM64 dumps (no fixture yet: cl can build one, but
  only an ARM64 machine runs it); macOS's `_sigtramp`; x86 frames by
  FrameData programs that read `$T0` from a register the walk doesn't
  know. x86 symbols show decorated (`@BaseThreadInitThunk@12`).
- **Values.** x86's virtual frame (`$T0`: the unwinder computes it, and
  doesn't say), TLS, implicit pointers, DWARF's typed operations, NatVis,
  and a value whose pieces are partly available (all or nothing now).
- **Sessions.** `dump-analyzer serve` keeps every dump it opens for its
  life (a session's parts borrow from one another): free them, without
  `unsafe` (owning the parts differently).
- **The extension.** The web host's debug-UI shim (its panes, for the web
  version); a disassembly view with the dump's values beside the code
  (shared with hexcavator's); `paths` and `reconstruct` in the UI, not
  only the debug console.
- **The IR and its lifters.** `turbonop-ir`, and x64 and ARM64 lifters for
  the instructions compilers emit, tested on real CPUs (the access data,
  done for both, gives their write sets).
- **How did we get here.** Now: branches decided by control flow, or by
  the flags' operands where their values survive to the PC, on x86-64.
  Left: values computed from others (the IR's symbolic execution and a
  solver), memory operands past stores that can't alias, the dead
  stack's evidence below the stack pointer, outer frames' stale return
  addresses, ARM64, likely (heuristic) claims beside certain ones, and
  ground truth from TTD and single-step traces.
- **macOS and iOS.** The dyld shared cache; real Mach-O core files and
  Crashpad dumps on a rented Mac VM, with lldb as the oracle (and which
  byte order Crashpad's and Breakpad's Mach-O UUIDs are in: Crashpad's
  is assumed).
- **Instruction access data's questions.** Should `syscall`-like
  instructions write every register (now), or only what each OS's ABI
  lets the kernel? Should `rep` string instructions' memory be their
  whole range (rCX elements)? ARM64's memory accesses have no size yet,
  and MSR SVCR and SMSTART's operands don't decode.
- **SLEIGH, then the decompiler.** Ghidra's, ported.
## Procupine

- **The extension's server test on macOS.** On CI's macOS runner,
  `editors/vscode/procupine/test/server.test.js` waited 20 s for a sample
  event that never came, though `apps/procupine/tests/serve.rs` (the same
  watch, kill and end, from Rust) passes there. The test says which wait
  timed out, the last events, and procupine's stderr; its live part is
  skipped on macOS until that's found and fixed.
- **The system's CPU on macOS.** In `turbonop-process`'s live
  `cpu_usage` test on CI's macOS runner, the system's CPU share over a
  300 ms busy loop was once 0 (this process's own share was right): the
  `host_statistics` load ticks, unchanged or misread. That check is
  skipped on macOS (`crates/system/process/tests/live.rs`) until fixed.
- **A release.** `procupine-release.yml` on `procupine-v*` tags, as
  hexcavator's: signed binaries for each platform, the extension packaged
  per platform with `bin/procupine`, its licenses test and third-party
  notices (serde_json, mimalloc, libc, rustc-hash).
- **An elevated helper.** Without administrator rights (root), other
  users' processes show less and can't be acted on. A second `procupine
  serve`, started elevated on request (UAC on Windows, `pkexec` on Linux,
  an authorization prompt on macOS), that the extension talks to over a
  pipe only it may open.
- **Details on demand.** For the selected process: its modules (DLLs,
  shared libraries), threads (with stacks, symbolized by
  `turbonop-symbolize`), handles and open files, environment, memory
  regions, and network connections. On Windows, its image's version
  information (description, company) from `turbonop-pe`, as Process
  Explorer's Description and Company columns.
- **Finding a handle or module** across every process (Process
  Explorer's Find), on every core.
- **Lifetimes.** Processes too short-lived for a sample, from the
  system's own events (ETW's kernel process provider, Linux's process
  connector, macOS's `kqueue`): a timeline of every process that started
  and exited.
- **More counters:** GPU use (D3DKMT on Windows), per-process network
  bytes, Linux's cgroups and namespaces (which container), Windows' job
  objects, services' names for svchost.
- **macOS, run.** The collector is built and its parsing tested on every
  platform, and CI runs its live tests on macOS; it hasn't been used by a
  person on a Mac.

## Stackrabbit

The trace viewer (`apps/stackrabbit`, `editors/vscode/stackrabbit`). The
plan, the decisions behind it, and where the work stands are in
[docs/stackrabbit](docs/stackrabbit/README.md). Nothing is published
until the extension is mostly done. What's left, roughly in order:

- **Trees in 50 ms** on the big build traces (0.29 s on l01, measured on
  a busy machine): build them from the stack trie, not each stack's
  frames. `loops` (time by loop) costs a whole `functions` pass, 0.35 s
  on l01, and gets faster with it.
- **Import and reopen** budgets on s04 (6.9 GB): 14 s to import now.
- **Saved names for every image**: images named from their own files
  (DWARF, exports) are not saved in `.hxsyms` yet, only PDBs' tables.
- **One set of parallel helpers**: Stackrabbit's `on_every_core` and
  friends against `turbonop-par`.
- **WPA filters and regions left**: the sampled CPU table's DPC/ISR
  column, Disk's Init Time, filters on Regions; regions by Duration,
  endpoints, Parent, SelfNest and Filter.
- **Differences from WPA still open**: Idle's sample weight (WPA counts
  time no sample covers as Idle); DPC/ISR runs of KMDF drivers (WPA
  credits the client driver, Stackrabbit names the framework); WPA leaves
  out each processor's first DPC/ISR run; WPA's CPU Frequency shows
  nominal speeds (Power event 51). GPU and Frequency have no comparison:
  no built-in profile has the columns. Context switch version 5's last 4
  bytes are still unnamed (8c9fa26 says what they are not).
- **Re-record l03 and l04**: neither contains a build (cargo failed at
  once), which is why they have almost no stacks. Delete both, then run
  `record-corpus.cmd large` as administrator.
- **M5 tables left**: hardware counters (PMC samples); then TCP/IP, UI
  delays, boot phases, working sets, battery and energy. Each needs a new
  recording. Done: CPU idle states, frequency, GPU, .NET.
- **M6, the Profile Explorer ideas, left**: source servers and
  SourceLink; annotations by call path or thread; time per branch arm
  (needs LBR or branch counters: samples alone can't say which way a
  branch went). Done: annotated assembly with time per instruction,
  blocks and loops, time by inlinee, sampled call targets, source lines
  over the instructions, and a CodeLens per loop. Gap: no end-to-end
  engine test of `assembly` (it needs a trace naming an image file that
  is here: patch a synthetic trace's image path, size and timestamp to
  testdata's lines.exe).
- **Robustness**: fuzz the ETL reader, the index, saved names and WPA
  profiles; peak memory on the largest traces; 32-bit, ARM64 and
  unmerged traces end to end; the Edge harness on corpus traces, not only
  synthetic ones.
- **Release (M7)**: a `stackrabbit-release.yml` and VSIX packaging. The
  Marketplace publisher is `turbonop`; the name isn't final.

## Every format alike

Every command and analysis should work the same, and as well, on PE,
ELF, Mach-O, and WebAssembly, with a PDB or DWARF. What's left:

- **MIPS16.** MIPS16 code isn't decoded: LLVM's disassembler has no
  decoder for it to generate one from (its records encode MIPS16, for
  clang's `-mips16`, but no decoder tables), nor llvm-objdump text to
  test one against. What's left is a decoder written from the MIPS16e
  specification, tested against clang's own MIPS16 output (`-mips16 -S`
  against `-c`).

## arm64e

- **hexcavator as arm64e.** macOS and iOS 26 run third-party arm64e
  programs (big apps, at least; command-line programs too, to be
  checked). Rust's `arm64e-apple-darwin` is a tier-3 target, not yet
  fully ABI-compatible with Apple's arm64e; build and ship one (a
  universal program with arm64) once it is tier 2, or sooner if it
  proves sound.

## ARM64

- **SMSTART and SMSTOP don't decode.** `D503417F` to `D503477F` (and so
  MSR SVCRSM, SVCRZA, SVCRSMZA) are None: MSR (immediate)'s
  `<pstatefield>` has no SVCR rows, and the SMSTART alias's `<option>`
  doesn't render. LLVM decodes them. (Their access data, zeroing the
  vector state, is in place for when they do.)

## The web version

- **Faster reading by range.** A PDB is read by its streams in the
  browser (as a command asks for them), which keeps clang.exe's whole
  session within 4 GiB, but its reads go through the file system worker
  one at a time (FileReaderSync, then a copy into WebAssembly's memory):
  clang.exe's `types` takes 6 s, against 2.6 s with the PDB read whole.
  Reads on several workers at once, or a stream's blocks read ahead.
- **DWARF by range.** DWARF (an ELF or Mach-O file's, a dSYM's) is still
  read whole in the browser: its sections could be read as a PDB's
  streams are.

## LTCG objects

- **The rest of the IL's records.** A function with a body is read
  whole (its IL's place and size, its types, what it refers to and
  how); other records (declared functions, variables, the kinds of
  record between them) are still found by their heads, and the byte
  that grows with a body isn't known. With every record walked, `hexcavator
  size` could weigh a `/GL` library's functions, and the public
  definitions could be told from inline functions nothing refers to,
  as lib.exe tells them.
- **The other IL sections.** `.cil$ex` (each function's IL), `.cil$sy`
  (its locals), and `.cil$in`: their contents.

## Speed

Every command should be as fast as the hardware allows (on clang.exe,
175 MB with a 1.1 GB PDB, as measured). What's left:

- **x86 decoding** (60 ns an instruction, on clang.exe's code): building
  the instruction is half of it (`Decoder::build`: its operands, and the
  instruction returned), finding the entry among an opcode's candidates
  a sixth. (Building the operands in the instruction returned, rather
  than gathering them and copying them in, was slower.) What would pay:
  operands built lazily, or a decode for cross-references and
  fingerprints that builds only memory operands and branch targets.
- **A program's symbols** (1.1 s under most commands on clang.exe): the
  PDB's symbols read (0.43 s, on every core), then named (0.15 s, on
  every core) and resolved (0.45 s); what is left is a few sequential
  steps of 50 ms each.

## Size

hexcavator itself has grown: 12.8 MB to 16.3 MB on Windows x64 from
0.0.7 to 0.0.10 (`hexcavator size -d crates,symbols` on each, with its
PDB), and 17.1 MB (glibc, dynamic) to 22.9 MB (musl, static) on Linux
x64. Where the 3.5 MB went:

- **The new decoders, about 1.1 MB.** PowerPC's code (365 KB) and
  LoongArch's (265 KB); MIPS's tables (about 275 KB: three of 64 KB and
  more) and RISC-V's (106 KB), anonymous read-only data (`anon.*`, by
  their objects). Tables of exactly 64 KB are likely indexed by 16 bits
  and mostly empty: a denser form (by ranges, or a second level) could
  take a fraction.
- **Generic code, about 1.15 MB**, counted to `core` (+668 KB), `std`
  (+306 KB), and `alloc` (+180 KB): our generics instantiated for each
  type, much of it likely `turbonop_par`'s (+111 KB itself, new)
  closures and sorts, a copy per caller. Which, by symbol; then fewer
  copies (a non-generic core taking `&dyn Fn`, as `map_range` could).
- **Features, about 0.5 MB**: the app (+178 KB), `turbonop_size`
  (+205 KB), `turbonop_debuginfo` (+85 KB), and the rest.
- **Base relocations, +88 KB**: more pointers in data (vtables, tables
  of strings).

## turbolib

- **The rest of phase 0 of [the plan](docs/turbolib.md).**
  - Whether incremental linking uses `.chks64`: neither linker checks it
    in a full link. And `.retplne`.
  - Which sections of ASan and SanCov objects are per object.
  - Whether `/DEFAULTLIB` directives that differ from the rest of a
    library's count as side effects for `--pull=preserve`.
- **Phase 1: the writers.**
  - A COFF object model that round-trips byte-identically.
  - The Microsoft archive writer, with `lib.exe`'s member order (the
    reverse of its command line).
  - CodeView symbol records' index fields: types have them, in
    `turbonop_codeview::type_refs`.
  - `turbolib` as `lib` with `--fold=none`.
- **A structural PDB comparison.** `link.exe`'s PDBs differ between two
  links of the same inputs, so `relink.py` compares images only. Compare
  PDBs by content (publics, procedures, lines, inlinees, locals, types by
  structure), for phase 1's check and phases 2 to 4.
- **The analyzer's speed.** It reads 13.7 GiB in 14.7 s on 8 threads, and
  scales 3.2× from 1 thread to 8 on this PC. Its shards now read only
  their own units and records. Measure again, and profile with
  Stackrabbit.

## Ideas

- **A size check for pull requests.** A GitHub Action that comments on a
  pull request with the size it adds and why (`-budget` and diffs exist).
- **Size history.** Sizes across many builds: when each regression came,
  and from which commit.
