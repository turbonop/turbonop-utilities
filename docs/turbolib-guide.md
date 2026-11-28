# turbolib: user guide

turbolib is a drop-in replacement for `lib.exe` (and `llvm-lib`) that can
keep one copy of what C++ objects duplicate: inline functions, template
instantiations and other COMDATs, and `/Z7` type records. This page says
how to use it; [turbolib.md](turbolib.md) is the design and its results.

## In one paragraph

Put `turbolib.exe` where the build looks for `lib.exe`, named `lib.exe`.
With no options it does what `lib.exe` does, byte for byte apart from
member dates (and the banner). Add `/TURBOLIB:fold=identical` and
`/TURBOLIB:precomp=library` to the archiver's flags to fold. If folding
ever fails, turbolib still writes the library, unfolded, exactly as
`lib.exe` would have, and prints one line saying why: a build never breaks
because of folding.

## Installing it

turbolib picks its personality by its file name:

| Named | Behaves as |
|---|---|
| `lib.exe` (any case) | `lib.exe` 14.52: options, messages (on standard output), LNK numbers and exit codes |
| `llvm-lib.exe` | `llvm-lib` (messages on standard error, exit code 1) |
| anything else | `turbolib lib ...` or `turbolib llvm-lib ...` selects one |

Build it with `cargo build --release -p turbonop-turbolib`; the binary is
`target\release\turbolib.exe` and needs nothing beside it.

- **Buck2.** Point the `cxx_toolchain`'s archiver at a copy of turbolib
  named `lib.exe` (the toolchain's `archiver` attribute, or the tool the
  toolchain rule wraps), and add the options to its archiver flags:
  `archiver_flags = ["/TURBOLIB:fold=identical", "/TURBOLIB:precomp=library"]`.
  Prefer flags over the environment variable: remote execution doesn't
  pass the local environment, and flags are part of the action's key, so
  turning folding on or off changes the cache key as it should.
- **CMake.** `-DCMAKE_AR=C:/path/to/lib.exe` (the copy), and
  `-DCMAKE_STATIC_LINKER_FLAGS="/TURBOLIB:fold=identical /TURBOLIB:precomp=library"`.
- **MSBuild.** `/p:LibToolPath=C:\path\to\dir` (the directory holding the
  copy named `lib.exe`), and in the project or a `Directory.Build.props`:
  `<Lib><AdditionalOptions>/TURBOLIB:fold=identical /TURBOLIB:precomp=library %(AdditionalOptions)</AdditionalOptions></Lib>`.
- **Anything else.** Set `TURBOLIB_OPTIONS=--fold=identical --precomp=library`
  in the environment the archiver runs in. It is read before the command
  line, so `/TURBOLIB:` options on the command line win.

The real `lib.exe` warns about `/TURBOLIB:` options it doesn't know
(LNK4044) and ignores them, so flags left in place while the real tool
is swapped back in cost a warning, not a failure (unless the build uses
`/WX`).

## Options

On the `lib.exe` command line, `/TURBOLIB:<option>` (one option each);
in `TURBOLIB_OPTIONS`, `--<option>` words. Names and values are in any case.

| Option | Default | What it does |
|---|---|---|
| `fold=none\|identical\|select-any` | `none` | `identical` keeps one copy of each COMDAT whose copies are identical; the library links as the original does. `select-any` is less conservative (see turbolib.md) |
| `precomp=none\|library\|clustered[=n]` | `none` | `library` moves the type records two or more members share into one precompiled-types member, which the others refer to as `/Yu` objects refer to a PCH |
| `verify=on\|off` | `on` | after folding, check the written library against the unfolded one (`turbolib verify`'s checks, in the same process); if the check fails, write the unfolded library instead |
| `strict` | off | a fold that fails is an error (`LNK1000`) and nothing is written, instead of the unfolded library: for testing turbolib, not for builds |
| `stats` | off | print what was folded and shared, and how long each stage took |
| `diagnose` | off | say what was passed through, and why |
| `threads=N` | one per core | `N` threads; `1` creates none. The output is the same for every `N` |
| `pull`, `locals`, `pick`, `report=odr`, `extract-whole` | | less conservative modes and reports: see turbolib.md |

Without `fold` or `precomp`, nothing is folded and the library is
`lib.exe`'s, byte for byte apart from member dates: with `/Brepro`, byte
for byte.

## What it prints

- Without `/NOLOGO`, a banner where `lib.exe` prints Microsoft's:
  `turbolib 0.0.1 (TurboNOP), a library manager compatible with lib.exe`.
- `lib.exe`'s own warnings and errors, numbered and worded as `lib.exe`
  words them (LNK4006, LNK1104, LNK1181, ...), with its exit codes.
- If folding fails, one line, and the library is written unfolded:

      turbolib: warning: folding x.lib failed, so it was written unfolded: <why>

  `<why>` is a panic's message and place (`panic: ... at crates\link\fold\src\...:123`),
  a library that couldn't be laid out, or the self-check's first failure
  (`the self-check failed: <what> (<count>, e.g. <member and symbol>)`).
  This line isn't a `lib.exe` warning: `/WX` doesn't turn it into an
  error, and `/IGNORE` doesn't silence it. MSBuild may show it as a
  warning. Please report it, with the library's inputs if they can be
  shared.
- With `stats`, lines starting `turbolib:` with counts, sizes, and times.

## Checking a library

The self-check (`verify=on`, the default) already runs on every folded
library before it replaces the output. To check one by hand:

    lib /OUT:plain.lib <the same inputs>                       (real lib.exe, or turbolib without options)
    turbolib verify plain.lib folded.lib

It prints what it checked and any failure by category, and exits with 1
if anything failed. `turbolib compare-images a.exe a.pdb b.exe b.pdb`
compares two programs linked against the two libraries (functions, data,
unwind, and the PDBs' contents).

## Turning it off

- Drop the `fold` and `precomp` options (or set `/TURBOLIB:fold=none
  /TURBOLIB:precomp=none`): turbolib then writes `lib.exe`'s library.
- Or put the real `lib.exe` back. The libraries are ordinary libraries
  either way: nothing downstream needs turbolib.

## Known limitations

- **COFF only for folding in this personality**; `/GL` (LTCG) objects,
  import objects, resources, and anything turbolib doesn't recognize pass
  through untouched (the library still links).
- **`/EXTRACT`** of a member of a folded library gives the object without
  the COMDATs that moved to a synthesized member.
- **Thin libraries** (`/llvmlibthin`) aren't written.
- **`/DEBUG:FASTLINK` and incremental linking** were checked on small
  programs only (`tools/turbolib/incremental/run.py`: x64, x86, ARM64),
  not on large ones. link.exe 14.52's incremental relink can fail after a
  `/Yu` member of a library changes (LNK1235 with `lib.exe`'s library,
  LNK1236 with a folded one); the next link is a full one and is right.
- **`/GL` members' index entries** can differ from `lib.exe`'s (turbolib
  doesn't read every IL reference, so it lists fewer of a `/GL` object's
  inline functions), with or without folding: a link may then load
  another member that defines the same COMDAT.
- **Libraries of folded libraries** fold again (a library folded before,
  alone or with more objects, and two folded libraries together):
  copies in the inputs' synthesized members fold with other copies, and
  the result passes the self-check. It saves only what the inputs still
  duplicate (`LLVMSupport.lib` and `LLVMCore.lib`, each folded, folded
  together: 1.0% smaller than `lib.exe`'s combination of them, 0.3%
  larger than one fold of both), and each fold adds synthesized members
  of its own: two small folded libraries combined can come out larger
  than `lib.exe`'s combination of them.
- **Libraries folded before 2026-11-28** (each synthesized member defines
  `__turbolib$alias`, the old default of the aliases): `lib.exe` combining
  one with anything else warns LNK4006 for it, once per extra definition,
  and so fails under `/WX`. Libraries folded since don't define it (the
  default is `__ImageBase`, which the linkers define; [the alias
  technique](turbolib.md#the-alias-technique)), so `lib.exe` and `llvm-lib`
  combine them without a warning. Fold an old library again (turbolib
  turns it into the new form), or make it again; don't link old and new
  folded libraries of the same code together, as link.exe may refuse a
  name aliased with both defaults (LNK1227).
- **The self-check rejects some folds**, which are then written unfolded
  with the warning: some SDK libraries (`libucrt.lib`, `Uuid.lib`: one
  COMDAT held by two synthesized members; `BufferOverflow.lib`:
  `.retplne`).
- **The self-check's cost**: on a loaded machine it adds about 0.65× the
  CPU folding takes on `clangSema.lib` (3.7 s against 5.7 s; it was 4.95 s)
  and 0.70× on `LLVMCore.lib` (1.14 s against 1.62 s), and a fifth to a
  third more peak working set (mostly the written library, mapped to be
  read back); `verify=off` saves it.
- **Two folded libraries with identical shared types** in one link share
  one precompiled-types member, which is fine; two whose 32-bit PCH
  signatures collide by chance (about n²/2³³ for n libraries) make
  lld-link fail the link loudly (link.exe links correctly).
- **Memory**: folding holds the members' parsed form; peak memory is
  several times what `lib.exe` uses, and many turbolib processes at once
  share the machine's cores and memory.
- **Output in use**: if another process holds the output open, turbolib
  tries the rename again for up to 5 seconds, then fails with LNK1104 as
  `lib.exe` would. A read-only output fails at once (LNK1104), as with
  `lib.exe`.
