# To do

What's planned for Hexcavator, and what's waiting on something. Items
leave this list when they ship.

## Every format alike

Every command and analysis should work the same, and as well, on PE,
ELF, Mach-O, and WebAssembly, with a PDB or DWARF. What's left:

- **WebAssembly's data.** A module's data segments aren't mapped (only
  its code has addresses), so its model, strings, duplicated data, and
  `-fdata-sections` advice see none; and wasm-ld's `-map` isn't applied
  to its report. Its code's addresses (code section offsets) and its
  data's (linear memory) are two spaces that DWARF mixes: they need
  telling apart first.
- **Trace symbolization** (`hexcavator-symbolize`, for traces) reads
  PDBs only; it should build on `DebugInfo` and `Image::symbols` as
  `hexc symbolize` does. The unwind crate's `Image` trait is PE's alone
  (`u32` RVAs), and nothing picks an unwinder by format and
  architecture.
- **dumpbin's `/symbols`** (a COFF symbol table) isn't dumped, for
  objects or for MinGW's images.
- **Cleanups.** `hexc size`'s bloaty-style maps are built by four
  per-format paths; Mach-O's section relocations are read in the app, not
  its crate; startup and the graph's roots each enumerate the
  initializer lists; objects of every format but Mach-O have no
  addresses (`bytes_at`); `Section::index` counts differently by format.
- **More advice.** Unused libraries (`--as-needed`,
  `-dead_strip_dylibs`); Mach-O's rebase and bind opcodes, where chained
  fixups would be smaller.
- **More instruction sets.** RISC-V, PowerPC, MIPS, and LoongArch images
  are read, but their code isn't decoded: no disassembly, references, or
  fingerprints.

## Size analysis

- **Rust's Debug implementations on Windows.** Rust's PDB names say
  `impl$12::fmt`, not which trait: `-Zfmt-debug`'s saving is measured on
  ELF and Mach-O only.

## Waiting

- **GCC, GNU ld, gold, and mold.** `hexc recommend` judges what an ELF
  link did (`--gc-sections`, ICF) from what lld leaves in the image, and
  is tested only with lld; and GNU ld's maps (and its map's archive
  members and discarded sections, for `hexc linkerlog`) are read, but
  tested only on text written for the tests; gold's and mold's maps not
  at all. Check them against each linker's, with test binaries and maps
  from each in `testdata`. Waiting on a Linux environment (WSL) to build
  them.

## Ideas

- **A size check for pull requests.** A GitHub Action that comments on a
  pull request with the size it adds and why (`-budget` and diffs exist).
- **Size history.** Sizes across many builds: when each regression came,
  and from which commit.
