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
- **What isn't read yet.** Threaded binds (arm64e's first ABI, iOS 12's:
  `BIND_OPCODE_THREADED`) and `DYLD_CHAINED_PTR_ARM64E_SEGMENTED`
  (firmware's), neither of which lld can make, for test inputs; and
  `hexcavator size` names an arm64e image's architecture `arm64` (the
  Model knows only the ISA).

## The web version

- **Reading files piecewise.** WebAssembly's 32-bit address space (4
  GiB, and the browser's copy of each file besides) won't hold clang.exe
  and its 1.1 GB PDB with everything built from them; and 64-bit memory
  isn't in Safari, is tier 3 in Rust, and costs speed. So: read files by
  their parts instead of whole, behind a source that reads a range
  (`read_at`), mapped where there's mapping and read on demand where
  there isn't (a browser `File`'s slices): a PDB's streams one at a time
  (an MSF's pages are scattered anyway; most commands need a few
  streams), an image's sections as asked for, and the big tables (a
  PDB's symbol records, DWARF's .debug_info) walked in windows rather
  than held.

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

- **x86 decoding** (49 ns an instruction): cross-references and
  fingerprints need only an instruction's length, flow, and addresses,
  not its operands as text would; a lighter decode for them, or a faster
  full one (table lookups for the candidates, not a search).
- **A program's symbols** (1.1 s, under query, xref, size, recommend):
  naming functions from their types and undecorating publics are most of
  what is left after reading the PDB.
- **size's range maps** (0.8 s for `-d symbols`): a symbol at a time
  into a B-tree; symbols come sorted, so most could be appended.
- **`hexcavator serve`'s JSON**: a command's `-json` output is a string inside
  the response, parsed twice by the editor.

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
