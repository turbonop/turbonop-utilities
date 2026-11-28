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

- **Very large `/INCREMENTAL` links fail with `LNK1140`.** Folding adds
  synthesized members (one for each set of members that gave up copies:
  4,574 in `clangAST.lib`), and each one a link loads is a module in the
  PDB. In a Debug LLVM build with `fold=identical`, `clang.exe` loads
  50,119 modules (2,752 with `lib.exe`'s libraries), and link.exe 14.52's
  incremental link fails with `LNK1140: limit exceeded for program
  database` (`clang.exe`, `libclang.dll`, `clang-repl.exe`, and four more;
  `lld.exe`, 24,640 modules, links). The same links with `/INCREMENTAL:NO`
  succeed (`clang.exe`'s PDB: 47,571 streams), most likely because an
  incremental PDB needs about two streams a module and the stream numbers
  are 16 bits; a non-incremental link would fail the same way near 65,535
  modules. Until turbolib makes fewer synthesized members, link programs
  of that size with `/INCREMENTAL:NO`, or leave folding off for them.
- **Switching an incremental build to turbolib**: the first incremental
  relink of a program whose libraries' `/Yu` members changed can fail
  with `LNK1236` (seen on 5 of 58 LLVM programs); link.exe has then
  deleted the `.ilk`, and the next build links in full and succeeds.
  Deleting the `.ilk` files (or the programs) when switching avoids it.
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
- **The self-check rejects some folds**, which are then written unfolded
  with the warning: a library folded before and archived again with more
  members; some SDK libraries (`libucrt.lib`, `Uuid.lib`: one COMDAT held
  by two synthesized members; `BufferOverflow.lib`: `.retplne`).
- **The self-check's cost**: on LLVM's libraries it took about as long as
  folding itself on a quiet machine (`LLVMCore.lib`: 0.45 s against 0.38 s)
  and two to five times as long on a loaded one; `verify=off` saves it.
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
