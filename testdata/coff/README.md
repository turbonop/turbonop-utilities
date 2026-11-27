# COFF test inputs

Built by `tools/coff/build.cmd` with MSVC (x64, `/Brepro`) from
`tools/coff/objects.cpp` and `tools/coff/imports.def`:

- `objects.obj`: `/O1 /Gy /Z7` (COMDATs, associative `.pdata`/`.xdata`,
  `.debug$S` per function, a function's static, `.bss`, `.data`, `.rdata`).
- `objects-big.obj`: the same with `/bigobj`.
- `objects-zi.obj`: the same with `/Zi`: its types and ids in
  `objects-zi.pdb`, its type server (which it names as
  `R:\testdata\coff\objects-zi.pdb`, where the build ran).
- `objects.lib`: a static library of both.
- `imports.lib`: an import library (short import objects by name, by
  ordinal, undecorated, data, and renamed).
- `ltcg.obj`: `tools/coff/ltcg.cpp` with `/GL` (an LTCG object: the
  compiler's IL), and `ltcg.lib` of it; `ltcg-x86.obj`, `tools/coff/ltcg.c`
  for x86 (C's common symbols, decorated names). Beside them: dumpbin's
  `/headers` (`*.headers.txt`, which `hexcavator dump` matches), lib.exe's symbol
  index (`*.lib.linkermember.txt`, which `hexcavator ltcg -publics` holds), and
  the directives of `ltcg.cpp` compiled without `/GL`
  (`ltcg.directives.txt`).
- `*.symbols.txt`: dumpbin's `/symbols` of each object, which
  `hexcavator dump -symbols` matches.

`model/` holds turbonop-coff's test objects, built by `tools/coff/model.cmd`
from the sources in `tools/coff/model`: what each tool writes (cl for x64,
x86, ARM64EC, and /clr; clang-cl, with CodeView and with DWARF; llvm-mc;
ml64; armasm64 -g; cvtres), for the layouts, string tables, and auxiliary
records each one has. The script's comments list what each object is for.

`tables/` holds turbolib's side-table test objects, built by
`tools/coff/tables.cmd` from `tools/coff/tables/one.cpp` and `two.cpp`
(which share `shared.h`'s inline functions): each configuration's pair has
the `/guard` tables, `.sxdata` and FPO, `.voltbl`, or `.retplne`, as cl
(x64, x86, ARM64) and clang-cl write them. The script's comments list
them.

`fold/` is built by `tools/turbolib/debug/build.cmd` (MSVC 14.52 and
clang-cl of LLVM 24) from the sources there, for turbonop-fold's tests
of synthesized members' CodeView: `/Od` and `/O2` objects (inline sites,
inlinee lines, a file change inside an inlined function), a PCH object and
its users, `/Zi` objects with their type server, and clang-cl's.

`loose/` is built by `tools/turbolib/loose/build.cmd` (MSVC 14.52 and
clang-cl of LLVM 24) from the sources there, for turbonop-fold's tests of
phase 4's modes (`--fold=select-any`, `--locals=copy` and `merge`,
`--report odr`): three objects compiled with `/Od /Gy`, `/Od` alone,
`/O2 /Gy`, and clang-cl's `/Od /Gy`, whose inline functions reach
internal-linkage functions and data, and differ between the objects.

`archives/` is built by `tools/coff/archives.cmd` (lib.exe of MSVC 14.52,
with and without `/Brepro`, and llvm-lib of LLVM 24), for the archive
writer's and the `.def` reader's tests, which write each library again
and compare:

- `<name>.lib`, `<name>-brepro.lib`, `<name>-llvm.lib`: libraries of the
  objects `tools/coff/archives/symbols.py` writes (every kind of symbol;
  the same names defined, common, and weak in several members; names that
  sort; member names of 13 to 17 bytes, and paths), by lib.exe without
  and with `/Brepro`, and by llvm-lib.
- `ec-*.lib`, `x-*.lib`, `mix-*.lib`: ARM64EC and ARM64X libraries of
  ARM64EC, ARM64, and x64 objects of `tools/coff/archives/*.c`.
- `def-<name>[-<machine>].lib`: lib /DEF's import libraries of
  `tools/coff/archives/def/<name>.def` (x64 unless named), one of them
  given an x86 object too (`-objects`), one without `/Brepro` (`-dated`),
  and llvm-lib's (`-llvm`).
