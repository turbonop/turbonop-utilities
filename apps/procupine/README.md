# Procupine

The processes running on a machine, as a process explorer shows them:
Windows, Linux, and macOS.

- **`procupine list`**: every process, with its CPU usage over an
  interval, memory, threads, handles (or open files), and user; as a
  tree (`-tree`), sorted (`-sort cpu|memory|io|name|pid`), filtered
  (`-filter TEXT`), or as JSON (`-json`).
- **`procupine kill|terminate|suspend|resume PID[@START]...`**: a process
  named by its ID and start (as `list -json` gives it, as `key`) is that
  process or none: never a later one given its ID.
- **`procupine serve`**: samples for an editor, as JSON lines (see
  `src/serve.rs`). The VS Code extension
  ([editors/vscode/procupine](../../editors/vscode/procupine/README.md))
  runs it.

What each platform's collector reads, and how CPU is counted, is in
`crates/system/process` (`turbonop-process`).
