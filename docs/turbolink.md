# turbolink: plan

turbolink is a PE/COFF linker that takes `link.exe`'s command line and
does what `link.exe` does, as fast as the machine allows: every core busy,
no pass that could overlap another waiting for it. It is written in Rust,
as the rest of this repository is.

## Status (2026-11-27)

- Started. The command line is read as `link.exe` reads it
  (`crates/link/linker/src/args.rs`, `config.rs`): words, response files,
  `LINK` and `_LINK_`, `.drectve` directives, and every option in
  `link.exe` 14.52's own option table (known, ignored, or "not yet").
- Two prongs run in parallel:
  1. **The linker** (this document): native COFF objects and libraries to
     an x64 image, then its PDB.
  2. **LTCG** (`docs/turbolink-c2.md`, branch `turbolink-c2`): the
     interface between `link.exe` and `c2.dll`, learned from Microsoft's
     public symbols and a logging stand-in for `c2.dll`, and how hard
     driving `c2.dll` from turbolink would be.

## Scope

- **Everything `link.exe` does for native user-mode images.** Not kernel
  mode, not `/CLR`, not WinMD.
- **x64 first.** The design keeps x86, ARM64, ARM64EC, and ARM64X in mind
  (machine-specific code is relocations, thunks, and unwind data).
- **No LTCG in the first version, but designed for it.** `c2.dll` takes
  the `/GL` objects and gives back COFF; the link waits on it only for
  what needs its output, and everything else (native objects, libraries,
  the PDB's types from native objects) goes on meanwhile.
- **No PGO in the first version.**
- **Incremental linking:** `/INCREMENTAL` is accepted and ignored, as
  lld-link does: a full link fast enough makes it unnecessary.
- **Faster than lld-link by a wide margin.** mold is several times faster
  than lld on ELF; the same is the aim on PE, with the PDB (where
  `link.exe` spends most of a large link's time) the largest part.

## Design

The ideas come from mold (MIT, Rui Ueyama) and wild (MIT or Apache-2.0,
the Wild Linker contributors), both linkers in Rust for ELF; credit is
given where an idea is taken, and any ported code keeps its notice. wild
has an early COFF back end (x64, no imports, base relocations, `.pdata`,
exports, or PDB yet); turbolink is its own, on this repository's crates
(PE, COFF, archives, CodeView, PDB, `/GL` objects).

- **Inputs are mapped, never copied** (`turbonop_mmap`); small files may be
  read instead, if mapping them costs more (to measure).
- **Libraries are read by their index.** A Microsoft library's second
  linker member indexes every symbol by member; turbolink parses a member
  only when the link needs one of its symbols. (Neither mold nor wild uses
  the index: they parse every member. Import libraries such as
  `kernel32.lib` hold thousands of members, of which a program uses few.)
- **Determinism from order, not from scheduling.** Each loaded file gets
  its place in command-line order (mold's priorities, wild's contiguous
  ID ranges); every choice (which definition, which COMDAT copy, the order
  of sections) is the lowest place among the candidates, which is the same
  whatever the threads did. Output never depends on a hash's seed or a
  map's order.
- **Symbols are interned in two phases** (mold's `SymbolTable::gather`,
  wild's buckets): each thread hashes its files' names once and bins them
  by shard; then each shard is filled by one thread, with no locks.
- **Resolution by rank:** a definition's rank (strong, weak external,
  common, lazy in a library) and its file's place, compared as one number;
  the least wins (mold's `rank << 32 | priority`). COMDAT selection
  (`IMAGE_COMDAT_SELECT_*`) is done the same way, per COMDAT.
- **The output is sized first, then written by every core at once into
  disjoint parts of one buffer** (mold's `split_ranges`, wild's
  `split_output_by_group`): `split_at_mut` gives each input section its
  own `&mut [u8]`; relocations are applied in place. No `unsafe`.
  Whether the buffer is a mapped file or memory written with positioned
  writes is measured per volume (NTFS, ReFS, Dev Drive).
- **The PDB is written alongside the image** (mold's `Background`), its
  types merged in parallel (hashes in parallel, then one deterministic
  pass that assigns type indices: lld's GHASH).
- **No time spent tearing down:** inputs and tables are dropped by the
  process's exit, not freed one by one.
- **mimalloc**, as every program here.

### Pipeline (x64, no LTCG)

1. Read the command line; read `.def`, `/ORDER`, `/NATVIS` files.
2. Map the inputs (in parallel); identify each (object, `/bigobj`, `/GL`
   object, import object, library, `.res`).
3. Parse the objects (in parallel): sections, symbols, directives.
   Directives add libraries, exports, alternate names, and more, in
   object order.
4. Resolve symbols: intern, then rank. Undefined symbols that a library's
   index defines load that member (in parallel, a wave at a time, until
   nothing new is needed), in `link.exe`'s order: the libraries searched
   in command-line order, then default libraries; a symbol from the first
   library (in that order) that defines it.
5. COMDAT selection; `/ALTERNATENAME` and weak externals; commons.
6. `/OPT:REF` (a parallel mark from the entry point, exports,
   `/INCLUDE`s, and what's always kept); `/OPT:ICF` (iterated hashes of
   COMDAT functions and their references, as mold's ICF).
7. Synthesized sections: imports (`.idata`: directories, lookup tables,
   names, the IAT; thunks for functions), delay-load imports, exports
   (`.edata`), base relocations (`.reloc`), resources (`.rsrc`), the load
   configuration's tables (`/GUARD:CF`'s, `/GUARD:EHCONT`'s), the debug
   directory, `.pdata` sorted.
8. Layout: sections by name, `$` suffixes sorted (`.CRT$XCA` before
   `.CRT$XCU`), `/MERGE`s, `/SECTION` attributes, `/ORDER`; addresses.
9. Write: headers, then every input section copied and relocated, every
   synthesized one built, in parallel; the PDB in parallel with all of it.

## Verification

- **lld's COFF tests** (`llvm-project/lld/test/COFF`, Apache-2.0 with LLVM
  exception): run with lld-link replaced by turbolink. They need LLVM's
  tools (`llvm-mc`, `yaml2obj`, `llvm-readobj`, `FileCheck`, `lit`, ...),
  built from the LLVM checkout outside the worktrees. A list of what
  passes and what is expected to fail is kept; where lld and `link.exe`
  disagree, `link.exe` is right.
- **Against `link.exe`:** programs compiled here, linked by both, compared
  by what they mean (sections, imports, exports, relocations, unwind data,
  the load configuration, the PDB's contents), not byte for byte; and run.
  What is collected from this machine (programs' objects, timings of
  private builds) stays out of the repository.
- **Speed:** each phase timed (`/TIME`); compared with `link.exe` and
  lld-link on the same links.

## Phases

1. **A program links**: a C program with the dynamic CRT (`/MD`), then
   the static one (`/MT`), C++ with exceptions; x64. Runs, and agrees with
   `link.exe`'s image.
2. **lld's tests and the rest of the image**: DLLs and exports, `.def`
   files, delay loading, resources and manifests, `/GUARD:CF`, TLS,
   `/OPT:REF` and `/OPT:ICF`, `/MERGE`, `/SECTION`, `/ORDER`, `/MAP`.
3. **The PDB**: written by turbolink, in parallel; checked with DIA,
   WinDbg, and this repository's PDB reader.
4. **Speed**: large links measured and made faster.
5. **LTCG**, from what prong 2 learns.
6. **Other machines**: x86, ARM64, ARM64EC, ARM64X.
