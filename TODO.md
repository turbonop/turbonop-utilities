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

hexcavator itself has grown: 12.2 MB to 15.5 MB on Windows x64 from
0.0.7 to 0.0.9, and 17.1 MB (glibc, dynamic) to 22.9 MB (musl, static)
on Linux x64 (and the release now ships the bare programs, not ~4 MB
archives). What's left:

- **Where it went.** `hexcavator size` and `recommend` on hexcavator,
  with its symbols: by crate, by symbol, and against 0.0.7. The likely
  parts: the TableGen decoders (RISC-V, LoongArch, PowerPC, MIPS,
  microMIPS) and their tables, 32-bit ARM, monomorphized generics,
  formatting, and what the static C library and mimalloc add on Linux.
  Then make it smaller, as hexcavator advises others to.

## Ideas

- **A size check for pull requests.** A GitHub Action that comments on a
  pull request with the size it adds and why (`-budget` and diffs exist).
- **Size history.** Sizes across many builds: when each regression came,
  and from which commit.
