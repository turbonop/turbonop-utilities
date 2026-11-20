# To do

What's planned for Hexcavator, and what's waiting on something. Items
leave this list when they ship.

## Every format alike

Every command and analysis should work the same, and as well, on PE,
ELF, Mach-O, and WebAssembly, with a PDB or DWARF.

1. **One loader.** The MCP server's `query`, `findings`, `size_report`,
   `lookup`, and `symbols`; `linkerlog -image`; `size`'s default report;
   `batch`'s directory scan; and the extension's checks go through
   `Opened`, as the CLI's other commands do.
2. **`Image` complete enough that nothing reads a format by hand.**
   Symbols (Mach-O kinds and sizes, PE exports' kinds, Thumb bits), import
   slots naming their import (PE's IAT too), exports and imports, entry
   points, and one demangler for every scheme.
3. **Accuracy.** The image's own symbols merged with DWARF's (code with
   no debug information); Mach-O's dead-strip roots (Objective-C, Swift,
   `S_ATTR_NO_DEAD_STRIP`) and ELF's (`SHF_GNU_RETAIN`, `__start_`
   sections); one DWARF attribution of compile units; system libraries on
   ELF and Mach-O; duplicated variables judged alike.
4. **What's missing.** DWARF in PE images (MinGW); DWARF in relocatable
   objects relocated; 32-bit ARM's xrefs and fingerprints, and its
   exception tables (`.ARM.exidx`); a WebAssembly toolchain (wasm-opt, not
   ICF), data segments mapped, and its dependencies, initialization,
   relocations, and build information; export costs for PE; pointer edges
   for PE images without relocations.
5. **One vocabulary.** `-debug` (with `-pdb` its alias), one address
   field in JSON, "debug information" for "PDB", `hexc pdb` and
   `hexc dwarf` alike.

## Size analysis

- **Mach-O exception handling.** Its compact unwind (`__unwind_info`) and
  LSDAs (`__gcc_except_tab`), for the exception handling costs and
  needless cleanups ELF and PE have.
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
