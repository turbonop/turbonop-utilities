# Procupine

The processes running on this machine, as a process explorer shows them, in
VS Code: Windows, Linux, and macOS. In a remote window (SSH, WSL, a dev
container), the remote machine's.

Run **Procupine: Show Processes** from the Command Palette.

- **A live tree** of every process by parent, or a flat list: CPU (as
  Process Explorer counts it on Windows: by cycles), private bytes and
  working set, I/O per second, threads, handles (or open files), user,
  and command line. Sort by any column, in the tree as well (siblings are
  sorted); choose columns by right-clicking the header.
- **History** for every process: a CPU sparkline in its row, and CPU,
  private bytes, and I/O graphs in its details below, for the last 120
  samples. The machine's CPU, memory, and commit charge above.
- **Started and exited** processes are marked green and red for a sample,
  as Process Explorer marks them; suspended ones are greyed, other users'
  tinted.
- **Filter** (Ctrl+F) by name, ID, user, path, or command line: in the
  tree, the matches' ancestors are kept around them.
- **Kill** (Delete), **kill a tree** (Shift+Delete), **terminate**
  (SIGTERM, on Linux and macOS), **suspend**, and **resume**, for one or
  many (Ctrl- and Shift-click). A process is acted on by its ID *and*
  start, so a process that has exited since is never confused with a
  later one given its ID.
- **Copy** a process's ID, name, path, or command line; **reveal** its
  executable; or open it in [Hexcavator](https://marketplace.visualstudio.com/items?itemName=turbonop.hexcavator-vscode)
  if it is installed.

The keyboard: arrows move (Left and Right collapse and expand; Left on a
leaf goes to its parent), typing a name finds it, Space pauses, T toggles
the tree.

Without administrator rights (root), other users' processes show less,
and can't be acted on: the panel says so.

## Settings

| Setting | |
| --- | --- |
| `procupine.interval` | How often to sample, in milliseconds (1000) |
| `procupine.confirmKill` | Ask before killing (true) |
| `procupine.path` | The `procupine` to run (the one carried, if empty) |
