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

## Status (2026-11-26)

| Phase | State |
|---|---|
| 0 Measurements and linker spikes | Mostly done (see [Phase 0 results](#phase-0-results)): `turbolib analyze`, both corpora's numbers, and the PCH, weak-external, size-limit, and `.chks64` spikes. Not yet: incremental linking and `.chks64`, sanitizers. |
| 1 Writers, round trips, and `lib.exe` without folding | In progress (see [Phase 1 results](#phase-1-results)). Done: 1.1, `turbonop-coff`, which every corpus object round-trips through byte-identically; 1.2, the archive writer, import objects, and `.def` files, which every library tried writes back through byte for byte; 1.3, CodeView's fields, C13 subsections, and renumbering, clean on both corpora; 1.4, the `lib` and `llvm-lib` personalities, which remake all 245 libraries of each corpus build as the tool did. 1.2's `/DEF` too: import libraries and export files as `lib.exe` and `llvm-lib` write them. Not yet: thin libraries. |
| 2 Identical folding (COFF) | In progress (see [Phase 2 results](#phase-2-results)). Done: 2.1 and 2.2, the engine (`turbonop_fold::rewrite`) in the `lib` personality (`/TURBOLIB:fold=identical`), the side tables (2.3), and `--pull=preserve` and `relaxed` with `--locals=keep` (2.5). 2.4, folded units' debug information (see [2.4](#debug-information-24)). Done too: the link-equivalence checks (2.6: `turbolib verify`, `compare-images`). |
| 3 Shared types: a synthesized precompiled-types object | In progress (see [Phase 3 results](#phase-3-results)): 3.1 to 3.4 done, as `--precomp=library` (opt-in; `none` stays the default until it is proven in more links); 3.5 done, as `--precomp=clustered[=<n>]` (opt-in; see [Phase 3.5 results](#phase-35-results-clustered)). |
| 4 Less conservative folding | Done (see [Phase 4 results](#phase-4-results)): `--fold=select-any` with `--pick`, `--locals=copy` and `merge`, `--report odr`, and `verify`'s modes for them; opt-in (the defaults are unchanged). On LLVM they save little more than `identical` (0.15% of `build-z7` under `preserve`). |
| 5 Speed | In progress (see [Phase 5](#phase-5-speed)): on the 20 largest libraries, turbolib takes 0.50× `lib.exe`'s time without folding, about 1.0× with it, and 1.06× with folding and shared types (0.92× on `build-z7-nopch`). Memory (2026-11-27): folding clangSema.lib peaks at 1.69 GiB of commit, from 2.39; 8 libraries at once share the cores (slots). Round 2 (2026-11-28): every mode reads its objects one at a time; with shared types clangSema.lib peaks at 1.33 GiB, from 1.97 (836 MiB on one thread, from 1,581). |
| 6 ELF, with split DWARF | In progress (see [Phase 6 results](#phase-6-results)). Done: 6.1, the GNU and BSD archive writers (every archive of the gcc corpus written back byte for byte) and the `ar`, `llvm-ar`, and `ranlib` personalities (60 of 60 command lines as GNU `ar` and `llvm-ar` behave; all 265 libraries of the gcc build remade by its own commands, in 0.27× GNU `ar`'s time); 6.2, `turbonop-elf-object`, which every corpus object round-trips through byte-identically; 6.3, `--fold=identical` for ELF (`llvm-nm` and `llvm-readobj` link to the same members, symbols, and outputs with GNU ld, gold, lld, and mold); 6.4, ELF `verify` and `compare-images`; 6.5, folded functions' DWARF in synthesized members (DWARF 4 and 5, split DWARF 5 described unsplit): all 265 libraries of the gcc build folded (1.0% smaller, every folded function with DWARF) and verified; in the relinked programs every function and table equivalent and every subprogram described, 7 to 22 per program by another unit's copy; gdb sessions in folded functions the same; GNU's split DWARF 4 too; initializer entries in groups kept from folding; the original members loaded in the original order by all four linkers; the clang corpus (relib byte for byte, folded 2.4% smaller, verified, relinked; see 6.6); every differing description proven another unit's copy; folding the 20 largest libraries in 0.34× GNU `ar`'s time, 1.88× a Release `llvm-ar`'s (6.8, from 3.46×; noisy measurements); `compare-images` reading the clang corpus's last differences; CREL objects read, written, and folded (6.7). Not yet: `select-any` and `--locals` for ELF. |
| 7 Mach-O | In progress (see [Phase 7 results](#phase-7-results)). Done: 7.1, Darwin archives (all 160 archives of the Mach-O corpus written back byte for byte and remade by their own commands; 41 of 41 command lines as `llvm-ar`, `llvm-ranlib`, and `llvm-libtool-darwin` behave) and the `libtool -static` personality (`llvm-libtool-darwin`'s; 0.70× the real tools' time); 7.2, `turbonop-macho-object` (all 5,732 corpus objects round-trip byte for byte, and laid out afresh reproduce LLVM's bytes); 7.3, a first `--fold=identical` for Mach-O, with `verify`; 7.4, x86-64's FDEs and LSDAs move with their units (x86-64 now folds as arm64 does: 36,348 units, 169,617 copies), givers' DWARF moves with their code, synthesized members carry their units' DWARF (every folded unit's), and `compare-images` for Mach-O (dSYMs' DWARF too); 7.5, givers' DWARF without the moved copies' DIEs (the folded links' dSYMs equivalent to 7.4's). Not yet: Apple's `libtool`/`ranlib`, linkers, and dsymutil (not available here); the size the DWARF costs (the folded archives are 5.9% larger; 7.4: 9.1%); speed (3.55× the real tools' time folding in 7.4, and pruning adds about a quarter more CPU). |

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
- **Initializer entries.** A unit holding a `.CRT$X*` section (an inline
  variable's or a template static member's dynamic initializer, which cl
  writes as a COMDAT `.CRT$XCU` associative to the variable; terminators;
  TLS callbacks) doesn't fold: in a synthesized member it would run at
  that member's place in the order, and inline variables' initialization
  is ordered within each translation unit. Measured cost: none on either
  corpus (`build-z7` 12,565,125,290 bytes folded with and without the
  rule, `build-z7-nopch` 21,089,233,154), since LLVM has no such units
  that folded; code with many inline variables would pay more. cl 14.52
  with `/std:c++17`: both kinds are caught (a test library of two objects:
  4 copies held back).

**On the corpus** (`build-z7`, with the build's own archiver commands,
`relib.py` and `TURBOLIB_OPTIONS=--fold=identical`), 188 of the 245
libraries change (the rest have nothing to fold), from 13.21 GiB to 11.38
GiB (13.9%), in 59 s. The analyzer projects 2.7 GiB for
`identical, pull=preserve`; the difference is still to account for.
(Here folded units' `.debug$S` was dropped, which counted as saved; see 2.4 for the sizes with it.)

`llvm-nm` and `llvm-readobj` link against the folded libraries
(`relink.py`), and on 27 inputs (`LLVMDemangle`'s objects and library,
`LLVMBinaryFormat`, `testdata/coff`'s objects and libraries, and
`llvm-nm.exe`) print what the originals print, with `nm` and with
`readobj --all`, in all six configurations (link.exe and lld-link,
`/OPT:REF` and `NOREF`, and lld-link's `/DEBUG:GHASH`): 324 runs, none
different.

**Link equivalence** (2.6's `compare-images`, with [the alias
technique](#the-alias-technique)): in every configuration, both programs'
`.text` and `.pdata` are the originals' to the byte in size, and `.rdata`
within 0.003%; both linkers load the same members in the same order
(with the anchors first: `/VERBOSE` on `llvm-nm`, link.exe's 637 and
lld-link's 631; with the anchors last, link.exe's order differed and two
C++ initializers moved). What still differs:
which copy of identical code a link keeps (so names, line blocks, and
procedures' symbols of a few dozen implicit functions: each translation
unit's copy has its own lines), and in link.exe's PDBs about 0.3% fewer
type and id records (lld-link's keep them all; not explained yet).

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

### Debug information (2.4)

Synthesized members carry their units' CodeView
(`turbonop_fold::rewrite::debug`). Measured on the corpus (MSVC,
`crates/link/fold/examples/debug_layout.rs`) and the fixtures
(`tools/turbolib/debug`, MSVC and clang-cl): each COMDAT's `.debug$S` is
associative to it, with its symbols (procedures with their locals and
inline sites, `S_GDATA32` of COMDAT data) and lines; the per-object
`.debug$S` has `S_OBJNAME`, `S_COMPILE3`, `S_BUILDINFO`, the global
`S_UDT`s and `S_CONSTANT`s, non-COMDAT functions, the file checksums, the
string table, and, with inlining, the inlinee lines of every function
inlined in the object. No frame data or cross-scope imports and exports
occur on x64. On the corpus, no per-object `.debug$S` relocates to a
COMDAT's external symbol; 27,246 relocations reach COMDATs' local
symbols, and those units already don't fold.

A synthesized member gets its units' `.debug$S`, renumbered in place
into its own `.debug$T`, checksums, and string table (a section whose
inline sites' file changes take more or fewer bytes is rebuilt, its
relocations moved); a `.debug$T` of the records its units reach, merged
by their renumbered bytes, each record after those it names; the inlinee
lines its inline sites need; and `S_OBJNAME` (`turbolib:<member>`) and the
units' `S_COMPILE3`, without `S_BUILDINFO`. Units of `/Yu` objects go to
members that name the same `LF_PRECOMP` and refer to the PCH object's
symbol: LLVM's libraries reuse each other's PCHs, and copying the PCH's
records from the library instead left 1.75 million copies on `build-z7`
unfolded. `/Zi` members keep their indices and name their type server.

**On the corpus**, with debug information kept, `build-z7`'s 184 changed
libraries go from 13.21 to 11.63 GiB (1.58 GiB saved, 11.6% of 13.68), in
35 s, and `build-z7-nopch`'s 163 from 21.13 to 19.29 GiB (1.84 GiB, 8.5% of
21.77). The 45,120 and 53,094 synthesized members hold 152 and 165 MiB of
`.debug$S` and 59 and 124 MiB of `.debug$T`. `llvm-nm` and `llvm-readobj`,
linked with link.exe against each build's folded libraries, print what
the originals print on 25 inputs, and their PDBs
(`tools/turbolib/corpus/pdbcompare.py`) hold the same procedures (144,587
to 152,520; 17,106 to 23,080 of them now in synthesized members), with
the same signatures, locals, inline sites, and lines, except 21 to 50 per
program whose lines name another file and line: the compiler gives an
implicit member function the line where each translation unit needs it,
and turbolib keeps the first member's copy, link.exe the first loaded.
Records outside procedures (`S_UDT`, `S_CONSTANT`) differ in which copies
link.exe keeps, as they do between two links of the same inputs. With
lld-link, the images grow (2.8 MiB for `llvm-nm`); on `build-z7-nopch`
by exactly as much with the phase 2.2 engine, which drops debug
information, so not from 2.4.

## Phase 3 results

`--precomp=library` (`/TURBOLIB:precomp=library`, or in
`TURBOLIB_OPTIONS`) writes one precompiled-types member for each machine
whose members share records (`turbonop_fold::rewrite::precomp`), with or
without `--fold`. As [Shared types](#shared-types-phase-3) says, with
these choices and findings:

- **The shared set.** A record is shared when at least 2 members hold it
  (`Precomp::Library { min }`; the command line has no option for `min`
  yet). Never shared: `LF_FUNC_ID`, `LF_MFUNC_ID`, `LF_BUILDINFO`, and in
  each member the records only its `LF_BUILDINFO` reaches (its command
  line, directory, and source: that member keeps its own copy, even of a
  record others share). The set is closed under references: a record
  naming one that can't be shared isn't (none is known to; a test makes
  one). Records are hashed structurally with the member's machine, so a
  library of two machines gets two members.
- **Order and names.** The records are ordered by depth (each names only
  earlier ones), then by hash: the same for any thread count. The
  signature is the low 32 bits of a hash of the records (never 0); the
  member is `turbolib-types-<64-bit hash>.obj`, its symbol
  `__@@_PchSym_turbolib_<128-bit hash>`, and its `S_OBJNAME`
  `turbolib:<member name>` with the signature (real `/Yc` objects have 0
  there; lld-link finds a PCH by its `S_OBJNAME` signature or else by the
  file name in `LF_PRECOMP`, which is the member's name, so both lead to
  it). The same records always give the same names, so two libraries that
  share the same records name one PCH, and either copy serves.
- **The symbol is absolute**, not a 4-byte `.bss` as `/Yc` objects and
  phase 0's spike had: both linkers accept it (the fixtures' links, and
  the corpus's below), and the image gets no data from it.
- **A member is rewritten only if that makes it smaller**: one sharing a
  few small records would grow by its `LF_PRECOMP` and the symbol (2 of
  the fixtures' 6 /O2 objects; 86 of 2,536 candidates on `build-z7`, 83
  of 3,632 on `build-z7-nopch`). Members whose `.debug$S` can't be read
  through (a kind with no known layout, an index out of range) keep their
  types, and are reported (none on the corpora).
- **PCH users (3.4).** A user whose PCH object is in the library is
  absorbed, named or not: its indices into the PCH hash as the PCH
  object's records, and the records of the PCH it reaches (from its own
  records and its `.debug$S`, transitively) count as its own, kept or
  shared. **The PCH object itself stays as it is**, a deviation from the
  plan's step 5: LLVM's libraries reuse each other's PCH objects, so a
  user in another library may name it, and it can't name a PCH itself.
  Its users keep their `/INCLUDE` of its symbol, so it is still loaded as
  before. A user whose PCH object isn't in the library keeps its types:
  it can name only one `LF_PRECOMP`. (A PCH that names another PCH
  would let those users share too; lld-link reads a `.debug$P` as plain
  records, judging by its source (`InputFiles.cpp`), so it would likely
  not work there. Not tried.)
- **What else changes in a member**: its `.debug$H` (lld's precomputed
  type hashes, stale now) goes; its `.chks64` keeps the checksums of the
  sections that stay as they were, with 0 (as cl gives `.bss`) for the
  others. (Before, `.chks64` went from every changed member: link.exe
  takes its PDB's contribution CRCs from it, and without it
  `compare-images` labeled every contribution of a changed member
  differently, 28,064 failures for `llvm-nm` under `/OPT:REF`.)
- **Synthesized members (2.4)** name the same PCH, and their records that
  are shared are the PCH's; absorbed users' units go to the same
  synthesized members as everyone's (their key is the library's PCH now).
- **Compacting again** changes nothing: a precompiled-types member of
  turbolib's in the input counts every record for each of its users, so
  the same set is made, the same bytes, and it is kept rather than
  written twice.
- `turbolib verify` takes the PCH symbol as an anchor (defined by an added
  member, referred to by members), and an original member's `/INCLUDE`
  as a name it refers to (a synthesized member holding a PCH user's units
  refers to its PCH object's symbol). `compare-images` expects the PCH
  symbols' publics, and types and ids only in the second PDB when it has
  turbolib's PCH modules.

**On the corpus** (`relib.py`, the build's own archiver commands, all 250
libraries; sizes in GiB):

| | `build-z7` | `build-z7-nopch` |
|---|---|---|
| The build's libraries | 13.68 | 21.77 |
| `--fold=identical` | 12.17 (1.51 saved, 11.0%) | 20.28 (1.49, 6.8%) |
| `--precomp=library` | 12.61 (1.07, 7.8%) | 11.57 (10.20, 46.8%) |
| both | 11.08 (2.61, 19.0%) | 9.98 (11.79, 54.1%) |
| The analyzer's projection, types | 3.1 (22.7%) | 10.2 (47.1%) |
| The analyzer's projection, the conservative default | 5.8 (42.5%) | 13.0 (59.7%) |
| Precompiled-types members | 96, 200 MiB | 169, 815 MiB (projected: 816) |
| Members sharing, of those that could | 2,450 of 2,536 (2,490 absorbed PCH users) | 3,549 of 3,632 |
| Their `.debug$T` | 1,701 MiB to 404 MiB | 12,074 MiB to 814 MiB |

Kept `.chks64` sections take 0.07 to 0.14 GiB of that (without them,
shared types alone saved 1.16 and 10.35 GiB). `build-z7-nopch` saves
about what the analyzer projected. `build-z7` saves 1.07 GiB of the
projected 3.1: the analyzer counts every PCH user's own
records as shareable, but about 1,100 of its 3,604 users name a PCH
object in another library, and keep their types (see above).

**Links** (`relink.py`'s matrix for `llvm-nm` and `llvm-readobj`: link.exe
and lld-link, `/OPT:REF` and `NOREF`, `/DEBUG:FULL` and lld's `GHASH`;
12 links each):

- `--precomp=library` alone: `compare-images` finds all 8 lld-link links
  equivalent on each corpus. lld-link's PDBs hold every record of the
  PCHs it loads, so they have more: on `build-z7-nopch`, for `llvm-nm`,
  14,064 types and 699 ids (`LF_UDT_SRC_LINE`), for `llvm-readobj`, 4,901
  and 263, and none is missing (expected; this is the trade-off the plan
  measured). link.exe's PDBs have no more records (measured; phase 0's
  spike had all of a `/Yc` object's records arrive). The 4 link.exe links
  fail on 4 to 49 records, all picks among same-named records: an `S_UDT`
  or `S_CONSTANT` naming the other of two definitions of one name (each
  translation unit's `std::vector` instantiation, for one; both in both
  PDBs), or another `LF_UDT_SRC_LINE` (`cl::opt<std::string>` at
  `CommandLine.cpp(79)` or at `CommandLine.h(1573)`). Two link.exe links
  of the build's own libraries differ in the same categories (4 to 49
  records: a control run of `relink.py`).
- `pdbcompare.py`: the same procedures (147,113 in `llvm-nm`, 144,608 in
  `llvm-readobj`), with the same signatures, locals, inline sites, and
  lines, in every configuration checked; on `build-z7`, link.exe's records
  outside procedures differ in the same picks (3 and 9, 38 and 21).
- With folding too: `pdbcompare.py` finds exactly the procedure
  differences folding alone has (4,577, 2,092, and 57 on
  `build-z7-nopch`; 3,294, 2,015, and 35 on `build-z7`: which copy's lines
  a link keeps), and `compare-images` the categories folding alone has,
  with slightly fewer failures in each of the 24 links (for `llvm-nm` on
  `build-z7-nopch`: 15,577 against 16,057 with link.exe `/OPT:REF`,
  44,438 against 44,922 with lld-link `/OPT:REF`), and without folding's
  own types only in the second PDB (425 for `llvm-nm`, lld-link).
- Keeping `.chks64` helps folding alone too: link.exe `/OPT:REF` links of
  `llvm-nm` against folded libraries had 40,623 failures (`build-z7-nopch`)
  and 34,477 (`build-z7`) with it dropped, and 16,057 and 9,065 with it
  kept; the rest are folding's known differences (see 2.6).
- The programs print what the originals print: `llvm-nm` and
  `llvm-readobj --all` on 16 inputs (`testdata/coff`'s objects and
  libraries, `LLVMDemangle`'s objects, `LLVMDemangle.lib`,
  `LLVMBinaryFormat.lib`), 192 runs for each of shared types alone and
  with folding, on each corpus: none different.
- `turbolib verify` passes all 169 and 190 libraries folded with shared
  types (`build-z7-nopch`, `build-z7`).
- The fixtures (`tools/turbolib/debug/check.py --options=...`): with
  shared types, and with folding too, both programs print the same and
  their PDBs have the same procedures and other records, with both
  linkers.

**Determinism**: `clangCodeGen.lib` (`build-z7-nopch`, `/Brepro`) is the
same bytes on 8 threads and on 1, and from two builds of the engine.

**Speed** (`timelib.py`, the 20 largest libraries, best of 2, warm cache,
8 threads):

| | `build-z7` (7.26 GiB) | `build-z7-nopch` (13.17 GiB) |
|---|---|---|
| `lib.exe` 14.52 | 11.02 s, 1.00× | 17.59 s, 1.00× |
| `--fold=identical` | 11.03 s, 1.00× | 14.34 s, 0.82× |
| `--fold=identical --precomp=library` | 11.73 s, 1.06× | 16.18 s, 0.92× |
| `--precomp=library` | 10.71 s, 0.97× | 15.61 s, 0.89× |

Finding the shared types (hashing every record, counting, ordering, and
writing the PCH) takes about 0.35 s of `clangCodeGen.lib`'s 1.6 s
(`build-z7-nopch`, 1.5 GiB, 1 GiB of it `.debug$T`), and runs while the
definers are found; writing the output takes less, as it is smaller.

**After merging with phase 2's synthesized-members-last order** (`build-z7`,
`--fold=identical --precomp=library`, 190 libraries changed): in all six
configurations, `llvm-nm`'s and `llvm-readobj`'s `.text` and `.pdata` are
the originals' to the byte in size; the programs print what the originals
print in 324 of 324 runs; link.exe under `/OPT:REF` has 8,950 and 8,315
failures (against 34,490 and 30,484 with folding alone before `.chks64`
was kept), and the C++ initializers' order still differs with link.exe
(the open item in TODO.md).

### Phase 3.5 results: `clustered`

**The cost first** (`tools/turbolib/corpus/linkcost.py`: each program
linked as its build statement links it, `/DEBUG:FULL /OPT:REF`, best of
3, against the build's libraries, `--precomp=library`'s, and
`clustered`'s). The plan's trade-off holds for lld-link only: its PDBs
hold every record of each PCH a link loads. link.exe's PDBs have exactly
the TPI records they have without shared types (in all 20 link.exe links
measured), so it merges only what is used. Shared types make both
linkers faster and smaller even so: for `llvm-nm` on `build-z7-nopch`,
link.exe takes 3.87 s and 2,392 MiB at its peak without them, 3.63 s and
1,812 MiB with `library`; lld-link 3.65 s and 1,497 MiB, then 2.80 s and
1,242 MiB. The unused records lld-link merges (TPI records beyond
`none`'s, `build-z7-nopch` / `build-z7`):

| Program (lld-link) | Members loaded | `library` | `clustered` |
|---|---|---|---|
| `llvm-nm` | 645, of 47 libraries | +14,064 / +14,022 | +7,453 / +3,906 |
| `llvm-readobj` | 496, of 30 | +4,901 / +4,892 | +2,484 / +2,345 |
| `mlir-src-sharder` (58 of `LLVMSupport`'s 184, 1 of `MLIRSupport`'s 8) | 147, of 14 | +11,480 / +8,781 | +3,561 / +1,575 |
| `llvm-min-tblgen` | 162, of 14 | +9,508 / +8,294 | +3,324 / +1,451 |
| `llvm-tblgen` | 168, of 15 | +8,425 / +7,216 | +2,503 / +1,921 |

`library` gives 2 to 17% more TPI records (PDBs 0.4 to 4.7% larger);
`clustered` removes 47 to 83% of them. Link times and peak memory stay
`library`'s within the noise (lld-link's peak 0 to 4% lower, link.exe's
up to 2% higher; a link's time varies by up to 0.3 s between rounds).

**How it works** (`turbonop_fold::rewrite::precomp::cluster`):

- **What links load**, the only signal turbolib has: a member's
  *closure* is the members that define (as the archive index's first
  definer) the names it refers to and doesn't define, transitively.
  Closures vary widely: in `clangSema`, `clangCodeGen`, and
  `LLVMCodeGen` most members reach nearly all (median 85 of 88, 103 of
  105, 209 of 236); in `clangStaticAnalyzerCheckers` almost none (median
  1 of 140); `LLVMSupport` has a core (median 54 of 184; nothing refers
  to 85 of its members). Each sharing member is taken as one link that
  loads its closure (a *seed*).
- **Classes**: records held by the same members behave alike under any
  grouping, so they are counted once per set of holders (4,041 classes
  for `clangSema`'s 292,678 shared records). Under a grouping, a class is
  in a group's PCH when at least `min` (2) of its holders are in the
  group, and otherwise in each of them.
- **The score**: `seeds × S + w × C`. `S` is the library's bytes of those
  records; `C` is what the seeds' links read of them (each group's PCH
  once for each seed whose closure touches the group, each kept copy once
  for each seed whose closure holds its member). So: the library's bytes
  plus `w` times what an average one-member link reads. `w` is 16. On
  the 27 libraries those five programs use (`build-z7-nopch`), `w` = 1
  removed 0 to 4% of lld-link's extra records, 4 removed 21 to 40%, 16
  removed 64 to 79%, 64 removed 65 to 85%, and 256 removed 76 to 92%.
  The libraries grow with `w`: 0.2%, 0.9%, 2.3%, 3.1%, and 3.8% over
  `library`'s.
- **The search**, per machine: it tries each number of groups
  (`clustered`: 1, 2, 4, 8, 16; `clustered=n`: the powers of two below
  *n*, and *n*). Heads are chosen by their loaders (first the most loaded
  member, then each time the member least like any head), and each
  member joins the head most like it. Then each member in turn moves to
  the group, or out of all of them, that lowers the score most (the change
  computed exactly and incrementally; a test checks it against
  recomputing), until no move does; then whole groups merge while that
  lowers the score. The lowest score wins. A member out of every group
  keeps its types (as a group of one would). Everything is integers in
  member order: the same bytes on 1 and 8 threads and from two builds
  (`LLVMSupport` and `clangCodeGen` of `build-z7`, `LLVMObject` and
  `clangBasic` of `build-z7-nopch`, with `--fold=identical`).
- **The members**: one PCH member for each group with shared records,
  named by its records' hash, so each has its own signature, name,
  `S_OBJNAME`, and symbol (two groups with the same records share one
  member). Each member names its group's. A synthesized member names the
  PCH that most of its units' members name (the first such group), and
  holds its other records itself. `library`'s output is unchanged, byte
  for byte.

**On the corpus** (all 250 libraries, the build's own commands):

| | `build-z7` | `build-z7-nopch` |
|---|---|---|
| `--precomp=library` | 12.61 GiB, 96 PCH members | 11.57 GiB, 169 |
| `--precomp=clustered` | 12.70 GiB (+0.7%), 271 PCH members in 96 libraries, 53 with more than one | 11.88 GiB (+2.6%), 331 in 169, 75 with more than one |
| `--fold=identical --precomp=clustered` | 11.17 GiB | 10.29 GiB |

The libraries whose members reach nearly all the others keep one group
(`clangSema`, `LLVMCodeGen`), as does `clangStaticAnalyzerCheckers`,
whose checkers share mostly the same headers. `LLVMSupport` gets 5 groups
on `build-z7`.

**Verification:**

- `turbolib verify`, with the PCH pairing check (each PCH member's
  `S_OBJNAME` is the name its users give), passes all 96 and 169
  clustered libraries, and all 190 and 169 with `--fold=identical` too.
- `relink.py`'s matrix for `llvm-nm` and `llvm-readobj`: all 16 lld-link
  links are equivalent (`compare-images`) on both corpora. The 8 link.exe
  links fail on picks among same-named `S_UDT` and `S_CONSTANT` records
  and on modules' own records, phase 3's categories and those of two
  links of the same inputs: 150, 66, 9, and 1 failures on
  `build-z7-nopch`, and 46, 20, 21, and 28 on `build-z7` (with `library`,
  measured the same day: 4, 4, 11, 9 and 75, 17, 40, 45).
  `pdbcompare.py` finds the same procedures in every link checked
  (147,113 for `llvm-nm`); the records outside procedures differ for
  link.exe by 1 to 66 (the picks), and are the same for lld-link.
- With `--fold=identical` too, on `build-z7-nopch`: lld-link's failures
  are exactly those of `--fold=identical --precomp=library` in all 8
  links (folding's own, 35,601 to 141,713), and link.exe's are within 14
  of them (4,004 against 4,018 for `llvm-nm` under `/OPT:REF`).
- The programs print what the originals print: `llvm-nm` and
  `llvm-readobj --all` on 27 inputs (`testdata/coff`'s objects,
  libraries, and `model` objects; `LLVMDemangle`'s objects and library;
  `LLVMBinaryFormat.lib`; `llvm-nm.exe`), 324 runs for each of `clustered`
  and `--fold=identical --precomp=clustered` on each corpus: none
  different.
- Unit tests: the incremental score against recomputing (200 random
  cases); the model's figures for one group and for none; separate parts
  split and parts loaded together kept; and the fixtures with clustered
  types (every member reads the same through its group's PCH, the
  signatures are distinct, and a heavy weight splits them).

**Speed** (`timelib.py`, the 20 largest libraries, best of 2):
`clustered` takes 0.99× `lib.exe`'s time on both corpora, against
`library`'s 0.89×. The grouping costs about 0.1 to 0.25 s a library, most
of it finding the first definers and closures again, beside the
definers the engine already finds. With folding, the same day:
`--fold=identical` 2.47× (`build-z7`) and 1.69× (`build-z7-nopch`),
`--fold=identical --precomp=library` 2.60× and 1.95×, and
`--fold=identical --precomp=clustered` 2.77× and 2.10×. Folding alone is
slower than phase 5 measured, mostly in `clangSema` (10.0 s against
`lib.exe`'s 2.1 s); that isn't from shared types.

**Not done:** compacting a clustered library again keeps its PCH
members' records as each user's (as for `library`), but may group them
differently, so getting the same bytes again isn't promised. The weight
and the numbers tried have no option. The seeds are single members, not
real links: a link that loads many members of a library touches more
groups than the model's average.

## Phase 4 results

`--fold=select-any` with `--pick`, `--locals=copy` and `merge`, and
`--report odr` (`/TURBOLIB:fold=select-any`, `pick=largest`,
`locals=copy`, `report=odr`, or in `TURBOLIB_OPTIONS`), in
`turbonop_fold::rewrite` and `rewrite::locals`; what each folds, refuses,
and gives up is in [Conservativeness](#conservativeness). The defaults
are unchanged (`identical`, `locals=keep`; and the `lib` personality's
default is still `--fold=none`). Choices and deviations from the plan:

- **Copies of one name** under select-any are units with the same
  external names, selection, and environment key. Copies with internal
  linkage are never select-any's (their names mean nothing across
  objects); they fold only when identical.
- **`SAME_SIZE` and `EXACT_MATCH`** compare the COMDAT section's size (and
  checksum); when copies disagree, the identical ones among them still fold
  (rather than "both copies stay"), and the name and its copies are listed
  by `--stats`. Neither MSVC nor clang-cl was seen writing these
  selections (nor `LARGEST`); they are tested on objects made in the
  tests.
- **Under `--pull=preserve`**, select-any's extra conditions (every
  definition of the names a copy of the same kind; all copies referring to
  the same names) keep which members a link loads the same, as shown
  statically by `verify`, and in the links below (no module of an
  original member only in one of the two PDBs, in any of the 84 links):
  what changes is which copy's code a link gets.
- **`--locals=copy` copies, never shares.** A local COMDAT that code which
  stays reaches stays, and the synthesized member gets its own copy; one
  that only moved units reach leaves its member ("absorbed"). The plan's
  "folding a COMDAT whose own symbol is local is `--locals=copy`" is only
  this: such a COMDAT never folds by itself (giving it an external name
  would let members share it: TODO.md).
- **The ODR report** compares code as `--locals=copy` does (locals by
  content), so the `$SG` string each translation unit names doesn't make a
  finding; and it says when a finding's definitions only come from members
  of different environments.
- `turbolib verify` takes `--fold=select-any` and `--locals=copy|merge`,
  and, under `preserve` in every mode, checks what a synthesized member
  refers to (see [the tools](#the-tools-verify-and-compare-images-26)).
  That check found a gap older than phase 4: a synthesized member holding a
  PCH user's copy of a unit that non-users gave up too names the PCH
  object's symbol, so a link loading such a member alone now loads the PCH
  object (2 libraries of `build-z7`: LLVMAnalysis and clangCIR; none of
  `build-z7-nopch`). Reported as expected, and in TODO.md.

**The fixtures** (`tools/turbolib/loose`, built by its `build.cmd` into
`testdata/coff/loose`: cl `/Od /Gy`, `/Od`, `/O2 /Gy`, clang-cl `/Od /Gy`;
three objects whose inline functions reach `static inline` and
anonymous-namespace functions, a constant table, a writable counter, and
differ between objects in `variant()`, `selected`, and `refers()`):
`check.py` links `main.cpp` against lib.exe's library and the folded one
in six combinations of the modes, with link.exe and lld-link under
`/OPT:REF` and `NOREF`: 96 pairs of programs, all printing the same (with
select-any, each copy-choice value one some choice of copies gives), and
`verify` passes all 24 libraries in their modes. Without `/Gy`, and with
clang-cl, `static` functions are code outside COMDATs, so what calls them
stays (as measured: only `lookup()`, which reaches the table, moves).

**On the corpus** (`relib.py`, the build's own archiver commands; saved,
of all 250 libraries):

| Mode | `build-z7` (13.68 GiB) | `build-z7-nopch` (21.77 GiB) |
|---|---|---|
| `--fold=identical` | 1.511 GiB (11.0%) | 1.487 GiB (6.8%) |
| `--fold=select-any` | 1.527 GiB (11.2%) | 1.505 GiB (6.9%) |
| `--fold=identical --locals=copy` | 1.516 GiB (11.1%) | 1.491 GiB (6.8%) |
| `--fold=select-any --locals=merge` | 1.532 GiB (11.2%) | |
| the same, `--pull=relaxed` | 1.838 GiB (13.4%) | |
| the same, `--precomp=library` (preserve) | | 11.813 GiB (54.3%; phase 3's `identical` with types: 11.79) |

So phase 4's modes save little more on LLVM: 21 MiB of `build-z7` (0.15%)
under `preserve`, about what the analyzer projected (Phase 0: 20.7% for
`identical`, 20.9% for `select-any, locals=copy`, 0.2 points apart), and
0.33 GiB with `--pull=relaxed` as well (`relaxed` alone wasn't measured
here). Why so little: copies with other code are rare (on `build-z7`,
with `--locals=copy` too, 2,334 groups
folded whose copies differ, summed over libraries; 65 copies left for
referring to other names; no `SAME_SIZE` or `EXACT_MATCH`), and LLVM's
Debug build has no `/Gy`, so its `static` functions are code outside
COMDATs, which isn't copied: `--locals=copy` copied 3,252 pieces (string
literals, constant tables) and no local unit. `--report odr` lists 2,786
names over `build-z7`'s libraries (summed; 2,758 with two definitions in
one environment). Every folded library passes `turbolib verify` in its
mode: 1,605 libraries over the nine runs.

**Links** (`relink.py`'s matrix for `llvm-nm` and `llvm-readobj`: link.exe
and lld-link, `/OPT:REF` and `NOREF`, `/DEBUG:FULL` and lld's `GHASH`, 12
links each): the programs print what the originals print, with `nm` and
`readobj --all`, on 27 inputs (`LLVMDemangle`'s objects and library,
`LLVMBinaryFormat.lib`, `testdata/coff`'s objects and libraries, and
`llvm-nm.exe`): 324 runs for each of the seven libraries sets relinked
(the four modes on `build-z7`, three on `build-z7-nopch`), none
different. `compare-images` failures, with `identical` as the baseline
(the same binary; the categories are folding's known ones, see 2.6 and
phase 3):

| | link REF | link NOREF | lld REF | lld NOREF |
|---|---|---|---|---|
| `build-z7`, `llvm-nm`: `identical` | 9,155 | 10,886 | 44,150 | 140,853 |
| `select-any` | 9,522 | 11,235 | 44,437 | 141,392 |
| `identical, locals=copy` | 9,526 | 11,306 | 44,593 | 141,379 |
| `select-any, locals=merge` | 9,679 | 11,475 | 44,678 | 141,682 |
| `build-z7`, `llvm-readobj`: `identical` | 8,426 | 8,542 | 35,962 | 105,282 |
| `select-any, locals=merge` | 8,941 | 9,029 | 36,409 | 105,982 |
| `build-z7-nopch`, `llvm-nm`: `identical` | 16,057 | 29,374 | 44,922 | 141,964 |
| `select-any` | 16,506 | 29,927 | 45,292 | 142,552 |
| `select-any, locals=merge, precomp=library` | 16,249 | 29,710 | 45,008 | 142,393 |

(GHASH gives lld's FULL numbers in every run.) The extra failures are
what the modes change: functions whose bytes differ (another copy kept),
operands naming another copy, the copied locals as contributions of
synthesized members. `.text` and `.pdata` keep `identical`'s sizes but in
`llvm-nm` under `/OPT:REF` with `select-any, locals=merge`: 4,320 bytes
and two unwind entries less, the two `getSectionContentsAsArray<Elf_Sym>`
instantiations, which the folded program doesn't have and the original
does; likely ICF folding them now that they name one merged string copy
(not shown).

**Determinism**: `clangCodeGen.lib` (`build-z7-nopch`, `/Brepro`) is the
same bytes on 1 thread and on 8, and from two processes, with
`select-any, locals=merge, precomp=library`, `identical, locals=copy`, and
`select-any, pick=largest, pull=relaxed`.

**Speed** (`timelib.py`, the 20 largest libraries, best of 2, warm cache,
8 threads):

| | `build-z7` (7.26 GiB) | `build-z7-nopch` (13.17 GiB) |
|---|---|---|
| `lib.exe` 14.52 | 11.00 s, 1.00× | 17.57 s, 1.00× |
| `--fold=identical` | 11.37 s, 1.03× | 14.92 s, 0.85× |
| `--fold=select-any` | 11.60 s, 1.05× | 15.19 s, 0.86× |
| `--fold=identical --locals=copy` | 11.76 s, 1.07× | 15.37 s, 0.87× |
| `--fold=select-any --locals=merge` | 11.90 s, 1.08× | 15.50 s, 0.88× |
| `--fold=identical --report=odr` | 12.10 s, 1.10× | 15.78 s, 0.90× |

Finding what units reach costs about 15% of reading (`clangSema.lib` of
`build-z7-nopch`: 0.39 s against 0.34 s); a first version that read every
relocation again and searched every symbol for each piece took 1.34 s.

## Phase 6 results

### The ELF corpus

`tools/turbolib/elf-corpus` builds it in WSL (Ubuntu 26.04: gcc 15.2,
clang 21.1, GNU binutils 2.46, mold, lld 21): `setup.sh` fetches cmake,
ninja, and llvm-project at `c74b358f44c3` without root; `configure.sh
gcc|clang|gcc-split|clang-split` configures a Debug (`-g`,
`-ffunction-sections -fdata-sections`) build of LLVM, clang, and lld
(X86 and AArch64) as static libraries; `build.sh` builds it at low
priority. The gcc build archives with GNU `ar qc` and `ranlib` (CMake's
rules); the clang build with the gcc build's `llvm-ar` and `llvm-ranlib`.
Its libraries hold DWARF 5 (7.5 GiB of `.debug_*` and 3.4 GiB of their
relocations in 13.5 GiB, measured on 170 of the libraries); the COMDAT
groups' code is 215 MiB, with 161 MiB of relocations.

### Archives and the `ar` personality (6.1)

`turbonop_archive::unix` writes the GNU dialect (the `/` and `/SYM64/`
symbol tables, `//`, thin archives, GNU `ar`'s `/<offset>:<origin>`
members of nested archives) and `llvm-ar`'s BSD and Darwin dialects,
each as GNU `ar` 2.46 and `llvm-ar` 24 write them; `unix::symbols` gives
the symbols each indexes for an ELF member (they differ on mapping
symbols, `STT_FILE`, other bindings than local, global, weak, and unique,
and local commons). Where the writers differ (read in `bfd/archive.c` and
`ArchiveWriter.cpp`, and measured): GNU `ar` writes a long name once per
member that has it, `llvm-ar` once (and a name with `/` long);
`llvm-ar` gives an empty symbol table three NULs; `/SYM64/` comes when a
member with symbols passes 32 bits (GNU, padded to 8) or the last member's
header reaches 2^32 (`llvm-ar`, padded to 2; `SYM64_THRESHOLD`).

| Check | Result |
|---|---|
| `roundtrip_unix` on all 265 libraries of the gcc build (22.11 GiB, 3,777 members) | all written back byte for byte (264 by either writer's conventions, one by GNU `ar`'s only), every index computed from the members as the archive has it, 17.3 s |
| the same on 26 archives each tool made from crafted members (long names twice, thin, nested, BSD, Darwin, no objects, commons, absolutes) | all 26 |
| `arcompare.py`: 60 command lines run with GNU `ar`/`ranlib` and `llvm-ar`/`llvm-ranlib`, and with turbolib in their place | 60 of 60 the same: every file left, standard output, standard error, and exit status |
| `relib.py`: every library of the gcc build remade by its own commands (`cmake -E rm`, `ar Dqc`, `ranlib`) with turbolib as `ar` and `ranlib` | 265 of 265 byte for byte (22.11 GiB) |
| speed (`relib.py --time`: the 20 largest libraries, 13.79 GiB, each library's commands, best of 2, warm cache) | GNU `ar` 36.62 s, turbolib 9.78 s: 0.27× |

(Before an index fix, 261 of 265: a symbol whose section index comes
from `SHT_SYMTAB_SHNDX` is in that section, even when the number is
`SHN_COMMON`'s, 0xFFF2, which objects of more than 65,280 sections
have.)

The scenarios cover `r` (replacing, positions `a`/`b`, `P`, `u`), `q`,
`d` (with `N`), `m`, `t` (`v`, `O`, thin), `p`, `x`, `s`, `S`, `T` and
`--thin` (nested archives, the same file twice), `U` (dates compared as
"a number"), `--record-libdeps`, `-M` scripts, `L`, `--format=bsd` and
`darwin`, `SYM64_THRESHOLD`, a missing file, and creating messages. GNU
`ar`'s `-M` puts each `ADDMOD`'s files first and an `ADDLIB`'s members
one by one (backwards), as `arsup.c` does. `ar tv` prints dates in local
time (`TZ`, else `/etc/localtime`, read as TZif with its footer's POSIX
rule).

### The ELF object model (6.2)

`turbonop-elf-object` reads an ELF relocatable object (either class and
byte order) into sections with typed contents where folding edits them
(symbols, `REL`/`RELA` relocations with MIPS64's `r_info`, groups,
`SHT_SYMTAB_SHNDX` kept in step) and writes it back: into the layout read
while every part keeps its size, else afresh; string tables are kept
while every name is at its offset (so GNU `as`'s suffix-shared tables
survive; LLVM's `.strtab` shared by section and symbol names is rebuilt
as one, tested on synthesized objects only), else rebuilt. All 5,885
objects of the
gcc build's `lib` directory (the 265 libraries' members and the `.o`
files, 29.43 GiB, some with extended numbering) write back byte for byte,
and laid out afresh read back to the same model, in 11.7 s.

### Folding (6.3, first version)

`turbonop_fold::elf` (run by the `ar` personality with `--fold=identical`,
`--pull=preserve|relaxed`; `--locals=keep` only) folds identical COMDAT
groups. See the module's documentation for the units, their identity,
the environment key, what stops a unit folding, and the rewriting. In
short: the first copy survives into a synthesized member per set of
givers and environment, which keeps its group, FDEs and CIEs, and notes;
givers lose the sections and FDEs, their definitions become weak
undefined references, and they refer (strongly) to the synthesized
member's anchor (a hidden weak absolute symbol named for its contents),
placed first among their globals.

**Debug information**: a giver's `.debug_*` relocations to the copy's
local symbols are removed and their places tombstoned (lld's values), as
a link of the original would treat every copy but the one it kept. In
this first version a folded function had no DWARF in the folded
library's links; it moves now (see [Debug information
(6.5)](#debug-information-65)). The measurements below are this first
version's.

**Measured** on the gcc build's first 170 libraries (13.96 GiB, 2,640
members; `foldall.sh` runs `ar s --fold=identical --stats` on a copy of
each, 210 s for all, one after another):

| | |
|---|---|
| COMDAT units | 3,251,711; 372,295 distinct ones with copies in two or more members |
| folded | 203,824 distinct units, 1,048,040 copies, into 29,069 synthesized members |
| stayed (copies) | 466,557 calling a name of their member that stays; 344,949 whose first definer has side effects (`--pull=preserve`: LLVM's `cl::opt` initializers); 20,797 whose names have other definitions; 8,630 reaching a local symbol outside them |
| debug | 4,361,268 relocations tombstoned; 1,042,476 folded copies' debug information dropped |
| size | 14.99 GB to 14.64 GB (2.4% smaller; the `.debug_*` sections, 7.5 GiB, stay) |

**Link equivalence** (`relink.py`): `llvm-nm` and `llvm-readobj` linked
against the build's libraries and against the folded ones (all of them),
each with GNU ld 2.46, gold, lld 21, and mold:

| | GNU ld | gold | lld | mold |
|---|---|---|---|---|
| members loaded (original ones) | the same 530 and 409 | the same | the same | the same (as `--trace` lists them) |
| synthesized members loaded | 3,863 and 3,464 | the same | the same | 4,947 and 4,243 |
| images' symbols (names, types, sizes; `nm -S`) | the same, 195,367 and 180,651 | the same | the same | the same |
| output of `llvm-nm -C`, `--size-sort -S`, `llvm-readobj --sections --symbols` on the build's libraries | the same | the same | the same | the same |
| image size | +0.09%, +0.10% | +0.09%, +0.10% | +0.09%, +0.10% | -0.5% |

What the images hold is compared by `compare-images` (see 6.4): the same
but for the folded functions' DWARF.

**Determinism**: `libLLVMSupport.a` and `libLLVMCore.a`, folded with
`-j 1`, `-j 3`, and `-j 8` in separate processes, are the same bytes
each time (and `ar s` without folding gives back the build's archive).

**Speed** (`relib.py --time --options=--fold=identical`, the same 20
libraries, 13.79 GiB): GNU `ar` 37.98 s, turbolib folding 101.08 s,
2.66× (CMake's `ar qc` and `ranlib` each fold: the second finds nothing
new, but reads everything again). A first version took 305 s: its
`.eh_frame` splitting scanned every relocation for each record. What is
left is mostly decoding and re-encoding every relocation of every member
(the debug sections' 3.4 GiB of them among them) and copying each
rewritten member twice; see TODO.md.

What was found on the way:

- **The OS ABI** isn't part of the environment: GNU `as` marks objects
  with `STB_GNU_UNIQUE` or `STT_GNU_IFUNC` symbols `ELFOSABI_GNU`, which
  split identical copies (on `libLLVMSupport.a`, 845 groups folded with
  it, 3,483 without). A synthesized member is `ELFOSABI_GNU` when its own
  symbols need it.
- **TLS references** must stay `STT_TLS` when a definition becomes a
  reference (GNU ld and gold refuse a TLS definition for a non-TLS
  reference).
- **Without COFF's alias technique, a strong reference to a moved name
  lets lld and mold take it from the first library on the command line
  that has any copy of it**: a giver's definition, become a strong
  undefined reference, made lld load 41 other members of earlier
  libraries (`libLLVMBinaryFormat.a`'s, holding their own copies) for
  `llvm-nm`, with their static initializers; mold, as many. mold ranks a
  lazy definition by its file's place on the command line; lld extracts a
  lazy member the moment a loaded object's undefined symbol meets it, and
  a member extracted while its own archive is being read meets the
  reference before the synthesized members (at the archive's end) are
  known (`--trace-symbol` shows the anchor's reference with no lazy
  definition yet). So a giver's moved names are **weak** references, which
  no linker loads anything for, and its strong reference to the anchor is
  what loads the synthesized member that defines them (the anchors first
  among its globals); a synthesized member refers to names moved into
  another the same way, with that one's anchor.
- **A unit naming what stays in its member must stay too.** A
  synthesized member's reference to a name each giver defined itself (an
  inline function whose own copies stay) is resolved by the linker, and
  mold took it from another member (`libLLVMCore.a`'s `Core.cpp.o`, for
  `llvm-readobj`: 7,996 symbols more in the image). So a unit folds only
  if every name its relocations give that another unit of its member
  defines folds too, to a fixpoint: the largest reason copies stay
  (466,557), as the units with side-effect definers keep their callees.

**Initializer entries in groups** (ELF's counterpart of COFF's
`.CRT$X*` rule). How the compilers write C++17 inline variables' and
templates' static data members' dynamic initialization (gcc 15.2, clang
21.1, `-ffunction-sections -fdata-sections`):

- **clang** puts each variable's in the variable's COMDAT group: the
  variable, its guard (`_ZGV...`), its `__cxx_global_var_init` in
  `.text.startup`, that function's `.gcc_except_table`, and its
  `.init_array` entry (`.init_array.NNNNN` with `init_priority`; `.ctors`
  with `-fno-use-init-array`). Moved into a synthesized member, the entry
  would run at that member's place in the order, not among its object's
  initializers.
- **GCC** puts the variable and its guard in groups of their own (data
  only) and the guarded initialization in the object's own
  `_GLOBAL__sub_I_` function, registered by an `.init_array` outside any
  group. Those units fold safely; the member has side effects.

So a unit holding an `.init_array`, `.fini_array`, `.preinit_array`,
`.ctors`, or `.dtors` section (by type or by name, with a priority or
not), or a `.gnu.warning` section, stays, and such an entry gives its
member side effects (`--pull=preserve`: loading it runs the
initializer). Before this rule the engine folded them (they were only
looked for outside groups). Checks:

| Check | Result |
|---|---|
| `tools/turbolib/elf-init/run.sh` (inline variables, a variable template, a template's static member, `init_priority`, a destructor, `thread_local`; gcc and clang at `-O0` and `-O2`, clang also with `-fno-use-init-array`; both `--pull` modes; GNU ld, gold, lld, mold) | all pass: no synthesized member holds an entry, the programs print the same initialization order, `verify` and `compare-images` pass. Without the rule, clang under `--pull=relaxed` moved 4 grouped entries: the printed order changed with every linker (with `.ctors`, every one but lld), and `compare-images` failed every link |
| size cost on the gcc corpus (all 265 libraries, `foldall.sh`) | none: 23,508,957,816 bytes with and without the rule, every archive the same bytes (GCC writes no grouped entries) |

The clang corpus's cost is in its results below.

### Verification (6.4): `verify` and `compare-images` for ELF

Both are in `turbonop_verify::elf`; `turbolib verify` takes GNU and BSD
archives of ELF objects, and `turbolib compare-images a b` two ELF
programs (no PDBs: they carry their symbols and DWARF). Both run on every
core and report by category as for COFF.

**`turbolib verify`** (`elf::library`) hashes each COMDAT group as a unit
by a canonical form written apart from the engine (sections by name,
type, flags, alignment, entry size, and bytes; relocations with targets
by name outside the unit and by place inside it; the symbols it defines;
its FDEs and their CIEs), and a member's environment apart. It checks
every original member present in order and every added one synthesized
(it defines a hidden weak absolute `__turbolib.anchor.` name); in each
rewritten member, the environment, the sections outside units with their
bytes and relocations unchanged, `.eh_frame`'s records the original's but
the lost units' FDEs, `.debug_*` unchanged but at the relocations to lost
units, which are gone and whose places hold the tombstone, the kept units
the original's, every lost unit held identically (hash and environment)
by a synthesized member the member anchors, every definition kept or (in
a lost unit) a weak undefined reference of its type, and no new
undefined names but moved ones and anchors; in synthesized members, units
that givers lost, none in two members, no sections but units',
environments', `.eh_frame`, tables, and DWARF (reported as left to
`compare-images`), and only names the original library knows; each
archive's index against its members (`unix::symbols`, GNU `ar`'s rules);
and, under `--pull=preserve`, each name of the original's index sent to
the same member or, from a definer without side effects, to a
synthesized member.

**`turbolib compare-images`** (`elf`, `elf::program`, `elf::tables`,
`elf::dwarf`) names every address by a label: `.symtab`'s sized globals,
then sized locals (by name where no other symbol has it or it has a
visibility, else `name@file` and an ordinal), other symbols, PLT entries
and GOT slots by what their dynamic relocations name, sections' starts;
an address outside every symbol in a non-code section is named by its
contents (up to 32 bytes or a NUL), since merged strings and constants
move with the link. Then:

- **functions and data**: each sized symbol (one per extent) paired by
  its key, its dynamic relocations compared by what they name (and
  masked), and its bytes compared with COFF's `image::bytes` (x86-64
  PC-relative operands by their targets' names, other differing 4-byte
  words as addresses naming the same thing), plus thread-pointer offsets
  (local-exec TLS displacements and immediates) naming the same TLS
  variable: the TLS block's layout follows the link. The linker's tables
  (`.dynamic`, GOT, PLT, relocations, hash tables, versions,
  `.eh_frame*`) are compared as tables, not bytes.
- **tables**: header fields and the entry point, program headers and
  sections (type and flags in order), `.symtab` as a set (anchors only in
  the second, expected; mold's absolute group-signature locals as a set
  of names), the dynamic section (`DT_NEEDED` and other strings, flags,
  `DT_INIT`/`DT_FINI` by name, the tags), dynamic symbols with versions,
  dynamic relocations as a multiset, the initializer arrays in order,
  notes (build IDs expected to differ), FDEs by function (instructions
  without trailing `DW_CFA_nop` padding, CIE, personality and LSDA by
  name), `.eh_frame_hdr` as a set.
- **DWARF**: every live subprogram (its entry address in an executable
  section: lld and mold tombstone discarded copies) keyed by its entry's
  label, its subtree DIE by DIE (attributes sorted, forms ignored,
  addresses and range and location lists by names, expressions with
  addresses as names and references as identities, `DW_AT_GNU_locviews`
  as view pairs, files as paths; a nested subprogram with code of its own
  compared apart, and by identity inside its parent), its unit's
  producer, name, directory, language, and version, and the unit's line
  rows over its ranges (by offset from each range's start; an end of
  sequence only if one of its sequence's rows is in the range);
  variables at fixed addresses by label; named type definitions as
  (qualified name, contents). A reference compares as the identity of its
  target (tag, qualified name, name, linkage name, and what a
  specification or abstract origin names; anonymous DIEs structurally),
  so a type's declaration and its definition compare equal. Descriptions
  are compared as sets: GNU ld and gold give a discarded copy's DWARF the
  kept copy's addresses, so the original may describe a subprogram once
  per unit that had a copy (expected when the second's descriptions are
  among them), and a description with the same DIEs and rows from
  another unit is expected (the link keeps the first loaded copy's,
  turbolib the first member's).

What they can't prove: `verify` doesn't follow references (a definer
without side effects may load members that have some), relies on 128-bit
hashes, and doesn't read synthesized members' DWARF; `compare-images`
infers differing words as addresses or TLS offsets (and equal bytes
outside a function's code that decode as PC-relative operands are
accepted), doesn't compare bytes outside every sized symbol (counted:
11.3 MB in `llvm-nm`, mostly string and constant pools, jump tables,
`.gcc_except_table`, and the linker's own tables), decodes only x86-64
code (ARM64's decoding is COFF's, untested on ELF), takes trailing zeros
of an FDE's instructions as padding, compares DWARF references by
identity rather than following them (type definitions are compared
apart), and runs nothing.

**Results** (gcc corpus, WSL, 8 threads, at low priority beside a build):

| Check | Result |
|---|---|
| `verify` of the gcc build's 170 folded libraries (`folded-gcc`, 13.96 GiB) against the build's | 166 pass; 4 fail (`libLLVMVectorize.a`, `libMLIROpenACCDialect.a`, `libMLIROpenMPDialect.a`, `libclangCIR.a`): the folded archive's index lists 1 or 2 empty names for a rewritten member (`SLPVectorizer.cpp.o`, with 99,156 sections) whose symbols give none; 62.6 s for all |
| ... what it checked | 2,640 original members, 2,488 rewritten, 29,069 synthesized; 1,048,040 moved units, each held by a synthesized member its giver anchors (the engine's count of folded copies); 4,361,268 debug places tombstoned (the engine's count); 3,867,349 definitions; 2,033,763 names of the indexes sent where `--pull=preserve` allows |
| `compare-images` of a second link of the original against the first (`relink.py --compare-images`), `llvm-nm` and `llvm-readobj` with GNU ld, gold, lld, and mold | 8 of 8 equivalent, 3.2 to 4.3 s each (about 1.8 GB of memory) |
| the same, original against folded | all 8 equivalent in every category but DWARF: the folded functions' subprograms are only in the original (`llvm-nm`: 11,197 with GNU ld, 13,586 gold, 14,691 lld, 14,666 mold; `llvm-readobj`: 9,801, 10,648, 11,022, 11,031; fewer with GNU ld and gold, which give other libraries' discarded copies' DWARF the kept copy's addresses). Expected: the anchors (3,863 and 3,464), up to 30 subprograms described by another unit's copy, sections' and segments' sizes |
| ... checked in `llvm-nm` with lld | 171,103 functions and data (31.1 MB), 1,012,838 PC-relative operands, 36,787 dynamic relocations in them, 148,947 FDEs, 147,181 live subprograms, 140,549 distinct type definitions |
| `breaklib.py`: one byte of `raw_ostream::write` changed in the folded `libLLVMSupport.a`, `llvm-nm` relinked with lld | caught by both: `compare-images` (that function's bytes differ: 1 failure) and `verify` (that member's section changed) |

What was found on the way: linkers make hidden globals locals after
another object's `STT_FILE` (a synthesized member has none), so locals
are keyed by name where they can be; TLS variables in COMDAT groups move
into synthesized members, which changes TLS offsets (`.tbss` 0x30 to 0x29
bytes in `llvm-nm` with lld, mold uses imm32 and disp8 forms); a lambda's
DWARF is nested in its enclosing function's, whose kept copy may come
from a unit where the lambda's code was discarded; `turbonop-elf-object`'s
`symbols()` searches the sections each call (a first `verify` took 164 s
on `libLLVMVectorize.a`, 3.5 s once the table was looked up once).

### Debug information (6.5)

A synthesized member describes its units with a compile unit of its own
(`turbonop_fold::elf`'s `dwarf` module, which documents it in full).
Every unit of a synthesized member comes from one member, the first that
held a copy, and the new unit is built from that member's:

- **DIEs**: the roots are the DIEs with addresses in the moved sections
  (`DW_AT_low_pc`, a range list, `DW_OP_addr`, directly or through
  `.debug_addr`), each taken up to its subprogram. Each root's subtree is
  kept, then what kept DIEs refer to (specifications, abstract origins,
  types, the DIEs location expressions name), to a fixpoint. Ancestors
  are kept as skeletons (a namespace with only its kept children).
- **Types**: a named class, structure, or union in named scopes is kept
  as a *declaration* (its name, `DW_AT_declaration`, and only its kept
  members, such as the declaration a moved member function's
  `DW_AT_specification` names), as clang's `-fno-standalone-debug`
  writes them. The rest is kept whole: base and derived types,
  enumerations, anonymous and local classes, abstract instances. **A
  deviation from the plan**, which said "the types they use": kept whole,
  LLVM's classes' members pull in most of a unit's types, 3.35 GB of DWARF
  for `libLLVMCodeGen.a`'s 1,343 synthesized members (4.9 GB of synthesized
  members for a 1.34 GB library). As declarations it is 4.1 MB. The definitions are
  still in the givers' units (which keep their DIEs, tombstoned), and
  every link that loads a synthesized member loads one of its givers.
  Debuggers resolve a declaration by name (gdb's `ptype this` and
  `print *this` in a folded member function print the same as the
  original's).
- **The unit's DIE** is the original's attributes. Its addresses, bases,
  and macros are left out. `DW_AT_low_pc` is 0, `DW_AT_ranges` lists the
  moved code sections, and `DW_AT_stmt_list` names the new line table.
  There is a `.debug_aranges` set if the original had one.
- **The line table** is the original's header. It keeps only the files
  the kept DIEs (`DW_AT_decl_file`, `DW_AT_call_file`) and the moved
  sequences name, plus the unit's primary file, and their directories,
  renumbered. Then come the sequences whose `DW_LNE_set_address` names a
  moved section (assemblers write one per section), with their
  `DW_LNS_set_file`s renumbered. With every file kept, the header was most
  of a synthesized member's DWARF: 13 KB each in `libLLVMCodeGen.a`, most
  of it the files' `.debug_line_str` relocations.
- **Lists and forms**: range and location lists are rewritten as
  start-and-length entries, with GCC's location views
  (`DW_AT_GNU_locviews`). Addresses are written `DW_FORM_addr`, so no
  `.debug_addr` is written: an index is resolved, `DW_OP_addrx` becomes
  `DW_OP_addr`, and `DW_OP_constx` becomes `DW_OP_const8u` with the
  entry's relocation (TLS offsets). Strings go to the member's own
  `.debug_str` and `.debug_line_str`, references are `DW_FORM_ref4`, and
  expressions' unit-relative references are padded ULEB128 (their
  branches are re-aimed). `DW_AT_sibling` is left out.
- **Relocations** go to the copied sections' symbols, to the new debug
  sections, or to a global name by name, weak where the member doesn't
  define it (so that it loads nothing). An address of something not
  moved (a `static` variable a skeleton names) is written 0 and counted.
- **Split DWARF** (DWARF 5, `-gsplit-dwarf`): the member's skeleton names
  its `.dwo`, which the `ar` personality reads from `DW_AT_comp_dir`
  (`turbonop_fold::elf::fold_with` takes the reader). The synthesized
  member's unit is unsplit, built from the `.dwo`'s DIEs, strings, and
  lists and the member's `.debug_addr` and line table. This is the
  plan's first choice: no new outputs for the build rule to declare, and
  a link may mix split and unsplit units. The givers' `.dwo` files aren't
  touched: their addresses go through `.debug_addr`, which is
  tombstoned. GNU's split DWARF 4 (`-gsplit-dwarf -gdwarf-4`, gcc's and
  clang's) is read too: a DWARF 4 skeleton with `DW_AT_GNU_dwo_id` names
  its `.dwo` by `DW_AT_GNU_dwo_name`; the split unit's strings are
  `DW_FORM_GNU_str_index` into `.debug_str_offsets.dwo` (no header), its
  location lists `.debug_loc.dwo`'s GNU entry kinds (DWARF 5's first
  four, with a 4-byte length and 2-byte expression lengths, view pairs
  apart), its range lists the member's `.debug_ranges` from
  `DW_AT_GNU_ranges_base`. `compare-images` reads them the same way. In
  `elf-debug`'s fixture (gcc and clang, `-O0` and `-O2`), all 68 checks
  pass, gdb sessions included; with the other 12 configurations, 272 of
  272.
- **Not handled** (the synthesized member gets no DWARF, and `--stats`
  says why): 64-bit DWARF, REL machines (only x86-64 and AArch64 are
  rewritten), compressed debug sections, units that refer outside
  themselves (`DW_FORM_ref_addr` to another unit), and expression or form
  kinds not known.

Two kinds of differences from the original are expected:

- A folded function is described by the first member's copy. A link of
  the original keeps the first loaded member's copy, and GCC can describe
  one inline function differently in different units. For example,
  `RTTIExtends<FileSystem, RTTIRoot>::classID()` has a `DW_AT_type` in
  `FileCollector.cpp.o` and none in `VirtualFileSystem.cpp.o`. COFF has
  the same difference (2.4).
- The images' DWARF grows (`llvm-nm` with GNU ld: 317.9 to 325.5 MB, of
  which about 0.3 MB is from the first version): the givers' DIEs stay
  (tombstoned), and each synthesized member adds declarations and line
  headers of its own.

**Results**:

| Check | Result |
|---|---|
| fixture (`tools/turbolib/elf-debug/run.sh`: three members sharing inline functions, templates, a lambda, a static local, a TLS variable, virtual functions, exceptions; gcc 15.2 and clang 21.1, DWARF 4 and 5, `-O0` and `-O2`, and DWARF 5 split at both; 12 configurations), each linked with GNU ld, gold, lld, and mold | 204 of 204 checks pass: `verify`, programs' output, `compare-images` (DWARF included), and a gdb session stopped in folded functions (breakpoints by name and by line, `bt`, `info args` and `locals`, `list`, `next`, `step`, `finish`, `ptype this`, `print *this`, `info functions`, `types`, `scope`, `line`, `source`). The sessions are identical, addresses left out, except where the original describes a discarded copy too (GNU ld, and split units' `.dwo`): `info functions` lists it once per unit there, and once in the folded program |
| `ar s --fold=identical` of all 265 libraries of the gcc build (23.74 GB), one after another | 262.7 s. 310,708 distinct units folded (1,597,064 copies) into 40,402 synthesized members; DWARF moved for all 309,058 that had some (310,028 root DIEs, 178.6 MB), none dropped; 1,453 addresses tombstoned in the copies; 23.74 GB to 23.51 GB (1.0% smaller) |
| `verify` of those 265 against the build's | 265 pass. The 4 failures of 6.4 (on the older `folded-gcc`) don't occur with this build's output; why they did wasn't investigated |
| `compare-images` of `llvm-nm` and `llvm-readobj` linked against them (GNU ld, gold, lld, mold), against the build's | "subprograms only in the first program" went from 9,801 to 14,691 per program to 0. 7 to 22 per program are "subprograms whose DIEs differ": the first member's copy against the first loaded one's (above; spot-checked on `classID()` with `llvm-dwarfdump` of both members). 1,227 in `llvm-readobj` with mold are expected (the same DIEs and rows from another unit). Every other category is equivalent |
| gdb on `llvm-nm` (lld) against folded and original libraries, stepping into the folded `StringRef::size` (defined only in `__turbolib.11.o` of the folded `libLLVMSupport.a`) | the same session: frames, arguments, locals, source lines |
| determinism: `libLLVMSupport.a` folded with `-j 1`, `-j 3`, and `-j 8` | the same bytes |
| speed (`relib.py --time --options=--fold=identical`, the same 20 libraries as 6.3, 13.79 GiB, best of 2) | GNU `ar` 58.75 s, turbolib 249.28 s: 4.24× (6.3 measured 2.66×, but GNU `ar` itself took 37.98 s then: this run shared the machine with a gdb check at its start, so the ratios aren't comparable; reading each survivor member's DWARF whole adds work, not measured apart) |

The spot checks of `classID()` show how the differing descriptions arise,
not that all 7 to 22 per program arise that way. `compare-images` could
prove it by matching each one against the original's discarded copies
(their DIEs are in the original image, tombstoned): a TODO.

What `compare-images` needed for this (6.4's tool, extended): LSDAs
compared by content (call sites relative to their function, action
chains, and the types they catch by what they name; they were named by
their place in `.gcc_except_table`, which folding changes); an address
below which there's only a section's start named by its contents, to a
string's NUL (`__PRETTY_FUNCTION__`s share long prefixes); an empty
list entry not placed (it may be at a section's end); `DW_OP_addrx` and
`DW_OP_constx` compared as the plain operations; split units read from
their `.dwo` files; and which debug sections each program has compared
apart from the rest (a folded split program has unsplit ones too).

### Load order, the clang corpus, and the differing descriptions (6.6)

**Load order.** `relink.py` now compares the order in which each link
loads the original members (as `--trace` lists them: the load order for
GNU ld, gold, and lld; mold's file order, by command-line place and
member index), not only the set. Against the folded libraries of both
corpora, `llvm-nm` and `llvm-readobj` load the same original members in
the same order with all four linkers (16 of 16 links; 409 to 1,019
members each), and `compare-images` finds the initializer arrays
(`.init_array`, `.fini_array`, `.preinit_array`, `.ctors`, `.dtors`) the
same, entry by entry, in order. Nothing needed fixing: unlike link.exe,
these linkers don't end a sweep early, and a giver keeps every
undefined name its moved units used (so it pulls in the same members
when it loads); its strong reference to the anchor brings the
synthesized member, whose only new references are other synthesized
members' anchors. A five-member fixture (an inline function in two
members calling a third, the giver calling a fourth first) loads the
same order with each linker too.

**The clang corpus** (`build-clang`: clang 21.1, archived by the gcc
build's `llvm-ar`, 265 libraries, 7.44 GiB):

| Check | Result |
|---|---|
| `relib.py`: every library remade by its own commands (`llvm-ar Dqc`, `llvm-ranlib -D`) with turbolib as `llvm-ar` and `llvm-ranlib` | 265 of 265 byte for byte |
| `roundtrip_unix -llvm` | 265 of 265 written back (264 by either writer, 1 by `llvm-ar`'s only), 265 indexes computed from the members, 8.1 s |
| object round trip (`turbonop-elf-object`'s `roundtrip -relayout`): 5,885 objects in 2,373 files, 9.73 GiB, LLVM's shared `.strtab`, `.llvm_addrsig` in 180 of 184 of `libLLVMSupport.a`'s members; no CREL | all byte-identical, and laid out afresh read back the same, 17.6 s |
| `foldall.sh` (all 265) | 7,984,391,814 to 7,790,815,022 bytes (2.4% smaller); 289,024 distinct units folded (1,480,953 copies) into 36,978 synthesized members; DWARF moved for all 287,076 that had some |
| ... why copies stayed | 688,277 first definer with side effects; 582,766 calling a name of their member that stays; 16,981 names with other definitions; 13,101 reaching a local outside them |
| the initializer rule's cost | 2 grouped `.init_array` sections in the whole corpus (a template's static member, in `MachineUniformityAnalysis.cpp.o` and `UniformityAnalysis.cpp.o`, one copy each), whose members had no other initializer: with the rule they have side effects, and `libLLVMAnalysis.a` folds 4 units (8 copies) fewer and comes out 4,360 bytes *smaller* (a synthesized member costs more than the copies saved); `libLLVMCodeGen.a` the same size |
| `verify` of the 265 | 265 pass (586 s, one after another, beside other work) |
| `relink.py` (`llvm-nm`, `llvm-readobj`; GNU ld, gold, lld, mold) | the same members in the same order, the same symbols (`nm -S`), the same program outputs in all 8 |
| `compare-images`, original against folded | `llvm-readobj` with GNU ld, gold, lld: equivalent. 2 descriptions by another unit's copy, both proven (below). Left: 1 or 2 `llvm-nm` functions whose operand names a constant by contents that run past it (16 bytes of `identify_magic`'s, 8 of `expf`'s, then what follows them, which the layout moves), and one jump table not recognized (`VPIntrinsic::getVectorLengthParamPos`); and with mold, 21 functions of `llvm-readobj` whose `lea off(%reg)` after mold's `sub $tls_size` relaxation names a TLS variable by its offset in the TLS block (`PrettyStackTraceHead`; folding moves TLS variables, so the offsets change): not yet read as TLS offsets |

What the clang corpus found:

- **Rebuilt string tables lost LLVM's tail sharing.** LLVM's `.strtab`
  holds the section names and symbol names, and `.rela.text.f`,
  `.text.f`, and `f` share one string. Rebuilt name by name, a rewritten
  member's `.strtab` tripled (`CIRGenStmtOpenMP.cpp.o`: 85 MB to 255 MB),
  and the first fold of the clang corpus came out 9.1% *larger*
  (8,708,710,254 bytes). `turbonop-elf-object` now builds tables
  tail-merged (each name sharing the end of a longer one), in an order
  that depends only on the set of names. On the gcc corpus it saves
  another 21 MB (23,508,957,816 to 23,488,090,872 bytes, 1.05% smaller
  than the build's).
- **Jump tables** (clang puts x86-64 jump tables in `.rodata`, unlabeled,
  with entries relative to the table) were named by their bytes, which
  follow the layout: 23 to 148 functions per program failed.
  `compare-images` now compares an unlabeled table of 32-bit
  table-relative entries landing in one function by its targets.
- **An address one past a section's end** (a tail call's
  `DW_AT_call_return_pc` in the last function of `.text`) was "dead" in
  one program and named in the other; it is named from the byte before
  it.
- **`compare-images`' DWARF verdicts varied from run to run**: a
  subprogram's descriptions were sorted by their hashes, which are keyed
  per process, and classified against the first. Now each description
  of the second program that the first lacks is classified against the
  first's closest one (the same DIEs and rows from another unit: expected;
  rows differ; DIEs differ), the worst deciding, and examples are chosen
  by place.

**The differing descriptions** (the plan's 7 to 22 per program). A
folded function is described by the first member's copy; the link of the
original keeps the first loaded one's, and the first member may not even
be loaded. `witness.py` proves each one: for the unit `compare-images`
names as the folded program's, it links the original program again with
that unit's object first on the command line (every linker then keeps
its copies, and GNU ld and gold describe the others' too) and compares
that witness with the folded program; a subprogram is proven when the
witness describes it as the folded program does.

| Corpus | Differing descriptions (`llvm-nm`, `llvm-readobj`; GNU ld, gold, lld, mold) | Proven another unit's copy |
|---|---|---|
| gcc (`fold-gcc`, this build's output) | 5, 12, 20, 9; 5, 6, 15, 7 (79) | 79 of 79 |
| clang | 0, 1, 1, 0; 0, 0, 0, 0 (2) | 2 of 2 |

(The counts differ from 6.5's 7 to 22 for the classification fix above:
each run had reported its own mix of "DIEs differ" and "another unit's
copy".) For example, `SmallString<0>::~SmallString` has
`DW_AT_decl_file` `MD5.h` in `ELFObjectFile.cpp`'s copy and `XCOFF.h`
in `ArchiveWriter.cpp`'s; `RTTIExtends<FileSystem, RTTIRoot>::classID()`
has a `DW_AT_type` in one unit and not the other. A witness links more
than the original (that unit's object and what it pulls in) and keeps
that unit's copies of other functions too; only the subprograms it was
made for are looked at.

**Folding's speed.** `--stats` now gives each phase's time (reading,
choosing, rewriting, reading DWARF, synthesizing). On the gcc build's
libraries they showed two places where the work grew with the square of
a member's size, both in huge members (`libclangSema.a`'s): each unit's
symbols were found by scanning every symbol of its member, once while
hashing and again in each synthesized member it went to; and each
synthesized member's compile unit walked every DIE of its source unit
(marks, files, children, offsets, which classes are declarable), where a
large member gives DIEs to many synthesized members. Each unit's symbols
are now listed once, and a unit's DIEs are walked once; a synthesized
member's work follows the DIEs it keeps. Smaller changes: hash maps for
choosing's lookups, names borrowed rather than copied, and debug
relocations tombstoned in place (and copied only where one names a
moved unit). The output is the same bytes (`libclangSema.a`,
`libLLVMCodeGen.a`, `libLLVMSupport.a` against the corpus fold before
the changes; `-j 1`, `3`, and `8` the same).

| `ar s --fold=identical` (8 threads, beside another session's builds) | before | after |
|---|---|---|
| `libclangSema.a` | 89.2 s (reading 47.1, choosing 3.5, rewriting 2.8, DWARF 3.7, synthesizing 28.7) | 15.5 s (1.1, 2.8, 2.0, 2.6, 4.6) |
| `libLLVMCodeGen.a` | 11.6 s | 4.4 s |
| `libLLVMSupport.a` | 0.78 s | 0.41 s |

`relib.py --time --options=--fold=identical` (the 20 largest libraries,
13.79 GiB, each library's CMake commands, best of 2): GNU `ar` 95.70 s,
turbolib 76.70 s, **0.80×** (it was 2.66× before 6.5 and 4.24× after).
Not a quiet measurement: another session's builds ran throughout (load
average 4 to 7), and GNU `ar`'s own time ranged from 38 s to 163 s
across this phase's runs, so the ratio is the run's, not a constant.
What is left, by phase: choosing runs on one thread (2.8 s of
`libclangSema.a`'s); a huge member is rewritten and its DWARF read on
one thread each; and system time is about two fifths of the total (21 s of
`libclangSema.a`'s 51 s of CPU), most of it mimalloc returning and
refaulting memory: with `MIMALLOC_PURGE_DELAY=-1`, 68 s of system time
became 16 s before the changes above. Setting the option takes an
unsafe call into mimalloc, so it isn't done. `ranlib` after `ar qc`
still folds again (finding nothing new).

### Speed, the clang corpus's last differences, and CREL (6.7)

**Folding's speed.** What 6.6 left, measured on the gcc build's
libraries (8 threads; `--stats`' phases):

- **Choosing**: the closure "a unit naming what stays in its member stays
  too" rebuilt the set of every folded name and rescanned every unit
  until nothing changed. It is now a worklist: each name of a folding
  unit belongs to that unit alone (every definition of it is a copy), so
  a unit whose needed name belongs to none stays, and so does each unit
  needing one of its names. The needs are found on every core.
  `libclangSema.a`: 0.81 s to 0.02 s.
- **Scheduling**: members were split into equal chunks per thread, while
  their sizes differ a thousandfold (`libclangCIR.a`'s largest member is
  588 MB of its 1.63 GB). `turbonop_par::map_costed` takes the dearest
  item first, one at a time; reading, rewriting, reading DWARF, and
  synthesizing use it.
- **Huge members**: a member of many units hashes them on every core; a
  unit's DIEs are searched for roots (DIEs with addresses in units'
  sections) in chunks on every core; string tables are sorted comparing
  names backwards eight bytes at a time, and on every core
  (`turbonop-elf-object`'s `parallel` feature, which the fold engine turns
  on). Writing `CIRGenStmtOpenMP.cpp.o` (its 266 MB string table, 228,727
  names, rebuilt) took 1.33 s; about 0.4 s now.
- **Relocations as bytes**: `turbonop-elf-object` keeps a relocation
  section as the file's bytes (`Relocations`), reads entries from them on
  demand, renumbers symbols in a copy of the bytes, and writes unchanged
  ones back as they are; only an edit as values decodes them. Sections'
  bytes are no longer copied before being placed in the written member,
  and in the layout read, bytes already in place aren't copied again.
- **Writing**: on Unix the archive is written straight from the members'
  bytes (small pieces gathered); Windows keeps its parallel chunk copies
  (NTFS, see `output.rs`). Writing folded `libclangSema.a`: 0.38 to 0.63 s
  before, 0.32 to 0.35 s.
- **`ranlib` after `ar qc`**: an archive that would be written as the
  bytes it already holds (every member the archive's own bytes at its
  place; the index, long names, headers, and padding the same) isn't
  written again; only its date is set. Refolding a folded archive finds
  nothing new (the units that folded are single copies now), so CMake's
  `ranlib` costs reading and choosing: on folded `libclangSema.a`, 0.88 s
  became 0.35 s; `ar s` of the build's (already indexed) `libclangSema.a`,
  1.21 s became 0.18 s (GNU `ranlib`: 3.49 s).
- **mimalloc**: turbolib now calls `turbonop_global_alloc::for_batch_work`
  first (purge delay -1). Best of 3, before and after: system time 3.03 to
  1.95 s (`libclangSema.a`), 2.28 to 1.13 s (`libclangCIR.a`), 1.96 to
  1.55 s (`libLLVMCodeGen.a`). Three more options were tried on top
  (`arena_eager_commit`, `eager_commit_delay` 0, `purge_decommits` 0;
  each, and all together; seven interleaved rounds of four libraries):
  medians 7.41 to 7.69 s against 7.96 s, within the rounds' spread (6.5 to
  10.7 s; another session's builds ran at times), so none is set.

| `relib.py --time --options=--fold=identical` (the 20 largest libraries, each library's CMake commands, best of 2) | archiver | turbolib | ratio |
|---|---|---|---|
| gcc build (13.79 GiB), against GNU `ar` and `ranlib` 2.46, this session's first build | 44.97 s | 39.58 s | 0.88× |
| the same, after the speed changes (three occasions: a build before the parallel hashing and DWARF roots, then the last build twice) | 36.84 s, 40.20 s, 40.24 s | 17.89 s, 20.94 s, 24.22 s | 0.49×, 0.52×, 0.60× |
| gcc build, against a Release `llvm-ar` and `llvm-ranlib` (`release-tools.sh`, the corpus's LLVM) | 5.94 s | 23.26 s | 3.92× |
| clang build (4.29 GiB), against the same | 1.91 s | 8.28 s | 4.32× |
| gcc build without folding, against the Release `llvm-ar` | 6.57 s | 7.03 s | 1.07× |

None of these is a quiet measurement: another session's Mach-O runs came
and went throughout (load averages 0.4 to 4.6 around them; the scripts
waited for a minute below 0.8 or 1.5 before each). The corpus builds' own
`llvm-ar` is a Debug build, far slower than a user's, hence the Release
one. Against it, folding costs about 4× an archiver that only indexes:
GNU `ar` reads each member through BFD, `llvm-ar` barely reads them.

| `ar s --fold=identical`, one library (`--stats` phases) | first build | last build |
|---|---|---|
| `libclangSema.a` (1.45 GB) | 4.25 s (reading 0.33, choosing 0.84, rewriting 0.65, DWARF 0.58, synthesizing 1.16; 5.1 s of system time) | 3.62 s (0.17, 0.10, 0.45, 0.81, 1.15; 4.8 s) |
| `libclangCIR.a` (1.77 GB) | 2.97 s (0.30, 0.19, 0.81, 0.57, 0.28) | 2.14 s (0.23, 0.06, 0.32, 0.40, 0.21) |

(The per-library runs vary by a third from run to run on this shared
machine; the first build's `libclangSema.a` was measured on a quiet one.)

The output is the same bytes as before these changes: all 265 libraries
of each corpus, folded by this session's first and last builds, compare
equal; `-j 1`, `3`, and `8` give the same bytes (`libclangSema.a`,
`libclangCIR.a`, `libLLVMSupport.a`); `relib.py` remakes all 265 of each
corpus byte for byte; `arcompare.py`'s 60 scenarios behave as GNU `ar` and
`llvm-ar`; every object of both corpora round-trips byte for byte, and
decoded to values and encoded again too (`roundtrip -relayout` now checks
that, for REL, RELA, and CREL).

What is left: a huge member's string table is assembled, and its sections
copied into the member, on one thread; a rewritten member is built whole
in memory before the archive writer copies it (borrowing the input's
pieces would save that copy, but the index is computed from the member's
bytes); `Source::new` copies each debug section to apply its relocations.

**`compare-images` on the clang corpus.** The three kinds of difference
6.6 left are read now:

- **A constant named by its contents** ran on past its end into what the
  layout put after it (16 bytes of `identify_magic`'s, 8 of `expf`'s). A
  RIP-relative load of a known size (`movdqa`, `movsd`) reads that many
  bytes, so those are compared; an address taken (`lea`) still names the
  contents to a NUL or 4096 bytes.
- **The jump table** of `VPIntrinsic::getVectorLengthParamPos` (298
  entries) was read on into the next function's table, whose entries,
  read relative to this table's start, landed in this function too, at
  places the layout set. A table now ends at the next address outside
  code that code names (every function's RIP-relative operands, decoded
  once per program, on every core).
- **mold's relaxed TLS**: mold rewrites a dynamic TLS access as
  `mov %fs:0,R; sub $S,R` (S the TLS block's aligned size, so R is the
  block's start), and the compiler's `d(R)` follows (`lea 0x20(%rax)`),
  possibly after copies (`mov %rax,%rcx`). S and d are named in each
  program's TLS layout; R is followed through register copies and
  forgotten when written (every register, at an instruction that might
  write one otherwise). The same bytes naming other variables fail (`code
  whose TLS offsets name other variables`). Unit tests cover the copy, a
  write in between, another variable, and another block size.

| `compare-images` of the relinks (`llvm-nm`, `llvm-readobj`; GNU ld, gold, lld, mold), original against folded | failures |
|---|---|
| clang corpus | 0, 1, 1, 0; 0, 0, 0, 0: the 2 descriptions `witness.py` proved another unit's copy (6.6). Before: also 1 or 2 per `llvm-nm` link (operands) and 21 for `llvm-readobj` with mold (bytes) |
| gcc corpus | 5, 12, 20, 9; 5, 6, 15, 7: the 79 descriptions proven another unit's copy (6.6), nothing else |
| each original against a second link of itself (16) | equivalent |

**CREL** (LLVM's compact relocations, `SHT_CREL`; clang's
`-Wa,--crel,--allow-experimental-crel`). `turbonop-elf-object` decodes a
CREL section as LLVM's `decodeCrel` does into relocations (their `r_info`
packed as the class's REL entries pack it), keeps the bytes read to write
back while nothing changes the entries, and otherwise encodes them as
LLVM's `encodeCrel` does. The fold engine folds members whose CREL
sections have addends (LLVM's assembler always writes them; without, or
on MIPS64, the member passes through); a synthesized member's copied
sections stay CREL, its DWARF's relocations are RELA. GNU ld refuses CREL
objects, and gold accepts them and applies none of their relocations, so
only lld and mold are checked.

| Check | Result |
|---|---|
| unit test: LLVM's encoding of a hand-checked pair; 2,000 entries in each class, with and without addends, offsets falling back and jumping | encoding and decoding are inverse; trailing or missing bytes are refused |
| `elf-debug`'s `clang-{4,5}-{O0,O2}-crel` (lld, mold) | 36 of 36 checks pass (`verify`, output, `compare-images`, gdb); the same folds as without CREL (63 units, 189 copies at `-O0`) |
| clang's 16 CREL objects and the folded archives' members (`roundtrip -relayout`) | 67 of 67 written back byte for byte, laid out afresh the same, and every relocation section encoded again from values byte for byte (LLVM's encoder reproduced) |

### Folding's speed, profiled (6.8)

Profiled with `perf` in WSL (user-space samples, `cpu-clock:u`; a
frame-pointer build for callers) folding the gcc build's
`libclangSema.a` and `libclangCIR.a`, with a timeline of samples per
50 ms for the cores left idle. What the profile showed, and what changed:

- **Synthesizing** was 28% of the samples in `Object::symbols`: each
  relocation a synthesized member copied looked the symbol table up by
  scanning every section, and each synthesized member found its first
  member's environment sections, `.eh_frame`, and its relocation section
  by name (17% more). `parse` finds them once. `libclangSema.a`:
  synthesizing 1.2 s to 0.13 s.
- **Reading DWARF.** A debug section no relocation to a debug section
  patches (`.debug_str`, `.debug_abbrev`, often the lists) is read in
  place, not copied (`libclangSema.a` still copies 190 MB of
  `.debug_info`, 20 MB of `.debug_line`, 6 MB of `.debug_str_offsets`).
  Walking a unit's DIEs takes each abbreviation's shape: all forms of
  fixed size are passed over at once, the others by a list of skips that
  read what `Unit::read_value` reads (so a DIE fails to read as before);
  the search for roots reads only DIEs whose abbreviations have
  address-bearing attributes and no `DW_AT_declaration`; attributes are
  read into one buffer per loop; the DIE and relocation vectors are sized
  up front (their growth was a quarter of the copying); `turbonop-dwarf`'s
  `Abbrevs::get` finds code n at place n − 1 first. A synthesized member's
  line-table sequences are found by section, not by reading the unit's
  every sequence once per synthesized member; the DWARF builder's hash
  maps use the engine's hasher (SipHash's was 1.4%).
- **A rewritten member is written as parts.** It was built whole in
  memory (its sections copied from the input; a debug section copied
  first to write its tombstones), copied again into the archive, and its
  index read back from those bytes. `turbonop-elf-object`'s
  `Object::write_parts` gives `write`'s bytes as parts, the sections'
  contents borrowed (a rebuilt string table moved in); `fold_parts` gives
  each rewritten member as parts of its input and bytes made for it, the
  tombstones written over the parts where they fall (overlapping ones in
  order), and the symbols its index takes (`turbonop-archive`'s
  `elf_symbols`, the rule `member_symbols` applies, for an object known
  as values); `ar` writes the parts. The fold tests check each rewritten
  member gathered is `fold`'s bytes and that its index from values is
  what reading those bytes gives (GNU `ar` and `llvm-ar`); `roundtrip
  -relayout` checks the parts of every object gathered are `write`'s.
  `libclangSema.a`: 4.1 GB at most in memory, from 5.2 GB.
- **Hashing only what can have copies.** Reading hashed every unit's
  contents. A unit now has a key of what its hash covers that is read
  without its contents (the environment, its group's flags, its sections'
  names, types, flags, and sizes, its symbol and FDE counts); units of
  different keys have different hashes (barring a collision of the 128-bit
  hash, which folding already relies on), so only units whose key is in
  two members or more are hashed; the others get hashes of their own.
- **On every core:** the index's symbols (members were read in chunks of
  eight in order, leaving one thread with the rewritten ones); choosing
  (the maps of names and of units built in 64 shards, a name's
  definitions summed up as its first member and the unit all are copies
  of; each unit decided in parallel and counted in order); a large
  string table's assembly (each name placed by the one before it in order
  alone, which is the same as by the last one written whole; a test
  checks 60,000 names against the old builder); rewriting and reading
  DWARF as one pool of work, the dearest first, so neither waits on the
  other's largest member; freeing what was read. A small library's maps
  are built on one thread.
- Smaller: the redundant pass over debug relocations before dropping the
  tombstoned ones; the member's symbols not copied to rewrite it.
- **Tried and taken back:** reading a debug section's relocations from
  its relocation section where they are in order, instead of decoding
  them into a vector. Each lookup of an offset decoded twenty-odd entries:
  relocation decoding went from 3.5% of the samples to 8.1%, the search
  4% more.

What the profile shows now (`libclangSema.a`, the share of samples):
copying 8% (most of it renumbered relocations and the `.debug_info`
copies), walking DIEs 8%, reading members (`turbonop-elf-object`'s parse
and the engine's) 11%, decoding relocations 3.5%, the DWARF builder 7%,
renumbering relocations 2.7%; the rest under 2.5% each.

| `relib.py --time --options=--fold=identical` (the 20 largest libraries, each library's CMake commands, best of 2; the load average at the start of each run, rising to 2 to 4 during it as another session's runs came and went) | archiver | turbolib | ratio |
|---|---|---|---|
| gcc build, against a Release `llvm-ar` and `llvm-ranlib`: 6.7's build, then this one (load 0.61, 0.64) | 11.48 s, 8.30 s | 39.74 s, 15.59 s | 3.46×, 1.88× |
| clang build, the same (load 0.63, 0.55) | 2.12 s, 2.39 s | 10.05 s, 6.48 s | 4.73×, 2.72× |
| gcc build, against GNU `ar` and `ranlib` 2.46, this build (load 0.77) | 39.68 s | 13.50 s | 0.34× |

| `ar s --fold=identical`, one library (`--stats` phases; best of 3 interleaved rounds) | 6.7's build | this build |
|---|---|---|
| `libclangSema.a`, load 0.62 | 3.78 s (reading 0.19, choosing 0.10, rewriting 0.22, DWARF 0.64, synthesizing 1.57) | 1.74 s (reading 0.25, choosing 0.07, rewriting and DWARF 0.38, synthesizing 0.08) |
| `libclangCIR.a`, load 0.62 | 6.61 s | 4.69 s |
| `libclangSema.a` on a quiet machine (load 0.04; two rounds): wall, user and system time, peak memory | 2.94 to 3.22 s; 10.5 to 11.2 s and 2.3 to 3.2 s; 5.1 GB | 1.40 to 1.59 s; 3.6 s and 1.8 to 2.0 s; 4.0 GB |

None of these is a quiet measurement either: the `llvm-ar` times alone
differ by 40% between runs, and the per-library rounds' load rose to 3
(the later rounds of `libclangSema.a` took 9 to 13 s for 6.7's build). The
ratios compare runs made one after the other. The timeline of a fold of
`libclangSema.a` shows every core busy but for its last 0.7 s, which is
writing the archive (the kernel's copy, which more threads don't speed
up: 1 to 8 threads writing 1.4 GB with `pwrite` took the same 0.24 to
0.37 s) and freeing memory.

The output is the same bytes as before these changes: all 265 libraries
of each corpus folded compare equal with the first build's (three times:
after the parts, after the walk and choosing changes, and after hashing
only what can have copies); `-j 1`, `3`, and `8` give the same bytes
(`libclangSema.a`, `libclangCIR.a`, `libLLVMSupport.a`); ten Mach-O
archives of the Mach-O corpus (`libtool -static`, arm64, x86-64, and
universal) fold to the same bytes as before (the DWARF reader and
`Abbrevs` are shared).

What is left: reading members is now the largest phase (parsing every
symbol and relocation of every member); `.debug_info` is still copied to
apply its relocations to debug sections (reading through them needs the
shared DWARF reader to read through relocations); a unit's DIEs are walked
on one thread (the walk is sequential; units of a member run in
parallel); renumbering a rewritten member's relocations copies each of
its relocation sections.

## Phase 7 results

### The Mach-O corpus

There is no Mac here, and no macOS SDK. `tools/turbolib/macho-corpus/build.py`
(in WSL) compiles each object of the libraries `llvm-nm` and `llvm-readobj`
link, and those tools' own objects, again from the ELF corpus's clang build
(its `compile_commands.json`, generated headers, and `build.ninja`) for
`arm64-apple-macos13` and `x86_64-apple-macos13` with `-g` (clang 21.1).
The C and C++ headers are Linux's: glibc and libstdc++ (x86-64's
installed; arm64's from Ubuntu's `*-arm64-cross` packages, unpacked
without root). `__APPLE__`, `__MACH__`, and the nullability keywords are
undefined so that those headers and LLVM take their Linux paths, and
x86-64 glibc's `bits/floatn.h` is overlaid (Darwin's clang has no
`_Float128`). The code is not a Mac build's, but the objects are what
clang writes for those targets: `MH_SUBSECTIONS_VIA_SYMBOLS`, weak private
externs (under `-fvisibility-inlines-hidden`), `__compact_unwind`, DWARF 4
in `__DWARF` with Apple's accelerator tables. 1,462 compiles, 8 left out
of the archives: 4 x86-64 BLAKE3 `.S` files written for ELF; `Path.cpp`
for both (`Unix/Path.inc` takes a Darwin path on `statvfs`); arm64's
`APFloat.cpp` (glibc's `uint64_t` is `unsigned long`, Darwin's
`arm_acle.h` wants `unsigned long long`) and `xxhash.cpp` (clang 21
crashes on it for this target). 32 libraries, each archived three ways: `llvm-ar qc` then
`llvm-ranlib` (CMake's rules), `llvm-libtool-darwin -static`, and
`llvm-libtool-darwin` of both architectures' objects (a universal file
of two archives). 160 archives, 4.29 GiB.

### Darwin archives and the `libtool` personality (7.1)

The BSD and Darwin writers of 6.1 (`turbonop_archive::unix`) already wrote
`llvm-ar`'s Darwin dialect: `#1/` names, `__.SYMDEF` (`__.SYMDEF_64` past
32 bits), each member's data padded to 8 bytes with `\n`, and in
deterministic mode each name's copies dated 1, 2, ... (`llvm-ar`'s unique
timestamps, for the debug map). What was added:

- **Mach-O members' index** (`unix::symbols`, `llvm-ar`'s rules: external,
  not stabs, not undefined; private externs and commons listed; a
  universal file is not an object).
- **A Darwin table without symbols**: `llvm-ar` writes `__.SYMDEF` even
  when no member is an object, and then without the three NULs it puts in
  an empty table of an archive with objects (`Options::filler`).
- **The `libtool -static` personality** (`turbolib` run as
  `llvm-libtool-darwin` or `libtool`, or `turbolib libtool`):
  `llvm-libtool-darwin`'s command line (`-static`, `-o`, `-D`/`-U`,
  `-filelist file[,dir]`, `-arch_only`, `-l`/`-L` with its search
  directories, `-dependency_info`, `-no_warning_for_no_symbols`,
  `-warnings_as_errors`, `-syslibroot`), its messages, archives and
  universal files flattened, members grouped by architecture as it groups
  them, the duplicate-name and no-symbols warnings, and several
  architectures written as a universal file (`turbonop_macho::fat_layout`:
  `FAT_MAGIC`, each archive slice aligned to 8 bytes for 64-bit members,
  arm64 last, as `MachOUniversalWriter.cpp` sorts them).
- **Apple's own `libtool` and `ranlib`** (cctools: `__.SYMDEF SORTED`,
  their own padding) aren't emulated: they can't be run here to measure
  (building cctools-port wasn't permitted in this session). The reader
  takes their tables; the personality writes as `llvm-libtool-darwin`
  does, under either name.

| Check | Result |
|---|---|
| `roundtrip_unix -llvm` on the corpus's 160 archives (4.29 GiB, 4,278 members; universal files slice by slice, and their headers rebuilt) | 160 of 160 written back byte for byte, every index computed from the members as `llvm-ar` computes it, 10.4 s |
| `darcompare.py`: 41 command lines (11 of `llvm-ar`/`llvm-ranlib` on Mach-O members, 30 of `llvm-libtool-darwin`: options, errors, warnings, `-l`, `-filelist`, `-arch_only`, `-dependency_info`, universal inputs and outputs, `-U`) with the real tools and with turbolib | 41 of 41 the same: every file left, standard output, standard error, exit status |
| `relib.py`: every archive of the corpus remade by its own commands with turbolib in place of the three tools | 160 of 160 byte for byte |
| speed (`relib.py --time`: all 160 archives' commands, one after another, best of 2) | the real tools 20.16 s, turbolib 14.20 s: 0.70× |

### The Mach-O object model (7.2)

`turbonop-macho-object` reads an `MH_OBJECT` (32- and 64-bit, either byte
order) into load commands, segments and sections (bytes or zero-fill,
relocations decoded, scattered ones kept exactly), symbols, the string
table, the indirect symbol table, and link-edit data (data in code,
optimization hints), and writes it back: into the layout read while every
part keeps its size, else afresh as LLVM's `MachObjectWriter` lays an
object out (sections in order at their alignment, zero-fill last; each
section's relocations; link-edit data; the indirect symbols; the nlists in
LLVM's order, locals then external definitions then undefined names, each
sorted by name; the string table as `StringTableBuilder` builds it,
reversed-name order with shared suffixes). Edits: `retain_symbols`,
`sort_symbols`, `retain_sections`, `assign_addresses` and `move_sections`
(moving what the model holds with each section: symbols, scattered values,
the addresses x86-64, arm64, arm64_32, and i386 local relocations encode
in the bytes, `__eh_frame`'s PC-relative pointers that no relocation
covers, data in code, hints), and `cut` (ranges taken out of sections,
everything after them moved back: what folding uses).

| Check | Result |
|---|---|
| the corpus's objects, loose and in its archives (5,732 objects, 5.5 GiB, 13.3 s on 8 threads) | 5,732 written back byte for byte; 5,732 laid out afresh byte for byte (the writer reproduces LLVM's layout exactly); 5,732 moved and moved back to the same bytes; 8,925,796 local relocations' addresses decoded, all inside their target sections |
| 23 crafted objects (`crates/formats/macho-object/corpus/build.sh`: exceptions, TLS, `__mod_init_func`, weak definitions and imports, commons, i386 scattered relocations, armv7, arm64_32, assembly with `SIGNED_1/2/4`, local `SUBTRACTOR` pairs, `ADDEND`, `POINTER_TO_GOT`, hints, data in code, stubs, `N_INDR`, `.alt_entry`, another segment's zero-fill, `LC_LINKER_OPTION`, a universal file) | all round-trip and lay out afresh byte for byte |
| a section removed (`remove_section`: built with and without a 100-byte `__TEXT,__pad`), against LLVM's object built without it | 6 of 7 byte for byte (x86-64, arm64, i386, arm64_32, C and C++ with exceptions, TLS, and DWARF); armv7's instruction encodings aren't moved (reported) |

What the model can't see: arm32 and PowerPC instruction fields; references
inside a section the assembler resolved without a relocation (inside one
atom, which `cut` never splits).

### Folding (7.3, first version)

`turbonop_fold::macho` (run by the `ar` and `libtool` personalities with
`--fold=identical`, `--pull=preserve|relaxed`) folds identical weak
definitions. The module's documentation has the details; in short:

- **Units.** Mach-O has no COMDATs. A unit is an atom (a section of an
  object marked `MH_SUBSECTIONS_VIA_SYMBOLS`, split at its symbols that
  aren't `N_ALT_ENTRY`, as ld64 and ld64.lld split it) whose external
  symbols are all weak definitions (`N_WEAK_DEF`; inline functions,
  template instantiations, vtables, typeinfo, inline variables), with its
  relocations, its symbols, and its `__compact_unwind` entries. LLVM's
  arm64 section-start labels (`ltmp0` and on) belong to no atom: they stay
  at their section's start.
- **What folds** is ELF's rule: every definition of the names is a copy,
  and under `--pull=preserve` the first definer has no side effects
  (initializer or terminator sections, Objective-C lists,
  `LC_LINKER_OPTION`, sections of names the linkers give no meaning); a
  unit naming what stays in its member stays. **Initializer order**:
  initializers are `__mod_init_func` entries, atoms without symbols, which
  never move, so each member keeps its initializers in their order (and
  the members load in their order: below).
- **Rewriting.** The first copy goes to a synthesized member (one per set
  of givers and environment: CPU type and subtype, header flags,
  `LC_BUILD_VERSION`), with its unwind entries; alignment gaps between
  its units are atoms of their own (`l_turbolib.pad`), so every unit
  keeps its size. Givers lose the atoms (`Object::cut`: everything after
  them moves back, local relocations and `__eh_frame` pointers with it),
  their definitions become weak undefined references, and they gain a
  strong undefined reference to the synthesized member's anchor.
- **The alias technique's Mach-O counterpart, and a deviation from ELF's.**
  ld64.lld resolves an object's undefined names in symbol-table order as
  it loads the object, loading an archive member the moment a name meets
  a lazy archive symbol (recursively), and a weak reference loads one as a
  strong one does (`SymbolTable::addUndefined`). So, unlike ELF, a giver's
  weak references alone don't keep another library's copy out, and when a
  synthesized member loads decides the order in which other members do.
  What keeps both:
  - **the giver's undefined names in three runs**: its original ones, in
    LLVM's order (sorted), which load what they loaded, in the order they
    did; then its anchors (`~turbolib.anchor.`: `~` sorts after the names
    compilers make), which load the synthesized members, whose own
    undefined names were all the givers' too and are resolved by then;
    last the moved names, out of LLVM's order, defined by then. (A first
    version put the anchors first, as for ELF: the synthesized members'
    references then loaded members early, and four of `llvm-nm`'s 20
    initializers ran in another order on arm64.)
  - **synthesized members first** in the archive (ELF puts them last), so
    their index entries are the linker's before a member loaded while the
    archive's index is being read looks for its anchor.

  The anchor is a private external absolute (as ELF's is a hidden
  absolute), but ld64.lld makes no absolute symbol weak, so two libraries'
  anchors must never share a name: the name hashes the synthesized
  member's contents and the library's file name (`Options::salt`).
- **Debug information**: a giver's DWARF relocations into what it lost are
  removed and their places zeroed, so `dsymutil` (which reads the DWARF
  from the archive member through the program's debug map) finds no
  address for those DIEs and drops them, as it drops every copy but the
  linked one. (Since 7.4 the places are all ones, and synthesized members
  carry the DWARF: see below.) The members' dates: in deterministic mode a synthesized
  member is dated 0 with a unique name (`__turbolib.N.o`), which the debug
  map can name.
- **What stays**: units reaching a local symbol outside them (string
  literals' `l_.str`, `static` functions), units with an LSDA, an FDE, a
  local (section-relative) relocation, data in code, or hints, as `--stats`
  lists.

`turbonop_verify::macho` (`turbolib verify` on Darwin and BSD archives of
Mach-O objects) checks a folded library against the original, written
apart from the engine (its module documentation lists what it checks).

**Results** (the Mach-O corpus, WSL, 8 threads, at low priority):

| Check | Result |
|---|---|
| `--fold=identical --stats` of every library's objects, as `libtool -static` (32 libraries per architecture) | arm64: 387,904 units; 49,479 distinct ones with copies in two or more members; 36,371 folded (169,650 copies) into 5,183 synthesized members; 9.2 MB of units removed, 10.6 MB of synthesized members written. x86-64: 40 folded (345 copies) into 32 |
| ... what stayed (copies), arm64 | 173,616 with no copy in another member; 28,916 calling a name of their member that stays; 13,439 whose first definer has side effects (`--pull=preserve`: LLVM's `cl::opt` initializers); 1,759 whose names have other definitions; 522 whose other copies stayed; 2 reaching a local symbol outside them |
| ... x86-64 | 212,881 with an FDE: clang writes `__eh_frame` FDEs for every x86-64 function besides its compact unwind entries, and units with FDEs don't fold yet (the main thing left for x86-64) |
| size (`relib.py --options=--fold=identical`: every archive remade by its own commands) | the `llvm-ar` archives 1,536,552,752 bytes to 1,521,016,080 (1.0% smaller); the `libtool` ones (with the universal files) 3,073,107,040 to 3,042,033,696. The DWARF stays in the givers, and synthesized members carry symbol tables of their own |
| `turbolib verify` of the 128 single-architecture archives against the originals | 128 pass (the universal files aren't read by `verify`) |
| `relink.py`: `llvm-nm` and `llvm-readobj` for arm64 and x86-64 linked by ld64.lld against the original and the folded `llvm-ar` archives | 4 of 4 equivalent: the same original members loaded in the same order (527, 529, 403, 404; plus 5,066, 31, 4,409, 24 synthesized ones), the same symbols but the anchors, every function's code the same (170,231, 171,038, 162,460, 163,266, branch targets by name), the same initializers in the same order (20, 20, 24, 24), the same section sizes but arm64 `llvm-readobj`'s `__unwind_info` (0x322d8 to 0x312cc bytes) |
| `fixture.py`: two libraries sharing inline functions, templates, vtables, a static local, TLS, exceptions, and initializers, arm64 and x86-64, `-O0` and `-O2`, both library orders | 16 of 16 checks pass (`verify` of each folded library, and each link equivalent as above, initializers in order) |
| determinism: `libLLVMSupport.a` and `libLLVMCore.a` folded with `-j 1`, `-j 3`, `-j 8` | the same bytes |
| speed (`relib.py --time --options=--fold=identical`: all 160 archives' commands, best of 2) | the real tools 22.41 s, turbolib 101.94 s: 4.55× (0.70× without folding); not profiled |

What `relink.py` can't prove: nothing was run; `adrp`/`add` pairs aren't
followed to what they address (their shapes are compared), nor data's
bytes, nor DWARF (none is moved); and the links are ld64.lld's alone.

What was found on the way:

- **The order of a member's undefined names** decides the order members
  load in with ld64.lld. The first version put the anchors first (as
  for ELF): every original member still loaded, but four of `llvm-nm`'s
  20 initializers (`libLLVMCore.a`'s) ran in another order on arm64.
  Anchors after the original names, but before the moved ones, fixed the
  order; four extra members (`DwarfTransformer.cpp.o` and three it
  pulled) then loaded, because a synthesized member's own reference to a
  name moved into another synthesized member was looked up before the
  anchor that loads that one. Synthesized members now order their names
  the same way.
- **Weak references stay weak**: a synthesized member's reference to a
  name every copy referred to weakly (`__gthread`'s `pthread_*`) is weak
  too, or the program's import isn't.
- **Alignment gaps**: data units of mixed alignment left gaps in a
  synthesized member that the unit before took into its atom; gaps are
  atoms of their own now.
- **A range list's end at a section's end** (`__debug_ranges`) was taken
  for the next section's first atom, and its relocation removed when that
  atom moved: an address is now looked for in the section its relocation
  names.

### FDEs, LSDAs, DWARF, and `compare-images` (7.4)

- **x86-64's FDEs and LSDAs move with their units.** A unit's `__eh_frame`
  FDEs (clang writes one for every x86-64 function; LLVM writes their
  pointers PC-relative with no relocation) go into the synthesized member
  after copies of their CIEs (personalities by name, deduplicated), their
  pointers set to where the atoms are; its LSDAs (the atoms of
  `__gcc_except_tab` only its unwind entries and FDEs name) move as atoms
  of the unit, the unwind entries' LSDA fields local relocations to them.
  Givers lose the records and the atoms. A unit's identity covers its
  FDEs (pointers by atom and offset) and LSDAs (their `GCC_except_tableN`
  labels, numbered per object, left out). **A function's alignment
  padding** (the bytes after its end by its one unwind entry, up to the
  next atom, with no relocation or symbol in them) isn't part of its
  identity either: x86-64's code is aligned to 16 bytes, so a copy that
  ends its section and one that doesn't differ only there; synthesized
  members pad code with NOPs. `verify` hashes FDEs and LSDAs with the
  functions they are for, and leaves padding out the same way.
- **Givers' DWARF moves with their code.** What the cut can't move (what
  no relocation holds) is adjusted (`turbonop_fold::macho::adjust`): the
  line program is decoded into rows, the rows of code cut out dropped, the
  rest moved, and encoded again as LLVM's `MCDwarfLineAddr` encodes it
  (`turbonop_fold::dwarf::line`: the corpus's 1,393 line programs decode
  and encode back to the same bytes); DWARF 4's range and location list
  entries move to where their addresses went (a range of lost code
  becomes empty), relocated ones keeping their relocations (the cut would
  have taken a range's end at the start of what it cut for the start of
  what was cut); the unit's `DW_AT_low_pc` stays its section's start (it
  was tombstoned with the atom there before), its `DW_AT_high_pc` shrinks.
  A removed debug relocation's place is all ones (0 is an address of the
  object: `llvm-dwarfdump --verify` reported overlapping ranges). One unit
  of DWARF 2 to 4 per object is handled; anything else (DWARF 5's lists,
  `__debug_aranges`, several units) keeps the giver whole. **This fixed a
  7.3 bug**: givers' line rows, list entries, and unit lengths after a cut
  named the old addresses.
- **Synthesized members carry their units' DWARF**, built by the module
  that builds ELF's (`turbonop_fold::dwarf`, now the crate's), which sees a
  Mach-O member's code as its atoms: an address is its section and offset,
  located in an atom; the moved functions' DIEs, their lists, and each
  moved atom's rows as a sequence of their own (clang writes one sequence
  per section). As LLVM writes Mach-O DWARF: addresses of moved code are
  local `UNSIGNED` relocations, offsets into other debug sections plain
  values, anything else the tombstone. **Deviation from ELF**: dsymutil
  keeps only what the linked code's DIEs reach, so with classes as
  declarations a class only folded functions named had no definition in
  the dSYM (1,391 of 122,114 in `llvm-nm`'s); a Mach-O synthesized unit
  keeps whole the classes its code names (through pointers, references,
  qualifiers, typedefs, arrays) and those its functions are members of,
  what those name declarations again. Apple's accelerator tables aren't
  written (dsymutil makes its own; an lldb reading the objects without a
  dSYM indexes them itself).
- **`turbolib compare-images` for Mach-O** (`turbonop_verify::macho_image`):
  functions and data by content (fixups by what they name, padding left
  out, arm64's `adrp` completing every later load and store of its page,
  data's PC-relative words such as an LSDA's type table), symbols,
  initializers in order, unwind information per function (compact
  encodings, personalities, LSDAs, DWARF-mode FDEs), imports, and the DWARF
  of each image's dSYM by the ELF comparison's rules, now generic, with
  cross-unit references (dsymutil's type uniquing) resolved by identity
  and a type's members compared as a set. Mach-O symbols have no sizes: a
  datum's extent runs to the next symbol, so data are compared over the
  shorter of the two extents, and an address past a datum's bound (that
  shorter extent) is named by its contents. `relink.py` and `fixture.py`
  use it (with `dsymutil --linker=classic --verify-dwarf=none`: Apple's
  dsymutil uses the classic linker; this LLVM's defaults to the parallel
  one, and clang 21's `-O2` objects fail its input verification, after
  which it writes nothing).

| Check | Result |
|---|---|
| `--fold=identical --stats` (as `libtool -static`, 32 libraries per architecture) | x86-64: 36,348 folded (169,617 copies) into 5,199 synthesized members (7.3: 40, 345 copies); arm64 as in 7.3, 36,371 (169,650) into 5,183 |
| ... DWARF | every folded unit's moved (36,348 and 36,371; 0 dropped), 36,345 and 36,369 root DIEs; 88.0 and 88.7 MB of synthesized members' DWARF (14.9 MB with classes as declarations, 6.07 GB with every class whole) |
| size (`relib.py --options=--fold=identical`) | the `llvm-ar` archives 1,536,552,752 bytes to 1,676,251,064 (9.1% larger); the `libtool` ones 3,073,107,040 to 3,352,503,664. The givers keep their DWARF (the copies' DIEs, tombstoned), and synthesized members now carry DWARF of their own |
| line programs decoded and encoded again (`macho_lines` example, 1,402 objects) | 1,393 of 1,393 with a line table the same bytes |
| `turbolib verify` of the 128 single-architecture archives | 128 pass (FDEs and LSDAs with their functions; givers' line rows, list entries, and unit lengths by what they name) |
| `fixture.py` (now with exceptions folding on x86-64, and `compare-images` with dSYMs) | 16 of 16 |
| `relink.py`: `llvm-nm` and `llvm-readobj`, arm64 and x86-64, ld64.lld, `compare-images` with dSYMs | the original members loaded in the original order in all 4 (527, 529, 403, 404; plus 5,066, 5,088, 4,409, 4,430 synthesized). Not equivalent in every category; what's left, per program (`llvm-nm` arm64, x86-64; `llvm-readobj` arm64, x86-64), of 177,298 / 178,153 / 164,322 / 165,175 functions and data and 170,421 / 171,228 / 162,561 / 163,368 live subprograms: **code** whose operand names other data 2, 3, 24, 13, and data whose bytes differ 2, 1 (size), 1, 0: references past a local constant to the data that follows it, which a symbol with no size can't bound (`DefaultFloatSpecs`, `PoisonMaskElem`, `CastResults`; the bytes are the same, the bound comparison cuts them at another place); **unwind information** 1, 0, 2, 0 (`RTTIRoot::anchor` has encoding 0 in the original's `__unwind_info` and a frameless one in the folded; not yet explained); **subprograms whose DIEs differ** 9, 9, 1, 1, spot-checked as another unit's copy (`SmallString<0>::~SmallString`'s `DW_AT_decl_file` is `XCOFF.h` in `ArchiveWriter.cpp`'s copy, `SmallString.h` in the linked one's, as for ELF in 6.6; not all proven: there is no `witness.py` for Mach-O yet); **type definitions** only in the original 9, 9, 11, 11 (of 127,734 / 127,967 / 121,355 / 121,578 distinct), and 38 / 114, 38 / 115, 21 / 64, 21 / 66 with contents only in one (which units' versions of a class template dsymutil keeps follows which units' DWARF is in the link). Expected: 8,162 / 8,143 / 3,617 / 3,608 subprograms described by another unit's copy (the same DIEs and rows), 58 type definitions only in the folded (a synthesized unit built from another member's defines what the linked copy's declared), 57 / 57 / 25 / 26 with more members, the discarded copies' LSDAs the linker keeps (only in the original), `__eh_frame` larger by a CIE per synthesized member (ld64.lld keeps each object's CIEs, 0x20d90 for 0x3198 bytes in `llvm-nm` x86-64; the FDEs themselves are dropped for functions with compact unwind) |
| speed | `relib.py --time --options=--fold=identical` (all 160 archives' commands, best of 2, the machine otherwise idle): the real tools 4.96 s, turbolib 17.63 s, 3.55× (3.93× before this section's last changes: definitions and users found once, by hash; a worklist for what stays; no cloning while sorting synthesized members); without folding 0.82×. 7.3's 22.41 s for the real tools was measured on a busy machine. `libLLVMCore.a` (arm64, 86 members, 104,470 units): reading 0.03 s, choosing 0.10 s, rewriting givers and reading DWARF 0.33 s (2.7 s on 8 threads), synthesizing 0.15 s, against 0.10 s for `llvm-libtool-darwin`. The same bytes with `-j 1`, `-j 3`, `-j 8` (`libLLVMCore.a` arm64, `libLLVMSupport.a` x86-64) and from run to run |

What it can't prove: nothing was run (lldb isn't in this LLVM build:
no debugger sessions were compared); the links are ld64.lld's alone; the
dSYMs are this LLVM's dsymutil's.

### Givers' DWARF pruned (7.5)

- **Givers lose the moved copies' DIEs** (`turbonop_fold::macho::prune`,
  after the cut and `adjust`), instead of keeping them tombstoned. A DIE
  below the unit or a namespace is an entity, kept or removed whole.
  Dead: a subprogram or variable entity some of whose debug relocations
  named lost code (tombstoned, or naming a moved name: x86-64's are
  external) and none anything kept, without `DW_AT_const_value`, whose
  lists are all empty after `adjust`. Also removed: what only the dead
  reach (references and location expressions, transitively) when it is an
  unnamed type (pointers, qualifiers, arrays, subroutine types) or a
  subprogram without an address (declarations, abstract instances), and
  namespaces left empty. Everything else stays, and everything a kept
  entity refers to (a dead subprogram a kept call site names stays,
  tombstoned). `__debug_info` loses the subtrees (`Object::cut`), every
  kept reference, string offset, and list offset is set again,
  `__debug_str` loses the strings nothing kept names,
  `__debug_ranges`/`__debug_loc` the lists nothing kept names and the
  entries the cut emptied, and Apple's four accelerator tables the entries
  of removed DIEs (written again as LLVM's `AppleAccelTableWriter` does:
  the corpus's 5,800 tables read and written back byte for byte by the
  `macho_apple` example; with its bucket count, unless that would grow
  the table: then the original's). Anything not handled (DWARF 5, several
  units, references in expressions, `DW_FORM_ref_udata`, sections not
  known) keeps the giver's DWARF as 7.4 left it, counted (none in the
  corpus).
- **Named types stay.** The first version also removed named types only
  the dead reached; compared with 7.4's folded links, the dSYMs then
  lacked 2 type definitions per link (`std::ratio<1000000000, 1>` and
  `std::ratio<1000000, 1>`; and on x86-64 `llvm-readobj` one class with
  other contents): a moved `std::chrono` function's class names them as
  template arguments, its synthesized member holds them as declarations,
  and dsymutil (classic, ODR uniquing) had kept a giver's definition that
  nothing in the dSYM referred to. Why it keeps it wasn't established
  then (7.6 found it: an earlier `using namespace std::chrono` keeps the
  namespace whole); named types were kept since.
- **`verify`** checks pruned DWARF apart from the engine
  (`turbonop_verify::macho::pruned`): the two DIE trees aligned, only
  whole subtrees missing below the unit and namespaces; every kept DIE the
  original's (abbreviation, bytes, relocations, strings by content, lists
  by what they name, references to the matches of the original's
  targets, `DW_AT_sibling` to its subtree's end); every removed one dead
  code's, an unnamed type or address-less subprogram without
  relocations, or a namespace of such; the accelerator tables the
  original's entries of kept DIEs, each hash its name's, each findable
  through its bucket.
- **Sharing class definitions between synthesized members** (a class
  whole in one, a declaration in the others) wasn't done: it isn't correct
  for dsymutil's pruning. dsymutil keeps a definition only through the
  live DIEs of its own object (as far as seen here: see the previous
  point), and a synthesized member is loaded (by a name of the index, not
  only through givers' anchors) and dead-stripped independently of the
  others: the one holding the definition may be absent, or have no live
  code, while another's code that names the class is live. Even "the
  member holding the definition has a function this one's code calls"
  fails when that function's copy is coalesced with another library's.
  Each synthesized member stays self-sufficient.

| Check | Result |
|---|---|
| `--fold=identical --stats` (as `libtool -static`, 32 libraries per architecture) | every giver's DWARF pruned (675 arm64, 667 x86-64; none kept as they were): 169,502 / 169,450 dead DIEs removed (134 / 134 kept, named by kept DIEs), 43,579 / 43,337 others only they reached; 24.8 MB of debug sections per architecture (28.2 MB when named types went too) |
| size (`relib.py --options=--fold=identical`) | the `llvm-ar` archives 1,536,552,752 bytes to 1,626,635,896 (5.9% larger; 7.4: 1,676,251,064, 9.1%; 7.3, without DWARF in synthesized members: 1,521,016,080, 1.0% smaller); the `libtool` ones 3,073,107,040 to 3,253,273,328 (7.4: 3,352,503,664). With named types removed too: 1,619,914,792 |
| what synthesized members' DWARF is (`libLLVMSupport.a` arm64: 706 members, 6.37 MB) | `__debug_str` 3.65 MB, `__debug_info` 1.92 MB (of it whole class definitions 1.48 MB: 2,325 of 1,115 distinct classes, 0.37 MB once each), `__debug_line` 0.41 MB, `__debug_abbrev` 0.32 MB; per architecture 88.7 / 88.0 MB, as in 7.4 |
| accelerator tables (`macho_apple` example, the corpus's objects) | 5,800 of 5,800 read and written again byte for byte |
| `turbolib verify` of the 128 single-architecture archives | 128 pass (pruned DWARF checked as above) |
| `fixture.py` | 16 of 16 |
| `relink.py` (4 links, ld64.lld, dSYMs by `dsymutil --linker=classic`) | against 7.4's folded links (`compare-images` of each folded image with its dSYM): 4 of 4 equivalent, no failures (the same functions, data, symbols, initializers, unwind information, imports, live subprograms, variables, and 127,832 / 128,067 / 121,429 / 121,654 type definitions). Against the originals: the original members loaded in order, and the same categories and counts as 7.4 (type definitions only in the original 9, 9, 11, 11) |
| determinism | `libLLVMCore.a` arm64, `libLLVMSupport.a` both, `libLLVMObject.a` arm64 with `-j 1`, `-j 3`, `-j 8`: the same bytes |
| ELF | `libLLVMSupport.a`, `libLLVMCore.a`, `libLLVMObject.a` (the clang corpus) folded by 7.4's and this build: the same bytes (no shared code changed) |
| prune's tests | a unit emitted whole and without what goes, compared byte for byte (`__debug_info`, `__debug_str`, relocations, the accelerator table); twins, offsets, bucket counts, NUL search |
| speed (CPU, `-j 1`, 7.4's build and this one alternating, 3 runs each; the machine shared with another agent's ELF runs, so only the ratio is meaningful) | `libLLVMCore.a` arm64 +23%, `libLLVMSupport.a` x86-64 +26%: slower. Pruning reads every giver's DWARF (7.4 read only survivors'); single-threaded on `libLLVMCore.a` it is 0.69 s of 1.56 s of giver rewriting (walking the DIEs 0.18 s; the rest lookups, strings, lists, accelerator tables, a second cut). Not yet won back |

### What dsymutil keeps, and synthesized members' classes from their givers (7.6)

**dsymutil's rules** (the classic linker, Apple's; read in LLVM's
`llvm/lib/DWARFLinker/Classic/DWARFLinker.cpp` and
`DWARFLinkerDeclContext.cpp` at the corpus's LLVM, and checked with this
LLVM's `dsymutil --linker=classic` on small programs):

- **Per object, from its own roots.** `lookForDIEsToKeep` walks every DIE
  of each unit in file order from the unit's DIE. Roots (`shouldKeepDIE`):
  subprograms and labels whose `DW_AT_low_pc` relocation is in the debug
  map (`shouldKeepSubprogramDIE`); variables and constants outside
  functions with `DW_AT_const_value`, or with a location relocated into
  the debug map (`shouldKeepVariableDIE`); base types,
  `DW_TAG_imported_module`, `DW_TAG_imported_declaration`, and
  `DW_TAG_imported_unit`, always. A kept DIE keeps its children (the keep
  flag passes down), its ancestors (`lookForParentDIEsToKeep`), all the
  children of an ancestor that is a class, structure, union, enumeration,
  subprogram, lexical block, subroutine type, or common block
  (`dieNeedsChildrenToBeMeaningful`), and what every reference attribute
  but `DW_AT_sibling` names, with its children (`lookForRefDIEsToKeep`). A
  DIE already kept isn't walked again. References are followed only
  within the object (its units): an object's definition is in the dSYM
  only if that object's own roots reach it. A definition is not kept
  because it is "found in a live object".
- **ODR uniquing, per link.** Each named type, namespace, member, and
  external subprogram in named scopes has a declaration context
  (`getChildDeclContext`: the parent's, the tag, the linkage name or name,
  and but for namespaces the byte size, the resolved `DW_AT_decl_file`,
  and `DW_AT_decl_line`; a declaration has no size or file, so its
  context isn't its definition's). The first complete DIE of a context
  kept, in debug-map order, becomes its canonical one
  (`markODRCanonicalDie`; complete: not a declaration, and no member
  reaching, through members, pointers, references, and typedefs, an
  incomplete DIE: `updateChildIncompleteness`,
  `updateRefIncompleteness`). An ODR reference (`DW_AT_type`,
  `DW_AT_containing_type`, `DW_AT_specification`, `DW_AT_abstract_origin`,
  `DW_AT_import`, `DW_AT_LLVM_alloc_type`; `isODRAttribute`) to a context
  that has one isn't followed, and is pointed at it when cloned
  (`cloneDieReferenceAttribute`). An incomplete definition is never
  canonical, so it is kept from every object that reaches it (the
  original `llvm-nm` arm64 dSYM has 18 definitions of `llvm::StringRef`).
- **Why a giver's class definition nothing referred to was in a dSYM
  (7.5).** An always-kept `DW_TAG_imported_module` keeps the namespace it
  names through a dependency walk, with every child, unless something
  had kept the namespace before it in file order. In
  `ArchiveWriter.cpp.o`, `llvm/Support/Chrono.h`'s two `using namespace
  std::chrono` (at 0xeb81 and 0xec2e, inside namespace `llvm`) come before
  the `std::chrono` namespace DIE (0x3786e): in a link where nothing of
  the object reaches into `std::chrono` first, every duration of it is in
  the dSYM, with the `std::ratio` definitions they name. A small program
  (`namespace ns` with classes only dead code names, and `using namespace
  ns`) shows dsymutil keeping all of them; with the namespace's DIE kept
  before the import (a child of it an always-kept import), none.

**What follows for folding:**

- **Givers may lose named types no DIE dsymutil can keep reaches.** A
  giver's named type only dead code reaches (after 7.5's pruning) can be
  kept by dsymutil only as a child of a namespace an import names (or a
  descendant of one). `prune` now removes named types too, but those
  inside a namespace something refers to; `verify` checks the same rule.
  That is no change to any dSYM: a removed type was one no kept DIE
  reached. (The dead entities inside an imported namespace went in 7.5
  already: in a link that keeps such a namespace whole, the original's
  dSYM had their DIEs, without addresses, and what only they named.)
- **With `--debug-types=shared`, a synthesized member holds as
  declarations the classes its givers define** (the default,
  `--debug-types=complete`, keeps 7.5's whole classes: see the next
  point). A synthesized member is loaded whenever one of its givers is
  (by the giver's anchor). `turbonop_fold::macho::reach` works out, for
  each giver after its cut, the class definitions it puts in the dSYM of
  every link that loads it with all its code (no `-dead_strip`): its
  roots (DIEs whose relocations still name what it holds), what dsymutil
  keeps from them by the rules above, and the target of each ODR
  reference to a DIE that may have a declaration context (dsymutil keeps
  that from this object, or an earlier object's definition is the
  canonical one), but nothing beyond such a target (another object's
  version may be the dSYM's, and what that version names may differ),
  nor a namespace's other children. Keys are dsymutil's context keys
  (`turbonop_fold::dwarf::key`: tag, name, byte size, file by directory
  and name, line, and the parent's key). A class the moved code names,
  which 7.4 kept whole, is a declaration when every giver of the
  synthesized member defines it so.
- **What isn't kept: the versions.** dsymutil keeps one version of a
  class template specialization per object that reaches it while it is
  incomplete, and the first complete one, and which ones are complete
  depends on what earlier objects made canonical (a member reaching a
  type whose context is already canonical isn't followed, so doesn't
  make it incomplete). Any change to what a synthesized member holds
  changes that cascade. In the corpus, `llvm::gsym::SourceLocation` had
  `FunctionInfo.cpp.o`'s definition in the original's dSYM and
  `Symbolize.cpp.o`'s (an earlier object's) in the folded one's, though
  both links load the objects in the same order and the giver's
  surviving `FunctionInfo::lookup` still names it: `Symbolize.cpp.o`'s
  version was incomplete in the original link and complete in the folded
  one. 7.4's whole classes changed versions this way too (the classes
  "with contents only in" one image of 7.4's table).
- **Strings named once are inline** (`DW_FORM_string`) in a synthesized
  member: 4 bytes smaller each, and no change to the dSYM (dsymutil
  writes every string into its own table).

Tried and not kept:

| Variant | `llvm-ar` archives | `llvm-nm` arm64: types only in the original / with contents only in the original / only in the folded |
|---|---|---|
| 7.5 (classes the code names whole) | 1,626,635,896 | 9 / 38 / 114 |
| reach followed to the end (through the classes ODR references name) | 1,476,852,664 | 21 / 516 / 4 |
| reach to ODR targets and their ancestors | 1,522,627,136 | 4 / 134 / 52 |
| reach to ODR targets (kept) | 1,525,594,296 | 4 / 133 / 54 |
| ... and a class declared only if every giver's has its members | 1,597,908,384 | 9 / 112 / 112 |
| ... and roots only strong definitions (a weak one's copy may be another object's) | 1,573,510,800 | 9 / 69 / 92 |
| ... both | 1,610,137,096 | 9 / 58 / 114 |

None of these reproduces the original's versions: the variants trade
categories, and the ones that keep more classes whole (larger) do no
better than 7.5 overall. The variant kept, as `--debug-types=shared`,
loses the fewest definitions. The default stays 7.5's classes
(`--debug-types=complete`), with the two changes that leave every dSYM as
it was: givers' named types pruned and strings inline. The relinks show
it: with the default, `compare-images` reports exactly 7.5's categories
and counts in all 4 links (expected ones too).

| Check | Result |
|---|---|
| size (`relib.py --options=--fold=identical`, and with `--debug-types=shared`) | the `llvm-ar` archives 1,536,552,752 bytes to 1,613,682,840 by default (5.0% larger; 7.5: 1,626,635,896, 5.9%) and 1,521,951,128 with `shared` (0.95% smaller; 1,525,594,296 before strings went inline); the `libtool` ones (with the universal files) 3,073,107,040 to 3,227,367,216 and 3,043,903,792 (7.5: 3,253,273,328). Synthesized members' DWARF, arm64: 84.1 MB by default (`__debug_info` 74.6, `__debug_line` 3.9, `__debug_abbrev` 3.3, `__debug_str` 1.7, `__debug_ranges` 0.7), 40.0 MB with `shared` (`__debug_str` 23.8, `__debug_info` 10.4, `__debug_line` 3.2, `__debug_abbrev` 1.9, `__debug_ranges` 0.7) before strings went inline; 7.5: 88.7 |
| `relink.py` (4 links, against the originals) | by default: exactly 7.5's categories and counts, expected ones too, in all 4 links. With `shared`: the original members loaded in order; code, data, unwind information, imports, and subprograms as 7.5 (the same categories and counts); type definitions only in the original 4, 4, 6, 6 (7.5: 9, 9, 11, 11); with contents only in the original 133, 132, 71, 70 (7.5: 38, 38, 21, 21), only in the folded 54, 54, 30, 31 (7.5: 114, 115, 64, 66) |
| `relink.py --dead-strip` (new: `-dead_strip`, as a Mac release build links) | against the originals linked the same way, by default: exactly 7.5's categories and counts in all 4 links. With `shared` (the build before strings went inline): the original members loaded in order; type definitions only in the original 95, 86, 94, 85 (7.5: 150, 141, 98, 89), with contents only in the original 125, 124, 100, 100 (7.5: 49, 49, 37, 38), only in the folded 57, 57, 41, 42 (7.5: 92, 93, 59, 61); code and subprograms as 7.5. Both settings lose definitions under `-dead_strip`: a giver's code that names a class may be stripped while the moved code stays |
| `turbolib verify` of the 128 single-architecture archives | 128 pass |
| `fixture.py` | 16 of 16 |
| determinism | `libLLVMCore.a` arm64, `libLLVMSupport.a` x86-64, `libLLVMObject.a` arm64 (as `libtool -static`) with `-j 1`, `-j 3`, `-j 8`: the same bytes |
| ELF | `libLLVMSupport.a`, `libLLVMCore.a`, `libLLVMObject.a` of the clang and gcc corpora folded by 7.5's build and this one: the same bytes |
| speed (by default; CPU, user + system, `-j 1`, as `libtool -static`, 7.5's build and this one alternating, 3 runs each, the first left out) | `libLLVMCore.a` arm64 10.90 and 11.00 s against 10.73 and 10.44 s (3% more); `libLLVMSupport.a` x86-64 2.74 and 2.60 s against 2.75 and 2.48 s (2% more, within the runs' spread). The named types prune removes, and counting a synthesized member's strings, cost a little; not profiled |
| speed (with `shared`; CPU, user + system, `-j 1`, as `libtool -static`, 7.5's build and this one alternating, 3 runs each, the first of each cold and left out; the machine's load about 4 from other work, so only the ratio is meaningful) | `libLLVMCore.a` arm64 9.30 and 9.20 s against 10.79 and 9.95 s (7% less: synthesized members' DWARF is smaller to build, which pays for walking every giver's DWARF once more in `reach`); `libLLVMSupport.a` x86-64 2.54 and 2.50 s against 2.43 and 2.41 s (4% more) |

What it can't prove: with `shared`, the giver-supplied classes hold only for links
without `-dead_strip` that load the synthesized member through a giver
(a giver's code that names a class may be stripped while the moved code
stays; a synthesized member loaded by its own name, with no giver, has
only declarations); the versions of classes in a dSYM aren't preserved;
the dSYMs are this LLVM's dsymutil's, and Apple's wasn't run.


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
- (Written since: the import descriptor objects of `/DEF` import
  libraries, phase 1.2; the GNU and BSD archive writers and an ELF object
  model, phase 6: `turbonop_archive::unix`, `turbonop-elf-object`.)
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
     unit) an undefined external, and give the name a second symbol
     record, an alias weak external whose default is `__turbolib$alias`
     ([the alias technique](#the-alias-technique)).
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

### Folding a folded library

A build that makes libraries of libraries (Buck2's) gives turbolib its own
output as input: synthesized members (`turbolib<n>.obj`), a
precompiled-types member (`turbolib-types-<hash>.obj`), and members whose
moved names are alias weak externals. The engine reads them as it reads
any object, so:

- **Folded members fold again.** An input's synthesized member is an
  object like another: its units fold with other members' copies of them,
  and it gives them up as an original member does (its names become
  aliases, and it refers to the new synthesized member's anchor). A link
  that loads a member of the input still loads, through that member's
  anchor, the input's synthesized member, and through its anchor the new
  one: the chain keeps the alias technique's guarantees (each loaded
  member's moved names are defined by an alias until the anchors bring
  their strong definitions; the index names the original definers).
- **Synthesized members of several inputs merge** the same way: two
  folded libraries' synthesized members holding copies of one inline
  function are two members holding identical units, and the copy moves to
  one new synthesized member that both anchor.
- **Precompiled types.** Where the library holds a precompiled-types
  member turbolib made, the members of its machine keep their types as
  they are, and no other such member is made for that machine: its users'
  records are shared already. (Absorbed and shared again, as a compiler
  PCH's users are, their records went into a new precompiled-types member
  beside the inputs', which their other users keep: `LLVMSupport.lib` and
  `LLVMCore.lib`, each folded, folded together came out 1.0% larger than
  `lib.exe`'s combination of them, and 2.5% larger again folded alone.
  And what the other members shared among themselves made yet another
  member at each fold.) So objects added to a folded library don't share
  types; their COMDATs fold as any others. Should a member become a user
  of an input's types member, `verify` accepts its new reference to that
  member's PCH symbol when the member's `LF_PRECOMP` names it and it
  defines its PCH symbol alone and has no side effects (so that loading
  it resolves no other name).
- **Names.** New synthesized members skip the names input members have
  (`turbolib0.obj` is taken by the input's): two members of one name would
  be one too many, since `lib.exe` replaces one with the other when it
  archives the library again. A first fold's names are as before.
- **`__turbolib$alias`.** Every synthesized member defines it, as the same
  absolute 0. `lib.exe` warns LNK4006 for the second definition when it
  combines a folded library with anything else (and fails under `/WX`,
  LNK1218: measured with 14.52); turbolib's `lib` personality doesn't warn
  about it. `lib.exe` also indexes it; turbolib never does, and `verify`
  doesn't count that entry: a member that names it, as its aliases'
  default, names an anchor too, whose member defines it.

`tools/turbolib/refold/run.py` folds once, then folds again alone, with
more objects, two folded libraries together, and a third time, with
`--strict`; checks each with `verify` against `lib.exe`'s library of the
same inputs, for members of one name; and links a program against each and
against `lib.exe`'s library with both linkers, under `/OPT:REF` and
`/OPT:NOREF`, for `compare-images` and the programs' output.

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

As written in phase 4 (`turbonop_fold::rewrite::locals`), what `copy`
copies, and what it refuses:

- **Local units**: a COMDAT whose symbols are all local (`static inline`
  functions, anonymous namespaces' inline functions, and their
  `.pdata`, `.xdata`, and `.debug$S`), copied whole. Its original goes
  from its member too when only units that move refer to it ("absorbed");
  when anything that stays refers to it (code outside COMDATs, a unit that
  stays, an auxiliary record, the table of code that stays) it stays, and
  the synthesized member has its own copy. A local unit nothing refers
  to never moves: in a registration section (`.rtc$IMZ`'s
  `_RTC_InitBase`, one per object under `/RTC1`), the number of copies is
  the number of calls.
- **Pieces**: the bytes of a read-only data section outside COMDATs
  (`.rdata`) from the local symbol to the next symbol of the section (or
  its end), with their relocations: `$SG` string literals, internal
  constant tables. The original always stays.
- **Never**: writable data (`.data`, `.bss`: a copy would be another
  variable, which is more than address identity); registration sections
  (`.CRT$*`, `.rtc$*`, `.tls*`, `.00cfg`); code outside COMDATs (without
  `/Gy`, and clang-cl's `static` functions, each in a section of its own
  that isn't a COMDAT), whose symbols don't bound functions; a place named
  by a section symbol; anything a side table names; a local symbol of a
  unit that has external symbols too; a reference whose in-place addend
  points outside the piece (read for x64 and x86 relocations; other
  machines' relocations to pieces aren't read, so their units stay); and
  a unit whose debug information refers to a local outside it.
- **Identity** is structural: each local reached is named by its place
  in the order the unit's code names them, followed by the contents of
  everything reached, numbered breadth first. A piece's contents leave
  out the zeros at its end (alignment padding before the next symbol,
  which depends on where the compiler put it): `{1, 2, 0}` and `{1, 2}`
  are the same, as two definitions of an inline function that name them
  can be the same function only if they are, by the ODR.
- **What a program can tell**: the addresses of the copied locals (two
  functions that return `&k` from one translation unit can return two
  addresses, when one of them moved and `k` stayed too). C++ leaves which
  copy of an inline function a link keeps to the linker, and an inline
  function that names an internal-linkage entity in several translation
  units is an ODR violation unless only the entity's value is used, so
  only a program already depending on that choice can tell.
- **`merge`** makes one copy of each local of given contents (its
  structural hash, as the identity) in a synthesized member: copies from
  different members whose bytes are the same, as string pooling (`/GF`)
  and ICF do. Locals in different synthesized members are never merged.
- Copied local units keep their debug information (their `.debug$S`,
  renumbered as a folded unit's); a copied piece has none (a constant
  table's `S_LDATA32` stays in its member's module). Per-object side-table
  entries naming a local unit (clang-cl's and `/Gy-`'s `.gfids$y`) are
  copied with it. Duplicate local COMDAT names in one synthesized member
  (two members' copies of one `static` function) get a `$turbolib<n>`
  suffix: one object can't hold two COMDATs of one name.
- Not written: giving a local unit that stays an external name in the
  synthesized member, so that the members that keep referring to it could
  share one copy (it would also fold the local units only code that stays
  reaches). Copying alone needs no names no compiler made.

## Conservativeness

Each axis is its own option, so a team can say exactly what it gives up.
The defaults are the conservative choice on every axis.

### `--fold`: which copies count as the same

| Mode | Folds |
|---|---|
| `none` | Nothing. turbolib is just `lib.exe` (phase 1). |
| `identical` (default) | Units whose identity hashes are equal. |
| `select-any` | Units that the linker could legally substitute for each other, by selection type. See the table below. |

How `select-any` treats each selection type (as written in phase 4: the
copies of one name are those with the same external names, selection, and
environment key):

| Selection | In `select-any` |
|---|---|
| `NODUPLICATES` | Never folded (not a fold unit in any mode). |
| `ANY` | Folded. The survivor is chosen by `--pick`: `first` in member order (the default, the copy the archive index names), or `largest` (the largest COMDAT section, the first of those). |
| `SAME_SIZE` | Folded when the COMDAT sections' sizes agree. When they don't, the copies stay (identical ones still fold as `identical` folds them), and `--stats` lists the name and its copies, because the linker would fail only if two disagreeing copies reached one link. |
| `EXACT_MATCH` | Folded when the checksums and sizes agree. Otherwise, as for `SAME_SIZE`. |
| `LARGEST` | Folded, keeping the largest, whatever `--pick` says. The linker keeps the largest among the copies in a given link, so this can choose a copy from a member that link never loads. That is allowed by the semantics, but it is a different choice. |
| `ASSOCIATIVE` | Always follows its leader. |
| Mixed | Copies of one name with different selections never fold together. |

In both `identical` and `select-any`, only units with the same
environment key fold together.

**What `select-any` gives up, exactly.** A link that loads members A and B
both holding `ANY` copies of F keeps one of them, the first it reads;
turbolib keeps one for every link, the survivor. When the copies' code
differs, a link that loads only B now gets A's copy (through the
synthesized member B anchors), which no link of the original library
could have given it: `ANY` says the copies may stand for each other, not
that the linker would have chosen that one. Copies whose code differs are
an ODR violation in C++ (unless they differ only by compiler options in
other environments, which `select-any` never mixes), so a program that
can tell is already one whose behavior depends on link order;
`--report odr` lists them. Under `--pull=preserve`, `select-any` folds a
name's differing copies only if every definition of the name in the
library is such a copy (no other selection, no definition outside a
COMDAT unit, no member passed through), and all of them refer to the same
external names (by relocations and side-table entries, through the locals
they reach too). Each member that gives up a copy keeps that copy's
references, so a synthesized member then needs nothing its members didn't,
and the same members load as before (`turbolib verify` checks this
statically for every synthesized member under `preserve`: the names it
refers to are names each member anchoring it referred to or defined).
The member the index names must still have no side effects, as for
`identical`. Copies that refer to other names (an inline function whose
`#if` calls another function in each translation unit) stay, reported as
"copies with other code refer to other names".

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
| `preserve` (default) | Nothing about which members load changes ([the alias technique](#the-alias-technique)). A group is folded only if every definition of its names in the library is a copy with the same code (so whichever copy a link keeps is the same; under `select-any`, a copy of the same kind that refers to the same names: see "What `select-any` gives up"), and not if the member the index names for them has side effects. The weak-external technique below was meant for those, but it fails in `link.exe` (phase 2's results), so they aren't folded. |
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

#### The alias technique

What `preserve` does for the units it folds. Each member that gave up a
copy keeps each of the copy's names defined, as an alias weak external
(`IMAGE_WEAK_EXTERN_SEARCH_ALIAS`) whose default is `__turbolib$alias`,
and refers to the anchor of the synthesized member holding the copy.
Synthesized members are indexed for their anchors alone (under
`preserve`), and never for `__turbolib$alias`. So:

- **The index is the original's.** `lib.exe` indexes aliases, so each
  name still sends a link to the member it sent it to.
- **A loaded member never sends the linker to the library for a name it
  gave up.** An alias counts as defined, and the anchor brings the
  synthesized member, whose strong definition overrides the alias. With
  the name merely undefined, link.exe sometimes reached it before the
  anchor and loaded the first definer through the index: members the
  original link never loaded (on `llvm-nm`, 652 original members against
  637, not all the same), and with them references no library could
  resolve.
- **One default everywhere.** link.exe refuses two weak externals for one
  name with different defaults (LNK1227), which two libraries that fold
  the same inline function would otherwise give it (each its own anchor,
  as a first version did). Both linkers require the default resolved, even
  when a strong definition wins, so every synthesized member defines
  `__turbolib$alias` as the absolute 0; both linkers accept the equal
  absolutes. The default is never used: a strong definition always comes
  with the alias.
- **Two records of the name, the alias first.** The alias takes the
  name's original record; a plain undefined external of the name comes
  after it, and the member's relocations name that one. link.exe's ICF
  takes a reference through a weak external for another target than a
  reference to the name, so with the alias as the only record, code calling
  a moved function stopped folding with identical code elsewhere (4,870
  more functions, 408 KB more code, in `llvm-nm` under `/OPT:REF`; none
  under `/OPT:REF,NOICF`). With two records, it folds as before. The order
  is lld-link's: it reads the records in order, and a plain undefined name
  first loads the member the index names for it (95 more members in
  `llvm-nm`, the plain record first), while after the alias it loads
  nothing.
- **A direct index entry for the synthesized member** isn't an option:
  link.exe refuses an entry for a member that doesn't define the name
  (LNK1377, "symbol not found in object; the containing library is
  corrupt"), and an entry naming the synthesized member would change which
  members load.

- **The anchors first.** A member's references to its anchors come
  before its first external or weak external record. link.exe searches a
  library for the undefined names in the order they became undefined (a
  loaded member's in its records' order), and with the anchors last, some
  sweeps ended early once an anchored synthesized member loaded: the
  members they would have loaded came in a later pass, and the order of
  C++ initializers across members follows the load order (two of
  `llvm-nm`'s 55 moved; C++ leaves it unspecified, but programs can depend
  on it). With the anchors first, the synthesized members load before a
  sweep reaches the moved names, and `llvm-nm` and `llvm-readobj` load
  their members in the original order (`/VERBOSE`; compare-images finds the
  initializers in order). Which records end a sweep early isn't known: it
  reproduces with five of `LLVMObject.lib`'s members and one synthesized
  member, not with hand-made objects of the same shape. It isn't the
  members' order (synthesized members first or last changed nothing).

### `--precomp`: shared types (phase 3)

| Mode | Behavior |
|---|---|
| `none` | Each member keeps its own `.debug$T`. Synthesized members get merged types of their own. |
| `library` (written; the default once it is proven in more links) | One precompiled-types member for the whole library (see [Phase 3 results](#phase-3-results)). |
| `clustered`, `clustered=<n>` (written) | Up to *n* precompiled-types members (16 without *n*), each for a group of members, the groups chosen by a cost model of what links load (see [Phase 3.5 results](#phase-35-results-clustered)). |

### `--debug-types`: a Mach-O synthesized member's classes (7.6)

| Mode | Behavior |
|---|---|
| `complete` (default) | The classes the moved code names, and those its functions are members of, are whole in the synthesized member (dsymutil keeps only what the linked code's DIEs reach): the corpus links' dSYMs compare with the originals' as 7.5's did. |
| `shared` | Such a class is a declaration when every giver of the synthesized member defines it in every link that loads the giver (`turbonop_fold::macho::reach`): the synthesized members' DWARF less than half. But which unit's version of a class template specialization dsymutil keeps then changes (70 to 133 classes per corpus link), and under `-dead_strip` a giver's code that defines a class may be stripped while the moved code that names it stays. See [7.6](#what-dsymutil-keeps-and-synthesized-members-classes-from-their-givers-76). |

ELF and COFF ignore it.

### `--report odr`: names whose definitions differ

`--report odr` (`/TURBOLIB:report=odr`) lists, after the library is
written, each set of external names that COMDAT units in more than one
member define with different code, with each definition's size,
selection, and members. Code is compared as `--locals=copy` compares it
(the locals a unit reaches by content), so a unit naming its own
translation unit's `$SG` string literal isn't a finding. A finding whose
definitions all come from members of different environments (other
compiler options, `/FAILIFMISMATCH` values) says so: it is likely options,
not source, that differ. It is information, printed as `turbolib: odr:`
lines, never a warning, and changes nothing in the library (with
`--fold=none` it folds nothing and writes the members as they are). Not
listed: `NODUPLICATES` COMDATs and definitions outside COMDATs (two of
those are the linker's duplicate-symbol error, and `lib.exe` already
warns, LNK4006).

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
5. **Members already built with `/Yu`.** (As built, the PCH object stays
   as it was, since users in other libraries may name it: see
   [Phase 3 results](#phase-3-results).) If their PCH object is in the
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
partitioning, and `--stats` reports the cost model's figures for each
choice. (In the corpus's links, measured in
[3.5](#phase-35-results-clustered), only lld-link's PDBs gain the unused
records; link.exe's have the same records as without shared types.) Shared
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
- **Make:** in every mode, a member's object isn't kept once its facts
  (and what shared types, select-any, and `--report odr` need of it) are
  read: each member's object is read again for the synthesized members
  whose units are its own and then its rewriting, which edits it in place
  when nothing else holds it, and let go once they are made. A
  synthesized member whose units are several members' (select-any's
  largest pick, copies kept for side effects) reads them on its own.
  A rewritten or synthesized member is held as its bytes but its section
  data (holes, which are the inputs' bytes).
- **Write:** in order, from one thread, while up to 11 others copy the
  chunks ahead (4 MiB each, at most 12 in memory whatever the number of
  threads): NTFS zero-fills a file written out of order.
- **Many processes:** each `lib` run takes free slots (lock files, one per
  core) and runs that many threads, so that a build running many
  archivers at once doesn't run cores × cores threads; a library under
  4 MiB runs on one thread.
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

  `--plugin` is accepted and ignored, as LTO is out of scope. Written in
  phase 6 (`apps/turbolib/src/unix`), with `--output`, `--target`,
  `--record-libdeps`, `L`, `O`, `P`, `o`, `v`, `f`, and the `ranlib`s'
  `-D`, `-U`, and `-t`; turbolib's own options are `--<option>` words and
  `-j N` there.
- **turbolib's own options.** These are `--fold`, `--pick`, `--pull`,
  `--locals`, `--precomp`, `--debug-types`, `--report odr`, `--diagnose`, `--stats`, and
  `-j`. A personality accepts
  them in its own spelling (`/TURBOLIB:<option>` for `lib`) or from the
  `TURBOLIB_OPTIONS` environment variable, so a build system that can't
  add arguments can still set them.
- `turbolib compact in.lib -o out.lib [options]`: rewrites an existing
  library.
- `turbolib analyze x.lib [--json]`: what each mode would save, by
  category (code, unwind, `.debug$S`, `.debug$T`, other), the hazards each
  mode would accept, and the projected PDB cost of each `--precomp`
  choice. It writes nothing.
- `turbolib verify [--pull=preserve|relaxed] [--fold=identical|select-any]
  [--locals=keep|copy|merge] original.lib compacted.lib`:
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

### The self-check (`verify=on`)

After folding, turbolib checks the library it wrote against the one it
would have written unfolded, with `turbolib verify`'s checks
(`turbonop_verify::library`), in the same process; if the check fails, the
unfolded library is written instead. What it proves is `verify`'s (see
below); what it costs:

- **Reading.** Each member of both libraries is read once into a model
  that borrows its names from the member's bytes (no copy of the millions
  of names a large library has), with the relocations of `.debug$`
  sections left unread (most of a `/Z7` object's; nothing checked looks
  at them: debug information is `compare-images`'s). Each unit's
  fingerprint is one pass of a two-lane 128-bit hash, keyed per process
  with keys that aren't the folding engine's, over a canonical form whose
  fixed-width fields are packed into words; associative sections are
  ordered by name, characteristics, and size, and hashed for ordering only
  when those tie. The facts the PCH checks need are read in the same pass
  (they read every member again before). Members are read on every core,
  the largest first.
- **When.** The original library is laid out and read on the other cores
  while the folded library is written (the write is I/O); then the written
  file is mapped and read, and the two compared.
- **Comparing.** Each original member is checked against its counterpart
  through one table of the names either defines or refers to (each name
  hashed once, then every check a walk of the table), with each unit's key
  and identity computed once when it is read; the archive indexes'
  names are gathered without copying each member's list; members are
  checked the largest first.
- `--stats` prints the time of each stage (`self-check ... original laid
  out and read ..., written library read ..., compared ...`).

Measured with `tools/turbolib/corpus/perflib.py single` (median of 3
interleaved rounds, `--fold=identical --precomp=library`, check on minus
off, user and kernel CPU), against `origin/main` before the change, on a
machine other sessions kept at 100% load (so the numbers are high, and
noisy; old and new ran interleaved):

| | `clangSema.lib` | `LLVMCore.lib` |
|---|---|---|
| CPU, 8 threads | 4.95 s to 3.70 s | 1.31 s to 1.14 s |
| CPU, 1 thread | 4.55 s to 3.74 s | 1.44 s to 0.96 s |
| peak commit added, 1 thread | 346 MB to 45 MB | 103 MB to 33 MB |
| peak working set added, 8 threads | 554 MB to 517 MB | 236 MB to 182 MB |

That leaves the check at about 0.65x folding's CPU (8 threads; folding
itself 5.7 s and 1.6 s). A third of it is kernel time, most of it the
faults of mapping the library just written to read it back (its pages are
the file cache's: they count in the working set, not in the commit).

**A cheaper mode, not written.** `verify=links` would run every check but
the units' and sections' fingerprints: it would prove that the same names
are defined (with their kinds), referred to, indexed, and loaded, that
every moved COMDAT's names are aliases anchored to a synthesized member
defining them, and that no strong definition moved; it would no longer
prove that a moved COMDAT is identical to the copy that stands for it
(the engine's identity hash), nor that what each member keeps (its
COMDATs and its sections outside them) is unchanged byte for byte.
Hashing is about a third of reading a member, so it would save perhaps
a quarter of the check.

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
  `--pull=relaxed` lists the symbols sent elsewhere, as expected. Under
  `preserve` too (phase 4): each synthesized member a member anchors
  refers to no name that member didn't refer to or define, so loading it
  loads nothing new; a compiler PCH object's symbol is the known exception,
  reported apart (see Phase 4 results).
- `--fold=select-any` (phase 4): a moved COMDAT may instead be held, by a
  synthesized member its member anchors, as another copy of its name in
  its environment with the same selection, as that selection allows
  (`SAME_SIZE`: the same size; `EXACT_MATCH`: the same size and checksum;
  `LARGEST`: one no smaller); reported as expected.
- `--locals=copy` or `merge` (phase 4): units are fingerprinted with the
  static symbols they reach outside them by content (the unit or the piece
  of a section that holds each, transitively, as the engine copies them,
  computed independently of it), a local unit's key leaves its name out,
  local units may be held by any number of synthesized members, and static
  side-table entries are compared without their names.

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

- **3.1** to **3.4**: done (see [Phase 3 results](#phase-3-results)).
  **3.5**: done (see [Phase 3.5 results](#phase-35-results-clustered)),
  with the open questions below answered there: groups by what links
  load (the library's own references), *n* fixed or chosen, a score of
  the library's bytes plus 16 times what an average link reads, and
  members out of every group when that scores lower.
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

Done: see [Phase 4 results](#phase-4-results); what is left is in TODO.md.

- **4.1** `--fold=select-any` and `--pick`.
- **4.2** `--locals=copy` and `merge`.
- **4.3** `--report odr`.

### Phase 5: speed

- Profile on the largest libraries in the corpus with Stackrabbit, and
  remove every idle core outside I/O waits. Also compare link times
  before and after compaction, which is the second benefit to prove.

**Measured** (`tools/turbolib/corpus/timelib.py`, `build-z7`'s 20 largest
libraries, 7.26 GiB, each archiver on the build's own command, best of 2,
warm cache, 8 threads, 2026-11-25):

| | Total | Against `lib.exe` |
|---|---|---|
| `lib.exe` 14.52 | 11.76 s | 1.00× |
| turbolib, `--fold=none` | 5.84 s | 0.50× |
| turbolib, `--fold=identical` (with 2.4's debug information) | 11.37 s | 0.97× |
| Again, 2026-11-26 (after phases 3, 4, 6, 7, the load-order and initializer fixes, and the anchor-insert fix 863ae3d6): `lib.exe` | 10.94 s | 1.00× |
| `--fold=none` | 5.49 s | 0.50× |
| `--fold=identical` | 11.21 s | 1.03× |
| Again, with phase 3 (same day): `lib.exe` | 11.02 s | 1.00× |
| `--fold=identical` | 11.03 s | 1.00× |
| `--fold=identical --precomp=library` | 11.73 s | 1.06× |

What made the difference: writing the library in order. Setting the
file's length first and writing at offsets on every thread made NTFS
zero-fill ahead of each write (1 GiB in 1.2 to 1.4 s, against 0.3 s in
order); now every thread copies 4 MiB chunks into buffers while one writes
the batch before. On `clangSema.lib` (984 MiB), with folding: reading and
finding units 0.31 s, the rest of folding 0.5 s, the index 0.25 s,
writing 0.35 s, and freeing the inputs' mappings and renaming 0.13 to
0.2 s. Tried and dropped: a buffered streaming hash with four chains
(faster on large inputs, but the many small values units hash made
finding units 25% slower); reading members largest first (no change: reading is 4.3× faster on 8 threads than on 1, which this PC's cores bound). The fold engine's maps use turbonop-hash rather than SipHash (definers and grouping 35% faster). On one thread, `clangSema.lib`'s reading is 1.19 s: parsing 0.30, finding units 0.42, and checking debug information 0.47.

**Memory and many processes (2026-11-27).** Measured with
`tools/turbolib/corpus/perflib.py` (each run in a job object, so
`lib.exe`'s `link.exe` counts; peak commit is private memory, the working
set includes the mapped inputs). The machine was shared with another
session building LLVM (CPU at 50 to 100% throughout), so wall times are
not comparable between runs; CPU time and memory are. `build-z7`'s 20
largest libraries, median of 2 interleaved rounds, the largest peak of
any library (clangSema.lib) and the summed CPU time:

| | CPU before | CPU after | peak commit before | after | peak working set before | after |
|---|---|---|---|---|---|---|
| `lib.exe` 14.52 | 17.9 s | | 237 MiB | | 1,700 MiB | |
| `--fold=none` | 14.6 s | 13.3 s | 485 MiB | 231 MiB | 1,308 MiB | 1,111 MiB |
| `--fold=identical` | 38.0 s | 39.4 s | 2,392 MiB | 1,694 MiB | 2,882 MiB | 2,398 MiB |
| `--precomp=library` | 45.9 s | 47.0 s | 2,408 MiB | 2,078 MiB | 2,893 MiB | 2,727 MiB |
| `--precomp=clustered` | 48.2 s | 50.0 s | 2,440 MiB | 2,093 MiB | 2,902 MiB | 2,723 MiB |

`clangSema.lib` folded on 1, 2, 4, and 8 threads: peak commit 836, 919,
1,216, and 1,457 MiB (CPU 5.2 to 5.7 s); each thread holds about one
member's objects and buffers at a time, and mimalloc keeps what each
thread freed (its purge delay is off: with it at 0, 300 MiB less but 30%
more CPU).

All 245 libraries of `build-z7`, folded 8 at a time (as a build with
`-j8` runs them) on this 8-core laptop, the job's peak commit and the
largest sum of working sets:

| | wall | CPU (kernel) | peak commit | peak working sets |
|---|---|---|---|---|
| `lib.exe` | 28.6 s | 46.2 s (31.7) | 1.14 GiB | 4.35 GiB |
| before | 28.6 s | 91.5 s (44.7) | 11.51 GiB | 12.39 GiB |
| one at a time, compact members, bounded writer | 29.0 s | 91.4 s (41.6) | 7.20 GiB | 9.99 GiB |
| the same, `-j1` in every process | 29.7 s | 80.4 s (27.5) | 3.29 GiB | 6.49 GiB |
| slots (as shipped) | 27.8 s | 80.9 s (28.5) | 4.50 GiB | 7.53 GiB |

What the memory was, and what changed:

- **Members held whole.** A rewritten or synthesized member was held as
  the whole file with zeros in its holes, which Windows commits (the size
  of the output, 726 MiB for clangSema.lib's 984): a `Skeleton` is now the
  bytes but the holes', the index's symbols read from it where it has its
  symbol table (`PeFile::parse_object_at`), and data of 32 bytes or more
  is a hole (4,096 before). Held now: 222 MiB for clangSema.lib's
  rewritten members, 58 MiB for its synthesized ones.
- **Every object at once.** The object model is about 5× the file's
  symbol and section tables (96 bytes a symbol, 128 a section;
  clangSema.lib's 88 objects: 606 MiB, and their units' facts 270 MiB; the
  `modelsize` example measures it). Folding identical copies now keeps
  only the facts, and reads each member's object again for its rewriting
  and the synthesized members whose units are its own (a synthesized
  member's survivors are all its first giver's), its work scheduled
  together and the object let go when it is done. Reading twice costs
  about 4% more CPU.
- **The writer's buffers** were 2 × threads 4 MiB chunks, up to three
  batches (192 MiB on 8 threads, 1.5 GiB on 64): now 12 chunks at most.
- **mimalloc** committed each thread's segments whole (32 MiB); its eager
  commit is off for batch work. (It still commits 32 MiB at start, before
  `main` can say otherwise: every turbolib run, even of a tiny library,
  has 32 MiB of commit, 6 MiB of it used.)
- **Many processes**: each `lib` run takes slots (one lock file per core
  in the temporary directory, never waited for) and runs that many
  threads; under 4 MiB of input, one thread and no slot. Alone it gets
  every core; 8 at once on 8 cores, 12% less CPU and 37% less commit than
  8 threads each, and close to `-j1` everywhere.

**Memory, round 2 (2026-11-28).** The same tools and the same caveat:
another session's builds held the CPU at 100% throughout, so only CPU
time and memory compare. `clangSema.lib`, median of 2 interleaved rounds:

| | peak commit before | after | peak working set before | after |
|---|---|---|---|---|
| `--fold=identical`, 8 threads | 1,486 MiB | 1,296 MiB | 2,249 MiB | 2,135 MiB |
| `--fold=identical`, 1 thread | 847 MiB | 820 MiB | 1,749 MiB | 1,718 MiB |
| `--precomp=library`, 8 threads | 1,966 MiB | 1,333 MiB | 2,616 MiB | 2,138 MiB |
| `--precomp=library`, 1 thread | 1,581 MiB | 836 MiB | 2,312 MiB | 1,720 MiB |
| `--precomp=clustered`, 8 threads | 1,977 MiB | 1,330 MiB | 2,658 MiB | 2,134 MiB |

CPU time was the same within the noise (4.9 to 5.5 s in every run).
`build-z7`'s 20 largest libraries, 8 threads, the summed CPU time and
the largest peak of any library (clangSema.lib), median of 2 rounds:

| | CPU before | after | peak commit before | after | peak working set before | after |
|---|---|---|---|---|---|---|
| `--fold=none` | 15.0 s | 13.2 s | 227 MiB | 230 MiB | 1,112 MiB | 1,115 MiB |
| `--fold=identical` | 38.9 s | 38.7 s | 1,536 MiB | 1,399 MiB | 2,316 MiB | 2,188 MiB |
| `--precomp=library` | 46.0 s | 46.0 s | 2,136 MiB | 1,364 MiB | 2,753 MiB | 2,174 MiB |
| `--precomp=clustered` | 48.6 s | 48.6 s | 1,978 MiB | 1,426 MiB | 2,625 MiB | 2,201 MiB |
| `--fold=select-any` | 39.3 s | 40.1 s | 2,012 MiB | 1,399 MiB | 2,665 MiB | 2,199 MiB |
| `--locals=copy` | 40.5 s | 40.2 s | 2,156 MiB | 1,451 MiB | 2,799 MiB | 2,229 MiB |
| `--report odr` | 40.7 s | 41.0 s | 2,180 MiB | 1,337 MiB | 2,798 MiB | 2,169 MiB |

(The modes after `--fold=none` fold identical copies, but for
`select-any`.)

- **Every mode one object at a time.** Shared types, select-any,
  `--locals`, and `--report odr` kept every object. Now what they need of
  an object beyond its facts is read with it: shared types' scan of its
  `.debug$S` (whether it can be renumbered, and which of its PCH's
  records it names), whether it defines one of turbolib's PCH symbols,
  and under `clustered` its undefined names; select-any's references of
  each unit. The types themselves were already borrowed from the inputs.
  A member that may share types is rewritten from its object read again,
  as one that gives up copies is. Shared types now cost what folding
  alone does (1 thread: 836 MiB against 820).
- **`strip` edits the object in place.** It cloned the object it
  rewrote. A member's synthesized members are made first, and its
  rewriting last takes the object (it copies it only if another thread is
  still making one of them). The side tables are split from the symbols
  as read, before the symbols change, and `.chks64` compares each
  section's bytes with where they were when read. About 91 MiB a thread
  became about 68 (the rewriting step: the 8 largest members' objects at
  once, and what is made of them).
- **Freed memory and new threads.** Not the cause it was thought to be:
  mimalloc reclaims the segments that exited threads leave (with
  `MIMALLOC_MAX_SEGMENT_RECLAIM=0`, 240 MiB more commit; raising it
  changed nothing). The rest of the gap to what is live is pages left
  part-used: purging at once (`MIMALLOC_PURGE_DELAY=0`) peaks at 1,036 MiB
  against 1,368 MiB, but takes 23% more CPU.
- **Slimmer unit facts.** A unit was 288 bytes and copied its external
  names: now 176 bytes, the names borrowed from the members, and what
  only `--locals` and `--report odr` use (structural code, edges,
  closure, referrers, and so on) in a box of its own, there only in those
  modes. clangSema.lib's 437,527 units: 73 MiB (with the vector trimmed
  to size), from at least 120 MiB and 46 MiB of copied names, each its
  own allocation (the `modelsize` example).
- **mimalloc's 32 MiB at start**: accepted, since a run that does real
  work uses it.

Output is the same, byte for byte with `/Brepro`, on all 245 libraries
of `build-z7`, at 1, 3, and the default number of threads, against the
build before, in eight modes: `--fold=none`, `identical`,
`identical --precomp=library`, `identical --precomp=clustered`,
`select-any`, `select-any --pick=largest --precomp=library`,
`identical --locals=copy`, and `identical --locals=merge --report=odr`
(`perflib.py identical`).

Profiling: without administrator rights, `tools/turbolib/sample.py`
(suspends each thread, walks its stack with dbghelp, names frames from
the PDB) worked, but on a machine at 100% CPU it got too few samples to
rank functions (a few hundred per run); its leaf samples showed parsing
(`Object::parse`), copying, and `NtWriteFile` on top. Profile again on a
quiet machine.

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
  lld does for discarded sections. (Done: see [6.5](#debug-information-65).
  Types reached by reference are declarations where a debugger finds
  their definitions by name, and addresses are written without
  `.debug_addr`.)
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
    (Done for DWARF 5, unsplit, the `.dwo` found from the skeleton's
    `DW_AT_comp_dir` and `DW_AT_dwo_name` rather than given as an input.)
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
  command line (done: see [Phase 6 results](#phase-6-results)).

### Phase 7: Mach-O

- **No COMDATs.** There are weak definitions within sections that
  `.subsections_via_symbols` divides into atoms. Folding cuts atoms out of
  `__text` and rewrites relocations, including paired
  `SUBTRACTOR`/`UNSIGNED` relocations, plus `__compact_unwind` and the
  `__eh_frame` FDEs.
- **DWARF is never linked.** The executable's debug map (N_OSO) names
  `library.a(member.o)`, and dsymutil reads the DWARF from there later.
  A moved function's DWARF must be in the synthesized member, and the
  member's modification time must be what the debug map records. (Done:
  see 7.4.)
- **Per-object load commands:** `LC_BUILD_VERSION` and `LC_LINKER_OPTION`.
  (Done: `LC_BUILD_VERSION` is part of the environment and copied into
  synthesized members; `LC_LINKER_OPTION` stays with its member and counts
  as a side effect under `--pull=preserve`.)
- **The tool:** `libtool -static`'s command line, and the BSD archive
  writer. (Done as `llvm-libtool-darwin` behaves: see [7.1](#darwin-archives-and-the-libtool-personality-71).)
- **Status** (see [Phase 7 results](#phase-7-results)): archives (7.1), the
  object model (7.2), folding with `verify` (7.3), and FDEs, LSDAs, DWARF,
  and `compare-images` (7.4), and givers' DWARF pruned (7.5) are done. There is no
  Mac here: ld64 and ld-prime, Apple's `libtool`, and running the programs
  are untested; links are ld64.lld's, compared structurally.

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
