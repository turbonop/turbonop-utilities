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

## Procupine

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

## Waiting

- **GCC, GNU ld, gold, and mold.** `hexcavator recommend` judges what an ELF
  link did (`--gc-sections`, ICF) from what lld leaves in the image, and
  is tested only with lld; and GNU ld's maps (and its map's archive
  members and discarded sections, for `hexcavator linkerlog`) are read, but
  tested only on text written for the tests; gold's and mold's maps not
  at all. Check them against each linker's, with test binaries and maps
  from each in `testdata`. Waiting on a Linux environment (WSL) to build
  them.

## Ideas

- **A size check for pull requests.** A GitHub Action that comments on a
  pull request with the size it adds and why (`-budget` and diffs exist).
- **Size history.** Sizes across many builds: when each regression came,
  and from which commit.
