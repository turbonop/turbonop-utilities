# Hexcavator

*Dig into your binaries.*

A fast, free toolkit for exploring executables and their debug information:
what a binary holds, how it was built, what its code refers to and why,
and where its bytes go.

- **Formats:** PE images, COFF objects, and libraries; ELF and Mach-O
  (thin and universal) files and archives; WebAssembly modules; and
  MS-DOS, NE, LE, and LX executables.
- **Debug information:** PDBs (CodeView symbols, types, and lines) and
  DWARF (in the file, beside it, in a dSYM, or from a symbol server or
  debuginfod), for any format: a MinGW-built PE image's DWARF too, and a
  relocatable object's (ELF, COFF, WebAssembly), relocated; and a COFF
  object's own CodeView, read as the PDB a linker would write of it.
- **Every format alike:** each command and analysis reads PE, ELF,
  Mach-O, and WebAssembly images the same way, with whatever debug
  information they have (`-debug <path>` gives it; `-pdb` is another name
  for it).
- **Exploring:** headers and file-format dumps (as `dumpbin`,
  `llvm-readobj`, and `llvm-dwarfdump` show them), symbols, type layouts,
  source files and line tables, inlined functions, data values,
  disassembly (x86/x64, ARM64, ARM32, RISC-V, LoongArch, PowerPC, MIPS, WebAssembly), demangled names
  (Microsoft C++, Itanium C++, Rust, Swift), code signatures checked on
  any platform, and exploit mitigations and the compiler options behind
  them.
- **Analysis:** cross-references (why each function and datum is in the
  image), function fingerprints that survive rebuilds, build-to-build
  diffs, queries over an image's symbols, and size reports with measured
  recommendations.

`hexcavator` does all of it on the command line, with JSON for scripts and CI,
and as an MCP server for AI agents. The VS Code extension
(`editors/vscode/hexcavator`) puts it in a sidebar of binaries and builds
compared.

## Getting it

Each release (tagged `hexcavator-v<version>`: see
[the releases](https://github.com/turbonop/turbonop-utilities/releases?q=hexcavator-v&expanded=true))
has `hexcavator` for each platform as a single program, named by its
target triple, at
`https://github.com/turbonop/turbonop-utilities/releases/download/hexcavator-v<version>/<name>`:

| Platform | Program |
| --- | --- |
| Windows x64 | `hexcavator-x86_64-pc-windows-msvc.exe` |
| Windows ARM64 | `hexcavator-aarch64-pc-windows-msvc.exe` |
| macOS (Apple silicon) | `hexcavator-aarch64-apple-darwin` |
| macOS (Intel) | `hexcavator-x86_64-apple-darwin` |
| Linux x64 (static: any distribution) | `hexcavator-x86_64-unknown-linux-musl` |
| Linux ARM64 (static: any distribution) | `hexcavator-aarch64-unknown-linux-musl` |
| WASI (wasmtime and the like) | `hexcavator-wasm32-wasip1.wasm` |

The Windows programs are signed; the macOS ones are signed and
notarized. Each program's symbols are beside it
(`<name>-symbols.zip` or `.tar.gz`), and `SHA256SUMS` has every file's
hash. On Windows, winget installs it too: `winget install
TurboNOP.Hexcavator`.

Or use it in a browser, nothing to install: the web version
(https://turbonop.github.io/turbonop-utilities/hexcavator/) is the VS Code extension's
sidebar and views with hexcavator compiled to WebAssembly. Drop a binary
and its debug information on the page; nothing leaves your computer (see
`editors/web`).

## Using it

`hexcavator help` lists the commands. `hexcavator info` identifies files, `hexcavator dump`,
`hexcavator pdb`, and `hexcavator dwarf` show their contents, and the commands below
analyze them. Most analyses take `-json` for scripts and CI, and the
tabular ones `-csv` or `-tsv` too. `hexcavator ltcg` dumps Visual C++ LTCG
objects (`/GL`), and libraries of them: the compiler's intermediate
language, undocumented, read as far as it is known (the back end and its
command line, the source files and their checksums, the linker
directives, the functions and variables, and the CodeView types).

### Size analysis

```
hexcavator size app.exe                          # sections (and COFF groups), libraries, compilands, by its debug information
hexcavator size -d sections,symbols app.exe      # bloaty-style: every byte, by data sources, a level each
hexcavator size -d compileunits -n 10 app.exe -- old\app.exe   # a diff against another build
hexcavator size -d armembers,symbols foo.lib     # objects, import objects, and libraries need no PDB
hexcavator size -d segments,symbols app.elf      # ELF executables, shared objects, objects, and .a archives too
hexcavator size -d archs,segments App.app/App    # Mach-O and universal files too
hexcavator size -d symbols app.wasm              # and WebAssembly modules
hexcavator size -disassemble main app.wasm      # one function's code (PE, ELF, Mach-O, WebAssembly), calls named
hexcavator size -d compileunits -debug-file app.debug app  # DWARF in a separate file (a dSYM next to a Mach-O image is found)
hexcavator size -d compileunits,symbols app.elf  # ELF compile units from DWARF; symbols demangled (fullsymbols, rawsymbols)
hexcavator size -d symbols -source-filter '^std::' app  # only labels a regular expression matches
hexcavator size -stripped -d symbols app.elf     # as if stripped: DWARF and .symtab name things but aren't counted
hexcavator size -c project.bloaty app             # bloaty config files: custom sources rewriting labels
hexcavator size -list-sources                    # sections, segments, groups, symbols, compileunits, ...
hexcavator size -findings app.exe                # what wastes space (duplicate data, template bloat), with advice
hexcavator size -map app.map app.exe             # no PDB? the linker's map attributes symbols to objects and libraries
hexcavator linkerlog link.log -image app.exe     # the largest library members, and why each was linked
hexcavator linkerlog link.log -why gshandler.obj # why one was (from `link ... /VERBOSE > link.log`, or lld's --why-extract)
hexcavator linkerlog new.log -- old.log          # members linked now and not before, and why
```

Reports print as bloaty prints them, or with `-csv`, `-tsv`, or `-json`.

### Queries

```
hexcavator query app.exe "kind=function file>4k sort by file desc limit 20"
hexcavator query app.exe "group by library"                       # count, file and memory bytes per library
hexcavator query app.exe "name~'^std::' group by compiland limit 10"
hexcavator query app.exe "static group by name sort by count desc" # file-scope variables copied into many objects
hexcavator query -columns                                         # name, kind, file, vm, rva, section, group, compiland, library, folded, static, source, line
```

A query is a filter (`column op value`, joined by `and` or a space, `or`,
`not`, and parentheses; `~` is a regular expression; numbers take `k`, `M`,
`G`), then any of `group by`, `sort by ... [desc]`, `select`, and `limit`.
Output is a table, `-csv`, or `-json`; the MCP server's `query` tool takes
the same language.

### Dependencies

```
hexcavator deps app.exe                          # what it loads, as a tree, and what's missing
hexcavator deps -imports app.exe                 # each library's imported symbols, the missing marked
hexcavator deps -problems app.exe                # only what's missing (exit 1 if it won't load)
hexcavator deps -sysroot /mnt/linux-root prog    # another system's libraries (an ELF or Mach-O image on Windows)
```

Like Dependency Walker, for PE, ELF, and Mach-O, from the files alone:
each platform's loader search (known DLLs, the application's directory,
`System32` or `SysWOW64`, API sets by the system's API set schema,
side-by-side assemblies in `WinSxS`; rpath, runpath, `ld.so.conf`, the
default directories; `@rpath`, `@loader_path`, `@executable_path`, the
dyld shared cache), each library's architecture checked, and imports
against exports (PE functions by name and ordinal, forwarders followed; ELF
symbols with their versions; Mach-O binds, re-exports followed). System
libraries are leaves unless `-system`. `tools/deps` builds the test's
images, each platform's with a system of stand-ins.

### Signatures

```
hexcavator signature app.exe                     # its company, product, and versions; Authenticode: signer, timestamp, chain, and whether it's unchanged and trusted
hexcavator signature -json App.app/Contents/MacOS/App   # Apple code signing: each architecture's pages, cdhashes, requirements, Developer ID
hexcavator signature -root my-ca.cer app.exe     # trust another root too (DER or PEM)
```

A Windows binary verifies on macOS or Linux, and a Mac binary on Windows:
the file's hash, the signer's signature, each certificate of the chain,
the timestamps, and what each certificate may be used for are checked here,
with RustCrypto's hashes, RSA, and ECDSA. Trust is a bundled list of roots
(Microsoft's, Apple's, and the public code-signing and timestamping
authorities; `tools/signature/roots.py` fetches and checks them), not the
machine's, so the answer is the same everywhere. Revocation isn't checked
(it needs the network), nor are signatures outside the file (a Windows
catalog's, a notarization ticket). `tools/signature/oracle.ps1` compares the
verdicts with Windows' own (`Get-AuthenticodeSignature`): 3,000 of 3,000
signed files agree. Before the signatures, it shows what the file says it
is, as `sigcheck` does: a PE image's version resource (company, description,
product, versions, copyright; U.S. English where there are several
languages), or a Mach-O image's versions and Info.plist.
`tools/signature/version-oracle.ps1` compares those with Windows'
`FileVersionInfo`: 1,984 of 1,985 agree (Windows takes a localized file's
strings from its `.mui` satellite, which has its own versions; hexcavator reads
the file's own, so those are skipped).

### Resources and metadata

```
hexcavator resources app.exe                     # each resource (by type, name, language) and what it says, in a line
hexcavator resources -show VERSION/#1/1033 app.exe   # one decoded: version information, a dialog's controls, a menu, strings, ...
hexcavator resources -save GROUP_ICON/#1/1033 app.ico app.exe   # an icon group as an .ico file (a cursor's as .cur, a bitmap as .bmp)
hexcavator resources -show Entitlements App.app/Contents/MacOS/App   # a Mach-O image's entitlements (or Info.plist, Versions)
hexcavator resources -all -json libfoo.so        # everything, decoded, as JSON: an ELF image's notes and .comment
```

What a binary holds besides its code, and what it says of itself. A PE
image's resources, decoded: version information, manifests (in a line: the
assemblies it needs, the privileges it asks for), string tables, message
tables, dialogs (`DIALOG` and `DIALOGEX`), menus (`MENU` and `MENUEX`),
accelerators, icon and cursor groups and their images (PNG or bitmap),
bitmaps, HTML, and the rest as text or bytes. A Mach-O image's Info.plist
(`__TEXT,__info_plist`), its code signature's entitlements, and its
versions (platform, minimum OS, SDK, UUID, the bundle's). An ELF image's
notes (build ID, ABI tag, package metadata, GNU properties: IBT, SHSTK, BTI,
PAC, ...), `.comment`, and `.gnu_debuglink`. A WebAssembly module's
`producers`, `target_features`, and other custom sections. A 16-bit
image's resources. `tools/resources` builds the test's images.

### Type layouts

```
hexcavator types app.exe                         # classes, structures, and unions by the bytes their padding wastes
hexcavator types -layout Widget app.exe          # one type: bases, vfptr, vbptr, members (bitfields by bit), holes
hexcavator types new.exe -- old.exe              # the types whose layout changed between builds
```

Layouts come from the PDB's type records (an image's, or a PDB given
directly). An empty base takes no space; virtual bases, whose offsets the
records don't give, are placed after the rest of the object, aligned, as
MSVC places them. A type with a member of a type that has no definition in
the PDB is marked incomplete rather than counted as padding. On clang's
1.45 GB PDB, 292,170 types are laid out in about 2.5 seconds.

### Inlining and source files

```
hexcavator inlines app.exe                       # functions inlined into the code, by their copies' bytes
hexcavator inlines -inlinee "Twine::Twine" app.exe   # one function's copies, by the function each is in
hexcavator sourcefiles app.exe                   # the code by source file (headers too)
hexcavator sourcefiles -file "DenseMap\.h$" app.exe  # one file's code, by line
```

Inline sites come from the PDB's `S_INLINESITE` records and their binary
annotations (`turbonop-pdb`'s `annotations`), which give each inlined
copy's code ranges. A copy's bytes are its own: a site nested in another
claims its bytes first, and the copies of one function that identical code
folding merged count once. Source files come from the line tables; inlined
code counts where it was inlined. On clang: 4.3 million inline sites of
361,000 functions, 40.0 MB of its 82 MB of code, in about 5 seconds.

### What runs before main

```
hexcavator init app.exe                          # what runs when it loads, before main, in order, with what each calls
hexcavator init -depth 4 libfoo.so               # a library: what runs when it's loaded, callees four levels down
hexcavator init -json app.exe                    # as JSON (hexcavator init -json -callees <node>: what a function calls)
```

The phases in the order they run, from the image and its debug
information: a PE image's TLS callbacks, its entry point, and the CRT's
lists (`.rtc$I*`, `.CRT$XI*`, `.CRT$XC*`: global objects' constructors),
each group's entries in order, then `main` (or `DllMain`); an ELF image's
`.preinit_array`, `DT_INIT`, `.ctors`, `.init_array` (through the loader's
relocations, in a position-independent image), and entry point; a Mach-O
image's `__mod_init_func` (or `__init_offsets`) and entry point. What each
function calls comes from the cross-reference graph, in the order its code
calls them (a tail call marked). `tools/init` builds the test's images.

### Symbolizing addresses and crash logs

```
hexcavator symbolize app.exe 0x140001043                   # function, file:line, and inlined functions, innermost first
hexcavator symbolize app.exe@0x7ff6a1b20000 0x7ff6a1b21043 # an address as loaded at a base
hexcavator symbolize libfoo.so libfoo.so+0x1a2b            # an offset from the image's start
hexcavator symbolize app.exe@0x7ff6a1b20000 helper.dll@0x7ffb10c40000 < crash.txt   # a log, each address annotated
```

A lookup reads only what holds the address: a PDB's module (by the section
contributions; type and id records a few KB at a time, not a 350 MB type
stream), or a DWARF compile unit (by its ranges; then only the function's
DIEs). Inline frames come from `S_INLINESITE` annotations or
`DW_TAG_inlined_subroutine`s, each frame's line where the call inlined into
it is. Without debug information, the image's own symbols name addresses
(exports, ELF and Mach-O symbol tables). On every byte of the source lines
test program's code, hexcavator answers as DIA does (the PE image) and as
llvm-symbolizer does (ELF, plain and split DWARF); see
`tools/lines/symbolize-oracle.py`. On clang's 1.4 GB PDB, three addresses
take about 90 ms; 1,000 random addresses in .NET's `libcoreclr.so` about
0.3 s.

### Annotations, batches, and findings compared

```
hexcavator annotations app.exe                   # strings code attached to addresses (__annotation), with file:line
hexcavator batch -r -findings C:\build\bin > sizes.jsonl   # one JSON line per image: sections, libraries, compilands, findings
hexcavator size -findings new.exe -- old.exe     # each finding's change from a base build, item by item
```

### Recommendations

```
hexcavator recommend app.exe          # build changes that would make the image smaller, each with its saving
```

Each recommendation is measured from the image and its debug information,
and what can't be measured isn't made, in the terms of the toolchain that
built it (MSVC's; GCC's and Clang's with GNU ld or lld; Apple's; wasm-ld's):
compile an object so the linker can drop what nothing reaches (`/Gy`,
`/Gw`; `-ffunction-sections`, `-fdata-sections`: what nothing reaches that
the linker kept with the sections it shares, from the cross-reference graph
and the debug information's contributions); link so it does (`/OPT:REF`,
`--gc-sections`, `-dead_strip`), or folds identical functions (`/OPT:ICF`,
`--icf`), when the link didn't (from the linker's command line in a PDB, or
by what the link left); define a duplicated variable once; link without
the libraries nothing of the image uses (`--as-needed`,
`-dead_strip_dylibs`, as its loader finds them); pack relative relocations
(RELR); and more of strings, exports, exception handling, and inlining.
Objects of the toolset's and the system's own libraries are left out: they
can't be rebuilt.

### Cross-references

```
hexcavator xref app.exe                          # references, roots, and what no root reaches
hexcavator xref -why "Square::area" app.exe      # the chain from a root: why a function or datum is in the image
hexcavator xref -refs "Square::\`vftable'" app.exe  # what refers to it, and what it refers to
hexcavator xref -unreachable -n 0 app.exe        # everything no root reaches, largest first
hexcavator xref -vtables app.exe                 # vtables no constructor or destructor stores, and what refers to them
hexcavator xref -identical app.exe               # functions /OPT:ICF could have folded but didn't
hexcavator xref app.exe -- old\app.exe           # functions unchanged, changed, renamed (by fingerprint), added, removed
```

Fingerprints (`turbonop-fingerprint`) hash each function's instructions
with every address masked (a branch inside it counts by offset; anything
else it refers to is a reference, in order), then rehash each with its
references' hashes until the classes stop splitting, so two functions
alike but for their callees differ unless the callees are alike, and
recursion needs nothing special. Read-only data counts by its bytes,
variables by name, and imports by the import table's `dll!function`.
`-identical` counts only what the linker could fold (COMDAT code and
read-only COMDAT data by contents; anything else is only itself): none in
the test program, which is linked with `/OPT:ICF`. Comparing builds pairs
functions by name, then the rest by fingerprint (renamed); a function is
unchanged if its code is and it refers to the same things, or their
renamed pairs.

`hexcavator xref` builds a graph of a PE image (x64, x86, ARM64) from its PDB's
symbols, its code (direct calls and branches, RIP-relative and
image-relative operands, ARM64 `ADRP` pairs), its base relocations
(pointers in data, such as vtable slots), and its exception data (a
function owns its `.pdata`, unwind data, and EH tables, which name its
funclets). The roots are the entry point, the exports, what the loader
reads, and the CRT's initializer tables. A virtual function needs no
special case: its vtable points to it, and the constructors and
destructors that store the vtable refer to it, so the chain runs through
whoever constructs the object (or through the object itself, for one
initialized at compile time; on clang, all 278 vtables no code stores are
such objects'). Data holding pointers that has no symbol (a Rust trait
object's vtable) is split where code takes its address, so each such
object keeps only what it points to.

What no root reaches is code and data the linker kept anyway, and the PDB's
section contributions say why: its section was not a COMDAT (code compiled
without `/Gy`, data without `/Gw`), which the linker keeps whole when
anything in it is used; or it is a COMDAT that such a section refers to; or
it is a COMDAT kept for a reason the graph doesn't see (`/OPT:NOREF`,
`/INCLUDE`, a reference it misses). `testdata/xref` holds the test program
(`tools/xref`), built for x64 and ARM64, where every unreachable item is of
the first two kinds. On clang's 108 MB image (in about 2 seconds, with its
symbols indexed), 16 data items (562 bytes) and no code are unreached; the
reference patterns found by checking the rest against clang's object files'
relocations (MSVC's tables by image offset, scaled and with the base added
after, and across forward jumps) are in `crates/analysis/xref/src/scan.rs`.

`hexcavator mcp` serves the same analysis to LLM agents over the Model Context
Protocol (stdio), with tools `size_report`, `symbols`, `findings`, `why`, `recommend`,
`linker_log`, `query`, and `lookup`; the
first question about an image reads its PDB, and later ones reuse it. For
example, for Claude Code:

```
claude mcp add hexcavator -- C:\path\to\hexcavator.exe mcp
```

## License

Apache-2.0: see the repository's [LICENSE](../../LICENSE) and [NOTICE](../../NOTICE).
