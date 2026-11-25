# TurboNOP Utilities

Fast, free tools for looking inside programs, from TurboNOP Systems. The
tools share one set of libraries (`crates/`): file formats, debug
information, instruction decoders, demanglers, and analyses, written here
and kept fast.

| Tool | What it is |
| --- | --- |
| [Hexcavator](apps/hexcavator/README.md) (`apps/hexcavator`) | *Dig into your binaries.* Executables, objects, libraries, and WebAssembly modules with their debug information: on the command line, as an MCP server, in VS Code, and in a browser |

Each tool is released on its own, with tags named for it
(`hexcavator-v0.0.8`).

## Principles

- **Few dependencies.** Every parser, decoder, demangler, and analysis is
  written here. Crates are used only for general-purpose utilities
  (regular expressions, decompression) and for cryptography.
- **Zero-copy.** Files are memory-mapped and parsers borrow from the mapped
  bytes. Parsing is lazy and keeps minimal state; decoded instructions are not
  cached long-term but re-decoded from the original bytes.
- **Host-independent.** Any target's binaries can be analyzed on any host.
  On-disk integers have an explicit byte order (`U32Le`, `U32Be`, or a
  runtime `Endian`), on-disk structures have alignment 1, and target-derived
  sizes are `u64` until checked against the host buffer. Host-specific code
  lives only in `turbonop-mmap`.
- **Hostile input.** Parsers never panic on malformed data; they return
  errors or partial results.
- **Read view and edit model are separate.** Reading is a borrowed view; editing
  (rare) builds an owned copy-on-write model and serializes it.

## Layout

| Crate | Purpose |
|---|---|
| `crates/base/core` (`turbonop-core`) | Endian types, `Pod` overlays, bounds-checked readers, typed addresses, a bump arena |
| `crates/base/mmap` (`turbonop-mmap`) | Read-only file mapping (Win32 / POSIX `mmap`) |
| `crates/formats/archive` (`turbonop-archive`) | `ar` archives (`.lib`, `.a`) in their GNU, Microsoft, and BSD forms, and thin archives: members, long names, symbol tables |
| `crates/formats/elf` (`turbonop-elf`) | ELF files of either class and byte order: headers (extended numbering), segments, sections, symbols, notes and build IDs, relocations, `.eh_frame`/`.eh_frame_hdr` |
| `crates/formats/etl` (`turbonop-etl`) | Reader for Windows event trace log (ETL) files |
| `crates/formats/image` (`turbonop-image`) | A format-independent view of executables and objects: architecture, sections, debug identity |
| `crates/formats/linkerlog` (`turbonop-linkerlog`) | The MSVC linker's `/VERBOSE` output: why each library member was linked, what `/OPT:REF` discarded and `/OPT:ICF` folded |
| `crates/formats/linkmap` (`turbonop-linkmap`) | Linker maps (MSVC `/MAP` so far): COFF groups, and every symbol with its object and library |
| `crates/formats/lx` (`turbonop-lx`) | LE and LX (linear executable) files: VxDs, DOS extenders, and 32-bit OS/2 modules |
| `crates/formats/macho` (`turbonop-macho`) | Mach-O files of either width and byte order, and universal files: load commands, segments, sections, symbols, link-edit tables |
| `crates/formats/minidump` (`turbonop-minidump`) | Minidumps (MDMP): Windows, Breakpad, and Crashpad crash dumps (threads, modules, memory, exception, system information) |
| `crates/formats/mz` (`turbonop-mz`) | MS-DOS MZ headers; dispatch to PE/NE/LE/LX new headers |
| `crates/formats/ne` (`turbonop-ne`) | 16-bit NE (New Executable) files: Windows 1.x-3.x and OS/2 1.x modules and fonts |
| `crates/formats/pe` (`turbonop-pe`) | PE images and COFF objects: headers, sections, symbols, RVA mapping, debug directory, imports, exports, relocations, load config (Guard CF, DVRT, CHPE/ARM64EC metadata), exception and unwind data, C++ EH and SEH handler data, TLS; Visual C++ LTCG (`/GL`) objects' IL |
| `crates/formats/signature` (`turbonop-signature`) | Code signatures, verified the same on every platform: DER, X.509, CMS, Authenticode (nested signatures, RFC 3161 and PKCS #9 timestamps), and Apple code signing (code directories, page hashes, requirements, entitlements); the cryptography is RustCrypto's, trust a bundled set of roots |
| `crates/formats/wasm` (`turbonop-wasm`) | WebAssembly modules: sections, the name section, imports, globals, function bodies, data segments |
| `crates/symbols/codeview` (`turbonop-codeview`) | CodeView records, as cvinfo.h defines them: symbols, types (with a C++-style formatter and type layouts), C13 line and checksum subsections, inline annotations, register numbers |
| `crates/symbols/debuginfo` (`turbonop-debuginfo`) | A format-independent view of debug information (a PDB, DWARF, or a COFF object's CodeView, as a PDB): compilands, contributions, groups, symbols, types, lines, and finding an image's |
| `crates/symbols/demangle` (`turbonop-demangle`) | Symbol demanglers: Microsoft C++ (a syntax tree of the name, for queries, printed exactly as `UnDecorateSymbolName` does), Itanium C++, Rust (v0 and legacy), and Swift |
| `crates/symbols/dwarf` (`turbonop-dwarf`) | DWARF 2-5: units, abbreviations, DIEs and attributes, string and address indirection, range and location lists, aranges, name tables, each with where its bytes lie |
| `crates/symbols/pdb` (`turbonop-pdb`) | The PDB container: MSF, info, DBI, TPI/IPI with their hash streams, publics and globals, module streams |
| `crates/symbols/symbolize` (`turbonop-symbolize`) | Finding an image's symbols and the function at an address, for traces and dumps |
| `crates/isa/arm32` (`turbonop-arm32`) | AArch32 (A32 and T32) instruction decoder |
| `crates/isa/riscv` (`turbonop-riscv`) | RISC-V instruction decoder (RV32 and RV64, the standard extensions: compressed, vector, crypto, ...), from LLVM's TableGen records, written as LLVM writes it |
| `crates/isa/loongarch` (`turbonop-loongarch`) | LoongArch instruction decoder (LA32 and LA64; FP, LSX, LASX, LVZ, LBT), from LLVM's TableGen records, written as LLVM writes it |
| `crates/isa/powerpc` (`turbonop-powerpc`) | PowerPC instruction decoder (32- and 64-bit, big- and little-endian; Altivec, VSX, ISA 3.1's prefixed instructions, MMA), from LLVM's TableGen records, written as llvm-objdump writes it |
| `crates/isa/mips` (`turbonop-mips`) | MIPS instruction decoder (MIPS I to MIPS64 Release 6, big- and little-endian; MSA, DSP, Octeon; microMIPS, its 16- and 32-bit instructions and Release 6's), from LLVM's TableGen records, written as llvm-objdump writes it |
| `crates/isa/tablegen` (`turbonop-tablegen`) | Instruction decoding by tables generated from LLVM's TableGen records (`tools/tablegen`): the decoder, aliases, complex operands, and compressed instructions' full forms that the RISC-V, LoongArch, PowerPC, and MIPS crates share |
| `crates/isa/arm64` (`turbonop-arm64`) | AArch64 decoder (base, FP, AdvSIMD, SVE/SVE2, SME/SME2, ...) generated from Arm's AARCHMRS and LLVM's AArch64 target, with a dumpbin-style formatter, typed operands, and each instruction's reads and writes |
| `crates/isa/wasm` (`turbonop-wasm-isa`) | The WebAssembly instruction decoder (the whole instruction set, through GC, threads, and relaxed SIMD) with text output |
| `crates/isa/x86` (`turbonop-x86`) | x86/x86-64 decoder (legacy, x87, SSE, AVX, AVX-512, AMX, ...), what each instruction reads and writes, and dumpbin-style formatter |
| `crates/debug/abi` (`turbonop-abi`) | Calling conventions for each platform: which registers a call preserves, argument, result, and return-address registers, red zones, home space, and DWARF register numbers |
| `crates/debug/unwind` (`turbonop-unwind`) | Virtual stack unwinders: recover a caller's registers from a frame's registers, its stack, and the image's unwind data (Windows' `.pdata`, DWARF CFI, Apple's compact unwind, ARM EHABI), picked for an image of any format |
| `crates/analysis/code` (`turbonop-code`) | Code independent of the instruction set: instructions' control flow, and functions' basic blocks |
| `crates/analysis/fingerprint` (`turbonop-fingerprint`) | Fingerprints of functions and data that don't depend on where they are: instructions with addresses masked, then what they refer to, refined until stable (as linkers fold identical code), so builds compare and unfolded duplicates show |
| `crates/analysis/program` (`turbonop-program`) | Loading a program once: an image, its debug information, its model and symbols, for every analysis and front end |
| `crates/analysis/query` (`turbonop-query`) | A small query language over tables of an image's facts: filters, grouping with counts and summed sizes, sorting, limits; shared by `hexcavator query` and the MCP server |
| `crates/analysis/size` (`turbonop-size`) | Size attribution: which sections, COFF groups, libraries, compilands, and symbols account for each byte (a SizeBench replacement), and bloaty-style reports by data source for images, objects, and libraries |
| `crates/analysis/deps` (`turbonop-deps`) | What an executable or library loads: its libraries, found as each platform's loader would find them (PE, ELF, Mach-O), and the imports they lack |
| `crates/analysis/xref` (`turbonop-xref`) | Cross-references: a graph of what each function and datum refers to (calls, branches, addresses code uses, pointers in data), found by disassembly, and why each is reachable from the image's roots |
| `crates/trace/etw` (`turbonop-etw`) | Decoders for Windows kernel and trace-control events in ETL files |
| `crates/trace/trace` (`turbonop-trace`) | A platform-neutral model of performance traces, and importers that build it |
| `crates/trace/trace-analysis` (`turbonop-trace-analysis`) | Queries over traces: stack trees, callers and callees, CPU and wait analysis |
| `crates/debugger/target` (`turbonop-target`) | Debugging targets: minidumps and Linux core files as one model (threads and their registers by DWARF number, modules and their identities, the exception, memory), the address space over a target's memory and its modules' images with each byte's provenance, and its modules' images and debug information found by identity, locally or from symbol servers, all at once |
| `crates/debugger/eval` (`turbonop-eval`) | A frame's variables, evaluated: PDB def-ranges and DWARF location expressions (including entry values, through the caller's call sites) read with the frame's registers and the target's memory, or the reason they can't be |
| `crates/debugger/frames` (`turbonop-frames`) | Stack walking: every thread's frames, with each register's provenance in each frame by the platform's ABI, signal frames, and frame-pointer and stack-scan fallbacks that check for calls, each frame symbolized with its inlined calls |
| `crates/debugger/paths` (`turbonop-paths`) | How did we get here: which way each branch of a crashed frame's function went, by its control flow and by the values that survive in the dump, each decision with its proof |
| `apps/hexcavator` | `hexcavator`: the command line, the MCP server for agents, and the server the VS Code extension runs |
| `apps/dump-analyzer` | `dump-analyzer`: post-mortem debugging of crash dumps (minidumps; Linux and Mach-O core files) from the command line, the server the "Dump Analyzer" VS Code extension runs, and an MCP server for agents |
| `apps/fuzz` | The fuzzer: every parser and command, on mutated inputs, for crashes, hangs, and blowups |
| `apps/xtask` | Development tasks: `cargo xtask test` builds every test and runs them all at once |
| `editors/vscode/hexcavator` | The VS Code extension: a sidebar of binaries and builds compared, with their headers, symbols, types, sources, data, and disassembly, size treemaps, and why each thing is there |
| `editors/web` | The web version: the extension's sidebar and views in a browser, with hexcavator compiled to WebAssembly (wasi-threads, its threads Web Workers) |

## Building

```
cargo build
cargo xtask test
cargo run -- info C:\Windows\System32\kernel32.dll
cargo run -- dump /headers C:\Windows\System32\kernel32.dll
```

`hexcavator dump` mirrors `dumpbin`, and accepts options as `/name`, `-name`, or
`--name` (use the dash forms from Git Bash, which rewrites `/name` into a
path).

### Testing

`cargo xtask test` builds every test and runs all the test binaries at
once (`cargo test` runs them one after another): about five seconds once
built. Arguments go to `cargo test` (`-p turbonop-x86`), and after
`--` to the test binaries (a name filter). Each crate's integration
tests are one binary, `tests/it`, a module per file.

The tests are hermetic: it reads nothing outside this repository (not
the system's files, not installed tools), so it gives the same answer on
every machine. The extended tests compare Hexcavator with outside tools
(dumpbin, LLVM, XED, SizeBench, swift-demangle, Windows' own unwinder) on
files outside the repository (system DLLs, Visual Studio's, large
binaries, traces); they are in a separate repository, `hexcavator-private`,
whose README lists what each needs. What they find is kept here as golden
files (`tests/data` beside each crate), which `cargo test` checks.

`python tools/fixtures.py` regenerates the test data the tools here build
(test programs from MSVC and LLVM, and the tools' views of them that the
tests compare with), every step at once, with the paths they record the
same on every machine.

The dumpbin comparison covers system images, the fixtures in `tools/fixtures`
(x86, x64, ARM64, ARM64EC), and Visual Studio's ARM64 and ARM64X DLLs. ARM64
`/unwindinfo` shows each unwind code next to the instruction at its address,
as dumpbin does, and decodes the handler data the way dumpbin does: by what
it looks like, not by which handler it belongs to.

### turbonop-x86 testing

`turbonop-x86`'s opcode tables (`crates/isa/x86/tables/*.txt`) are written in the
Intel SDM's operand notation and compiled by `build.rs`. They are checked
against `dumpbin /disasm` by enumerating whole opcode spaces (every opcode with
every ModRM byte under prefix, VEX, and EVEX field combinations: about 11
million encodings) and by disassembling system DLLs (about 4.5 million
instructions). Where dumpbin disagrees with the SDM, the test classifies the
case (`known_divergence` in `crates/isa/x86/tests/dumpbin.rs`) rather than
copying dumpbin. A sample of the agreed results is kept in
`crates/isa/x86/tests/data/golden.txt` so that `cargo test` covers the decoder on
every host. Not yet supported: AVX512-FP16, most of AVX10.2, APX (REX2 is
decoded; EVEX promotions are not), AMX-AVX512, and Xeon Phi extensions.

What each instruction reads and writes (`crates/isa/x86/tables/access.rules`)
is checked against Intel XED: `tools/x86access` builds XED and runs it over an
encoding of every opcode table form, into
`crates/isa/x86/tests/data/access.txt`. Nothing XED reads or writes may be
missing, and every write XED doesn't have is listed with its reason.

### turbonop-arm64 testing

`turbonop-arm64`'s tables (`crates/isa/arm64/tables/a64.txt`) are generated by
`tools/arm64/extract.py` from openly licensed sources only: Arm's Machine
Readable Specification (AARCHMRS) and LLVM's AArch64 target (neither is
needed to build). They hold every encoding's fixed bits, its
UNDEFINED and CONSTRAINED UNPREDICTABLE conditions, alias preferences, and
assembler template; `build.rs` compiles the conditions to Rust. The output
follows Microsoft's ARM64 syntax, and is checked against `dumpbin /disasm` on
millions of pseudo-random words, which cover the encoding space in proportion
to its size. The test classifies every disagreement: features and encodings
dumpbin does not decode (listed in `crates/isa/arm64/tests/support/mod.rs`),
encodings ARM reserves that dumpbin decodes anyway, and dumpbin's bugs (in
`crates/isa/arm64/tests/dumpbin.rs`). `crates/isa/arm64/tests/data/golden.txt` keeps up
to four agreed words of every encoding for the hermetic test.

What each instruction reads and writes (`crates/isa/arm64/tables/a64-access.txt`)
is generated by `tools/arm64/access.py` from LLVM's records: each encoding's
operands, written or read (tied ones, writeback bases, both), implicit
registers, and MCInstrDesc's flags, with explicit rules for what LLVM leaves
out (FPSR, MSR's system registers, SMSTART's zeroing) or gets wrong (MRS
writing NZCV). `crates/isa/arm64/tests/data/access.txt` is llvm-mc's view of
four words of every encoding (`tools/arm64/access_golden.py`): every register
it says a word writes or reads, turbonop-arm64 does. The operands are checked
against the formatted text and the access data on the same words.

### turbonop-demangle testing

`turbonop-demangle` parses a decorated name into a tree in an arena (so a
`Demangler` reused across names allocates nothing once warm), with limits
on nesting depth and on the work back-references can cause, since names
come from untrusted files. Its output was compared with `undname` (which
shares dumpbin's undecorator) over about 50,000 distinct names from the MSVC
libraries and C++ DLLs, under the complete, `NO_PTR64`, and `NAME_ONLY`
flags (`examples/compare`), C++/CLI and C++/CX names included. Where the
two disagree, undname is wrong: it spells a back-referenced anonymous
namespace by its tag (`A0x1234abcd::`), does not count a variable
template's own name as a back-reference, and cannot read names containing
`<lambda_...>` or `<unnamed-type-...>`.
`crates/symbols/demangle/tests/data/golden.txt` keeps a sample for the hermetic
test.

### turbonop-size testing

`hexcavator size -sizebench <image>` prints the analysis in the form
`tools/sizebench-oracle` prints SizeBench's. `testdata/sizebench` holds
SizeBench's own test binaries (x86, x64, ARM32; MSVC, clang-cl/lld-link,
Rust, Zig, Fortran) and SizeBench's analysis of each; `cargo test` checks
that sections, COFF groups, libraries, compilands, every contributed byte
range, and every symbol (names included) agree, but for classified
divergences where SizeBench or DIA is wrong or arbitrary. An extended test
does the same for SizeBench's two large external binaries and for clang:
over a million symbols, none unexplained. Large real-world binaries for performance work
are built from LLVM with `tools/llvm/build-msvc.cmd`. `testdata/coff` holds
objects and libraries built by `tools/coff/build.cmd`.

ELF follows bloaty's `elf.cc` (labels, the segments base map, the
catch-all fallbacks, table entries charged to the symbols they describe),
with its bugs classified and not copied (see `crates/analysis/size/src/elf.rs`).
`testdata/elf` holds clang/ld.lld output for x86-64, i386, AArch64 (both
byte orders), and big-endian ARM, built by `tools/elf/build.cmd` with the
`llvm-readobj` dumps that `turbonop-elf` is checked against field by field.

## License

Hexcavator is licensed under the [Apache License 2.0](LICENSE) (see also
[`NOTICE`](NOTICE)). Some parts are derived from others' work: code that
follows the LLVM Project's or is generated from it, the Swift demangler
(translated from the Swift Project's), the ARM instruction tables
(generated from Arm's machine-readable specification), and some x86
tables (generated from Intel XED's data). Those parts also carry their
sources' licenses, all permissive. The crate holding each part has its
source's license file (`LICENSE-LLVM`, `LICENSE-SWIFT`, `LICENSE-ARM`,
`NOTICE-XED`), and [`THIRD_PARTY_NOTICES.md`](THIRD_PARTY_NOTICES.md)
says what came from where. `hexcavator licenses` prints the notices of
everything the program includes. The licenses test keeps that list
current, checks each crate's `license` against its license files, and
allows only permissive licenses.
