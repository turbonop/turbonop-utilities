# Dump Analyzer

Post-mortem debugging of crash dumps in VS Code's debugger: Windows,
Linux, and macOS minidumps (`MiniDumpWriteDump`'s, Breakpad's, Crashpad's)
and Linux core files. Nothing else to install: no WinDbg, no gdb.

Open a dump (**Dump Analyzer: Open Crash Dump**, or a `.dmp` file's
context menu in the Explorer, or a `dump-analyzer` launch configuration),
and the debugger stops at its exception:

- **Call Stack:** every thread's frames, with the calls inlined at each.
  A frame found by unwind data is certain; one found by the frame-pointer
  chain or a scan of the stack is shown dimmed (likely, not certain).
- **Variables:** each frame's parameters and locals, as their types say, or
  why one can't be read (optimized out here; in a register a call
  clobbered; in memory the dump didn't capture). **Registers**, each with
  where its value came from (the dump's context, saved on the stack, the
  CFA, a return address) or clobbered.
- **Disassembly** (Open Disassembly View), and **memory** (a pointer's
  memory, from the Variables view).
- **Debug Console:** `dump-analyzer` commands on the dump: `modules`
  (where each module's image and debug information were found, or looked
  for), `memory 0x1000 64`, `reconstruct -thread 4` (probable stacks,
  rebuilt from a corrupt stack's memory), `summary`.

Modules' images and debug information are looked for beside the dump, in
the launch configuration's `symbols` directories, in the symbol
directories (`_NT_SYMBOL_PATH`), and, if `dumpAnalyzer.symbolServers` or
`dumpAnalyzer.debuginfod` name any, on those servers (asking one tells it
which builds the dump has).

```json
{
  "type": "dump-analyzer",
  "request": "launch",
  "name": "crash",
  "dump": "${workspaceFolder}/crash.dmp",
  "symbols": ["${workspaceFolder}/build"],
  "sysroot": ["\\\\wsl$\\Ubuntu"]
}
```
