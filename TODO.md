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
    in a full link.
  - Which sections of ASan and SanCov objects are per object.
  - Whether `/DEFAULTLIB` directives that differ from the rest of a
    library's count as side effects for `--pull=preserve`.
- **Side tables (2.3): what is untested.** A large `/guard:cf` library
  (the corpus is built without it); whether the EH continuation checks
  were enforced when `tools/turbolib/tables` ran (CET shadow stacks); the
  ARM64 programs, linked but not run; `.voltbl` and `.retplne` from
  compilers other than cl 14.52.
- **Phase 1: what is left of the writers.**
  - CodeView: done (`symbol_refs`, `c13`, `write`, `remap`; checked on
    both corpora by `turbonop-codeview`'s `corpus` example). Still open:
    rewriting a record whose binary annotations' file offsets change
    length (`remap` reports it; `annotations::rewrite_files` does the
    bytes), and the kinds whose layouts nobody documents (S_FRAMEREG,
    S_FASTLINK, S_GPROC32EX and the rest `symbol_refs` lists as unknown)
    if a compiler is found writing them.
  - `/DEF`: done (plan 1.2). Left: `lib.exe`'s ARM64EC hints (memory it
    didn't set: not reproducible); LLVM's demangler's `$$h` place for
    template and special C++ names; `/DEF` with `/GL` objects, and
    ARM64EC objects' exports (untested); and the `Hint on symbols that
    could potentially match` lines after LNK2001.
  - What `lib.exe` and link.exe do with `.def` values the import library
    doesn't show: `BASE`, `HEAPSIZE`, and `STACKSIZE` past 32 bits or
    negative (read as `strtoul` reads them; untested what the image
    gets).
  - The SDK's and MSVC's libraries whose index isn't computed from their
    members (103 of 1,851; see the plan's 1.2): the ARM64 CRT's EC
    symbols, and UCRT's members that win a name.
- **Why link.exe ended library sweeps early** with the anchors last (fixed
  by putting them first; see the plan's "The alias technique"): it
  reproduces with `LLVMObject.lib`'s `Error`, `Binary`, `ObjectFactory`,
  `Archive` and `SymbolicFile` members and `turbolib197.obj` (anchored by
  `Binary` alone), and not without `Error`; hand-made objects of that shape
  (aliases, plain records, anchors, unresolved names) didn't. Knowing the
  rule would show whether some other shape of input can still end a sweep
  early.
- **Link equivalence (phase 2.6).** `turbolib verify` and
  `turbolib compare-images` are done (`turbonop-verify`; see the plan's
  "The tools"), and `relink.py` runs compare-images over link.exe and
  lld-link, /OPT:REF and NOREF, /DEBUG:FULL and GHASH. Still open:
  - compare-images on x86 (32-bit) images: decode x86 code, so relative
    calls between moved functions don't show as differences; and on an
    ARM64 image (only synthesized instructions are tested);
  - the members each link loads, from `/VERBOSE` (the PDB's module list is
    compared now, which shows the same for loaded members);
  - the test programs' own test suites, linked against folded libraries;
  - `/<ECSYMBOLS>/` in verify; C13 subsections other than lines and
    inlinees (frame data, cross-scope imports and exports) in
    compare-images.
- **Phase 2.4's open ends** (folded units' CodeView, done for x64 MSVC
  and clang-cl).
  - (Done: lld-link loaded members link.exe didn't, 2.8 MiB more code in
    `llvm-nm`, because a member that gave up a copy had the name
    undefined. With [the alias technique](docs/turbolib.md#the-alias-technique),
    lld-link's images against folded libraries have the original's
    sections to the byte in size.)
  - clang-cl objects pass through whole: `.llvm_addrsig` (symbol indices)
    is a side table (2.3). The fixtures use `-fno-addrsig`.
  - Untested: x86 frame data and cross-scope imports and exports in a
    COMDAT's `.debug$S` (renumbered, but no compiler seen writing them
    there), `/Zi` objects in a link, and clang-cl's PCHs.
  - PDBs in `relink.py`'s matrix: `corpus/pdbcompare.py` compares them
    structurally (2.4's check); run it there too.
- **Phase 3: shared types.** 3.1 to 3.4 are done (`--precomp=library`;
  see the plan's "Phase 3 results"). Still open:
  - **3.5, `--precomp=clustered=<n>`**: grouping members by the types they
    share, so a link that loads few members merges fewer unused types.
    Decide how to form the groups and choose and score *n* (the plan's
    3.5), measure with the analyzer, then write it.
  - Make `library` the default once it is proven in more links: the
    test programs' own suites, clang-cl's PCHs, `/Zi` neighbors, ARM64
    and x86 objects (all untested), and incremental linking with the
    kept `.chks64` (0 for changed sections; untested).
  - The analyzer's projection for PCH builds: it counts users of a PCH in
    another library as sharing (3.1 GiB projected on `build-z7`, 1.16
    GiB made). Project what the engine does: through in-library PCH
    objects' records, and not for users of other libraries' PCHs.
  - Users of a PCH in another library keep their types (most of what
    `build-z7` doesn't save). A precompiled-types member that itself
    names their PCH might let them share; likely not readable by
    lld-link (it reads `.debug$P` as plain records). Spike it with both
    linkers before writing anything.
  - An option for the threshold (`min`, 2 now), and the PDB cost per
    link that the plan's trade-off names (lld-link: 14,064 more types in
    `llvm-nm`; link.exe: none measured), measured with the analyzer for
    each choice.
  - Speed: finding the shared types is about 0.35 s of
    `clangCodeGen.lib`'s 1.6 s; rewritten members' `.debug$S` is copied
    twice (renumbered, then into the skeleton).
  - The PCH signature is 32 bits (the CodeView field's width): two
    different libraries' PCH members in one link could share one by
    chance (about n²/2³³ for n libraries). lld-link (read in
    `lld/COFF/DebugTypes.cpp`) matches a user to its PCH by signature
    alone, by name only when no PCH has the signature, and refuses two PCH
    objects with one signature ("a PCH object with the same signature has
    already been provided"): a collision fails the link. Still to measure:
    what link.exe does (it is said to match on name and signature), with a
    deliberate collision. Not handled.
- **Phase 4: less conservative folding.** `--fold=select-any`, `--pick`,
  `--locals=copy` and `merge`, and `--report odr` are done (see the plan's
  "Phase 4 results"). Still open:
  - Synthesized members that name a compiler PCH object's symbol for a
    member that didn't (a PCH user's copy of a unit non-users gave up too;
    phase 2.4's key is the survivor's): under `preserve`, a link loading
    such a member alone now loads the PCH object. `verify` reports it as
    expected (2 libraries of `build-z7`). Fix: key synthesized members by
    every giver's PCH, or keep a PCH user's copy out of non-users' groups.
  - `--locals=copy` folds no unit that reaches code outside COMDATs: LLVM's
    Debug build has no `/Gy`, so its `static` functions are there, and
    clang-cl puts each `static` function in a section of its own that
    isn't a COMDAT. Copying such a section whole (with its `.pdata` and
    `.xdata` entries) when it holds one function is the next step.
  - Local units that code which stays reaches are copied, not shared:
    giving them an external name in the synthesized member would let the
    members share one (and fold local units nothing that moves reaches).
  - Pieces are copied only for x64 and x86 relocations (their addends are
    read in place); ARM64's (PAGEBASE_REL21 with PAGEOFFSET_12A, and the
    rest) aren't read, so those units stay. Untested on x86 and ARM64
    objects of any compiler.
  - Under `preserve`, select-any requires every copy to refer to the same
    names; a survivor whose references are a subset of every copy's would
    be enough.
  - `--report odr` prints lines; a `--json` form, and an option to list
    only findings within one environment, if users want them.
  - lld-link's `.tls` is 8 bytes larger against folded libraries in
    `llvm-nm` (`/OPT:NOREF`, `build-z7`), with `--fold=identical` as with
    phase 4's modes: not explained yet.
- **Phase 6: ELF** (see the plan's "Phase 6 results"). Done: the GNU and
  BSD archive writers and the `ar`, `llvm-ar`, and `ranlib` personalities
  (6.1), the ELF object model (6.2), a first `--fold=identical` for ELF
  (6.3), ELF `verify` and `compare-images` (6.4), and folded functions'
  DWARF in synthesized members, split DWARF 5 included (6.5). Still open:
  - **DWARF (6.5)**: not read (the folded functions get none): 64-bit
    DWARF, REL machines (i386,
    ARM), compressed debug sections (`-gz`), or `DW_FORM_ref_addr` to
    another unit (`--stats` counts each). `.dwp` packages and type units
    (`-fdebug-types-section`, in COMDAT groups of their own, which stay)
    are untested. A synthesized member's DIEs are always the first
    member's copy; `compare-images` reports 7 to 22 per program as
    differing where a link of the original kept another unit's copy.
    Prove them by matching against the original image's discarded
    copies, which are still there, tombstoned. The images' DWARF grows
    about 2.4% (the givers' DIEs stay, and each synthesized member adds
    declarations and a line header): share line headers or declarations
    between a library's synthesized members (one unit per survivor
    member?), and measure what lld and mold then write. Untested: lldb
    (not installed), a clang-built corpus fold with DWARF, AArch64
    objects.
  - `--pull=preserve` keeps the units whose first definer has side
    effects: on the gcc corpus, 344,949 copies, mostly LLVM's `cl::opt`
    initializers (`.init_array`), and with them (a unit naming what stays
    in its member stays too) 466,557 more. Find what an ELF link could
    keep loading them instead (there is no `NOLIBRARY`); and whether a
    synthesized member could keep a reference to a name that stays in
    its givers without mold taking it from another member.
  - `--locals=copy` and `merge` for ELF: references into `SHF_MERGE`
    sections (string literals) are content references the linker merges
    anyway, so copying the pieces is semantics-preserving; the main reason
    units stay ("reaches a local symbol outside it").
  - `.note.gnu.property` is in the environment key as bytes: merge it
    instead (AND-type properties by OR over a unit's copies, OR-type by
    AND), so objects with different `ISA_1_USED` notes fold together.
  - `--fold=select-any` for ELF.
  - ELF `verify` and `compare-images` are done (the plan's 6.4). Open:
    the older folded index of 4 libraries of the gcc corpus listed 1 or 2
    empty names for a rewritten member with extended section numbering
    (`SLPVectorizer.cpp.o` of `libLLVMVectorize.a`); this build's output of
    all 265 verifies, but why the older one didn't isn't known. `compare-images`
    doesn't compare bytes outside sized symbols (string and constant
    pools, jump tables, `.gcc_except_table`: name them by content or by
    the references into them) and has decoded no ARM64 ELF image;
    `verify` leaves synthesized members' DWARF to `compare-images`. Run
    both on the clang corpus.
  - Untested: REL machines (i386, ARM) and big-endian objects in a fold;
    AArch64, RISC-V, MIPS objects at all; GCC LTO objects and LLVM
    bitcode members (the `ar` personality refuses to index them); BSD
    archives from GNU `ar` (this binutils can't write them); thin archives
    and `--fold` (refused); `ar` on Windows hosts beyond the unit tests.
  - GNU `ar -M`'s `DIRECTORY`, `EXTRACT`, `LIST`, `VERBOSE`, and `FULLDIR`.
  - **Folding's speed**: 2.66× GNU `ar`'s time on the gcc build's 20
    largest libraries before 6.5 (without folding, 0.27×); 4.24× with DWARF
    moved, in a noisy run: measure again on a quiet machine, and profile
    the DWARF reading (each survivor member's `.debug_info` is walked
    whole, and every DIE's attributes read twice). `turbonop-elf-object`
    decodes every relocation section (the `.debug_*` sections' 3.4 GiB of
    them too) and re-encodes them, and a rewritten member is copied twice
    (`Object::write`, then the archive writer): keep relocation sections
    as bytes until something edits them, tombstone in place, and write
    rewritten members straight into the archive. `ranlib` after `ar qc`
    folds again (finding nothing new): skip it when no member changed.
  - The clang corpus (`configure.sh clang`, archived by `llvm-ar`): build
    it, then `relib.py` (as `llvm-ar qc` and `llvm-ranlib` made each
    library), `roundtrip_unix -llvm`, the object round trip (LLVM's
    `.strtab`, `.llvm_addrsig`, `.crel`), and the fold and relink matrix.
- **The analyzer's speed.** It reads 13.7 GiB in 14.7 s on 8 threads, and
  scales 3.2× from 1 thread to 8 on this PC. Its shards now read only
  their own units and records. Measure again, and profile with
  Stackrabbit.

## Ideas

- **A size check for pull requests.** A GitHub Action that comments on a
  pull request with the size it adds and why (`-budget` and diffs exist).
- **Size history.** Sizes across many builds: when each regression came,
  and from which commit.
