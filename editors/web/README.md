# Hexcavator for the web

The VS Code extension's sidebar and views in a browser, with hexcavator
itself compiled to WebAssembly: drop a binary and its debug information
on the page (or pick them) and explore it as in VS Code. Nothing is
uploaded; every file is read where it is, in the browser. The release
workflow publishes it to GitHub Pages.

## How it works

- **The extension's own code.** `extension.js`, `sidebar.js`, and the
  views in `media/` (editors/vscode/hexcavator) run unchanged in the page:
  `host/vscode.js` is the part of VS Code's API they use (the tree view,
  webview panels as tabs, quick picks, notifications, the status bar,
  menus from the extension's package.json with their `when` clauses), and
  `host/node.js` the Node modules (path, fs, child_process).
- **hexcavator as a process.** `wasi/` runs hexcavator built for
  `wasm32-wasip1-threads` in the extension's `hexcavator serve`'s place:
  `process.js` gives it Node's child_process interface; each thread is a
  Web Worker (`thread-worker.js`) sharing one memory; `fs-worker.js` owns
  the files and standard input and output, answering every thread's
  WASI calls (it reads a browser File a slice at a time, as hexcavator
  asks).
- **Threads** need SharedArrayBuffer, which needs a cross-origin isolated
  page: `sw.js` adds the headers that make it so where the host can't
  (GitHub Pages).

WebAssembly's memory is 32-bit, 4 GiB: clang.exe with its 1.1 GB PDB
fits, with room to spare for most binaries. (See TODO.md for what's
planned: reading files by their parts, and an allocator that lets more
than two threads help.)

## Trying it

```
cargo build --release -p hexcavator --target wasm32-wasip1-threads
python editors/web/serve.py --wasm target/wasm32-wasip1-threads/release/hexcavator.wasm --codicons <codicons>/dist
```

(Build with `RUSTFLAGS="-C link-arg=--max-memory=4294967296"` so that its
memory may grow to 4 GiB. `<codicons>` is the `@vscode/codicons` package,
`npm pack @vscode/codicons@0.0.36`, for the icons.) Then open
http://127.0.0.1:8000/. `--local <dir>` serves a directory of your own at
`/local/`, and `?open=/local/app.exe,/local/app.pdb` opens files from it.

## Tests

```
python editors/web/test/run.py --wasm <hexcavator.wasm> --codicons <codicons>/dist
```

runs `test/runtime.html` (hexcavator serve in the page, on a test binary)
and `test/app.html` (the page as a person uses it: the sidebar, the
Symbols page, a tool's output, a row's context menu, disassembly) in a
headless Edge or Chrome.

## The site

`site.py` assembles what Pages serves: these files, the extension's code
and media under `ext/`, `hexcavator.wasm`, the icons, and the licenses.
