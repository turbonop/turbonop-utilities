# lld's COFF tests with turbolink

`apps/turbolink-lldtests` runs lld's COFF tests (`llvm-project/lld/test/COFF`,
lit tests) with turbolink in lld-link's place and sorts each result into a
class. The tests stay in llvm-project; the runner reads them there.

## Running it

```text
start /low /b /wait turbolink-lldtests --tests <llvm-project>\lld\test\COFF ^
    --tools <llvm-build>\bin --turbolink <turbolink.exe> --work <scratch dir> ^
    --report report.md --tsv results.tsv [--threads 8] [--timeout 600]
```

- `--tools` is an LLVM build's `bin` (FileCheck, yaml2obj, llvm-mc,
  llvm-readobj, llvm-objdump, not, split-file, ...). lld isn't needed:
  `lld-link` on the tests' PATH is a copy of turbolink.
- Each test's RUN lines run in one Git bash (lit keeps a `cd` from one line to
  the next), `set -o pipefail`, with lit's substitutions (`%s %S %p %t %T
  %basename_t %errc_* %python`) and MSYS's argument conversion off (`/dll`
  stays `/dll`).
- Tests run in parallel; start it at low priority on a shared machine.
- It prints the counts per class; `--report` writes the Markdown below (each
  class's causes, most tests first), `--tsv` a line per test: its name,
  class, cause, and the failing RUN line.

## Classes

- **passes**: every RUN line succeeded.
- **skipped**: not run here: another machine than x64 (ARM, ARM64, ARM64EC,
  32-bit x86, MIPS, by REQUIRES or by what the test names), LTO (bitcode
  inputs), another lld flavor, or a feature this setup lacks (DIA SDK,
  mt.exe, libxml2, GNU tar).
- **lld's wording only**: turbolink did what lld does (failed where it
  fails, succeeded where it succeeds) and only lld's own text didn't match:
  its messages' wording, `/verbose` output checked from a file.
- **lld-only option**: the test uses an option link.exe doesn't have
  (`/lldmap`, `-mllvm`, `/lldltocache`, `/wrap`, `/threads`, ...).
- **turbolink rejects**: turbolink fails on an option or input it should
  take, or crashes.
- **turbolink accepts**: turbolink succeeds where the test expects an error.
- **image differs**: a dump of turbolink's image (llvm-readobj, llvm-objdump,
  llvm-nm, llvm-pdbutil, ...) doesn't match the test's checks.
- **harness**: a tool other than turbolink failed.

Where link.exe and lld differ, turbolink follows link.exe; such tests are
listed below as they're found, with what link.exe does.

## Results

turbolink at main after #83 (the runner's first full run):

| Class | Tests |
|---|---:|
| harness | 2 |
| image differs | 76 |
| lld's wording only | 49 |
| lld-only option | 36 |
| passes | 55 |
| skipped | 252 |
| turbolink accepts | 16 |
| turbolink rejects | 77 |
| **all** | 563 |

### harness

- after lld-link: cp: cannot stat '…': No such file or directory (1): pdb-relative-source-lines.test
- not:  (1): implib-name-mingw.test

### image differs

- FileCheck \| FileCheck (3): ar-comdat.test, cgprofile-print.s, export.test
- FileCheck failed: CHECK: expected string not found in input (1): cl-gl.test
- FileCheck failed: CHECK: expected string not found in input (1): conflict-mangled.test
- FileCheck failed: MISSING: expected string not found in input (1): driver.test
- FileCheck failed: ERROR: expected string not found in input (1): force-multiple.test
- FileCheck failed: ERROR: expected string not found in input (1): force.test
- FileCheck failed: CHECK1: expected string not found in input (1): include.test
- FileCheck failed: CHECK: expected string not found in input (1): include2.test
- FileCheck failed: CHECK: expected string not found in input (1): includeoptional.yaml
- FileCheck failed: MANIFEST: expected string not found in input (1): manifest.test
- FileCheck failed: MAP-EMPTY: is not on the line after the previous match (1): map.test
- FileCheck failed: CHECK1: expected string not found in input (1): nodefaultlib.test
- FileCheck failed: CHECK: expected string not found in input (1): section-order.test
- cat \| FileCheck (1): time-trace.s
- llvm-objdump failed: CHECK-NEXT: expected string not found in input (1): eh_frame_terminator.s
- llvm-objdump failed: CHECK-NEXT: expected string not found in input (1): icf-vtables-itanium.s
- llvm-objdump failed: DATA: expected string not found in input (1): imports-gnu-only.s
- llvm-objdump failed: TEXT-NEXT: expected string not found in input (1): imports.test
- llvm-objdump failed: CHECK: expected string not found in input (1): reloc-x64.test
- llvm-objdump failed: CHECK-NEXT: expected string not found in input (1): string-tail-merge.s
- llvm-objdump failed: CHECK: expected string not found in input (1): tls_suffix_sorting.s
- llvm-objdump failed: UNWIND: expected string not found in input (1): unwind.test
- llvm-pdbutil failed: CHECK-NEXT: expected string not found in input (1): pdb-comdat.test
- llvm-pdbutil failed: CHECK-NEXT: expected string not found in input (1): pdb-empty-sec.s
- llvm-pdbutil failed: CHECK-NEXT: expected string not found in input (1): pdb-file-static.test
- llvm-pdbutil failed: CHECK-NEXT: is not on the line after the previous match (1): pdb-global-gc.yaml
- llvm-pdbutil failed: CHECK-NEXT: expected string not found in input (1): pdb-import-gc.yaml
- llvm-pdbutil failed: CHECK-NEXT: expected string not found in input (1): pdb-procid-remapping.test
- llvm-pdbutil failed: CHECK-LABEL: expected string not found in input (1): pdb-publics-import.test
- llvm-pdbutil failed: RAW-NEXT: expected string not found in input (1): pdb-same-name.test
- llvm-pdbutil failed: CHECK-NEXT: expected string not found in input (1): pdb-secrel-absolute.yaml
- llvm-pdbutil failed: CHECK-NEXT: expected string not found in input (1): pdb-source-lines.test
- llvm-pdbutil failed: CHECK-NEXT: expected string not found in input (1): pdb-symbol-types.yaml
- llvm-pdbutil failed: CHECK: expected string not found in input (1): pdb-tpi-aligned-records.test
- llvm-pdbutil failed: DUMP: expected string not found in input (1): pdb-type-server-guid-collision-valid.test
- llvm-pdbutil failed: CHECK-NEXT: expected string not found in input (1): pdb.test
- llvm-pdbutil failed: CHECK-NOT: excluded string found in input (1): pgo-pubs.s
- llvm-pdbutil failed: CHECK: expected string not found in input (1): s_udt.s
- llvm-readobj failed: CHECK-NEXT: expected string not found in input (1): associative-comdat-mingw-weak.s
- llvm-readobj failed: CHECK-NEXT: expected string not found in input (1): associative-comdat-mingw.s
- llvm-readobj failed: CHECK-NEXT: expected string not found in input (1): associative-comdat.s
- llvm-readobj failed: CHECK-NEXT: expected string not found in input (1): baserel-sort.yaml
- llvm-readobj failed: SHARED: expected string not found in input (1): bss-shared-no-merge.test
- llvm-readobj failed: CHECK: expected string not found in input (1): cfguard-off-instrumented.s
- llvm-readobj failed: CHECK-NEXT: is not on the line after the previous match (1): code-merge.s
- llvm-readobj failed: CHECK: expected string not found in input (1): combined-resources.test
- llvm-readobj failed: IMPORT-NEXT: expected string not found in input (1): delayimports.test
- llvm-readobj failed: CHECK-NEXT: expected string not found in input (1): delayimporttables.yaml
- llvm-readobj failed: NOBC-NOT: excluded string found in input (1): discard-section.test
- llvm-readobj failed: CUI: expected string not found in input (1): entry-inference-mingw.s
- llvm-readobj failed: CHECK: expected string not found in input (1): entry-weak-external.s
- llvm-readobj failed: CHECKCETCOMPAT: expected string not found in input (1): exdllcharacteristics.test
- llvm-readobj failed: CHECK-NEXT: is not on the line after the previous match (1): gfids-export.s
- llvm-readobj failed: CHECK-NEXT: expected string not found in input (1): gfids-relocations64.s
- llvm-readobj failed: CHECK: expected string not found in input (1): guard-ehcont.s
- llvm-readobj failed: CHECK: expected string not found in input (1): guard-longjmp.s
- llvm-readobj failed: CHECK-NEXT: expected string not found in input (1): guardcf-thunk.s
- llvm-readobj failed: EXPORT: expected string not found in input (1): imports-gnu-autoexport.s
- llvm-readobj failed: IMPORT-NEXT: is not on the line after the previous match (1): imports-gnu.test
- llvm-readobj failed: CHECK: expected string not found in input (1): imports-mangle.test
- llvm-readobj failed: CHECK-INCLUDEOPTIONAL: expected string not found in input (1): includeoptional-export.s
- llvm-readobj failed: CHECK-NEXT: expected string not found in input (1): merge-resource-manifest.test
- llvm-readobj failed: CHECK: expected string not found in input (1): merge.test
- llvm-readobj failed: CHECK-RESOURCES-NEXT: expected string not found in input (1): mixed-resource-obj.yaml
- llvm-readobj failed: CHECKNOTMERGED-NEXT: expected string not found in input (1): nodbgdirmerge.test
- llvm-readobj failed: CHECK: expected string not found in input (1): resource-objs.test
- llvm-readobj failed: RESOURCE_INFO: expected string not found in input (1): resource.test
- llvm-readobj failed: CHECK: expected string not found in input (1): secidx-absolute.s
- llvm-readobj failed: CHECK: expected string not found in input (1): secrel-common.s
- llvm-readobj failed: W-NEXT: is not on the line after the previous match (1): section.test
- llvm-readobj failed: CHECK1: expected string not found in input (1): stub.test
- llvm-readobj failed: CHECK2: expected string not found in input (1): subsystem.test
- llvm-readobj failed: CHECK: expected string not found in input (1): tls-used-gc.s
- llvm-readobj failed: llvm-readobj.exe: error: '…': no such file or directory (1): wholearchive.s

### lld's wording only

- output or warning text (18): align.s, debug-fastlink.test, deploadflag-cfg-short.s, empty-subsection.s, export-deleting-dtor.s, icf-assoc-order.s, and 12 more
- lld-link's output, checked from a file (12): entry-inference.test, entry-inference2.test, entry-inference3.test, entry-inference4.test, icf-associative.test, icf-circular.test, and 6 more
- error: error LNK2001: unresolved external symbol … (6): associative-comdat-empty.test, filename-casing.s, imports-static-lib.test, line-error.yaml, undefined-symbol.s, weak-preference.test
- error: error LNK1104: cannot open file '…': The system cannot find the file specified. (os error 2) (2): could-not-open.test, error-limit.test
- error: error LNK1221: a subsystem can'…' (2): duplicate-absolute.s, pdbpagesize.test
- error: error LNK1123: duplicate resource: type Number(6), name Number(3), language 0x409 (1): force-multipleres.test
- error: error LNK1327: failure during running mt.exe: program not found (1): manifestinput-error.test
- error: error LNK2005: thunk-replace.s/thunk-replace.s.tmp.other.lib(thunk-replace.s.tmp.other.obj): MessageBoxA already defined (1): thunk-replace.s
- error: error LNK2005: main already defined (1): duplicate-cv.s
- error: error LNK2005: symbol … (1): comdat-selection.s
- error: error: relocation (type 0x1) at 0x1000 to a symbol … (1): reloc-discarded-early2.s
- error: fatal error: /EXPORT:func,EXPORTAS,: EXPORTAS needs a name (1): exportas.test
- error: fatal error: invalid option '…' (1): invalid-debug.test
- error: fatal error: option '…' needs a value (1): invalid-entry.s

### lld-only option

- /lldmingw (21): autoimport-debug.s, autoimport-gc.s, autoimport-list-ptrs.s, autoimport-refptr.s, comdat-gcc-compatibility-size.test, comdat-jumptable.s, and 15 more
- /call-graph-ordering-file (3): cgprofile-bad-clusters.s, cgprofile-err.s, cgprofile-icf.s
- /lldmap (3): empty-section-decl.yaml, gc-dwarf.s, order.test
- /wrap (2): wrap-with-archive.s, wrap.s
- /build-id (1): build-id-sym.s
- /end-lib (1): start-lib-cmd-diagnostics.ll
- /includeoptional (1): includeoptional2.yaml
- /inferasanlibs (1): inferasanlibs.test
- /pdbstripped (1): pdbstripped.test
- /reproduce (1): linkrepro-thin-archives.s
- /vfsoverlay (1): vfsoverlay.test

### skipped

- another machine: x86 (32-bit) (73): alias-implib.s, allow-unknown-debug-info.test, associative-comdat-mingw-i386.s, autoimport-gnu-implib.s, autoimport-handler-func.s, autoimport-nowarn.s, and 67 more
- LTO (bitcode) (70): alternatename-lto.ll, autoimport-lto.ll, conflict.test, dll.test, entry-mangled.test, entrylib.ll, and 64 more
- requires aarch64 (66): arm64-delayimport.yaml, arm64-dynamicbase.s, arm64-import2.test, arm64-localimport-align.s, arm64-relocs-imports.test, arm64-thunks.s, and 60 more
- requires arm (19): arm-thumb-branch20-error.s, arm-thumb-branch20t-encoding.s, arm-thumb-thunks-multipass.s, arm-thumb-thunks-pdb.s, arm-thumb-thunks.s, armnt-blx23t.test, and 13 more
- another machine: ARM (5): armnt-dynamicbase.test, armnt-entry-point.test, armnt-imports.test, locally-imported-armnt.test, reloc-arm.test
- requires diasdk (5): pdb-globals-dia-func-collision3.test, pdb-globals-dia-vfunc-collision.test, pdb-globals-dia-vfunc-collision2.test, pdb-globals-dia-vfunc-simple.test, pdb-natvis.test
- another machine: ARM64 (4): arm64-magic.yaml, locally-imported-arm64.test, pdata-arm64-bad.yaml, pdata-arm64.yaml
- unsupported here (4): color-diagnostics.test, linkrepro-res.test, linkrepro.test, lto-cache-errors.ll
- requires gnutar (2): linkrepro-manifest.test, linkrepro-pdb.test
- requires manifest_tool (2): manifestinput-nowarning.test, manifestinput.test
- requires libxml2 (1): manifest-uac.test
- requires mips (1): reloc-mips.test

### turbolink accepts

- — (16): alternatename-antidep.s, associative-comdat-order.test, comdat-gcc-compatibility.s, delayimports-error.test, directives-unsupported.s, duplicate.test, and 10 more

### turbolink rejects

- fatal error: invalid option '…' (19): cgprofile-obj.s, cgprofile-txt.s, comdat-drectve.s, debug-dwarf.test, gfids-corrupt.s, gfids-fallback.s, and 13 more
- error LNK2001: unresolved external symbol … (13): alternatename-lib.s, cfguard-weak-undef.s, directives.s, fixed.test, ignore-many.test, ignore4217.yaml, and 7 more
- crash (7): crt-chars.test, crt-dyn-initializer-order.test, default-alignment.test, export-limit.s, invalid-obj.test, no-ipi-stream.test, and 1 more
- error LNK1221: a subsystem can'…' (5): pdb-global-hashes.test, pdb-globals.test, pdb-linker-module.test, pdbaltpath.test, subsystem-inference-weak-external.s
- unrecognized option /LIB (4): def-export-cpp.s, imported-dllmain.test, out.test, thin-archive.s
- unrecognized option /SUMMARY (4): pdb-type-server-simple.test, precomp-link-samename.test, precomp-link.test, precomp-summary-fail.test
- error LNK2005: main already defined (3): base.test, common-alignment.test, common.test
- error: relocation (type 0x4) at 0x1007 to a symbol … (3): dependentflags.test, deploadflag-cfg-x64.s, functionpadmin.test
- fatal error: no input files (3): implib-name.test, lib.test, strtab.s
- unrecognized option /HELP (2): help.test, linkenv.test
- unrecognized option /TIMESTAMP:0 (2): checksum.test, linkreprofullpathrsp.test
- error LNK1104: cannot open file '…' (1): visitedlibs.test
- error LNK1104: invalid or corrupt file: a symbol … (1): debug.test
- error LNK1104: invalid or corrupt file: turbonop_pe::raw::SectionHeader: truncated at offset 0x595e (1): lto-directives.test
- error: '/DEBUGTYPE:pdata' is not supported yet (1): pdb-none.test
- error: '/DRIVER' is not supported yet (1): driver-opt.s
- error: relocation (type 0x1) at 0x3008 to a symbol … (1): reloc-discarded-dwarf.s
- unrecognized option /-RSP-QUOTING=FOOBAR (1): responsefile.test
- unrecognized option /NOTAREALOPT=ION (1): wx.s
- unrecognized option /PDBSTREAM:srcsrv=Inputs/stream.txt (1): pdbstream.test
- unrecognized option /SECTIONLAYOUT:doesnotexist.txt (1): sectionlayout.test
- unrecognized option /TIMESTAMP:4294967296 (1): timestamp.test
- warning LNK4099: bad-block-size.pdb: bytes: truncated at offset 0x0; its types are not in the PDB (LNK4099) (1): pdb-type-server-native-errors.yaml

## Where link.exe differs from lld

None recorded yet.
