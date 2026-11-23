# To do

What's planned for Hexcavator, and what's waiting on something. Items
leave this list when they ship.

## Every format alike

Every command and analysis should work the same, and as well, on PE,
ELF, Mach-O, and WebAssembly, with a PDB or DWARF. What's left:

- **Cleanups.** `hexcavator size`'s bloaty-style maps are built by four
  per-format paths (ELF's, Mach-O's, and WebAssembly's follow bloaty's own;
  PE's is the Model's). Libraries come from a PDB, a Unix linker's map,
  or a Mach-O debug map's archives (DWARF names none); COFF groups are
  PE's alone, as no other format has them.
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

## Size analysis

- **Rust's Debug implementations on Windows.** Rust's PDB names say
  `impl$12::fmt`, not which trait: `-Zfmt-debug`'s saving is measured on
  ELF and Mach-O only.

## LTCG objects

- **The IL's records, walked.** `hexcavator ltcg` finds `.cil$gl`'s functions
  and variables by their records' heads, as the rest of each record's
  grammar isn't known. Walked one after another, they would give each
  function's IL (its offset and size in `.cil$ex`, so `hexcavator size` could
  weigh a `/GL` library's functions), what each refers to (so the public
  definitions could be told from inline functions nothing uses, as
  lib.exe does), and their types.
- **The other IL sections.** `.cil$ex`, `.cil$sy`, and `.cil$in` are
  shown by size only.

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
