# turbolib: plan

turbolib (working name) is a static-library archiver that removes the
duplication C++ puts in static libraries. Each object that uses an inline
function, a template instantiation, a string literal, a vftable, or a
`selectany` variable carries its own copy (a COMDAT). Under `/Z7`, each
object also carries its own copy of every type it uses. A library of 1,000
objects can hold 1,000 copies of each. The linker throws all but one away,
but only after every copy has been written, copied around a distributed
build, read, and merged.

turbolib keeps one copy. It is a drop-in replacement for `lib.exe` (and
later `ar`, `llvm-ar`, and `libtool`). Its output links exactly as the
original does, or, in the less conservative modes the user chooses, as the
linker is allowed to link it.

This is a plan, not a specification. Each phase becomes TODO.md items, and
items leave TODO.md as they ship.

## Status (2026-11-25)

| Phase | State |
|---|---|
| 0 Measurements and linker spikes | Mostly done (see [Phase 0 results](#phase-0-results)): `turbolib analyze`, both corpora's numbers, and the PCH, weak-external, size-limit, and `.chks64` spikes. Not yet: incremental linking and `.chks64`, sanitizers. |
| 1 Writers, round trips, and `lib.exe` without folding | In progress (see [Phase 1 results](#phase-1-results)). Done: 1.1, `turbonop-coff`, which every corpus object round-trips through byte-identically; 1.2, the archive writer, import objects, and `.def` files, which every library tried writes back through byte for byte; 1.3, CodeView's fields, C13 subsections, and renumbering, clean on both corpora; 1.4, the `lib` and `llvm-lib` personalities, which remake all 245 libraries of each corpus build as the tool did. 1.2's `/DEF` too: import libraries and export files as `lib.exe` and `llvm-lib` write them. Not yet: thin libraries. |
| 2 Identical folding (COFF) | In progress (see [Phase 2 results](#phase-2-results)). Done: 2.1 and 2.2, the engine (`turbonop_fold::rewrite`) in the `lib` personality (`/TURBOLIB:fold=identical`), the side tables (2.3), and `--pull=preserve` and `relaxed` with `--locals=keep` (2.5). Not yet: folded units' debug information (2.4). Done too: the link-equivalence checks (2.6: `turbolib verify`, `compare-images`). |
| 3 Shared types: a synthesized precompiled-types object | Not yet. |
| 4 Less conservative folding | Not yet. |
| 5 Speed | Not yet (but every phase is built to be parallel from the start). |
| 6 ELF, with split DWARF | Not yet. |
| 7 Mach-O | Not yet. |

### Picking this up elsewhere

Everything needed is in the repository:

- **This plan**, and the `## turbolib` section of [TODO.md](../TODO.md),
  which tracks what's next.
- **The analyzer.** It is `apps/turbolib` (`turbolib analyze`), built on
  `crates/link/fold` (`turbonop-fold`) and
  `turbonop_codeview::type_refs`. It is run like this:

      cargo run --release -p turbonop-turbolib -- analyze --stats --top 20 <dir or .lib>...

  `--json` gives machine-readable results. `--diagnose` lists the members
  it couldn't read, and why. `-j N` sets the number of threads.
- **The corpus.** It is rebuilt with
  [tools/turbolib/corpus](../tools/turbolib/corpus). Set `LLVM_SRC` to an
  llvm-project checkout (the default is `C:\jm\dev\llvm-project`). Check
  out `c74b358f44c3` for the same numbers. Then run `configure-z7.cmd` and
  `build-z7.cmd`, and their `-nopch` versions. On this laptop (Core Ultra 5
  335), the PCH build took about 40 minutes and the other about an hour. The analyzer is then run on `build-z7\lib` and
  `build-z7-nopch\lib`.
- **The spikes.** They are in [tools/turbolib](../tools/turbolib/README.md).
- **The tools measured.** These were Visual Studio 18 Enterprise (MSVC
  14.52.37025; the scripts call its `vcvars64.bat`), and LLD 24.0.0 at
  `c74b358f44c3`, a Release build in `build-release`. `LLVM_BIN` overrides
  the default path to it.

The next step is phase 1, the writers.

## Phase 0 results

### What folding would save

The corpus is `c:\jm\dev\llvm-project\build-z7`: LLVM, clang (with
ClangIR, which brings MLIR), and lld libraries. It is a Debug build
(`/Od /Ob0 /RTC1 /MDd`) with `/Z7`, for the X86 and AArch64 targets, at
LLVM's `c74b358f44c3`.

LLVM's CMake uses precompiled headers by default: 3,604 of the 3,771
objects use one of 121 PCH objects. So types are already partly shared
in this build. A second build without PCH
(`-DCMAKE_DISABLE_PRECOMPILE_HEADERS=ON`, in `build-z7-nopch`) gives the
other case, below.

The corpus is 250 libraries, 13.7 GiB, of which:

| What | Size | Share |
|---|---|---|
| `.debug$T` | 4.9 GiB | 35.6% |
| Symbols and names | 3.8 GiB | 27.6% |
| `.debug$S` | 2.5 GiB | 18.5% |
| Code | 830 MiB | 5.9% |
| Unwind | 619 MiB | 4.4% |
| Data | 322 MiB | 2.3% |

What each mode would save:

| Mode | Saves | Of the libraries |
|---|---|---|
| COMDATs, `identical` | 2.8 GiB | 20.7% |
| COMDATs, `identical, pull=preserve` | 2.7 GiB | 19.8% |
| COMDATs, `identical, locals=copy` | 2.8 GiB | 20.7% |
| COMDATs, `select-any, locals=copy` | 2.9 GiB | 20.9% |
| Types, `precomp=library` | 3.1 GiB | 22.7% |
| **The conservative default (`identical, pull=preserve` plus types)** | **5.8 GiB** | **42.5%** |

Single libraries save more: clangSema 66%, clangParse 77%,
clangStaticAnalyzerCheckers 70%.

**Without PCH** (`build-z7-nopch`, the same sources and flags), the 250
libraries are 21.8 GiB. `.debug$T` is 12.0 GiB of it (55%): 227 million
records, of which 29 million are distinct.

| Mode | Saves | Of the libraries |
|---|---|---|
| COMDATs, `identical, pull=preserve` | 2.8 GiB | 12.7% |
| Types, `precomp=library` | 10.2 GiB | 47.1% |
| **The conservative default** | **13.0 GiB** | **59.7%** |

The largest libraries save the most:

| Library | Size | Saves |
|---|---|---|
| clangCodeGen | 1.5 GiB | 76% |
| clangSema | 1.5 GiB | 73% |
| clangStaticAnalyzerCheckers | 1.3 GiB | 79% |
| clangCIR | 1.3 GiB | 74% |
| LLVMCodeGen | 1.1 GiB | 67% |

The precompiled-types member would hold 816 MiB. Every link that loads
any member of the library merges all of it into its PDB, which is the
cost `--precomp=clustered` exists to bound. (Neither linker prunes types
that no symbol refers to, as measured under the shared types'
trade-off.) The analyzer read all 21.8 GiB in 22.5 s (989 MiB/s).

Other findings:

- Of 2.49 million COMDAT names, 634,000 are in more than one member.
- Only 3,732 names have copies that differ within one environment.
  Identity is strict, and the conservative mode loses almost nothing to
  `select-any`.
- The types would be shared through precompiled-types members holding
  407 MiB.

**Speed.** The analyzer reads the whole corpus in 14.7 s (955 MiB/s) on 8
threads. On four large libraries (2.1 GiB) it scales from 590 MiB/s on 1
thread to 1.9 GiB/s on 8. That is 3.2× on this Core Ultra 5 335, which
has fewer than 8 full-speed cores. Its grouping passes have each shard
scan every unit, which phase 5 removes.

### Linker spikes

The scripts and sources for every spike are in
[tools/turbolib](../tools/turbolib/README.md), with how to run each.

**A synthesized PCH object works with both linkers, with one rule.** A
COFF object written by our own script, with no compiler behind it, had:

- `.debug$S` holding only `S_OBJNAME`;
- a 4-byte `.bss` defining `__@@_PchSym_<name>`, as `/Yc` objects do;
- a `.debug$P` of shared records followed by `LF_ENDPRECOMP`.

Its users were rewritten to begin their `.debug$T` with `LF_PRECOMP` and
to add `-INCLUDE:` of the PCH symbol. With both `link.exe` and
`lld-link`, linked from the command line or through a library, the
programs ran, and the PDBs held the same types, functions, and
signatures as the baseline. The only difference was one more public, the
PCH symbol. lld's `/DEBUG:GHASH` works too.

The rule: **`LF_FUNC_ID` and `LF_MFUNC_ID` must not be in the shared
records.** With them in the PCH, `link.exe` silently drops the
signatures of the functions they name: the PDB shows `use_a` rather than
`int __cdecl use_a(int v)`. `LF_STRING_ID`, `LF_UDT_SRC_LINE`, and the
types are fine, and real `/Yc` PCHs hold the same kinds. lld takes all
of them. Since no type record refers to an id, the shared set stays
closed without function ids, and each member keeps its own. In LLVM,
that keeps 75 MiB of duplicated function ids.

A real `/Yc` object's `.debug$P` is laid out as: the shared records (as
many as the users' `LF_PRECOMP` count), then `LF_ENDPRECOMP` with the
signature, then the PCH source's own records. Its `S_OBJNAME` has
signature 0. The signature is only in `LF_ENDPRECOMP` and the users'
`LF_PRECOMP`.

**`NOLIBRARY` weak externals make `--pull=preserve` exact on COFF.** The
setup:

- Member A has a static initializer, so loading it has a side effect.
- B is the member the archive index names for the inline function F.
- In A, F became a weak external with
  `IMAGE_WEAK_EXTERN_SEARCH_NOLIBRARY`, whose alternate is a copy in a
  new member X.

Across six programs, A was loaded exactly when the original library
loaded it, with `link.exe` (`/OPT:REF` and `/OPT:NOREF`) and `lld-link`.
These included programs that reference only B, only A, or F directly, in
both orders. When A is loaded without B, A's calls go to X's copy.

**The archive index names the first definer in archive order, and
`lib.exe` reverses the command line.** `lib /OUT:x.lib a.obj b.obj` writes
b.obj first, so the index sends a symbol both define to b.obj.
`llvm-lib` does the same. When compacting a library, turbolib keeps the
definer that the original index names. When acting as `lib`, it uses
`lib.exe`'s order. Measured since, in phase 1 (see
`turbonop_archive::write` and `symbols`):

- `lib.exe` lets a later member supersede a common symbol (undefined,
  with a size), common or not, so of commons only the last wins.
  `llvm-lib` keeps the first.
- Libraries on `lib.exe`'s command line keep their order, each one's
  members reversed: `lib one.lib two.lib` writes one.lib's members last
  first, then two.lib's.
- `lib /DEF` lists an import library's members in the `.def` file's order
  in both linker members, while the archive holds them sorted by symbol.
- `lib.exe` writes `/<ECSYMBOLS>/` after `//`, and indexes ARM64EC and
  x64 members' symbols there; in a `/DEF` import library, the descriptor
  objects' symbols in both tables.

**Neither linker checks `.chks64` in a full link.** `.chks64` is the
compiler's table of per-section checksums. In the test:

- An object whose code was changed without updating `.chks64` linked
  without complaint, with both linkers, and the changed code ran.
- One whose `.chks64` was zeroed linked, and its program ran correctly.

Incremental linking (`/INCREMENTAL`) may use the table to find changed
sections, and that is still to test. Until then, turbolib drops
`.chks64` from every member it changes, rather than leave it stale.

**The size limit is 4 GiB for anything `link.exe` must read.** For a
library over 4 GiB:

- `lib.exe` stops with `LNK1248: image size (1130003C2) exceeds maximum
  allowable size (FFFFFFFF)`.
- `llvm-lib` writes it anyway, with a GNU `/SYM64/` index in place of
  the Microsoft linker members.
- `link.exe` rejects that library (`LNK4003: invalid library format`).
- `lld-link` links against it correctly, including a member at an
  offset above 4 GiB.

Measured in phase 1: `llvm-lib` writes `/SYM64/` (big-endian 64-bit
count and offsets, then the names, in member order, and no second linker
member) only when a member's offset is past 32 bits. A 4.3 GiB library of
four members whose offsets all fit keeps the Microsoft linker members:
`link.exe` rejects it too (LNK1127, "library is corrupt"), and `lld-link`
links against it. `lib.exe` also refuses more than 65,535 members
(LNK1189), whose numbers are 16-bit in the second linker member; `llvm-lib`
writes 65,536 with only the first linker member.

So turbolib can bring a library back under the limit, but it can't make
one that `link.exe` accepts above it.

## Phase 1 results

### The COFF object model (1.1)

`turbonop-coff` reads an object into values (headers, sections with
their data, relocations, and line numbers, and symbols with every
auxiliary record typed) and writes it back. Its `examples/roundtrip`
checks every object and library member under the paths it's given, on
every core:

| Corpus | COFF objects | Identical | Passed through |
|---|---|---|---|
| `build-z7` and `build-z7-nopch`, objects and libraries, with `testdata/coff` | 18,723 (18,565 big-obj) | 18,723 | 3,509 import objects, 3 `/GL` objects |
| Every `.obj` and `.lib` under MSVC 14.44's and 14.52's `VC\Tools\MSVC`, and under the Windows SDK's `Lib` (every architecture) | 142,930 (3,480 big-obj) | 142,930 | 581,441 import objects |

Objects of every tool the corpora hold round-trip, among them `cl` (x64,
x86, ARM64, ARM64EC, ARM32, `/clr`), clang, ml64, armasm, armasm64,
cvtres, and `lib.exe`'s import descriptors. So do the objects
`tools/coff/model.cmd` builds, and 70,000 sections or relocations of our
own writer's.

The tools lay objects out differently, so the reader records the layout,
and the writer follows it while every part keeps its size:

- `cl` and `lib.exe`: the headers, then each section's data and its
  relocations, in number order, packed; then the symbol table and the
  string table. The string table has an entry for each *use* of a long
  name: symbols' names in order, then sections' (MSVC's `.wowthk$aa`
  is there 19 times in one object, and two labels named `$again$10` get
  two entries).
- LLVM: data in the order it made the sections, which isn't their
  number order (it numbers associative sections last), a data pointer
  for every empty section that isn't uninitialized, each section's own
  number in its definition's `Number`, and a string table sorted by
  reversed bytes with names that end others stored inside them.
- The assemblers pad: ml64 starts relocations at even offsets, armasm
  and armasm64 align data. armasm64 shares one string between a section
  and its symbol, leaves bytes in a regular object's `HighNumber`, and
  gives labels function definitions; ARM32 armasm adds a record to a
  `.file` name that fills its records.
- `lib.exe`'s import descriptors give a section without relocations the
  previous section's relocation pointer. cvtres's string table says its
  size is 0.
- MSVC and LLVM both switch to the extended relocation count at 0xFFFF
  relocations, and the count includes its own record.

An object that is edited structurally is laid out afresh, as MSVC or LLVM
would (the style the reader detects). A fresh layout is the file the tool
wrote for 18,686 of the LLVM corpora's 18,723 objects, and for 91,535 of
the libraries' 142,930; every other one pads, shares strings, is an
import descriptor, or is LLVM's, as above (and reads back the same).

COMDAT checksums are JamCRC from 0 of the section's data
(`turbonop_coff::checksum`): all but 697 of the 62.7 million nonzero ones
in the LLVM corpora, and 3,514,639 of the libraries' 3,604,814. The rest
(mostly COMDATs with relocations) are computed some other way.

Not modeled: the CodeView in `.debug$S` and `.debug$T` (1.3), and the
section data that holds symbol indices (`.sxdata`, the `/guard` tables),
whose entries edits don't renumber; `Renumbering::table_index` maps them.

### The archive writer, import objects, and `.def` files (1.2)

`turbonop_archive::write` writes Microsoft archives as `lib.exe` (with
and without `/Brepro`) and `llvm-lib` write them, with the index computed
from the members as each tool computes it (`turbonop_archive::symbols`).
Its layout is planned before any data is written, and members are copied
in even pieces on every thread (4.6 to 5.1 GiB/s for LLVM's four largest
libraries, into memory). Its `roundtrip` example writes each library
again from its members:

| Libraries | Written back byte for byte | Index computed from the members |
|---|---|---|
| The LLVM corpora (`build-z7`, `build-z7-nopch`) | 582 of 582 (35.7 GiB) | 582 |
| Rebuilt from the corpus objects with `lib /Brepro` and `llvm-lib` | 490 of 490 | 490 |
| The Windows SDK 10.0.26100's and MSVC 14.52's | 1,851 of 1,851 | 1,748 |
| `testdata/coff` and `testdata/coff/archives` | 69 of 69 | 69 |

Every import object in them (566,000) is written back too. The 103 SDK
and MSVC libraries whose index isn't the one computed are Microsoft's own
builds: 43 index `__auximpcopy_` symbols that `lib /DEF` 14.52 doesn't
make, 33 merged import libraries keep every `__NULL_IMPORT_DESCRIPTOR`,
22 ARM64 libraries have other EC symbols, and 5 UCRT libraries send a name
to another member (the last two not looked into).

What `lib.exe` indexes (measured):

- External symbols in a section, absolute externals, and commons. Weak
  externals only with a section or a value, or as aliases
  (`SEARCH_ALIAS`); `NOLIBRARY`, `LIBRARY`, and anti-dependency weak
  externals aren't indexed, which is what `--pull=preserve`'s technique
  relies on.
- Each member's symbols sorted by byte value, members in archive order;
  the second linker member sorted by unsigned bytes. A name goes to the
  first member in archive order, except that among commons the last one
  wins. (`llvm-lib` keeps each member's own order and always the first.)
- An import object's code and constants get `__imp_X` and `X`, data only
  `__imp_X`; ARM64EC imports get their mangled and demangled names, and
  `__imp_aux_`, in `/<ECSYMBOLS>/`, after `//`.

Header fields: `lib.exe` dates tables and import members with the time of
the run (mode 0), and objects with their file's time (mode `100666`, or
`100444` read-only); under `/Brepro`, every date is `-1`. `llvm-lib`
writes `0 0 0 0` for tables and `0 0 0 644` for members. Above 4 GiB,
`lib.exe` stops (LNK1248), and above 65,535 members (LNK1189).

`turbonop-moduledef` reads `.def` files as `lib.exe` does (measured case
by case on about 200 files: statement keywords only in capitals, quotes
literal in `EXPORTS`, numbers as `strtoul` reads them, among others), and
plans the import objects `lib /DEF` makes, for x64, x86, ARM64, and
ARM64EC.

**Import libraries from `/DEF`** (`turbonop_moduledef`, run by
`apps/turbolib/src/msvc/def.rs`). turbolib now writes what `lib /DEF`
writes, the import library and the export file (`x.exp`, and for ARM64EC
and ARM64X `x.a64n.exp`), from the `.def` file, `/EXPORT`, `/NAME`, and
objects (their definitions, and their `.drectve` `/EXPORT:` directives),
for x64, x86, ARM64, ARM64EC, and ARM64X (`/DEFARM64NATIVE`); and what
`llvm-lib /def:` writes, with LLVM's own `.def` grammar and writer
(`turbonop_moduledef::llvm`: `==` imports and their weak externals). What
was measured on the way:

- **`/Brepro`'s time stamps** are the first four bytes of the SHA-256 of
  the member with its stamp set to `0xFFFFFFFF`, with the top two bits set
  (every member of every fixture). The export file's are `0xFFFFFFFF`.
- **The export file** is `.edata` (the export directory and tables, the
  exports sorted by import symbol for their ordinals), `.debug$S`
  (`S_OBJNAME`, `S_COMPILE3`, `S_ENVBLOCK` with the directory and the
  linker's path, an `S_EXPORT` for each export, and the `.def` file's
  SHA-256 and full path), and a `.drectve` of the `.def` file's
  statements (`/DLL /OUT: /BASE: /HEAP: /STACK: /VERSION: /SECTION:`);
  ARM64EC's adds x64 entry thunks (`.hexpthk`, `.gfids$y`, and their
  `S_THUNK32`s). Its layout is in `turbonop_moduledef::exp`. turbolib
  records its own path where `lib.exe` records `link.exe`'s.
- **Objects**: a `.def` export is found among their definitions as
  `link.exe` finds it (`_f`, `_f@8`, `@f@4`, a unique `?f@...`; LNK2001
  and LNK1120, LNK4022 and LNK4002 if not), and decorated it is imported
  `UNDECORATE`. Directives are taken as symbols. Objects aren't members.
- **What isn't reproduced**: `lib.exe`'s ARM64EC hints of imports by
  name are memory it didn't set (the same file gave 26982 in one run and
  0 in another); turbolib writes 0. `llvm-lib`'s `$$h` place in a C++
  name comes from LLVM's demangler; turbolib puts it after the first `@@`
  that a capital follows (likely the same for every ordinary name,
  untested for templates and special names).

Checked against the tools (each case run by both, everything written and
printed compared):

| | Cases | The same |
|---|---|---|
| `lib /DEF /Brepro` of 88 `.def` files (the fixtures', and measurement files) for each of x64, x86, ARM64, ARM64EC, and 3 ARM64X pairs | 355 | 355 libraries but ARM64EC hints; every export file (the S_ENVBLOCK's linker path aside) |
| `lib /DEF` command lines: names, `/EXPORT` forms, errors and warnings, x64 and x86 objects | 81 | 80 (the banner differs by design) |
| `llvm-lib /def:` of 38 `.def` files for 4 machines, errors, ARM64X | 157 | 157 (but `llvm-lib`'s missing newline after a parse error) |
| `testdata/coff/archives/def-*.lib`, written again by `turbonop_moduledef` | 41 | 41 |
| The corpus's `link.exe` import libraries with a `.def` (`LLVM-C`, `LTO`, `Remarks`: 1,321 imports) | 3 | 3, dates and stamps masked; `lib /DEF` gives the same bytes |

`tools/coff/imports.cmd` builds `testdata/coff/imports` (four `.def`
files of every feature, x86 objects, 16 command lines), which turbolib's
tests run again: 29 files the same.

### CodeView (1.3)

`turbonop-codeview` now knows every field of every type and symbol record
that holds a type index, an id index, a relocated address, a string-table
offset, or a file-checksum offset (`type_refs`, `symbol_refs`), reads and
writes every C13 subsection (`c13`), and renumbers sections in place
(`remap`). Every such field is 4 bytes, so a renumbered record keeps its
length: renumbering copies and patches at the offsets one layout table
gives, with no second encoder to disagree with it. The exception is a
checksum offset in compressed binary annotations, whose new value can
need more bytes; `remap` reports it, and `annotations::rewrite_files`
rebuilds the record.

Its `examples/corpus` checks every object and library member: every
record and subsection read and written back byte-identically, and every
index in range (with `LF_PRECOMP` users checked against their PCH's
records, matched by signature):

| Corpus | COFF objects | Type records | Symbol records | Failures |
|---|---|---|---|---|
| `build-z7` | 9,428 | 177 million | 71 million | 0 |
| `build-z7-nopch` | 9,287 | 460 million | 91 million | 0 |

Renumbering 13.65 GiB of `.debug$T` and `.debug$S` takes 1.12 s on 8
threads, with the files cached. On 600 sampled objects, every symbol's
indices agree with `llvm-pdbutil`'s, where it didn't crash (it did on 56).

Where cvinfo.h and LLVM disagree, or say nothing:

- LLVM's index discovery misses `S_INLINESITE2`, `S_HOTPATCHFUNC` (an
  id), `S_DEFRANGE`, `S_DEFRANGE_SUBFIELD`, and the managed kinds, so
  lld likely leaves their indices as they were.
- The managed records' field is a metadata token, not a type index: their
  relocations are `IMAGE_REL_AMD64_TOKEN` (proven for `S_GMANPROC`,
  `S_GMANDATA`, `S_MANSLOT`).
- `S_CALLEES`, `S_CALLERS`, and `S_INLINEES` hold ids, not types.
- `S_STATICLOCAL` (0x1176, from `cl /O1`) is documented nowhere; its
  layout comes from its relocations, so its fields' meaning is likely,
  not proven.
- Still only likely: `S_DEFRANGE`'s `program` is a string-table offset,
  and `S_LPROC32_DPC` names a type (no object has either).
- Kinds with no documented layout are errors, as are 16-bit kinds. None is
  in the corpora or the fixtures.

### The `lib` command line (1.4)

Run as `lib` or `llvm-lib` (or `turbolib lib ...`), turbolib takes each
tool's options, response files, and environment, orders members, checks
machines, and words its messages and exit codes as each does, all
measured with `lib.exe` 14.52 or read in LLVM's `LibDriver.cpp`
(`apps/turbolib/src/msvc`). The rules worth knowing:

- **Member order.** `lib.exe` writes the input libraries' members first
  (the libraries in command-line order, each one's members reversed),
  then the objects in reverse command-line order. An object replaces
  every library member with its name, and `/REMOVE` removes only library
  members. `llvm-lib` expands every input in order and reverses the
  whole list, replacing nothing.
- **Machines.** `lib.exe` checks only the objects on the command line,
  never library members, and never infers ARM64EC.
- **Options.** `_LIB_` is read after the command line, so it wins; a
  response file can't name another; quotes toggle anywhere in a word.
  The exit code is the number of the error that stopped it, and under
  `/WX`, the warning's.
- **Members and their headers.** A member copied from an input library
  keeps its header's date and mode (under `/Brepro`, its date is `-1`).
  LNK4006 is for a name defined again outside a COMDAT or in a
  `NODUPLICATES` COMDAT, not for other COMDATs or commons, each member's
  names in sorted order.
- **`/GL` members' index entries** come from the IL's global symbols: an
  inline function is listed only if something listed refers to it,
  transitively, with inline virtual members (which vftables refer to)
  and data no inline function refers to as roots. That matches both
  fixtures (x64 and x86, `__fastcall` names included); it is likely
  incomplete for objects of other shapes.

**The phase's check.** `tools/turbolib/corpus/relib.py` reran each corpus
build's archiver commands with turbolib in `lib.exe`'s place, and with it
in `llvm-lib`'s:

| | Libraries | The same as the tool's |
|---|---|---|
| As `lib.exe`, `build-z7` | 245 | 245, apart from header dates (29 s) |
| As `lib.exe`, `build-z7-nopch` | 245 | 245, apart from header dates (45 s) |
| As `llvm-lib`, `build-z7`, against `llvm-lib`'s | 245 | 245, byte for byte (27 s) |

The libraries are the same bytes but for dates, which a linker doesn't
read, so the programs linked against them are those `lib.exe`'s give
(`relink.py` showed two links of the same libraries give identical
images).

## Phase 2 results

`turbonop_fold::rewrite` folds a library's members as "Rewriting a
library" describes, and the `lib` personality runs it with
`/TURBOLIB:fold=identical` (or `TURBOLIB_OPTIONS=--fold=identical`).
Every step is parallel and decided in member order, so the output doesn't
depend on the thread count.

What doesn't fold, and why (`--diagnose` will say it per unit;
`crates/link/fold/examples/why.rs` does now):

- **Internal linkage.** A COMDAT whose own symbol is local (anonymous
  namespaces, `static` inline functions) is never shared between objects
  by the linker; folding it is `--locals=copy`. In `LLVMDemangle`, 816 of
  2,649 units.
- **References to locals.** A unit that refers to a local symbol outside
  it (a `$SG` string literal, an anonymous type's member), or whose local
  symbols something outside refers to.
- **Weak externals** fold when their default is an external symbol: each
  vftable refers to `??_E` (the vector deleting destructor), a weak
  external whose default is `??_G`. The synthesized member makes the weak
  external again.
- **Side effects** under `--pull=preserve`: see the weak-external
  technique's failure above.

**On the corpus** (`build-z7`, with the build's own archiver commands,
`relib.py` and `TURBOLIB_OPTIONS=--fold=identical`), 188 of the 245
libraries change (the rest have nothing to fold), from 13.21 GiB to 11.38
GiB (13.9%), in 59 s. The analyzer projects 2.7 GiB for
`identical, pull=preserve`; the difference is still to account for.
(Until 2.4, folded units' `.debug$S` is dropped, which counts as saved.)

`llvm-nm` and `llvm-readobj` link against the folded libraries
(`relink.py`), and on 22 inputs (`LLVMDemangle`'s objects and library,
and `testdata/coff`'s objects) print what the originals print, with `nm`
and with `readobj --all`. The images aren't byte-identical (COMDATs
move), so link equivalence waits for 2.6's comparison.

### Side tables (2.3)

Measured with cl 14.52 and clang-cl 24 (x64, x86, ARM64), and split as
"Side tables" above says ([`rewrite::tables`](../crates/link/fold/src/rewrite/tables.rs)).
Only a table turbolib can't read now keeps a member from folding.

- **`@feat.00`** bits: SafeSEH `0x1` (cl x86 always, clang-cl x86), `/GS`
  `0x100`, `/sdl` `0x200`, `/guard:cf` `0x800`, `/guard:ehcont` `0x4000`,
  `/kernel` `0x40000000`. cl 14.52 also always sets `0x10`, `0x80`,
  `0x10000`, and `0x80000000`; no option tried changed them. The bits are
  in the environment key, so a synthesized member has its givers'.
- **Entries naming undefined externals**: a `.gfids$y` entry naming an
  undefined external puts its definition (in another object) in the
  image's `GuardCFFunctionTable` with both linkers; without the entry, the
  CFG-checked call fails fast (`0xC0000409`). An `.sxdata` index naming an
  undefined external registers its definition with both linkers; without
  it, the handler isn't called and the fault kills the program.
- **With `@feat.00`'s `0x800` and no `.gfids$y`**, both linkers take the
  object to take no addresses (the call above fails).
- **`tools/turbolib/tables/run.py`** links a test program against a
  library of three objects and its folded copy, with link.exe and lld-link,
  `/OPT:NOREF` and `/OPT:REF`, for ten configurations (cl x64 `/Gy` and
  `/Gy-` with `/guard:cf /guard:ehcont /CETCOMPAT`, `/volatileMetadata`,
  `/d2guardretpoline`; cl x86 `/Gy` and `/Gy-` with `/guard:cf /SAFESEH`;
  cl ARM64; clang-cl x64 and x86). Every table compared is the same by
  name (function and offset) with both libraries: CFG functions, IAT
  entries, longjmp targets, EH continuations, SafeSEH handlers, volatile
  accesses, retpoline dynamic relocations, and the PDB's FPO records. The
  x64 and x86 programs (a CFG-checked call through an inline function's
  address, longjmp, throw, SEH, a call through an import's address) run
  and print the same; ARM64's weren't run.
- **link.exe and clang-cl's per-object tables**: under `/OPT:NOREF`,
  link.exe gives the entries naming the labels of COMDAT copies it
  discards addresses before the first section. With the original library
  the x64 program crashes (`0xC0000005`) and the x86 image doesn't load
  (likely because of those entries; not proven). The folded library has
  one copy, so it has no such entries, and its programs run. lld-link's
  programs run with both.
- lld-link 24 wrote no longjmp table for the x64 and ARM64 programs, with
  either library (it did for x86).

## Decisions

| | Decision |
|---|---|
| Order | Visual C++ COFF first (x64, then ARM64 and x86; ARM64EC later). ELF next, Mach-O last. |
| Form | One binary for every object and archive format. It is a drop-in replacement for `lib.exe` and `llvm-lib`, for `ar` and `llvm-ar`, and later for `libtool -static` (see [Commands](#commands)). There is also a `compact` command that rewrites an existing library. |
| Correctness | By default, the output links identically (see [Conservativeness](#conservativeness)). Every less conservative mode is opt-in and named for what it gives up. |
| LTO | Out of scope for now. `/GL` objects, LLVM bitcode, and GCC LTO objects pass through untouched. The builds this is for are not LTO builds. |
| Unrecognized input | A member with anything turbolib doesn't recognize passes through untouched. `--diagnose` (`/TURBOLIB:DIAGNOSE`) says, for each such member, what it didn't recognize. |
| Statistics | `--stats` (`/TURBOLIB:STATS`) prints, when the library is written, what was folded and shared, the bytes saved by category, what was passed through and why, and the time each stage took. |
| Threads | `-j N` (`/TURBOLIB:THREADS=N`). `-j 1` is strictly single-threaded, like `lib.exe` and `ar`: no thread pool is created, and nothing is spawned. The default is one thread per logical core. The output is byte-identical for every `N`. |
| Language | Safe Rust. No `unsafe` outside what `turbonop-mmap` already has. mimalloc, as everywhere. |
| Speed | Every stage saturates the cores or the disk (see [Pipeline](#pipeline)). |
| Determinism | The same inputs and options give byte-identical output on any machine and with any number of threads. Buck2's remote cache depends on this. |
| Reporting | Counts and sizes by default. ODR findings are available (`--report odr`) but are never warnings by default: in real code they are everywhere, and noise teaches people to ignore the tool. |
| Name | `turbolib` (app and command), part of the turbonop-utilities suite. On crates.io, `turbolib` is taken (an S3 library, last updated 2022), so the crate is `turbonop-turbolib`, which is free. |
| Crates | Writers go into the format crates that already read these formats (`turbonop-archive`, `turbonop-pe` or a new `turbonop-coff`, and `turbonop-codeview`). The folding engine is a new layer, `crates/link` (`turbonop-fold`, `turbonop-precomp`), at rank 3, which `layers.rs` learns about. The app is `apps/turbolib`. |
| Test data | Small fixtures are synthesized by our own writer or built by a script from sources we write. They are committed. The large corpus is built by a script from open-source C++ (LLVM, abseil, Boost, fmt) and is not committed. Libraries from work are measured at work, and their numbers stay there. |

## What exists, and what's missing

**Existing pieces (readers):**

- `turbonop-archive` reads all three archive dialects, thin archives, and
  import members. It writes the Microsoft dialect as `lib.exe` (with and
  without `/Brepro`) and `llvm-lib` do (`write`), and computes the symbols
  each indexes (`symbols`) (phase 1.2).
- `turbonop-moduledef` reads `.def` files as `lib.exe` does, and plans
  the import library `lib /DEF` makes of one (phase 1.2).
- `turbonop-pe` reads COFF objects and import objects. Its `ltcg.rs` can
  recognize `/GL` objects, which are passed through untouched.
- `turbonop-codeview` reads type and symbol records, line tables, and
  inlinee annotations.
- `turbonop-debuginfo`'s `coff_object.rs` already turns an object's
  CodeView into a PDB view: relocations applied, `S_GPROC32_ID` resolved,
  checksums and the string table read. That is most of what remapping
  needs to understand.
- `turbonop-elf`, `turbonop-macho`, and `turbonop-dwarf` read the other
  formats, including split DWARF (`program/tests/it/split_dwarf.rs`) and
  Darwin debug maps (`macho/src/debugmap.rs`).

**Missing pieces (writers, and everything that decides what to write):**

- **A COFF object model that round-trips.** Read an object into an editable
  form and write it back. Sections, symbols with every auxiliary record
  kind, relocations, string table, and big-obj headers in both directions.
- **The GNU and BSD archive writers** (phase 6), and the import
  descriptor objects of `/DEF` import libraries (their layout is in
  `turbonop_moduledef::imports`; they wait for the COFF writer).
- **A complete CodeView writer and type-index visitor.** For every type
  record, symbol record, and C13 subsection, this means knowing every field
  that holds a type index or an id index, and every field that holds a
  checksum offset or a string-table offset. LLVM's `TypeIndexDiscovery` is
  the model. It must be exhaustive, because a missed field quietly corrupts
  the debug information.
- **CodeView type hashing and merging.** Structural (Merkle) hashes of
  records: a record's own bytes, with each index replaced by the hash of
  the record it names. Then a merge that assigns new indices.
- **The folding engine**, the precompiled-types synthesizer, the policy
  layer, and the reports.

## How it works (COFF)

### Units of folding

`NODUPLICATES` COMDATs are never fold units, in any mode. With `/Gy`,
every function is a COMDAT, and ordinary functions with external linkage
are `NODUPLICATES`: they are strong definitions, and two copies of one
are a duplicate-symbol error that the linker must still report.

A **fold unit** is a COMDAT leader plus everything that must move with it:

- the leader's section;
- its associative sections (`.pdata`, `.xdata`, `.debug$S`, `.CRT$XCU`,
  sanitizer metadata), transitively;
- the static (local) symbols defined in those sections: `$LN` labels, jump
  tables, `$ip2state$`, `$cppxdata$`, `$stateUnwindMap$`;
- its **outside references**: every relocation target that is not inside
  the unit. These are external symbols, other COMDATs, and local symbols
  defined in non-COMDAT sections of the member (see
  [internal linkage](#internal-linkage-references)).

The unit's **identity hash** covers:

- the bytes of every non-debug section in the unit;
- the characteristics and names of those sections (`.text$mn` and
  `.rdata$r` order the image);
- each relocation as (section, offset, type, target), where a target
  outside the unit is named by its symbol name and a target inside it by
  its position;
- the selection type and the COMDAT checksum;
- the member's **environment key** (below).

Debug sections are not in the identity. They come along with whichever
copy survives, which is exactly what the linker does with the copies it
discards.

### The environment key

The environment key covers the facts about a member that a folded unit
must not lose. Synthesized members are grouped by this key, and in
select-any mode it bounds which copies may stand in for each other:

- the machine type;
- the `@feat.00` bits (SafeSEH, `/guard:cf`, `/guard:ehcont`, `/kernel`, and
  any others found in phase 0);
- `/FAILIFMISMATCH` values in `.drectve`, such as `_ITERATOR_DEBUG_LEVEL`
  and `RuntimeLibrary`;
- the compiler identity from `S_COMPILE3` (front end and back end
  versions, language, flags);
- for `/Zi` objects, the type server's signature;
- the instrumentation in use: ASan, SanCov, `/JMC`, `/hotpatch`.

### Rewriting a library

1. **Read** every input in parallel. Inputs are objects and libraries, and
   input libraries are taken apart into their members. Pass through
   untouched: `/GL` objects, import members, objects of another machine,
   and anything turbolib doesn't understand. (A member turbolib produced
   is understood, so compacting a compacted library is idempotent.)
2. **Find and hash** every member's fold units.
3. **Group** units by identity hash. For each group, decide by policy
   whether it folds, and choose its survivor deterministically: the first
   copy in member order, regardless of thread timing.
4. **Lay out the synthesized members.** Folded units are grouped by the
   *set of members that held a copy* and by environment key. Each group
   becomes one synthesized member. Groups are split when needed to stay
   within section limits (big-obj above 65,279 sections) and a size cap.
   Because a group is defined by its member set, a synthesized member is
   wanted exactly when one of those members is.
5. **Rewrite each original member**:
   - Remove the folded units' sections.
   - Make each removed leader (and each other external defined in the
     unit) an undefined external.
   - Add an undefined reference to the **anchor** symbol of each
     synthesized member it gave units to. This keeps unreferenced COMDATs
     alive under `/OPT:NOREF` (the default with `/DEBUG`) exactly when they
     were alive before.
   - Renumber sections and symbols, and rebuild the symbol table.
   - Split the per-object side tables ([below](#side-tables)).
   - Rewrite `.debug$S` and `.debug$T` (phase 3).
6. **Write each synthesized member**:
   - the units;
   - a defined anchor symbol;
   - a `.drectve` holding the environment's `/FAILIFMISMATCH` directives;
   - `@feat.00`;
   - its side-table entries;
   - CodeView: `S_OBJNAME` naming it (`turbolib:<library>:<n>.obj`),
     `S_COMPILE3` and `S_ENVBLOCK` copied from the environment, file
     checksums and a string table built from what its units' lines use,
     and the units' `.debug$S` with indices remapped into its own types
     (or into the shared precompiled types, phase 3).
7. **Write the archive**: members in a deterministic order (original
   members in input order, then synthesized members, then the
   precompiled-types member), both linker members, `/<ECSYMBOLS>/` when
   needed, and the long names.

### Side tables

Some sections are per-object tables that are not associative to any one
COMDAT. Each entry names its target through a relocation (or, for
`.sxdata` and the `/guard` tables, a symbol index), so each can be split:
an entry stays with the member whose code needs it, and is copied to the
synthesized member that receives that code.

| Section | Entry | Rule |
|---|---|---|
| `.gfids$y`, `.giats$y`, `.gljmp$y`, `.gehcont$y` | symbol index | The functions whose address is taken (and a vftable's functions), the `__imp_` symbols whose IAT entry's address is taken, the labels after `setjmp` calls, and the labels a catch continues at. cl `/Gy` writes each function's in a COMDAT associated with it; `/Gy-` code and clang-cl, one table per object. Entries naming an external stay (now undefined, which both linkers resolve: measured) and are copied to the synthesized member for the code that refers to them; entries naming a moved unit's labels go with it. A superset is harmless. A missing entry is a CFG failure at run time, so err toward more entries. |
| `.sxdata` (x86) | symbol index | The handlers the member's functions register (`__ehhandler$...` in the function's `.text$x`, `__except_handler4`); one table per object. Split as the `/guard` tables are; an index naming an undefined external resolves (measured, both linkers). |
| `.debug$F` (x86 FPO) | relocation | 16-byte `FPO_DATA`, a `DIR32NB` relocation at each record's start naming the function: in a COMDAT associated with it (`/Gy`), or one table (`/Gy-`). Each record moves with its function. Without `/Z7` cl writes one for every function; with it, only for SEH functions. |
| `.voltbl` | bytes, or symbol index | x64 `/volatileMetadata` (off by default in 14.52; ARM64 and ARM64EC write none): in a COMDAT associated with a function, its volatile accesses' offsets as bytes; outside COMDATs (`/Gy-`), records of `0xFFFFFFFF`, a symbol index, and offsets. Records move with their function. link.exe reads them only from objects with `@vol.md`, which synthesized members copy. lld-link writes no volatile metadata. |
| `.retplne` | section number | cl `/d2guardretpoline` (`/guard:retpoline` includes the WDK's `guardcfw.h`, and with an empty one cl 14.52 rejects it, C1049; the `/Qspectre` options make none): `RetpolineV1\0`, then records of a size, a section number, and (kind, offset) pairs for the section's indirect calls, import jumps, and switch jumps. One table per object. Records move with their section. `link /guard:retpoline` makes dynamic relocations of them; lld-link has no `/guard:retpoline`. |
| `.llvm_addrsig` (clang-cl) | ULEB128 symbol index | The address-significant symbols, for lld-link's ICF. Split as the `/guard` tables are. |
| `.chks64` | per section | Dropped from every member turbolib changes. Neither linker checks it in a full link (phase 0). Incremental linking is still to test. |
| `.msvcjmc` | COMDAT already | Nothing to do. |
| `.drectve` | directive | Stays in the original member. `/FAILIFMISMATCH` is also copied into synthesized members. `/EXPORT` of a folded symbol still resolves, through the synthesized member. |

Any section turbolib doesn't recognize in a member makes that member pass
through unfolded, with a report. turbolib never guesses about these.

### Internal-linkage references

A unit whose relocations reach a local symbol defined outside it is
handled according to `--locals`:

- a header's `const int k = 10;` has internal linkage, so each TU has its
  own `k`;
- `static` helpers;
- anonymous namespaces.

| Mode | Behavior |
|---|---|
| `keep` (default) | Such a unit doesn't fold. |
| `copy` | The unit folds, and the locals it reaches are copied into the synthesized member as locals. Only the address identity of those locals is observable, and comparing it across TUs is already an ODR violation. |
| `merge` | As `copy`, and identical copied locals within a synthesized member are merged. |

## Conservativeness

Each axis is its own option, so a team can say exactly what it gives up.
The defaults are the conservative choice on every axis.

### `--fold`: which copies count as the same

| Mode | Folds |
|---|---|
| `none` | Nothing. turbolib is just `lib.exe` (phase 1). |
| `identical` (default) | Units whose identity hashes are equal. |
| `select-any` | Units that the linker could legally substitute for each other, by selection type. See the table below. |

How `select-any` treats each selection type:

| Selection | In `select-any` |
|---|---|
| `NODUPLICATES` | Never folded. |
| `ANY` | Folded. The survivor is chosen by `--pick`: `first` in member order (the default, closest to what the linker picks), or `largest`. |
| `SAME_SIZE` | Folded when the sizes agree. When they don't, both copies stay and the report lists them, because the linker would fail only if both copies reached one link. |
| `EXACT_MATCH` | Folded when the checksums agree. Otherwise, as for `SAME_SIZE`. |
| `LARGEST` | Folded, keeping the largest. The linker keeps the largest among the copies in a given link, so this can choose a copy from a member that link never loads. That is allowed by the semantics, but it is a different choice. |
| `ASSOCIATIVE` | Always follows its leader. |

In both `identical` and `select-any`, only units with the same
environment key fold together.

### `--pull`: which members a link loads

A linker loads a member when it defines a symbol the link still needs.
The archive index sends each symbol to the first member defining it. If a
folded symbol F was the only reason some link loaded member A, that link
no longer loads A, because the synthesized member defines F now. When A
has side effects, they leave that link:

- dynamic initializers;
- self-registering factories;
- TLS callbacks;
- `.drectve` contents (`/DEFAULTLIB`, `/INCLUDE`, `/alternatename`).

| Mode | Behavior |
|---|---|
| `preserve` (default) | Nothing about which members load changes. A group is folded only if every definition of its names in the library is a copy in it (so the member the index names keeps being named for them, by a synthesized member), and not if its first copy's member has side effects. The weak-external technique below was meant for those, but it fails in `link.exe` (phase 2's results), so they aren't folded. |
| `relaxed` | Fold regardless, and report the members whose loading may change. |

**The weak-external technique.** This is for COFF. Phase 0 proved it in
both `link.exe` and `lld-link` (see [Phase 0 results](#phase-0-results)):

- F stays defined in A, the member the archive index names for it.
- In every other member, F becomes a weak external with
  `IMAGE_WEAK_EXTERN_SEARCH_NOLIBRARY`, whose alternate is a renamed copy
  in the synthesized member (`__turbolib$` and F's name, which can't
  collide with a name a compiler makes).
- The renamed copy shows up in the PDB's publics under its new name. The
  debugger names the function from its `LF_FUNC_ID`, so this matters
  only to tools that fall back to publics.

Expected result: a reference to F still loads A. A link that loads B
without A gets the copy. A link that has both uses A's F. Under
`/OPT:REF`, the copy is discarded when it is unused. ELF has nothing
equivalent, so in phase 6 `preserve` means skipping these units.

**It fails in `link.exe` 14.52** when a strong reference to F reaches the
link from a library member after the weak externals: from a synthesized
member whose code calls F, or from any member that refers to F without
defining it. Then F is reported unresolved (LNK2001), the weak externals
included, though the renamed copy is in the library. Phase 0's spike
passed because its strong reference came from an object on the command
line, which the linker reads before searching libraries. Reproduced with
three objects (an inline `F`, an inline `G` calling it, and a member with
a dynamic initializer first). So the engine doesn't use it
(`rewrite::Options::weak_externals` turns it on, for study), and such
groups aren't folded under `preserve`. Making all of a library's
references to F weak too might rescue it; that is untested.

What counts as a side effect: non-COMDAT dynamic initializers,
terminators, and TLS callbacks (`.CRT$XC*`, `.CRT$XI*`, `.CRT$XL*`,
`.CRT$XP*`, `.CRT$XT*`), and `/INCLUDE` directives, except a PCH user's
`/INCLUDE` of its PCH symbol. Still to decide: `/DEFAULTLIB` directives
that differ from the rest of the library's. A member loaded only for F
could be the only one that adds some library to the link.

The anchor references from step 5 are always added. They make the keep
and discard behavior under `/OPT:NOREF` exact in every mode.

### `--precomp`: shared types (phase 3)

| Mode | Behavior |
|---|---|
| `none` | Each member keeps its own `.debug$T`. Synthesized members get merged types of their own. |
| `library` (default once phase 3 ships) | One precompiled-types member for the whole library. |
| `clustered=<n>` | Up to *n* precompiled-types members, with members clustered by the types they share. |

## Shared types (phase 3)

Under `/Z7`, the duplicated `.debug$T` is likely the largest single part
of a large C++ library. The phase 0 analyzer measures this. CodeView
already has a way to share types between objects: precompiled headers.
An object built with `/Yu` begins its `.debug$T` with `LF_PRECOMP` (start
index, count, signature, PCH object name). Its type indices from `0x1000`
for `count` records then refer to the PCH object's `.debug$P`. That
section ends in `LF_ENDPRECOMP` with the same signature, which the PCH
object's `S_OBJNAME` also carries. The PCH object defines a
`__@@_PchSym_` symbol, and each user references it, so linking a user
brings in the PCH object.

turbolib makes a PCH object that no compiler ever made:

1. **Hash** every type and id record of every member structurally. Records
   in an object's stream refer only to earlier records, so one pass in
   stream order suffices. Phase 0 checks that this holds for every
   compiler in the corpus. MASM is known to violate it, so a member that
   breaks the rule doesn't share its types.
2. **Choose the shared set**: the records used by at least `k` members
   (default 2), closed under references. Some records stay in each
   member:
   - `LF_FUNC_ID` and `LF_MFUNC_ID`, which `link.exe` won't take from a PCH
     (it drops the functions' signatures). No type refers to an id, so the
     set stays closed without them.
   - Records that are inherently local, such as `LF_BUILDINFO` and the
     strings only it uses.
3. **Write the precompiled-types member**: the shared set in topological
   order as `.debug$P`, `LF_ENDPRECOMP`, `S_OBJNAME` with the signature, and
   a defined `__@@_PchSym_turbolib_<hash>` symbol.
4. **Rewrite each member's `.debug$T`** as `LF_PRECOMP(0x1000, |shared|,
   signature)` followed by its local records, renumbered from `0x1000 +
   |shared|`. Then remap every type and id index in its `.debug$S`
   (symbols, inlinee lines, `S_BUILDINFO`) and add the undefined reference
   to the PCH symbol.
5. **Members already built with `/Yu`.** If their PCH object is in the
   library, its types join the shared set and the old PCH object becomes
   an ordinary member. If it isn't, those members keep their
   `LF_PRECOMP`, because an object can name only one.

**The trade-off.** A linker merges all of a precompiled-types object's
records into the PDB, used or not. Neither linker prunes the types that no
kept symbol refers to, measured with `/DEBUG:FULL`. With 200 structures,
each used only by its own `/Gy` function:
- An object whose 200 functions `/OPT:REF` discards still brings all 200
  structures, and all 200 functions' `LF_FUNC_ID` records, into the PDB.
  Its symbols for those functions are gone.
- A `/Yc` object holding the 200 structures brings all 200, although its
  one `/Yu` user refers to one.

The same is true of link.exe and lld-link, with `/OPT:REF` and
`/OPT:NOREF`. A link that loads three members of a
library of 1,000 merges types for all 1,000. `clustered` bounds this by
partitioning, and the analyzer reports the cost for each choice. Shared
records are merged once per link instead of once per member, so for
links that use much of the library we expect faster links and smaller
peak memory. Phase 0 measures both.

**Possibly later:** global `S_UDT` and `S_CONSTANT` records. They are
duplicated in every module's `.debug$S`, and the linker deduplicates them
into the globals stream anyway. Moving them into the precompiled-types
member changes which module's stream holds them, so it is an opt-in mode,
if measurements say it pays.

## Pipeline

- **Read:** files memory-mapped, and members parsed in parallel, one task
  per member. Fold units are found and hashed in the same pass. Type
  hashing is parallel per member, since each member's records refer only
  to its own earlier records.
- **Reduce:** a sharded concurrent hash table from identity hash to the
  survivor, and from type hash to the shared index. Results are resolved
  deterministically by member order after the parallel insert, so the
  thread count cannot change the output.
- **Plan:** the sizes of every output member are computed first, then the
  archive's index and member offsets, all before any data is written.
- **Write:** every output member is written in parallel, straight into its
  offset in a preallocated output file. Any number of threads may write,
  and the file is complete when they finish.
- **Measured** with Stackrabbit (ETW): the goal is no idle core outside
  I/O waits, and the disk at its bandwidth during reads and writes.

## Commands

One binary serves every format. It chooses its personality by the name it
is run as:

- `lib`, `llvm-lib`;
- `ar`, `llvm-ar`, `*-ar` (for example `x86_64-linux-gnu-ar`);
- `ranlib`, `llvm-ranlib`;
- later, `libtool`;
- under any other name, `turbolib <personality> ...` selects one
  explicitly.

Within a personality, the format of the output follows the inputs (or
`--format`, as `llvm-ar` spells it).

- **The `lib` personality.** `turbolib /OUT:x.lib a.obj b.lib ...` takes
  `lib.exe`'s command line, including `@response` files and `/NOLOGO`,
  `/MACHINE`, `/WX`, `/IGNORE`, `/LIST`, `/EXTRACT`, `/REMOVE`, `/NAME`,
  `/DEF`, `/EXPORT`, `/LTCG`, and `/Brepro`. It also takes `llvm-lib`'s
  additions (`/llvmlibthin`). Import libraries from `/DEF` are written
  too, in full.
- **The `ar` personality.** It takes `ar`'s and `llvm-ar`'s operations and
  modifiers:
  - `r`, `q`, `d`, `m`, `t`, `x`, `p`, and `s`;
  - `c`, `u`, `D`, `U`, `T`, `S`, `a`, `b`, `i`, and `N`;
  - `--thin`, `--format`, and `--rsp-quoting`;
  - MRI scripts (`-M`);
  - `@response` files.

  `--plugin` is accepted and ignored, as LTO is out of scope.
- **turbolib's own options.** These are `--fold`, `--pull`, `--locals`,
  `--precomp`, `--diagnose`, `--stats`, and `-j`. A personality accepts
  them in its own spelling (`/TURBOLIB:<option>` for `lib`) or from the
  `TURBOLIB_OPTIONS` environment variable, so a build system that can't
  add arguments can still set them.
- `turbolib compact in.lib -o out.lib [options]`: rewrites an existing
  library.
- `turbolib analyze x.lib [--json]`: what each mode would save, by
  category (code, unwind, `.debug$S`, `.debug$T`, other), the hazards each
  mode would accept, and the projected PDB cost of each `--precomp`
  choice. It writes nothing.
- `turbolib verify [--pull=preserve|relaxed] original.lib compacted.lib`:
  static equivalence. Every symbol the original defines is still defined,
  and each member's undefined symbols are the original's plus the folded
  ones. Every folded unit is present in exactly one synthesized member, and
  every side-table entry is accounted for. See
  [the tools](#the-tools-verify-and-compare-images-26).
- `turbolib compare-images a.exe a.pdb b.exe b.pdb`: link equivalence of
  two programs, by content (see the same section).
- Later, `turbolib ar ...` and `turbolib libtool ...`.

`/EXTRACT` on a compacted library gives an object without its folded
units. This is documented, and `/EXTRACT` can also re-inline them
(`/TURBOLIB:extract-whole`).

## Verification

- **Round trips (phase 1):** every object and library in the corpus is
  read and written back byte-identically. With `--fold=none`, turbolib
  must make each of the build's libraries byte for byte as `lib.exe` made
  it, apart from header dates (`tools/turbolib/corpus/relib.py`, which
  reruns the build's own archiver commands). Programs linked against those
  libraries must be byte-identical images
  (`tools/turbolib/corpus/relink.py`, with `/Brepro`).

  PDBs can't be compared byte for byte: `link.exe` 14.52's PDBs differ
  between two links of the same inputs, even with `/Brepro` and
  `/THREADS:1`. Their module streams come in another order, the IPI
  stream has another size, and among same-named records it picks
  differently. So PDBs are compared as in phases 2 to 4, structurally.
- **Link equivalence (phases 2 to 4):** a set of test programs is linked
  against each library before and after compaction, with `link.exe` and
  with `lld-link`, under `/OPT:REF` and `/OPT:NOREF`, with `/DEBUG:FULL`
  and `/DEBUG:GHASH`. COMDAT order changes addresses, so images are
  compared by:
  - the set of functions and data, with their bytes compared modulo
    relocations;
  - imports, exports, and initializer order;
  - the load config and the CFG tables;
  - the unwind data.

  PDBs are compared through `turbonop-pdb`: every public, procedure, line,
  inlinee, local, and type, compared structurally by name. Under
  `--pull=preserve`, the set of members each link loads must be identical
  (read from `/VERBOSE` and the map file; hexcavator's linker-log reader
  already parses those). Then the test programs run their own test
  suites.
- **Debuggers:** spot checks with the VS debugger and WinDbg on the
  corpus. Our own PDB reader does the systematic checking.

### The tools: `verify` and `compare-images` (2.6)

Both are in `turbonop-verify` (`crates/link/verify`), run on every core,
and print what fails by category, with counts and examples; they exit 1
on any failure. "Expected" differences (a folded function's module, a
PDB's GUID) and what wasn't checked are listed too, apart.

**`turbolib verify`** reads the two libraries alone. Each COMDAT unit (a
leader with its associative sections) is reduced to a 128-bit hash of a
canonical form: selection, each non-debug section's name, characteristics,
and bytes (`/guard` tables' symbol indices as the symbols they name), each
relocation with its target by name outside the unit or by position inside
it, and the symbols it defines; sections in an order of their contents,
names without turbolib's `__turbolib$` prefix. It checks:

- members: every original one present, by name, in order; every added one
  synthesized (it defines an anchor: a `__turbolib$` name that is no
  original symbol), wherever it is placed; import objects, `/GL` objects,
  and anything unreadable byte-identical;
- definitions: every name the original defines still defined, with each of
  its kinds (code, data, uninitialized, absolute, common; COMDAT
  selection); no new names but anchors and renamed copies;
- each original member against itself: its directives, environment, and
  sections outside COMDATs unchanged (but debug sections, side tables, and
  `.chks64`); every unit it keeps unchanged; every unit it lost foldable
  (not `NODUPLICATES`) and held, in the member's environment, by a
  synthesized member it anchors; each lost definition now undefined (and
  defined elsewhere) or a `NOLIBRARY` weak external whose default is the
  renamed copy; its undefined symbols the original's plus the moved ones,
  anchors, and renamed copies' names; its side-table entries (`/guard`
  tables, `.sxdata`, `.voltbl`) still its own or those of a synthesized
  member it anchors;
- synthesized members: each unit matches a copy some original member held
  in the same environment; no unit is in two synthesized members; every
  name they refer to is one the original defines or refers to;
- both archive indexes: every definition indexed, each name sent to its
  first definer in archive order, the two linker members the same set.
  This model of `lib.exe`'s index matched every one of the LLVM corpus's
  250 libraries (each verified against itself: 37.8 s for 13.7 GiB, read
  twice).
- `--pull=preserve` (the default): every symbol of the original's index
  still sends to the same member, or to a synthesized member when the
  original definer has no side effects (non-COMDAT `.CRT$X*`, `/INCLUDE`).
  `--pull=relaxed` lists the symbols sent elsewhere, as expected.

What `verify` can't prove: anything about debug information (that is
`compare-images`'s); that a definer without side effects of its own loads
nothing that has some (it doesn't follow references); that hashes equal
mean bytes equal (a collision of two 128-bit keyed hashes, about 2^-128 a
pair, would pass); and anything a linker does other than load members by
the index and choose among COMDAT copies, which the phase 0 spikes
measured. ARM64EC's `/<ECSYMBOLS>/` is not checked (reported).

**`turbolib compare-images`** compares two programs through their PDBs.
Every address in either image is named by the nearest label at or below
it, and the distance from it: publics, then procedures and data of the
modules (a name used at two addresses qualified by its module), then
section contributions (each named by its module, characteristics, size,
CRCs, and an ordinal), then sections. Then:

- **functions and data:** every section contribution (a COFF section of
  one object) is paired with its counterpart by the label at its start
  (or its own label; those left over, by their content with addresses as
  names, reported as expected), and their bytes compared: base-relocated
  addresses (`HIGHLOW`, `DIR64`) by what they name; in code, PC-relative
  operands found by decoding (x86-64: relative branches and calls,
  RIP-relative operands; ARM64: `B`, `BL`, `B.cond`, `CBZ`, `TBZ`, `ADR`,
  literal loads, `ADRP` with the `ADD` or load or store that completes it)
  by what their targets name; any other differing 4-byte word read as an
  RVA in both, and accepted if both name the same thing. Contributions
  holding the linker's tables (export and debug directories, debug data,
  delay-import descriptors, `/guard` tables) are compared as tables.
- **tables:** imports and delay imports, by DLL and name (in order;
  another order is expected); exports (ordinal, name, what it names); the
  C runtime's `__xc_a`..`__xc_z`, `__xi_*`, `__xp_*`, `__xt_*` tables and
  the TLS callbacks, in order; the load configuration, field by field
  (pointers by what they name); the CFG, IAT, long-jump, EH-continuation,
  and SafeSEH tables as sets; and the unwind data function by function
  (`.pdata` matched by what its function's start names, with its extent
  and its `UNWIND_INFO` or `.xdata`).
- **headers:** machine, characteristics, subsystem, DLL characteristics,
  image base, entry point, and the sections' names and characteristics in
  order; other sizes are expected.
- **PDBs, by content:** every type and id by structural hash (each index
  replaced by the hash of what it names); every symbol record with its
  indices as those hashes, its addresses as names, its string and file
  offsets as strings, and scope links left out; then publics, globals,
  procedures by name (each its records from `S_*PROC32` to its end, in
  order; another module is expected), modules by name (a synthesized one,
  `S_OBJNAME` `turbolib:...`, only in the second, expected), line blocks
  by what their start names, inlinees' lines, and contributions. The
  linker's sections and COFF groups are compared by name (their sizes
  follow the layout).

What `link.exe` changes between two links of the same inputs, and so what
isn't compared as it is: S_UDT and S_CONSTANT are compared as one set over
the globals and every module (it keeps one of each name in the globals,
which one following the order it reads modules in);
`LF_UDT_MOD_SRC_LINE` is compared by the UDT's name, file, and line, only
for the UDTs both PDBs have one for (it writes them for some UDTs in some
links, gives them to one of the same-named type records, and numbers
their modules in its own order).

What `compare-images` can prove: that the two images hold the same
contributions, each with the same bytes once every address field it
found is replaced by what it names, and the same tables; and that the PDBs
hold the same records under the same reductions. What it can't:

- A data word holding an RVA that nothing relocates, if equal in both
  images, isn't checked (it can't be told from an integer); a differing
  word accepted as two RVAs naming the same thing is so by inference.
- Outside a procedure's code (as the PDB gives it), equal bytes that
  decode as PC-relative instructions are accepted (they may be data).
- Bytes outside every contribution (headers, most of the linker's own
  tables) aren't compared; their count is reported.
- x86 (32-bit) code isn't decoded, so x86 images are compared by base
  relocations and inference only; a relative call between functions that
  moved would show as a difference. ARM64 decoding is tested on
  synthesized instructions only: no ARM64 image has been compared.
- Symbol records whose kinds' fields aren't known are compared as bytes,
  and C13 subsections other than lines, inlinees' lines, and file
  checksums aren't compared; both are counted.
- Nothing is run. Equivalence of the bytes and tables is not a proof that
  the programs behave alike under anything that reads addresses as
  numbers (hashing pointers, say).

**Results** (on this laptop, the LLVM corpus `build-z7`):

| Check | Result |
|---|---|
| `verify` of each of the 250 libraries against itself | all pass, 37.8 s |
| `compare-images` of two `link.exe` links of llvm-nm from the same libraries, before handling link.exe's differences | 44,239 differences: `LF_UDT_MOD_SRC_LINE` (22,103 and 22,114), S_UDT and S_CONSTANT picks (8), modules' S_UDTs (14) |
| the same, after | equivalent; 77,798 contributions (33.6 MB), 850,894 PC-relative operands and 64,107 base relocations matched, 666,550 types, 196,328 ids, 147,113 procedures, 139,515 line blocks; 2.5 to 13 s |
| relink.py's matrix with the same libraries: link.exe and lld-link, `/OPT:REF` and `NOREF`, `/DEBUG:FULL` and (lld) `GHASH` | all 6 equivalent |
| llvm-nm against an LLVMSupport.lib with one byte of one function changed (breaklib.py), link.exe and lld-link | caught: that function's bytes differ (1 failure; lld's contribution CRCs too) |
| the fixtures (`testdata/turbolib`): two link.exe links; `/ORDER` (other addresses) with each linker; lld's GHASH | equivalent, but for the linker's command line |
| `verify` of the engine's folds (`/TURBOLIB:fold=identical`, at e8121b2) of llvm-nm's 32 libraries, against the build's | all pass (`--pull=preserve`): LLVMSupport, for one, 912 synthesized members, 31,273 COMDATs moved, 4,177 held |
| `compare-images` of llvm-nm linked against those folded libraries | not equivalent, in every configuration: see below |

What compare-images reports for the folded libraries (link.exe,
`/OPT:REF`; NOREF has fewer of the second kind), as of e8121b2:

- Folded functions have no debug information yet (2.4): about 19,000
  procedures, 20,000 line blocks, 18,000 ids, and 12,000 types are only in
  the original's PDB, with the publics and globals that go with them.
- Under `/OPT:REF`, the folded link keeps about 15,000 functions the
  original's discards (with their unwind entries), and code refers to
  them; under NOREF these differences mostly go. With lld-link the folded
  link also loads 43 more original members (more initializers and
  imports). Both look like real changes in what a link keeps and loads;
  not yet explained.
- Some of the rest are likely compare-images's own: contributions whose
  CRC link.exe wrote in one link and not the other get other labels
  (labels name unlabeled contributions by their CRCs; without them,
  contributions alike in size pair wrongly), and show as functions and
  operands that name other things.
- **Fuzzing:** a turbolib target in `apps/fuzz`. A hostile library must
  yield an error, never a panic and never an output that `verify` rejects.
- **Known differences to document:** a folded function's PDB module is
  the synthesized member rather than the original compiland. Tools that
  map functions back to compilands may care: Live++, Edit and Continue,
  per-module coverage. `analyze` reports how many functions move.

## Phases

### Phase 0: measurements and linker spikes

- **0.1 `turbolib analyze`**, first version: it reads libraries and
  reports duplicated bytes by category, with no writing at all. Run it on
  the corpus here and, by the user, on libraries at work. This is the
  go/no-go, and it orders the later phases.
- **0.2 Spikes**, each a small library produced by a throwaway writer and
  linked with both `link.exe` and `lld-link`:
  - A synthesized precompiled-types object (`.debug$P`) with no compiler
    behind it. Does each linker accept it? Is the PDB equivalent? Does
    either linker require the PCH object to be linked before its users?
    What does `/DEBUG:GHASH` do with it?
  - The weak-external `NOLIBRARY` technique for `--pull=preserve`.
  - A COMDAT moved to another member, with its `.pdata`, `.xdata`, and
    `.debug$S`.
  - The archive size limit. Members are addressed by 32-bit offsets in
    both linker members, so we believe a COFF library can't exceed 4 GiB.
    Confirm it, along with what `lib.exe` and the linkers do near it.
  - `.chks64` and `.retplne`: who reads them. (`.retplne`: link.exe's
    `/guard:retpoline`; see 2.3.)
  - ASan and SanCov objects: which of their sections are per object.
- **Done when:** the analyzer's numbers are in, and each spike has a
  yes or no answer. Done except incremental linking's use of
  `.chks64`, and sanitizer objects.

### Phase 1: writers, round trips, and `lib.exe` without folding

- **1.1** The COFF object model and writer, including big-obj. Done:
  `turbonop-coff` (`crates/formats/coff`).
- **1.2** The Microsoft archive writer, and import-library writing from
  `/DEF`. Done: the writer, short import objects, the `.def` readers
  (`lib.exe`'s and LLVM's), the descriptor objects, whole import
  libraries and export files from `/DEF` (see 1.2's results). Every
  library tried writes back byte for byte: all 582 of the corpus (570 by
  `lib.exe`, 12 import libraries by `link.exe /Brepro`), 490 rebuilt from
  its objects by `lib /Brepro` and `llvm-lib`, and 1,851 of the Windows
  SDK's and MSVC's (with their own conventions: NUL padding, dates
  `4294967295`, holes in the member list). The index computed from the
  members is the library's for all 1,072 of the corpus and its rebuilds,
  and for 1,748 of the SDK's and MSVC's. Of the other 103, made by
  Microsoft's own tools: 43 index `__auximpcopy_` symbols, 33 are merged
  import libraries that keep every `__NULL_IMPORT_DESCRIPTOR`, and in 22
  (ARM64 CRT and SDK libraries) the EC symbols differ and in 5 (UCRT) the
  member a name goes to, not investigated.
- **1.3** CodeView: every record kind written, the exhaustive index-field
  visitor, and the C13 subsections (symbols, lines, inlinee lines, file
  checksums, string table, frame data, cross-scope imports and exports).
- **1.4** `turbolib` with `--fold=none`: the full `lib.exe` command line.
- **Done when:** every corpus object round-trips byte-identically, and
  with `--fold=none`, builds of the corpus link identically.

### Phase 2: identical folding (COFF)

- **2.1** Fold units, identity hashes, and environment keys.
- **2.2** Synthesized members: member-set grouping, anchors, splitting.
- **2.3** Side tables.
- **2.4** Debug information for synthesized members: types merged per
  member (no precompiled types yet), checksums, the string table, and
  compiland records.
- **2.5** `--pull=preserve` and `relaxed`, and `--locals=keep`.
- **2.6** `turbolib verify`, and the link-equivalence harness.
- **Done when:** the corpus links equivalently with both linkers in every
  configuration above, and the test programs' suites pass.

### Phase 3: shared types

- **3.1** Structural type hashing.
- **3.2** The shared set and the precompiled-types member.
- **3.3** Rewriting `.debug$T` and remapping `.debug$S`.
- **3.4** Absorbing `/Yu` members.
- **3.5** `--precomp=clustered`. Each member names one `LF_PRECOMP`, so
  each is in exactly one group. A record shared across groups is copied
  into each group's precompiled-types member, and a record only one
  member uses stays in that member. More groups mean more copies in the
  library, but fewer unused types merged by each link. Still to decide:
  - How to form the groups: by the types members share, by directory, or
    by which members links load together.
  - How to choose *n*.
  - How to score a split. The library's size is easy to compute. The cost
    to a typical link depends on which members links load, which turbolib
    can't see. The analyzer can measure the size side on the corpus
    before any writer exists.
  - Whether a member that shares few types should stay out of every group
    and keep its own `.debug$T`.
- **Done when:** PDBs are equivalent, and the analyzer's projected sizes
  match what links actually produce.

### Phase 4: less conservative folding

- **4.1** `--fold=select-any` and `--pick`.
- **4.2** `--locals=copy` and `merge`.
- **4.3** `--report odr`.

### Phase 5: speed

- Profile on the largest libraries in the corpus with Stackrabbit, and
  remove every idle core outside I/O waits. Also compare link times
  before and after compaction, which is the second benefit to prove.

### Phase 6: ELF, with split DWARF

- **COMDAT groups**, in place of COFF's COMDAT sections. Inline functions
  have `STB_WEAK` binding, and the archive index still lists them (GNU ld,
  gold, lld, and mold load members for them).
- **Debug information is per CU, not per function.** Moving a function
  means building a CU in the synthesized member. That means copying:
  - the subprogram's DIE subtree and the types it uses;
  - a line-table sequence;
  - range lists and location lists;
  - `.debug_addr` entries.

  In the original member, the orphaned DIE's addresses are tombstoned, as
  lld does for discarded sections.
- **Split DWARF.** The compiler writes the `.dwo` beside the `.o`; the
  archiver and the linker never touch it, and `dwp` packs the `.dwo` files
  later. turbolib's archive step therefore takes the `.dwo` files as inputs
  too, paired with their objects by `dwo_id`.
  - The original objects' `.dwo` files need no rewriting: a function's
    addresses go through `.debug_addr` in the `.o`, so tombstoning there
    is enough.
  - Synthesized members either get `.dwo` files of their own (the build
    rule must declare them as outputs) or carry their DWARF unsplit in the
    `.o`, which a link may mix freely.
  - Unsplit is simpler for Buck2 integration, so it is the first choice.
- **Per-object properties:**
  - `.note.GNU-stack`: a synthesized member without it makes the stack
    executable;
  - `.note.gnu.property`: CET is the AND of all inputs, so missing it
    turns IBT and shadow stacks off for the whole program;
  - `.ARM.attributes`, RISC-V attributes, and MIPS ABI flags;
  - `.init_array` entries inside COMDAT groups.
- **`--pull=preserve`** skips the units that are at risk, since ELF has
  nothing like `NOLIBRARY`.
- **GNU and BSD archive writers**, `/SYM64/`, thin archives, and `ar`'s
  command line.

### Phase 7: Mach-O

- **No COMDATs.** There are weak definitions within sections that
  `.subsections_via_symbols` divides into atoms. Folding cuts atoms out of
  `__text` and rewrites relocations, including paired
  `SUBTRACTOR`/`UNSIGNED` relocations, plus `__compact_unwind` and the
  `__eh_frame` FDEs.
- **DWARF is never linked.** The executable's debug map (N_OSO) names
  `library.a(member.o)`, and dsymutil reads the DWARF from there later.
  A moved function's DWARF must be in the synthesized member, and the
  member's modification time must be what the debug map records.
- **Per-object load commands:** `LC_BUILD_VERSION` and `LC_LINKER_OPTION`.
- **The tool:** `libtool -static`'s command line, and the BSD archive
  writer.

## Build systems

- **Buck2.** Its prelude's C++ rules build a static archive for a
  `cxx_library` by default, with the toolchain's archiver (`lib.exe` or
  `llvm-lib` on Windows). We believe a toolchain can instead link
  libraries as lists of objects (a `supports_archives`-style setting in
  the prelude's linker info) and can choose thin archives. Check this in
  the toolchain at work: look for the archiver and these settings in the
  `cxx_toolchain` definition. Libraries big enough to near the format's
  size limit mean archives are in use somewhere.
- **Integration.** Point the toolchain's archiver at `turbolib`. Its output
  is deterministic and its outputs are the same files `lib.exe`'s are,
  unless synthesized `.dwo` files are chosen (phase 6). Smaller libraries
  also mean less traffic to and from the remote cache.
- **CMake, MSBuild:** set `CMAKE_AR`, or `LibToolExe` and `LibToolPath`.

## Risks

- **The linkers' unwritten rules.** Everything here relies on behavior
  that `link.exe` documents incompletely: synthesized PCH objects,
  `NOLIBRARY` weak externals, big-obj synthesized members, and unknown
  per-object sections. Phase 0 proves each one before anything relies on
  it, and both linkers are checked at every phase.
- **Exhaustive CodeView remapping.** A missed index field corrupts debug
  information silently. Mitigations: a visitor generated from one table of
  record layouts, PDB comparisons on a large corpus, and fuzzing.
- **The cost of precompiled types in small links.** It is measured and
  reported, and `clustered` bounds it. If it can't be bounded well enough,
  `--precomp` stays opt-in.
- **Unrecognized inputs.** Anything turbolib doesn't recognize passes
  through unfolded, with a report. The cost is a smaller saving, never a
  wrong link.
- **The size limit is the format's.** turbolib can bring libraries back
  under the 4 GiB limit, but it can't write a library above it that
  `link.exe` accepts (phase 0 confirmed this). It could write a `/SYM64/`
  archive past 4 GiB for lld-link alone, as `llvm-lib` does, as an
  option.
