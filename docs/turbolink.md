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

## Identical output

The goal: the image turbolink writes is byte for byte the image
`link.exe` writes from the same inputs and options (with `/Brepro`, or
with time stamps and GUIDs set aside); the PDB is logically the same
(the same types, symbols, lines, and so on), not necessarily the same
bytes. Every deliberate difference is listed here. The oracle is a
logical diff of images and PDBs (`hexcavator`, in progress).

What `link.exe` does, learned from its images (its `coffgrp` debug entry
lists every COFF group with its address and size) and by experiment:

- **Libraries:** the command line's first, in order, then the default
  libraries in the order objects name them; each searched for what is
  undefined, passes repeated. A library's members load in the order
  the symbols that need them were first seen (not by offset).
- **Groups in a section:** sorted by full name (`.CRT$XCA` before
  `.CRT$XCU`), sections merged in sorting among the section's own
  (`.00cfg` before `.CRT` before `.rdata` before `.xdata` in `.rdata`);
  except `.idata$5` (the IAT) first and the rest of `.idata$` last.
- **Imports:** built from the import libraries' own pieces (each short
  import's slots, the import descriptors, the null thunks), which sort
  by member name (the DLL's: byte order, upper case first): the IAT, the
  lookup tables, and the names (each DLL's hint/names, then its name)
  are in that order. The import directory is in the order each DLL's
  first import was loaded.
- **The library walk:** for each library, the undefined symbols in
  order; a member is loaded as soon as one is found (what it leaves
  undefined joins the end of the list and is reached in the same walk).
  `_load_config_used` is asked for only once the rest is resolved, in a
  last round. A member's directive `/INCLUDE`s join the list after its
  own symbols (libucrt's `_file.obj`'s `__acrt_stdio_initializer` is
  searched for after its `__pioinfo`).
- **Function overrides:** a name ending in `_$fo$` (`memcpy_$fo$`,
  `_guard_check_icall_$fo$`) is never searched for; if nothing defines
  it, it is the image base (RVA 0: a `REL32` to it reaches the image's
  start, an `ADDR64` is the base with a base relocation), even with an
  `/ALTERNATENAME` for it, and it is no error. Only that suffix:
  `f$fo$` and `f_$fo_bdd$` are ordinary names.
- **Groups' alignment:** a group starts at the largest alignment of its
  contributions (an 8-byte `.bss` starts on 16 when another `.bss` of the
  group wants 16). Uninitialized groups (`.bss`) come after the rest.
  Padding between contributions in code is `int3` (0xCC). A group
  whose every section `/OPT:REF` discarded is kept, empty, and still
  starts at their alignment: guard_support.obj's 128-byte-aligned
  `.rdata$CastGuardVftablesA` and `C` put the next group (`.rdata$r`,
  `.rdata$T`, `.rdata$voltmd`) on 128 bytes.
- **Commons** go at the end of `.bss`, module by module: the command
  line's objects in order, then each library's, the last loaded first.
- **What link.exe adds to `.rdata`:** the volatile metadata
  (`.rdata$voltmd`, 24 bytes, `__volatile_metadata`, for x64's load
  configuration but in every x64 image, with no load configuration
  too); the debug directory, at the end of the
  `.rdata` group; its data in `.rdata$zzzdbg`: the `coffgrp` list (each
  non-empty group's address, size, and name, after a zero signature),
  in room for every group something is in, empty ones too (an
  assembler's empty `.gssep$x` and `.gsspr$y`: the debug entry's size is
  the list's, the rest zeros), and, with
  `/Brepro`, the `repro` entry (32, then a 32-byte hash whose last 4
  bytes are the time stamp). `/GUARD`'s tables (`.gfids$y` and the rest)
  are empty groups of `.rdata`.
- **ICF:** folds code (only the C/C++ compiler's: an assembler's
  identical functions stay apart), unwind data (`.xdata`), and a
  function's associative read-only data (its C++ EH `FuncInfo` in
  `.rdata`, though the functions themselves don't fold), not other
  read-only data (`.00cfg`'s pointers, `static const` objects). The copy
  kept is the first, at the largest alignment of its class (main's
  unwind data on 8 when handlers.obj's, on 8, folds into it). A folded
  function's `.pdata` goes, even when it hangs off an unfolded parent (a
  C++ funclet's). Import thunks are in the `.text$mn` group.
- **The imports within one DLL** are in `qsort`'s order: link.exe's
  `SortPGRPByPMOD` sorts each `.idata` group's contributions with the
  C runtime's `qsort` and `CompareIdataPCON`, which compares the DLLs'
  file names (case-sensitive), then section and module flags, and
  otherwise returns 0 (so `qsort`, not stable, decides). turbolink sorts
  the same array with a port of the UCRT's `qsort`.
- **The Rich header** (link.exe's `CbBuildProdidBlock`): link.exe's own
  entry (0x102, build 37029) first in a list to whose front each new
  `@comp.id` goes, walking the command line's objects in order, then each
  library's members from the last in the archive to the first (link.exe
  makes a library's modules in the order of their offsets, each put at
  the front of its list: its `/VERBOSE` pass 2 lists them so). An import
  object is 0x00010000, an object without a `@comp.id` 0. The checksum is the stub's length, its bytes
  rotated left by their offsets (`e_lfanew` zero), and each entry's ID
  rotated left by its count; then `(checksum >> 5) % 3 + 1` empty
  entries, as many as fit in the headers without enlarging them.
- **Delay loading** (`/DELAYLOAD`, x64), as link.exe lays it out: a
  delay-loaded DLL's import wants no import descriptor but
  `__delayLoadHelper2` (asked for as the first one loads). Where the DLL's
  first import loads: its `__tailMerge_dll` (0x79 bytes of code: the
  argument registers saved, the helper called with the DLL's descriptor,
  `jmp rax`; its `.pdata` entry covers all but the `jmp rax`, its unwind
  data is a 0x18-byte prologue allocating 0x68), the DLL's name (in
  `.rdata`, on 16 bytes), its module handle (`__hmod__`, 8 bytes of
  `.bss`); where each import loads, its `__imp_load_x` (`lea rax,
  [slot]; jmp tail merge`, 12 bytes). The tables are groups that sort
  with `.idata`'s: `.didat$2` (32-byte descriptors: attributes 1, the
  name, handle, address table, name table, bound table and unload table
  RVAs), `$3` (the null descriptor), `$4` (each DLL's name table), `$6`
  (hint/names, in load order), `$7` (each DLL's bound table, zeros, then
  0x104 bytes of room; not with `/DELAY:NOBIND`), `$8` (each DLL's unload
  table with `/DELAY:UNLOAD`: a copy of the address table), and `$5` (each
  DLL's address table, in `.data`: each slot its thunk's address, with a
  base relocation). The DLLs are in the order their first imports
  loaded. A `/DELAYLOAD` DLL nothing imports from is LNK4199.
- **Resources** (`.res` files, cvtres's work): the directory (with its
  strings) is the `.rsrc$01` group, the data `.rsrc$02` from the next 16
  bytes, each resource's on 8 and the whole to 8; the Rich header has
  cvtres's entry (0xFF, build 37029, once) before the inputs'.
- **The headers** have room for exactly the section headers written: no
  `.reloc`'s when there are no base relocations (a minimal image's
  headers are 0x200 bytes, the Rich header unpadded).
- **The subsystem** without `/SUBSYSTEM`: the console's if `main` or
  `wmain` is defined, Windows' if `WinMain` or `wWinMain` is, else a
  DLL's is Windows' (entry point or not) and an EXE's is LNK1221.
- **Status:** link.exe's images byte for byte, time stamps aside, for C
  and C++ (exceptions, TLS, iostreams) with the dynamic and the static
  CRT, C with the debug CRT, a DLL with exports, delay loading, and
  resources (tests check each).
- **Still different** (to do): the
  `/Brepro` hash (it depends on the output's contents, not its name or
  the options, but isn't a plain SHA-1/256/512 or MD5 of the file with
  its time stamps and hash zeroed); unoptimized C++ with the static debug
  CRT and no `/DEBUG`: link.exe doesn't fold LIBCMTD's guard_support.obj's
  static `ReadNoFence64` and `ReadPointerNoFence` into identical
  functions (turbolink does), for a reason not found yet (not their names,
  symbol class or type, the object's `@comp.id` or `@feat.00`, or its
  `.debug$S`; its map lists them without the COMDAT `i` flag); a
  delay-loaded DLL's name and address tables with two or more imports
  (link.exe's order isn't load order, and no `qsort` of it found yet:
  loads C,D give D,C; C,D,S give S,D,C; C,D,X,S give S,D,X,C; C,Q,F,X,E,G
  give G,Q,F,X,E,C; C,Q,F,A,X,E,B,I,M,G give C,M,Q,F,A,X,E,B,I,G).

## Verification

- **Extended tests** (they need Visual Studio, so they live in the private
  repository, `hexcavator-private`'s `turbolink/`: this repository's tests
  read nothing outside it): programs compiled with the installed Visual C++
  tools, linked by turbolink, run, and compared with link.exe's images byte
  for byte (time stamps aside): a `/MD` C hello world and a DLL so far.

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
