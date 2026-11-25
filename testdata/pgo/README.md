# PGO test program

`tools/pgo/build.cmd` builds `tools/pgo/pgo.cpp` with MSVC for x64,
optimized by profile-guided optimization (PGO): compiled with `/GL`, linked
instrumented (`/GENPROFILE`), trained by running it once (`pgo.exe 1000`),
and linked again with the profile (`/USEPROFILE`): `pgo.exe` and its PDB.

What it holds, for `hexcavator about` (`apps/hexcavator/tests/it/about.rs`):

- `hot`, run 2000 times in training: optimized by PGO, in `.text$lp00pgo`
  (the hottest code), with S_POGODATA's counts (2000 calls).
- `cold`, run once: seen by training (S_FRAMEPROC's `fValidCounts`), but
  not optimized with the profile (no `fPogoOn`), and no S_POGODATA.
- `never`, never run: optimized by PGO (`fPogoOn`), with no counts, in
  `.text$zz` (whole functions training never ran).
