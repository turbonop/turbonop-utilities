# LTCG IL test inputs

Built by `tools/ltcg-il/build.cmd` with MSVC 14.52 (`cl /c /O1 /GL /GS-
/Brepro`), run from `R:` (see `tools/neutral.cmd`):

- `records.obj`: `tools/ltcg-il/records.cpp` for x64 (`/std:c++20 /EHsc`):
  a record of each kind `.cil$gl` holds (a namespace, classes with
  virtual functions and RTTI, a section, a constant, attributes, variables
  of each storage and alignment, a dynamic initializer), a function's
  scopes and locals (a label, a local static), initializers, and
  functions nothing refers to (an inline one and a static one: IL kept,
  no record).
- `records-a64.obj`: the same for ARM64.
- `records-c.obj`, `records-x86.obj`: `tools/ltcg-il/records.c` for x64
  and x86 (commons, calling conventions, an inline function's local
  static).
- `records.lib`: `records.obj` and `records-c.obj`.
- `*.linkermember.txt`: lib.exe's symbol index of a library of each
  object (`dumpbin /linkermember:2`), which `hexcavator ltcg -publics`
  matches exactly.

docs/ltcg-il.md describes the format.
