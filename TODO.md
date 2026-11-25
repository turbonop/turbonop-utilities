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
- **A new extension's first release.** For 0.0.10, the extension's first as
  `turbonop.hexcavator-vscode`, the Marketplace served one of eight
  platforms' VSIXs: the first, whose validation created the extension; the
  rest were reported published and never served (published after it was
  served, too). An extension that exists takes every platform's (as
  `hexcavator.hexcavator` did). The job waits up to two hours now; a
  release that fails so could publish the missing platforms again.
- **musl's speed.** With mimalloc in musl's allocator's place, the test
  suite still runs about four times as long under musl as under glibc
  (35 s against 9 s, in WSL): musl's `memcpy`, `memchr`, and string
  functions are simple loops. Measure where the time goes; what's fast
  enough to ship in the static Linux build matters more than the tests.

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

## Ideas

- **A size check for pull requests.** A GitHub Action that comments on a
  pull request with the size it adds and why (`-budget` and diffs exist).
- **Size history.** Sizes across many builds: when each regression came,
  and from which commit.
