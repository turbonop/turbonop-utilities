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
- `equiv/`, built by `tools/turbolib/equiv/build.cmd`: a Debug program
  (`cl /Od /Z7 /RTC1 /EHsc /MDd`; switch tables, inline thread_locals, a
  lambda, exceptions, RTTI, two functions of the same code) linked by
  link.exe and lld-link against a library (`*-original`) and against it
  folded by turbolib (`*-folded`); and by link.exe against the folded
  library broken by `breakfold.py` (`link-broken`: one of classify's
  constants; `link-retarget`: a relocation of shared_all sent to a
  function of the same code). `image::tests::equiv` changes the folded
  programs as faults could and checks that compare-images catches each.
