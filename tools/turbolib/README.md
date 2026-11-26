# turbolib's phase 0 spikes

These are the scripts and sources behind the measurements in
[docs/turbolib.md](../../docs/turbolib.md#phase-0-results). They are
throwaway experiments, not tests. Each one is kept so its result can be
reproduced, or checked again with a new compiler or linker.

All the scripts expect:
- Visual Studio 18 Enterprise. Each script calls its `vcvars64.bat`; edit
  the path for another edition.
- Python 3.
- An LLVM Release build for `lld-link`, `llvm-lib`, and `llvm-pdbutil`.
  `LLVM_BIN` overrides the default, `C:\jm\dev\llvm-project\build-release\bin`.

Every script runs in its own directory, whatever the current directory.
Outputs (objects, programs, PDBs, libraries, logs) land beside the sources,
and `.gitignore` keeps them out of the repository.

## `corpus`: the LLVM libraries measured

`configure-z7.cmd` and `build-z7.cmd` make a Debug `/Z7` build of LLVM,
clang, and lld, with LLVM's precompiled headers. The `-nopch` pair makes the
same build without them. `LLVM_SRC` names the llvm-project checkout (the
default is `C:\jm\dev\llvm-project`). The plan's numbers are at
`c74b358f44c3`.

## `pch`: a synthesized precompiled-types object

`run.cmd` runs every step in order:

1. `build.cmd` compiles `a.cpp`, `b.cpp`, and `main.cpp`, plus a real
   `/Yc`/`/Yu` pair (`pch.cpp`, `user.cpp`) for comparison.
2. `spike.py full a.obj` moves all of a.obj's type records into
   `a.pch.obj`. It writes `a.user.obj`, which names them with `LF_PRECOMP`.
3. `spike.py share a.obj b.obj` moves the records the two objects share
   (their longest common prefix) into `ab.pch.obj`. It writes
   `a.share.obj` and `b.share.obj`.
4. `link.cmd` links the baseline and both results:
   - with link.exe and lld-link;
   - from the command line and through a library;
   - with lld's `/DEBUG:GHASH`, and with link.exe's `/DEBUG:FASTLINK`.

   Then it runs them. Compare the PDBs with
   `llvm-pdbutil pretty -all`.
5. `spike.py prefix a.obj <count> <tag>` puts only the first *count*
   records in the PCH. `link2.cmd` links the `noids` and `ids` splits.
   - `ids` includes `LF_FUNC_ID`. With it, link.exe loses the functions'
     signatures.
   - The counts 16 and 22 are for `a.obj` as MSVC 14.52 built it. Check
     them with `llvm-pdbutil dump -types a.obj`.
6. `chks64.py` makes `a.stale.obj` (code changed, `.chks64` not updated)
   and `a.zeroed.obj` (`.chks64` zeroed). `chk.cmd` links both with both
   linkers.

## `weak`: `NOLIBRARY` weak externals for `--pull=preserve`

`weak.py <name>` makes `shared_f` in `<name>.obj` a weak external
(`NOLIBRARY`), whose alternate `shared_f_tl` is in `x.obj`. `a.obj` has
the static initializer (the side effect), and `b.obj` is the member the
archive index names for `shared_f`.
- `run.cmd` compiles everything and builds `orig.lib`. It then builds
  `weak.lib` with b made weak. This was the first attempt, and the wrong
  member: it changes which member the index names.
- `run2.cmd` builds `weak2.lib` with a made weak. This is the result in
  the plan. It links with link.exe (`/OPT:REF` and `/OPT:NOREF`) and
  lld-link. Run `run.cmd` first.
- `run3.cmd` adds `main5` and `main6`.
- `main1` to `main6` reference different members and symbols. Each
  link's output is in `<main>-<library>[-lld|-noref].link.txt`. Compare
  which members load, and run the programs.
- `idx.cmd` shows which member the archive index names when two define a
  symbol, and that `lib.exe` reverses its command line.

## `big`: the 4 GiB limit

`make.py` writes five objects, each with one 1.1 GiB section. `run.cmd`
archives them with `lib.exe` and `llvm-lib`, over and under 4 GiB.
`link.cmd` links against llvm-lib's 5.5 GiB library with both linkers.
Together the objects and libraries take about 14 GiB of disk.

## `prune`: do linkers drop types no symbol refers to?

`build.cmd` generates 200 structures, each used only by its own `/Gy`
function (`make.py`). It links them with `/OPT:REF` and `/OPT:NOREF`, with
both linkers. It also links a `/Yc` object holding all 200 with a `/Yu`
user that refers to one. `count.sh` (from Git Bash or WSL) counts what
reached each PDB.

Neither linker pruned anything: all 200 structures and all 200 function
ids arrived every time.

## `tables`: side tables through a fold (phase 2.3)

`run.py [config...]` compiles `tools/coff/tables/one.cpp` and `two.cpp`
and `three.cpp` here (which share `shared.h`'s inline functions), for each
configuration: cl for x64 (`/guard:cf /guard:ehcont`, with and without
`/Gy`; `/volatileMetadata`; `/d2guardretpoline`), x86 (`/guard:cf`, with
SafeSEH and FPO), and ARM64, and clang-cl for x64 and x86. It makes
`orig.lib` with lib.exe and `fold.lib` with turbolib
(`TURBOLIB_OPTIONS=--fold=identical`), links `main.cpp` against each with
link.exe and lld-link, under `/OPT:NOREF` and `/OPT:REF`, and compares the
images' tables by name (from the map file): the load config's CFG,
longjmp, EH continuation, and SafeSEH tables, the volatile metadata, the
dynamic relocations (retpoline), and the PDB's FPO data. Then it runs the
x64 and x86 programs, which take an inline function's address and call it
through CFG, longjmp, throw, fault, and call through an import's address.
`TURBOLIB` names turbolib.exe (the default is `target\release`'s). The
outputs land in `out\`.

## `debug`: synthesized members' debug information (phase 2.4)

Not a spike: the sources of turbonop-fold's debug-information fixtures.
`build.cmd` builds them into `testdata\coff\fold` (its comments list each
object). `check.py <turbolib.exe>` links `main.cpp` against libraries of
them, made by lib.exe and folded by turbolib, with link.exe and lld-link,
runs both programs, and compares the PDBs with
`corpus\pdbcompare.py`, which compares procedures (signatures, locals,
inline sites, lines) and other module symbols structurally, from
llvm-pdbutil's dumps.
