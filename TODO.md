# To do

What's planned for Hexcavator, and what's waiting on something. Items
leave this list when they ship.

## Every format alike

Every command and analysis should work the same, and as well, on PE,
ELF, Mach-O, and WebAssembly, with a PDB or DWARF. What's left:

- **WebAssembly's passive data.** A module's active data segments are
  in linear memory (at `WASM_MEMORY`), but passive ones (shared memory's,
  which `__wasm_init_memory` copies) and a position-independent module's
  (relative to its imported `__memory_base`) aren't placed; nor is its
  `.bss`, which no segment holds.
- **Cleanups.** `hexc size`'s bloaty-style maps are built by four
  per-format paths; objects of every format but Mach-O have no addresses
  (`bytes_at`): their sections would need laying out, as a linker would,
  the same way their relocated DWARF is.
- **Instruction sets' loose ends.** microMIPS and MIPS16 code isn't
  decoded. MIPS's dynamic pointers (`R_MIPS_REL32`, and the local GOT
  entries the loader relocates by `DT_MIPS_LOCAL_GOTNO`) aren't read as
  pointer slots (a dynamic image's GOT is read as its file holds it).

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
