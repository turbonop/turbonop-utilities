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
