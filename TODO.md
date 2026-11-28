# To do

What's planned for TurboNOP's tools (Hexcavator so far) and their
libraries, and what's waiting on something. Items
leave this list when they ship.

## TurboNOP's tools

- **`hexcavator diff`, what's still to do.**
  - Code is compared by its instructions, decoded (x64, ARM64: branches,
    RIP-relative operands, ADRP pairs, literal loads, each access in
    full); x86's (32-bit) code, and bytes that don't decode, still by
    bytes, a field's width at a time. Identical bytes are taken as the
    same: an instruction that refers to the same address in both images
    is not checked for what each image has there (the data parts tell).
  - With map-only names, statics one map lists and the other doesn't
    become "only in a"; functions without exception data are bounded by
    the next name either image has.
  - PDBs: symbol kinds the decoder doesn't model are compared as bytes
    (but S_CALLEES, S_CALLERS, S_INLINEES, S_HEAPALLOCSITE,
    S_CALLSITEINFO, and S_FILESTATIC have their indices mapped);
    LF_UDT_MOD_SRC_LINE's file is compared as its string and its module
    by name. FPO, OMAP, the section headers stream's contents beyond
    `sections()`, and LinkInfo aren't compared yet.
  - link.exe's own PDBs differ from one `/Brepro` link of the same inputs
    to the next (same GUID): which module's copy of a type
    `LF_UDT_MOD_SRC_LINE` names, and which copy of a type S_UDT names. A
    turbolink PDB should be compared knowing that. (compare-images now
    compares those picks by what they say: the globals' and every
    module's top-level S_UDT and S_CONSTANT as one set, tag-blind; a
    UDT's source line in the same header file, or one line in common,
    as a pick.)
  - compare-images on thread-local variables: `S_GTHREAD32` and
    `S_LTHREAD32` (`_Init_thread_epoch`, `PrettyStackTraceHead`) differ
    between an original and a folded llvm-nm link, and with them their
    modules' own records (`Parallel.cpp.obj`, `PrettyStackTrace.cpp.obj`,
    `TimeProfiler.cpp.obj`): likely their `.tls` offsets aren't named
    through the image as other addresses are. Find out, and compare them
    by what they name.

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
- **"ancestors dimmed" on Windows.** The extension's test of that name
  failed once on CI's Windows runner (in #54, which touched no
  Procupine code) and passed on its rerun: a race, likely the view's
  update and the test's read of it.
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

## Managed and other non-native binaries

Binary analysis and size analysis for code that runs on a virtual
machine, not only native images (PE, ELF, Mach-O) and WebAssembly:

- **.NET assemblies.** The parser is done: `turbonop-clr` reads a PE
  image's CLI header, the metadata root and every stream (`#~` and `#-`
  with its `Ptr` tables, `#Strings`, `#US`, `#GUID`, `#Blob`, `#Pdb`),
  every table of ECMA-335 II.22 and of the portable PDB with typed rows
  and the lookups between them, every signature kind, names as IL spells
  them, method bodies (tiny and fat, exception clauses) and their IL
  (every opcode, tokens resolved), custom attribute values, permission
  sets, marshalling descriptors, `.resources` files and their entries'
  sizes, the strong-name signature, v-table fixups, document names and
  sequence points, and the ReadyToRun header. Tested against
  System.Reflection.Metadata (tools/clr/ClrDump.cs) on its fixtures and,
  where there's a `dotnet`, on the framework's assemblies, every one. Left:
  - **`size`**: attribute IL bodies, metadata tables, heap entries
    (strings, blobs, user strings), and resources to their types and
    namespaces (as compilands for native code), with the padding and
    headers that belong to nothing.
  - **`dump`, `symbols`, disassembly**: the tables, IL (as ildasm shows
    it, from `il::decode` and `names`), and resources; `info -json`'s
    fields (only the text `info` has the metadata now).
  - **The extension's pages** for all of it.
  - **ReadyToRun decoded**: method entry points (MethodDefEntryPoints,
    InstanceMethodEntryPoints' native hashtables), RuntimeFunctions (the
    native code, for size and disassembly), import sections and fixups,
    inlining and debug info; composite images (whose header is the
    `RTR_HEADER` export, not ManagedNativeHeader).
  - **Native AOT images** (no CLI header: .NET's own metadata, the
    `__ReadyToRunHeader` of the runtime's type manager).
  - **Portable PDBs, the rest**: local constant signatures, import scope
    blobs, custom debug information by kind (embedded sources, source
    link, state machine scopes, dynamic locals); embedded PDBs (the debug
    directory's deflated entry) and finding a PDB from the CodeView entry.
  - **Custom attributes naming other assemblies' enums**: their
    underlying types are only known from those assemblies
    (`EnumTypes::add_assembly`); hexcavator should load the references it
    can find (int32 is assumed otherwise, as System.Reflection.Metadata's
    decoder does).
  - **C++/CLI mixed images**: the v-table fixups' native thunks, native
    method bodies (code type native, not IL), field-kind stand-alone
    signatures; and edit-and-continue deltas (ENCLog, ENCMap applied).
- **Java.** Class files (constant pool, fields, methods, attributes,
  bytecode decoded) and JARs (zip archives of them, with their
  manifests): size by package, class, and method.
- **Android.** DEX files (and APKs and AABs that hold them): their
  classes, methods, Dalvik bytecode, and size by package.
- **Others to weigh:** Python bytecode (`.pyc`), Lua bytecode, BEAM
  (Erlang), and anything else with its own instruction set and metadata
  a user would want sized.

Each format alike, as native ones: `info`, `dump`, `symbols`,
disassembly, `size` with its findings, and the extension's pages.

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
- **The back end's interface, the rest** (docs/turbolink-c2.md): the
  back-end switches each link option produces (the probe over a matrix
  of options), `CodeGen6`'s outputs, the second root list's rule,
  `SymType`'s values, and what hands native objects to the back end.
- **`ltcg-drive`'s gaps.** Libraries' members (opened at their offset),
  incremental LTCG, and the written object's debug information
  (`LNK4255` for two objects).
- **`hexcavator vtables` without RTTI.** With RTTI, tables are found and
  bounded by their locators (a stripped PDB's lambdas' tables too); an
  image built without it (`/GR-`: link.exe) still infers a table with no
  symbol, right after one, as more of it (link.exe's `std::function`s).
  Heuristics to weigh: a second deleting destructor, or a run of slots
  all of one other class. And a table RTTI names is named in the
  demangler's long form (`` `int __cdecl main(int,char * *)'::`2'::<lambda_1> ``)
  where functions' names have the short one (`` `main'::`2'::<lambda_1> ``):
  its own class's slots aren't told from others' yet.

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

- **The decoders' tables.** Their read-only data was 4.3 MB of the
  6.7 MB `.rdata` (16.8 MB in all, on 2026-11-25). The four TableGen
  decoders' (MIPS, PowerPC, RISC-V, LoongArch: 1.3 MB) are 315 KB
  smaller: buckets in one array with 16-bit starts (16 bytes a bucket
  before, and PowerPC's 4,096 64-bit ones held 105 entries), and an
  instruction's name and syntax one string (72 bytes an instruction
  before, 56 now). arm64's (2.0 MB) are 450 KB smaller: its 8,182
  encodings are 48 bytes, not 120 (names, `ne` pairs, conditions, and
  templates by index into shared tables). arm32's (690 KB) are 214 KB
  smaller the same way: names and templates in one string, features,
  aliases, operands, fields, and conditions by 16-bit index, and the
  buckets flat with 16-bit starts. What's left, the same way:
  - **arm64's other ~1 MB** (its symbols are 24 bytes now, not 168, and
    its templates' tokens 4, not 24: 597 KB): `SYSREGS` (44 KB), the
    32 KB `BUCKETS` (`&[u16]`s), `TOK_LISTS`' slices (16 bytes each),
    and its semantics and operand specs.
  - **arm32's rules** (`RULES`: `&str`s and slices of pairs of them, and
    their texts' and alternatives' `%{rule}` references, as templates'
    were before they became a character each) and its operand and field
    lists' `&str` names.
  - **x86's, 270 KB** (`ENTRIES`, 186 KB).
  - The TableGen decoders' instructions are still 56 bytes: an index
    into a string table, and into a table of operand lists, would make
    them about 24.
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
- **Hardening (docs/turbolib-guide.md).** Done: a failed fold (panic,
  layout, self-check) writes the unfolded library with one warning;
  `verify` (on by default), `strict`, `inject-failure`; output retries and
  cleanup; `tools/turbolib/incremental` (incremental, FASTLINK, LTCG,
  lld-link; x64, x86, ARM64); `tools/turbolib/corpus/everylib.py`. Open:
  - the self-check costs about as much as folding (more on a loaded
    machine): make it cheaper, then decide its default again;
  - (Found: the 305 SDK libraries whose `--fold=none /Brepro` output
    wasn't lib.exe's were lib.exe copying a lone library input byte for
    byte; turbolib now copies it too.) Forced to write such a library
    again (`/REMOVE` of a member that isn't there), lib.exe and turbolib
    still differ on ARM64X import libraries (234 of the 305; the rest not
    yet run): without `/MACHINE`, lib.exe drops the ARM64EC import
    members that share a name with a native one and writes no
    `/<ECSYMBOLS>/` (the library loses its EC half); turbolib keeps every
    member and lists their EC names (`#name`, `__imp_aux_name`) in the
    main index; with `/MACHINE:ARM64X` both keep every member but the
    tables differ. Only re-archiving ARM64 import libraries meets this;
    x64 and x86 libraries, and libraries made from objects, don't;
  - the engine puts a COMDAT in two synthesized members (`libucrt.lib`,
    `Uuid.lib`), and changes `.retplne` (`BufferOverflow.lib`): the
    self-check falls back;
  - re-folding a folded library (or combining folded libraries) falls
    back: a member newly refers to the input's precompiled-types member;
  - `/GL` objects' index: `use_shared`-like inline functions' IL
    references read as none (`turbonop_pe::ltcg::function_il`), so fewer
    names are listed than lib.exe lists (tools/turbolib/incremental);
  - link.exe's incremental relink after a `/Yu` library member changes
    fails with lib.exe's library too (LNK1235; LNK1236 folded).
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
  - 3.5, `--precomp=clustered[=<n>]`, is done (the plan's "Phase 3.5
    results"). Open: an option for its weight (16) and the numbers of
    groups tried; seeds that model real links (several members' closures
    at once) rather than one member's; the same groups when a clustered
    library is compacted again; and its 0.1 to 0.25 s a library, mostly
    finding the first definers and closures again (reuse the engine's
    definers).
  - (Found: folding's 2.47× `lib.exe` on `build-z7` came from inserting a
    member's anchors one at a time, each renumbering the whole object;
    fixed in 863ae3d6, `clangSema`'s rewriting 11 to 15 s back to 0.3 s.
    Measure the 20 largest libraries again on a quiet machine.)
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
  - An option for the threshold (`min`, 2 now). The PDB cost per link is
    measured (`corpus/linkcost.py`; lld-link only, 2 to 17% more TPI
    records with `library`); the analyzer could project it too.
  - Speed: finding the shared types is about 0.35 s of
    `clangCodeGen.lib`'s 1.6 s; rewritten members' `.debug$S` is copied
    twice (renumbered, then into the skeleton).
  - The PCH signature is 32 bits (the CodeView field's width), so two
    libraries' PCH members in one link can share one by chance (about
    n²/2³³ for n libraries). Measured (two libraries, the signature forced
    equal by a local patch): link.exe matches users by name and signature
    and links correctly (each function's locals have their own library's
    types); lld-link fails the link ("a PCH object with the same signature
    has already been provided"; it matches by signature alone, read in
    `lld/COFF/DebugTypes.cpp`). Two libraries whose shared records are
    the same get the same signature, name, and symbol: harmless with both
    (one PCH member loads). A signature of 0 isn't a way out (lld-link
    registers no PCH whose signature is 0). So the risk is a loud lld-link
    failure, never wrong debug information; not handled further.
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
    member's copy; `witness.py` proves each description `compare-images`
    finds differing to be another unit's copy (the plan's 6.6). The images' DWARF grows
    about 2.4% (the givers' DIEs stay, and each synthesized member adds
    declarations and a line header): share line headers or declarations
    between a library's synthesized members (one unit per survivor
    member?), and measure what lld and mold then write. Untested: lldb
    (not installed), AArch64 objects.
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
    `verify` leaves synthesized members' DWARF to `compare-images`.
    Content naming stops at a constant's end only for loads of a known
    size (the accessed bytes are compared, the plan's 6.7); an address
    taken (`lea`) still names its contents to 4096 bytes or a NUL
    (`SHF_MERGE` entry sizes are lost in the image).
  - Untested: REL machines (i386, ARM) and big-endian objects in a fold;
    AArch64, RISC-V, MIPS objects at all; GCC LTO objects and LLVM
    bitcode members (the `ar` personality refuses to index them); BSD
    archives from GNU `ar` (this binutils can't write them); thin archives
    and `--fold` (refused); `ar` on Windows hosts beyond the unit tests.
  - GNU `ar -M`'s `DIRECTORY`, `EXTRACT`, `LIST`, `VERBOSE`, and `FULLDIR`.
  - **Folding's speed** (the plan's 6.7 and 6.8): profiled and reworked
    (6.8); the gcc build's 20 largest libraries fold in 1.88× a Release
    `llvm-ar`'s time (3.46× before; clang build 2.72×, from 4.73×) and
    0.34× GNU `ar`'s; `libclangSema.a` in 1.40 to 1.59 s, from 2.94 to
    3.22 s (a quiet machine). The `relib.py` runs aren't quiet ones
    (another session's runs came and went; `llvm-ar`'s own times differ by
    40% between runs): measure them again when the machine is idle. Left:
    reading members is now the largest phase (every symbol and relocation
    parsed); `.debug_info` is still copied to apply its relocations to
    debug sections (reading through them needs the shared DWARF reader to
    read through relocations; reading the relocations in place by binary
    search was slower, 6.8); a unit's DIEs are walked on one thread;
    renumbering a rewritten member's relocations copies each relocation
    section; writing the archive (the kernel's copy, 0.25 to 0.6 s for
    1.4 GB) and freeing memory end each fold on one thread; minor page
    faults vary twofold from run to run. Other mimalloc options (arena
    eager commit, eager commit delay 0, no decommit on purge) showed
    nothing beyond the noise of a shared machine, so only the purge delay
    is set.
  - CREL objects are read, written, and folded (the plan's 6.7; LLVM's
    encoder reproduced byte for byte on clang's objects). Not handled: CREL
    without addends (the member passes through), CREL on MIPS64 (left as
    bytes, so the member passes through), and GNU ld, which refuses CREL
    (gold accepts it and applies none of its relocations). A synthesized
    member's DWARF relocations are RELA even when its units' are CREL.
- **Phase 7: Mach-O** (see the plan's "Phase 7 results"). Done: Darwin
  archives and the `libtool -static` personality (7.1), the object model
  (7.2), `--fold=identical` with `verify` (7.3), x86-64's FDEs and LSDAs
  moving with their units, givers' DWARF moving with their code,
  synthesized members' DWARF, and a Rust `compare-images` for Mach-O with
  dSYMs (7.4), givers' DWARF without the moved copies' DIEs (7.5), and the
  corpus scripts (`tools/turbolib/macho-corpus`). Still open:
  - **The size DWARF costs**: the folded corpus archives are 5.9% larger
    (7.4: 9.1%; 1.0% smaller before synthesized members carried DWARF):
    88.7 MB of synthesized members' DWARF per architecture, most of it
    whole class definitions (dsymutil keeps only what linked DIEs reach)
    and their strings. Sharing class definitions between synthesized
    members isn't correct for dsymutil (7.5: they load and dead-strip
    independently). Left: an opt-in `--debug-types=declarations` (14.9
    MB, but 1.1% of the dSYM's type definitions lost); inline strings
    (`DW_FORM_string`) for strings a synthesized member uses once (a few
    percent of its DWARF); find out why dsymutil kept a giver's
    unreferenced class definition (7.5), which decides whether givers may
    also lose named types only dead code reached (6.7 MB of both
    architectures' `llvm-ar` archives).
  - **`compare-images` on the corpus links isn't clean** (see 7.4): code
    and data past a local constant that a sizeless symbol can't bound
    (2 to 24 per program; bound them by the dSYM's variables' types);
    `RTTIRoot::anchor`'s unwind encoding (0 in the original, not
    explained); 1 to 9 subprograms described by another unit's copy with
    other DIEs (spot-checked, not proven: write a Mach-O `witness.py`);
    9 to 11 type definitions only in the original and some with other
    contents (which units' class templates dsymutil keeps).
  - **Speed**: 3.55× the real tools' time folding in 7.4 (0.82× without),
    and 7.5's pruning adds about a quarter more CPU (every giver's DWARF
    walked; lookups, strings, accelerator tables, a second cut); most
    of it rewriting givers (their whole DWARF read, line programs
    decoded and encoded again, every member written afresh) and building
    synthesized members' DWARF; `ranlib` after `ar qc` folds again.
  - **DWARF 5** (macOS 15 targets' default): givers with DWARF 5 lists,
    `__debug_aranges`, or several units are kept whole (their copies stay
    in synthesized members too, which is allowed); handle `__debug_rnglists`
    and `__debug_loclists`.
  - **lldb** isn't in this LLVM build: no debugger session was compared;
    Apple's accelerator tables aren't written for synthesized members.
  - **What keeps units from folding**: string literals and other
    `__cstring`/literal-section references (`l_.str`: `--locals=copy`
    for Mach-O); x86-64's local relocations (`L` labels), data in code,
    optimization hints (`-O2` arm64 objects have LOH: dropping the hints
    of moved code is allowed).
  - **x86-64's `__eh_frame` in links** grows by a CIE per synthesized
    member (ld64.lld keeps each object's CIEs while dropping FDEs of
    functions with compact unwind); FDEs are copied anyway (Apple's ld
    with `-keep_dwarf_unwind` keeps them).
  - **Apple's linkers** (ld64, ld-prime), Apple's `libtool`/`ranlib`
    (cctools: `__.SYMDEF SORTED`), and Apple's dsymutil were neither run
    nor read here: the loading-order argument is ld64.lld's. Check on a
    Mac: `ld -t` member order, `-why_load`, `dyld_info`, dsymutil, lldb,
    and running the programs.
  - `--pull=relaxed`, `--fold=select-any` and `--locals` for Mach-O;
    32-bit targets (i386, armv7, arm64_32: passed through); universal
    files with `--fold` (untested); `ar` (GNU) on Mach-O members (its index
    isn't modeled); `verify` doesn't read universal files.
- **COFF speed and memory (the plan's Phase 5, "Memory and many
  processes").** Done: compact members, objects read one at a time when
  folding identical copies, a bounded writer, mimalloc's eager commit off,
  slots shared among turbolib processes. Left:
  - Wall times on a quiet machine (this round's were taken with another
    build running): the 20 largest libraries against `lib.exe`, 1 to 8
    threads, and a profile with `tools/turbolib/sample.py`.
  - Shared types, select-any, `--locals`, and `--report odr` still keep
    every object (they compare or share across members): read them one at
    a time too.
  - Per-thread memory (about 90 MiB a thread on clangSema.lib): `strip`
    clones the object it rewrites; threads' freed memory isn't reused by
    the next step's new threads.
  - The facts (437,527 units of 288 bytes, their names copied): borrow
    names from the inputs, move what only `--locals` uses out of `Unit`.
  - mimalloc's 32 MiB committed at start of every run.
  - Slots for the `ar` and `libtool` personalities (only `lib` takes them).
- **The analyzer's speed.** It reads 13.7 GiB in 14.7 s on 8 threads, and
  scales 3.2× from 1 thread to 8 on this PC. Its shards now read only
  their own units and records. Measure again, and profile with
  Stackrabbit.

## turbolink

[The plan](docs/turbolink.md). What links so far: C and C++ with every
CRT, DLLs and import libraries, resources, delay loading, `/DEBUG` (the
PDB), LLVM's `llvm-tblgen`. Byte for byte link.exe's images (time stamps
aside): C and C++ with the dynamic, static, and debug CRTs, a DLL with
exports (and its x64 import library, byte for byte, and export object),
delay loading, resources; with `/DEBUG /INCREMENTAL:NO`, the
same but for the PDB's GUID, age, and name.

- **Not yet, for native x64 images:** delay loading on other machines;
  `/GUARD:CF` for objects not compiled with `/guard:cf` (link.exe's own
  address-taken analysis), `/GUARD:XFG`, and an exported function that is
  also a target (its flag); `/MAPINFO`'s
  other options (`FIXUPS`; `/MAPINFO:EXPORTS` is done), the "Creating
  library X and object Y" message, `/INCREMENTAL`'s padding (it links
  fully),
  `/WHOLEARCHIVE`, `/FAILIFMISMATCH` and `/DISALLOWLIB`'s checks,
  `/MERGE` cycles' error, long-form import objects (old `.idata$`
  members), `IMAGE_REL_AMD64_SECREL7`/`TOKEN`/`SREL32`.
- **Exports' diagnostics:** link.exe warns LNK4197 for a name exported
  twice (turbolink keeps the first silently, as link.exe does) and reports
  LNK2001 for a forwarder `x = dll.f` when nothing defines `f` (seen on
  the command line and in a `.def` file; turbolink links it); the map's
  statics for a forwarder (the `.exp`'s static named for the export, at
  its string) aren't checked against link.exe's map.
- **Byte identity, still different:** the import library and export
  object on other machines than x64 (x86's decorated names; the
  descriptors' CodeView machine for x86 and ARM64 is assumed, 3 and
  0xF6); the map's `$R` symbols with two or more
  `.res` files or an embedded manifest (cvtres's object for them: which
  offsets, which origin), and a map of an LLVM-size link checked line for
  line; a delay-loaded DLL's tables with
  two or more imports (link.exe's order, docs/turbolink.md);
  where 4 bytes of padding fall before `.idata$2` in `llvm-nm` and
  `llvm-objdump` (link.exe's delay-load room there is 0x108 with the
  descriptors starting on 8, which the rule in docs/turbolink.md gives
  0x104 for; it holds for llvm-tblgen, llc, opt and six small programs:
  not the number of DLLs, the delay-loaded names, the options, the
  tables' alignment, or the first DLL in the directory). llvm-tblgen,
  llc, opt, llvm-mc and llvm-ar are link.exe's images.
- **link.exe's behavior to check, not assumed:** `/OPT:ICF`'s default
  under `/DEBUG` with `/OPT:REF`; the Rich
  header's walk when a library is searched in several passes (each pass's
  modules made in offset order?).
- **Speed.** On a quiet machine, `llc` (73 MB, every target) links in
  1.29 s (median of five) against link.exe's 3.15 s. On a loaded machine
  (other programs on every core, the disk busy) turbolink on all threads
  lost to turbolink on one (11-27 s against 4-6 s, link.exe 8-12 s),
  using no more CPU: its threads wait on each other. Branch
  `turbolink-next` has experiments for that (work in pieces taken from a
  counter in turbonop_par; the mark's targets resolved once; higher
  thresholds before parallelizing), neutral when quiet, not yet measured
  cleanly under load. Also: layout hashes owned copies of names per chunk;
  symbols are interned on one thread.
- **The PDB (`/DEBUG`), the rest.**
  - **Speed:** llvm-tblgen with `/Z7` objects only ties link.exe (the
    PDB about 300 ms of 390). Per stage:
    - the merge's reachability pass: about 60 ms, serial;
    - hashing: 30 ms;
    - shard maps: 25 ms;
    - reading `.debug$T`: 30–50 ms;
    - `/names`, DLL modules, contributions: 25 ms;
    - the streams and the MSF: 40 ms;
    - writing the PDB file at the end: 40 ms, not overlapped.

    Ideas: reachability by shard; hashing ids and types in one pass; the
    file written as it is built (or mapped).
  - **Agreement:** about 20 more ids than link.exe's (it keeps fewer
    `LF_UDT_MOD_SRC_LINE`s: which?), and with `/Zi` 7 fewer types (what
    link.exe roots that turbolink doesn't).
  - **Not written:** `/LinkInfo`, `/TMCache`, `/UDTSRCLINEUNDONE`; x86
    FPO (only `FRAMEDATA`); incremental PDBs; page sizes over 4 KB (a PDB
    whose directory needs more than one block's list).
- **lld's COFF tests** (`llvm-project/lld/test/COFF`): the runner, and a
  list of what passes.
- **LTCG: the rest** ([docs/turbolink-c2.md](docs/turbolink-c2.md),
  "turbolink's LTCG"). `/GL` objects link and run as `link.exe /LTCG`'s
  builds do. Still to do:
  - **Libraries of `/GL` objects:** open a member at its offset, and
    take members in for IL references.
  - **The second root list:** native objects' references to IL
    definitions.
  - **The switch mapping:** link options to back-end switches (`/DEBUG`,
    `/OPT`, `/GUARD`, CET, `/DYNAMICBASE`...), learned with the probe.
  - **Byte identity with `link.exe`:** 3,200 to 5,000 bytes differ in
    the test images, and the generated `.text$mn` is 16 bytes shorter
    for `hello`. Diff the generated objects first.
  - **Incremental LTCG, PGO, `/LTCGOUT`, `/cgthreads`.**
  - **Speed:** run native work while `CodeGen6` runs, and read the
    `ICOFFObj` directly with no COFF round trip.
  - **One COM wrapper:** move `ltcg-drive` onto `turbonop-linker`'s
    `c2` module.
  - **DLLs** (`-GD`) through turbolink: driven by `ltcg-drive`, but not
    yet tested through turbolink.

## rtti-strip

- **Phase 0 of [the plan](docs/rtti-strip.md).** A corpus with RTTI
  (LTCG release builds); the bytes at stake; descriptor alignment; where
  `.data$r` lands; the FIXUP stream's coverage under LTCG; FH4's
  encoding.
- **Phase 1: pad mode**, its checker, and the test program.
- **The FIXUP stream's parser** in `crates/symbols/pdb`.

## Ideas

- **A size check for pull requests.** A GitHub Action that comments on a
  pull request with the size it adds and why (`-budget` and diffs exist).
- **Size history.** Sizes across many builds: when each regression came,
  and from which commit.
