# CodeView test inputs

Built by `tools/codeview/build.cmd` (MSVC 14.52 and clang-cl, `/Z7`,
`/Brepro`, from `R:`) from `tools/codeview`, for turbonop-codeview's
fixtures test (`crates/symbols/codeview/tests/it/objects.rs`): the record
kinds LLVM's own `/Z7` build lacks.

- `kinds-msvc-x64.obj`, `kinds-msvc-x86.obj` (`/Oy`: frame data),
  `kinds-msvc-arm64.obj`: `kinds.cpp` with `/O2 /GS /Gy`: optimized code's
  locals and live ranges (`S_LOCAL`, `S_DEFRANGE_*`), inline sites (one
  changing file, from `kinds-body.inc`), `S_INLINEES` and `S_CALLEES`,
  frame cookies, jump tables (`S_ARMSWITCHTABLE`), heap allocation and
  indirect call sites, annotations, an adjustor thunk, thread-local and
  file-static data.
- `kinds-msvc-x64-od.obj`: the same with `/Od /RTC1 /JMC`.
- `kinds-clang-x64.obj` (with `sum_of_squares` a secure hot-patch
  function: `S_HOTPATCHFUNC`), `kinds-clang-x86.obj`,
  `kinds-clang-arm64.obj`: `kinds.cpp` with clang-cl `/O2 /GS`.
- `kinds-masm.obj`: `kinds.asm`, `ml64 /Zi`.
- `managed.obj`: `managed.cpp`, `/clr`: managed procedures, slots and
  data, whose metadata tokens are relocated (`IMAGE_REL_AMD64_TOKEN`).
- `pch.obj` (`/Yc`, its `.debug$P`) and `pch-user.obj` (`/Yu`, its
  `.debug$T` starting with `LF_PRECOMP`): `pch.h`, `pch.cpp`,
  `pch-user.cpp`.
