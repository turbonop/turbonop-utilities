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
  `/headers` (`*.headers.txt`, which `hexc dump` matches), lib.exe's symbol
  index (`*.lib.linkermember.txt`, which `hexc ltcg -publics` holds), and
  the directives of `ltcg.cpp` compiled without `/GL`
  (`ltcg.directives.txt`).
- `*.symbols.txt`: dumpbin's `/symbols` of each object, which
  `hexc dump -symbols` matches.
