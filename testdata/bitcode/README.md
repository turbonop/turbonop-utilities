# LLVM bitcode fixtures

`tools/bitcode/build.cmd` builds these with clang and the LLVM tools (an
LLVM build's, 24), from our sources in `tools/bitcode` (with the
repository mapped to `R:`, so the paths they record are `R:\...`), and
`tools/bitcode/oracle.py` writes what the LLVM tools say about them. The
crate's tests (`crates/formats/bitcode/tests`) compare against those.

| File | What it is |
|---|---|
| `kinds.bc` | `kinds.c` at `-O1 -g` (Linux): every kind of instruction clang makes from C (varargs, vectors, a switch, computed goto, atomics, casts, a struct return, select, intrinsics, inline assembly, fneg), and globals of each kind (an alias, an ifunc, TLS, a section, a weak function, a pointer into an array). Debug information in the functions' own metadata blocks. |
| `shapes-linux.bc` | `shapes.cpp` at `-O0 -g` (Linux): classes with virtual functions, a template, exceptions (`invoke`, `landingpad`, `resume`), a static initializer, calls through pointers. |
| `shapes-windows.bc` | `shapes.cpp` at `-O1 -g` for Windows: the funclets (`catchswitch`, `catchpad`, `catchret`, `cleanuppad`, `cleanupret`), MSVC's mangling and RTTI, unnamed globals (`@0`). |
| `kinds-thin.bc` | `kinds.c` at `-O2 -flto=thin`: a ThinLTO summary, a symbol table (`irsymtab`), and the string table. |
| `every.bc` | `every.ll`, assembled by `opt`: every instruction, type, constant, linkage, comdat kind, and global property clang doesn't make, and a symbol table. |
| `embed-darwin.bc` | `embed.c` at `-O1 -flto` for macOS: bitcode in Darwin's wrapper. |
| `embed-elf.o`, `embed-macho.o`, `embed-coff.obj`, `embed-wasm.o` | `embed.c` with `-fembed-bitcode`: bitcode and the command line in each object format's sections. |
| `multi.bc` | `llvm-cat -b kinds-thin.bc every.bc`: two modules, each with its string table. |
| `libbitcode.a` | `llvm-ar`: `kinds-thin.bc`, `every.bc`, and `embed-elf.o`. |
| `warn.dia` | `warn.c`'s warnings, notes, and fix-its as clang's serialized diagnostics (`DIAG`). |
| `embed.opt.bitstream` | `embed.c`'s optimization remarks at `-O2`, as a remarks bitstream (`RMRK`). |

The oracles' outputs, each beside its file:

- `<file>.dump.txt`: `llvm-bcanalyzer --dump --disable-histogram`.
- `<file>.stats.txt`, `.nonsymbolic.txt`, `.blockinfo.txt`: its other
  options (the summary with record histograms; `--non-symbolic`;
  `--dump-blockinfo --show-binary-blobs`).
- `<file>.ll`: `llvm-dis`.
- `<file>.symtab.txt`: `llvm-lto2 dump-symtab`.

They are compared byte for byte (`.gitattributes` keeps them as they
were written, with `\n` line ends).
