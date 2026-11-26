# turbolib's verification test inputs

Built by `tools/turbolib/verify/build.cmd` with MSVC (x64, `/Brepro`) and
lld-link, from the sources beside it:

- `verify.lib`: a.obj, b.obj, and c.obj (`/O1 /Gy /Z7 /guard:cf`), each
  holding copies of shared.h's COMDATs (inline functions, templates,
  vftables, a `selectany` variable, a string literal); a.obj has a dynamic
  initializer; gl.obj is `/GL`; and the import objects of imports.def.
  `turbonop-verify`'s `library` tests rewrite its members as folds would.
- `broken.lib`: the same, compiled with `/DBROKEN`: `shared_add` has other
  bytes.
- Programs of main.cpp and loadcfg.c (no C runtime, `/guard:cf`, its own
  initializers, an import from kernel32, an export) linked with verify.lib:
  `link-1` and `link-2` (link.exe, twice), `link-order` (`/ORDER`, so other
  addresses), `link-broken` (against broken.lib), `lld-1`, `lld-order`, and
  `lld-ghash` (`/DEBUG:GHASH`). Each is an `.exe` and its `.pdb`, for the
  `image` and `pdb` tests.
