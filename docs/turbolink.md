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
  2. **LTCG** (`docs/turbolink-c2.md`): the interface between
     `link.exe` and `c2.dll`, learned from Microsoft's public symbols
     and a logging stand-in for `c2.dll`. turbolink now drives `c2.dll`
     itself: `/GL` objects linked with `/LTCG` run as `link.exe`'s LTCG
     builds do (one or two objects, C++, `/GL` with a native object).

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
  except `.idata$5` (the IAT) first and the rest of `.idata$` last. The
  export directory (`.edata`, an empty group without exports) follows
  the inputs' groups, among the last ones by name (after `.didat$`,
  before `.idata$` but `$5`, before uninitialized data); its directory
  entry's size is rounded to 4 bytes unless it is last in its section; then
  link.exe's `.rdata$voltmd` and `.rdata$zzzdbg` each go before the
  first group whose name sorts after theirs (a DLL without imports or
  unwind data: `.rdata`, `.edata`, `.rdata$voltmd`, `.rdata$zzzdbg`;
  with unwind data: `.rdata`, `.rdata$voltmd`, `.rdata$zzzdbg`, `.xdata`,
  `.edata`).
- **`/ORDER:@file`:** each line a decorated name; the functions named go
  first in their group (each group apart: a `.text$mn` function stays in
  `.text$mn`), in the file's order, the rest after them as they were;
  each function's associative sections (its unwind data) move with it.
  A name no external symbol defines (a static function's too) is
  LNK4037, `file : warning LNK4037: 'name' does not exist; ignored`.
- **`/FUNCTIONPADMIN[:n]`:** `n` bytes of `int3` (6 on x64, 5 on x86
  without a number) before each section that starts with a function (a
  symbol at offset 0 whose type is a function's: a COMDAT or not, an
  external or a static), then the section's alignment; and before
  import thunks, delay-load thunks, and tail merges. None before an
  assembler's code (its symbols have no type), nor before a section
  whose function isn't at its start. A group starts before its first
  section's padding (aligned).
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
- **Alternate names and weak externals** (found with lld's COFF tests,
  [docs/turbolink-lldtests.md](turbolink-lldtests.md)): a library's
  definition still replaces an `/ALTERNATENAME`'s alias (its target is
  then not needed); a regular weak external of a name wins over an
  anti-dependency of it, whatever their order; neither a weak external's
  default nor an `/ALTERNATENAME` target may be an anti-dependency's
  alias (the name stays undefined). A weak `main` or `WinMain` counts for
  the subsystem and the entry point.
- **Locally imported symbols:** `__imp_x` that nothing defines while `x`
  is defined is a pointer to `x` the linker adds after the debug directory
  in the `.rdata` group (8 bytes, a base relocation), listed as
  `<linker-defined>`; LNK4217 when a function refers to it, LNK4286
  otherwise.
- **Sections of one name, other attributes** stay apart (LNK4078: a
  read-only `.CRT` and a writable one are two sections). `/MERGE` moves
  only the first section of its name; `/SECTION` sets every one. Sections
  outside the fixed order (`.text`, `.rdata`, `.data`, `.pdata`, `.tls`,
  `.rsrc`) go in input order. With no code, BaseOfCode is 0x1000.
- **Inputs:** an object named twice is taken once (LNK4042); NULs
  separate directives as spaces do; an external absolute symbol defines
  its name; `/WHOLEARCHIVE[:lib]` loads every member (a `/WHOLEARCHIVE:lib`
  not among the inputs is one); an input that can't be read stops the link
  (LNK1xxx); a long section name with no string table is LNK1328.
- **`/FORCE`:** unresolved symbols are reported and linked to the image
  base (LNK4088); a discardable section's reference to a discarded symbol
  (DWARF's) is left as it is. An import library past 65,535 objects (three
  and one per export) is LNK1189, even with `/NOIMPLIB`.
- **`/LIB`** first on the command line runs turbolib (beside turbolink);
  `/HELP`, `/?` or no arguments print the usage.
- **Groups' alignment:** a group starts at the largest alignment of its
  contributions (an 8-byte `.bss` starts on 16 when another `.bss` of the
  group wants 16). Uninitialized groups (`.bss`) come after the rest.
  Padding between contributions in code is `int3` (0xCC). A group
  whose every section `/OPT:REF` discarded is kept, empty, and still
  starts at their alignment: guard_support.obj's 128-byte-aligned
  `.rdata$CastGuardVftablesA` and `C` put the next group (`.rdata$r`,
  `.rdata$T`, `.rdata$voltmd`) on 128 bytes. A discarded section counts
  in a group that keeps others too: LIBCMTD's `.rdata` starts on 32
  bytes though nothing kept in it wants more than 16.
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
  bytes are the time stamp). The hash, as link.exe's `IMAGE::Normalize`
  computes it: SHA-256 of the whole image with every time stamp (the file
  header's, each debug directory entry's, the export directory's) and,
  with a PDB, the CodeView record's GUID and age set to all ones, and the
  repro entry's length and hash to zeros; then its last 4 bytes are every
  time stamp and its first 16 the PDB's GUID (the age as it was).
  `/GUARD`'s tables (`.gfids$y` and the rest)
  are empty groups of `.rdata`.
- **ICF:** folds code (only the C/C++ compiler's: an assembler's
  identical functions stay apart), unwind data (`.xdata`), and a
  function's associative read-only data (its C++ EH `FuncInfo` in
  `.rdata`, though the functions themselves don't fold), and vtables (a
  COMDAT named `??_7`: LLVM's `CheckAllNode` and `CheckAnyNode`'s, once
  their destructors fold), not other read-only data however identical
  (`??_8` vbtables, `??_R` RTTI, string literals, constants). The copy
  kept is the first, at the largest alignment of its class (main's
  unwind data on 8 when handlers.obj's, on 8, folds into it). A folded
  function's `.pdata` goes, even when it hangs off an unfolded parent (a
  C++ funclet's). Copies are the same only if their unwind data names
  the function the same way: by a label in it (`$LN4`, as cl writes it
  for an external function), by its own static symbol (a static
  function), or by its own external symbol; LIBCMTD's static
  `ReadNoFence64` doesn't fold into cpp.obj's identical `_Bit_cast`.
  Likewise a reference to another function counts by the kind of symbol
  that names it as well as by what that function folds to: a jump
  through an external symbol and one through a static symbol stay apart
  even when their targets fold (LLVM's static `get_deleter`s). Dynamic
  initializers and atexit destructors (`??__E`, `??__F`) never fold,
  whatever their group; other functions fold across groups (a `.text$di`
  copy with a `.text$mn` one), the copy kept the one first in the image
  (its group by name, then the link's order), and a group whose every
  section folded away stays, empty and aligned (its padding before it, at
  the end of the section, zeros rather than `int3`). Import thunks are in the `.text$mn` group.
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
  0x104 bytes of room, or 0x108 so that the room ends 4 bytes past a
  multiple of 8 from the descriptors' start; not with `/DELAY:NOBIND`), `$8` (each DLL's unload
  table with `/DELAY:UNLOAD`: a copy of the address table), and `$5` (each
  DLL's address table, in `.data`: each slot its thunk's address, with a
  base relocation). The DLLs are in the order their first imports
  loaded. A `/DELAYLOAD` DLL nothing imports from is LNK4199.
- **`/GUARD:CF`'s tables**, groups of `.rdata` after `.CRT`'s: `.gfids`
  (the function table: what the kept `.gfids$y` sections name, the entry
  point, `__delayLoadHelper2` with delay loading, and each exported
  function not otherwise there with metadata flag 2, export suppressed),
  `.giats` (address-taken imports' slots, from `.giats$y`), `.gljmp`
  (long-jump targets, from `.gljmp$y`), and with `/GUARD:EHCONT`
  `.gehcont`. Each is its targets' RVAs, ascending, each once; the
  function table's entries take a metadata byte (5 bytes, stride 1 in
  `__guard_flags`' top nibble) once any has one. Every function in the
  table starts on 16 bytes (an import thunk, packed otherwise, too).
  `__guard_flags` is 0x17500 (instrumented, function table, delay-load
  IAT protected and in its own section, export suppression information,
  long-jump table), with 0x400000 for `/GUARD:EHCONT`; the delay-load
  address tables go to a `.didat` section of their own (read-write).
- **The manifest.** None by default (an EXE's or a DLL's, `/DEBUG` or
  not); but a `/MANIFESTDEPENDENCY` (an object's directive too) without
  `/MANIFEST` writes one beside the image, unless `/MANIFEST:NO`.
  `/MANIFEST` writes `out.exe.manifest` (or `/MANIFESTFILE`'s path),
  `/MANIFEST:EMBED[,ID=n]` makes it RT_MANIFEST (24), ID 1 for an EXE, 2
  for a DLL, or `n`, language 1033, among the `.res` files' resources (a
  `.res` with an RT_MANIFEST of that ID is CVT1100 then LNK1123). The XML:
  `<?xml version='1.0' encoding='UTF-8' standalone='yes'?>`, the
  `assembly` element, the UAC section (`requestedExecutionLevel
  level='asInvoker' uiAccess='false'`, `/MANIFESTUAC`'s fragment in its
  place, none with `/MANIFESTUAC:NO`), then a `dependency` per
  `/MANIFESTDEPENDENCY` (each text once, the command line's first), two
  spaces of indent, CRLF. `/MANIFESTINPUT` needs `/MANIFEST:EMBED`
  (LNK1220); link.exe then runs `mt.exe /out:merged /manifest inputs...
  its own` and embeds what mt.exe writes (its own serialization: a BOM,
  no whitespace), as turbolink does (LNK1327 if mt.exe fails).
  `/ALLOWISOLATION:NO` changes only the DLL characteristics (0x200). In
  the Rich header an embedded manifest counts as resources (cvtres's
  entry, before the export object's), and with `.res` files besides,
  each `.res` counts as 0x00970000 first.
- **The map** (`/MAP`, `crates/link/linker/src/map.rs`) is link.exe's,
  line for line, but the time stamp's (link.exe prints its date in local
  time; turbolink in UTC). CRLF lines: the output's stem, `Timestamp is`,
  `Preferred load address is`, then every section's groups but `.reloc`'s
  (`SSSS:OOOOOOOO LLLLLLLLH name CODE|DATA`), empty ones too: a group
  with contents runs to the next group with any, an empty one starts
  where what's before it ends and has no length (link.exe's room after
  the delay bound tables is 0x103 bytes, so an empty group after it
  starts a byte before turbolink's room ends). Then the publics by VA,
  then name: 29 x64 absolutes link.exe always defines (`<absolute>`, a
  `/GUARD` table that's in the image `<linker-defined>` where it is
  instead), each object's externals with `f` for a function (type 0x20)
  and `i` for one in an "any" COMDAT with an external leader, from
  `lib:member` (the library as the link named it, without extension; the
  member without directories) or the object's file name; commons
  `<common>`; `__ImageBase` (if referenced) and `__volatile_metadata`
  `<linker-defined>`; aliases at what they name (an import's thunk too);
  import slots and thunks, and the import libraries' descriptors, null
  descriptor, and null thunks where turbolink's tables put them; the
  delay-load tables' names link.exe makes (`__tailMerge_dll` lower case,
  `__imp_load_x` `f`, `__sz_DLL_dll`, `__hmod__DLL_dll`,
  `__DELAY_IMPORT_DESCRIPTOR_dll`, `__NULL_DELAY_IMPORT_DESCRIPTOR`,
  `\177DLL_NULL_THUNK_DATA_DLN`/`DLB`/`DLA`/`DLU`, from the DLL's first
  import's library). Control bytes print as `\ooo`. Then `entry point
  at`, and the statics: class 3 symbols without aux records, not in
  `.pdata` or an empty section (MASM's `$xdatasym`); at one address by
  the command line's objects first, then libraries' members, each by its
  whole name as the link has it (a library's path and the member's, byte
  order: libucrt's `internal\...` before `misc\...` before
  `startup\...`), then by name. With `/GUARD:CF` the `.gfids$y`,
  `.giats$y`, `.gljmp$y`, `.gehcont$y` statics are listed at their
  values as RVAs, against the section their group is in (offsets wrap:
  `0002:ffffe000`). The export object's statics (`.edata`, `rgpv`,
  `rgszName`, `rgwOrd`, `szName`, `$N` and the ordinal) come from
  `<out>.exp`; resources' `$R` and where cvtres put the data (six hex
  digits) from their `.res`; each tail merge's unwind data `xdata`. With
  `/MAPINFO:EXPORTS`, the exports by ordinal (`exported name:` when it
  differs from the internal name). Formatted on every core. hexcavator
  reads it (`turbonop-linkmap`).
- **Exports** (`crates/link/linker/src/exports.rs`): one per name, the
  first given (the command line's, then the `.def` file's, then the
  objects' directives; by its name, not what `EXPORTAS` exports it as);
  the unnumbered ones, in the order of their names, take the free
  ordinals from the smallest given (1 if none) up, filling the gaps
  between given ones (4 and 7 given: 5, 6, 8, ...). The directory's
  strings, after the DLL's name, go by export in the order of their names
  (`EXPORTAS` aside), each's exported name then its forwarder's string;
  the name pointers are sorted by exported name.
- **The export object** (`.exp`, `crates/link/linker/src/exp.rs`) is
  link.exe's byte for byte but the linker's own path in `S_ENVBLOCK`
  (and the lengths and offsets it moves): an x64 object stamped
  0xFFFFFFFF; `.edata` (the directory with its addresses zero, a
  forwarder's string in it, ADDR32NB relocations to `szName`, `rgpv`,
  `rgszName`, `rgwOrd`, then per export, by the exports' names, its slot
  to its symbol and, if it has a name, the next name pointer to its
  string's `$N`); `.debug$S` (S_OBJNAME the `.exp`'s path as formed,
  S_COMPILE3 for LINK 14.52.37029, S_ENVBLOCK cwd and exe, an S_EXPORT per
  export by name with its flags (a forwarder 0x20, named for itself);
  with a `.def` file its SHA-256 (0xF4) and absolute path (0xF3));
  `.drectve` only when a `.def` file names the image (` /DLL /OUT:x.dll`
  for `LIBRARY x`, ` /OUT:x.exe` for `NAME`), with a NUL after it; the
  symbols `@comp.id`, `@feat.00` 0x810, the sections', the directory's
  parts, `$N`s, then one per export by name: what it exports, undefined
  (twice if two exports export it), or for a forwarder a static named
  for the export at its string. It's written beside the import library
  (`/IMPLIB`'s name with `.exp`), with `/NOIMPLIB` too, not with
  `/NOEXP`.
- **The import library** (`.lib`, `crates/link/linker/src/implib.rs`) is
  link.exe's byte for byte, with and without `/Brepro` (it doesn't
  change it), for x64:
  - Members: the import descriptor, the null import descriptor, the null
    thunk, then a short import object per export not `PRIVATE`, in the
    order of their names (bytes: `B` < `_` < `a`). Each header's date is
    -1, uid and gid blank, mode 0; a name of more than 15 bytes in `//`
    (once, NUL-terminated, the member's `/0`).
  - The first linker member's symbols are in the order the exports were
    given (the exports' order above), each member's sorted (`Beta` before
    `__imp_Beta`, `__imp_alpha` before `alpha`), at the members' offsets;
    the second linker member's member table is in that order too (not the
    file's), its symbols sorted with indices into it.
  - Each member's time stamp is the first 4 bytes of the SHA-256 of the
    member with its stamp 0xFFFFFFFF, little-endian, with the top two bits
    set (proven on 84 members of 7 libraries).
  - The descriptor objects: `.debug$S` first (on 1, discardable,
    read-only: signature 2, `S_OBJNAME_ST` with signature 0 and the DLL's
    name as a length-prefixed string, `S_COMPILE2_ST` for LINK, AMD64,
    front end 0.0.0, back end 14.52.37029, "Microsoft (R) LINK"; not
    padded), and `@comp.id` 0x010190A5 (product 0x101) the first symbol.
    A section without relocations points at the last relocations before
    it (the descriptor's `.idata$6` at `.idata$2`'s), 0 if none.
  - An export given an ordinal (`@n`, on the command line or in the
    `.def` file) is imported by its ordinal, as a `NONAME` one is; the
    others by name with their hint (`EXPORTAS`: name type 4, the name
    after the DLL's).
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
  CRT, C with the debug CRT, a DLL with exports (its import library byte
  for byte too, and its export object), delay loading,
  resources, and `/GUARD:CF` (with indirect calls, long jumps,
  address-taken imports, delay loading, and `/GUARD:EHCONT`; tests check
  each); and LLVM's `llvm-tblgen` (4.8 MB) and `llc` (73 MB, every
  target). With `/DEBUG` (and `/INCREMENTAL:NO`: turbolink always links
  fully), the image is link.exe's but for the PDB's GUID, age, and name
  (the debug directory: CodeView, VC_FEATURE, then the coffgrp entry); the
  PDB is logically link.exe's (DIA reads it; `hexcavator diff` compares
  them), not byte for byte.
- **Still different** (to do): a delay-loaded DLL's name and address tables with two or more imports
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
5. **LTCG**, from what prong 2 learns. Started: `/GL` objects link and
   run (`docs/turbolink-c2.md`, "turbolink's LTCG"); libraries of `/GL`
   objects, the switch mapping, and byte identity remain.
6. **Other machines**: x86, ARM64, ARM64EC, ARM64X.
