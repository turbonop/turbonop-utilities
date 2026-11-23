// Hexcavator for the web: the VS Code extension's own code
// (editors/vscode/hexcavator, served at ext/), run in the page on
// host/vscode.js's VS Code API and host/node.js's Node modules, with
// hexcavator itself compiled to WebAssembly (wasi/process.js) in the
// extension's `hexcavator serve`'s place.
//
// Files come from a drop on the page or the file picker; each is read
// where it is (the browser's File), a part at a time as hexcavator asks,
// and never leaves the browser. Debug information dropped with a binary
// (a PDB, a .debug file, a dSYM) is beside it, where hexcavator looks.
"use strict";

(async function () {
  const $ = (id) => document.getElementById(id);
  const V = window.HexcavatorVscode;
  const N = window.HexcavatorNode;
  const vscode = V.api;
  const params = new URLSearchParams(location.search);

  // The theme: the system's (or ?theme=light|dark).
  const dark = matchMedia("(prefers-color-scheme: dark)");
  const theme = () => {
    const want = params.get("theme") || (dark.matches ? "dark" : "light");
    document.documentElement.classList.toggle("dark", want === "dark");
    document.documentElement.classList.toggle("light", want !== "dark");
  };
  theme();
  dark.addEventListener("change", theme);

  // Threads need SharedArrayBuffer, which needs the page cross-origin
  // isolated: on a host that can't send the headers, sw.js adds them,
  // once it controls the page (a reload, the first time).
  if (!self.crossOriginIsolated && "serviceWorker" in navigator && location.protocol !== "file:") {
    try {
      await navigator.serviceWorker.register("sw.js");
      await navigator.serviceWorker.ready;
      if (!sessionStorage.getItem("hexcavator.reloaded")) {
        sessionStorage.setItem("hexcavator.reloaded", "1");
        location.reload();
        return;
      }
    } catch (e) {
      console.warn("service worker:", e);
    }
  }
  sessionStorage.removeItem("hexcavator.reloaded");
  if (!HexcavatorRuntime.supported()) {
    const u = $("unsupported");
    u.hidden = false;
    u.textContent =
      "This browser can't run Hexcavator here: it needs SharedArrayBuffer (a cross-origin isolated page), for hexcavator's threads. Try a current Chrome, Edge, Firefox, or Safari, outside a private window.";
    return;
  }

  // The extension: its manifest (menus, commands, settings) and modules.
  const base = new URL("ext/", location.href);
  V.setExtensionBase("ext/");
  const manifest = await (await fetch(new URL("package.json", base))).json();
  V.setManifest(manifest);
  await N.preload(base.href, ["extension.js", "sidebar.js", "lib/hexcavator.js", "media/disasm.js"]);

  const runtime = new HexcavatorRuntime(params.get("wasm") || "hexcavator.wasm");
  N.setRuntime(runtime);
  runtime.warm().catch((e) => vscode.window.showErrorMessage(`Hexcavator couldn't load: ${e.message}`));

  // ------------------------------------------------------------ files

  /** Debug information, not a binary to open: kept beside the binaries. */
  const DEBUG = /\.(pdb|debug|dwp|dbg|dwo|sym|dwarf)$/i;
  const isDebug = (p) => DEBUG.test(p) || /\.dSYM(\/|$)/i.test(p);
  const ROOT = "/files";

  /** Adds files ([relative path, File]) under /files; their paths. */
  function addFiles(list) {
    const paths = [];
    for (const [rel, file] of list) {
      const p = N.path.join(ROOT, rel);
      runtime.add(p, file);
      N.addFile(p, file);
      paths.push(p);
    }
    return paths;
  }

  /** What to open of what was added: each binary, or each top bundle. */
  function openable(paths) {
    const tops = new Map();
    for (const p of paths) {
      const rel = p.slice(ROOT.length + 1);
      const top = rel.split("/")[0];
      if (rel.includes("/")) {
        // A folder dropped: a bundle (an .app, a .framework, a dSYM)
        // opens as the binary in it; any other, each binary in it.
        if (/\.(app|framework|appex|bundle|kext|xpc|plugin|dSYM)$/i.test(top)) {
          tops.set(N.path.join(ROOT, top), "bundle");
          continue;
        }
      }
      if (!isDebug(rel)) {
        tops.set(p, "file");
      }
    }
    // (A dSYM alone: open it; beside a binary, it is the binary's.)
    const bundles = [...tops].filter(([p, k]) => k === "bundle" && /\.dSYM$/i.test(p));
    if (bundles.length && tops.size > bundles.length) {
      bundles.forEach(([p]) => tops.delete(p));
    }
    if (!tops.size) {
      // Only debug information: open it as it is.
      paths.slice(0, 1).forEach((p) => tops.set(p, "file"));
    }
    return [...tops.keys()];
  }

  async function openPaths(paths) {
    for (const p of openable(paths)) {
      await vscode.commands.executeCommand("hexcavator.open", vscode.Uri.file(p));
    }
  }

  // The picker: every file chosen is added; the dialog's answer is the
  // binaries (or the first, for a single pick).
  V.setPickFiles(
    (options) =>
      new Promise((resolve) => {
        const input = $("picker");
        input.value = "";
        input.onchange = () => {
          const list = [...input.files].map((f) => [f.name, f]);
          const added = addFiles(list);
          const open = openable(added);
          resolve(options.canSelectMany ? open : open.slice(0, 1));
        };
        // (Closed without a pick: no change event; resolved on the next focus.)
        const cancelled = () =>
          setTimeout(() => {
            if (!input.files || !input.files.length) {
              resolve([]);
            }
          }, 500);
        window.addEventListener("focus", cancelled, { once: true });
        input.click();
      })
  );

  V.setDownload((name, bytes) => {
    const a = document.createElement("a");
    a.href = URL.createObjectURL(new Blob([bytes]));
    a.download = name;
    a.click();
    setTimeout(() => URL.revokeObjectURL(a.href), 10000);
  });

  // Drops: files and folders (a dSYM is a folder), anywhere on the page.
  let dragDepth = 0;
  addEventListener("dragenter", (e) => {
    if (e.dataTransfer && [...e.dataTransfer.types].includes("Files")) {
      dragDepth++;
      $("drop").hidden = false;
    }
  });
  addEventListener("dragleave", () => {
    if (--dragDepth <= 0) {
      dragDepth = 0;
      $("drop").hidden = true;
    }
  });
  addEventListener("dragover", (e) => e.preventDefault());
  addEventListener("drop", async (e) => {
    e.preventDefault();
    dragDepth = 0;
    $("drop").hidden = true;
    const entries = [...e.dataTransfer.items].map((i) => i.webkitGetAsEntry && i.webkitGetAsEntry()).filter(Boolean);
    const list = [];
    const walk = async (entry, prefix) => {
      if (entry.isFile) {
        const file = await new Promise((res, rej) => entry.file(res, rej));
        list.push([prefix + entry.name, file]);
      } else if (entry.isDirectory) {
        N.addDir(N.path.join(ROOT, prefix + entry.name));
        runtime.add(N.path.join(ROOT, prefix + entry.name));
        const reader = entry.createReader();
        for (;;) {
          const batch = await new Promise((res, rej) => reader.readEntries(res, rej));
          if (!batch.length) {
            break;
          }
          for (const child of batch) {
            await walk(child, `${prefix}${entry.name}/`);
          }
        }
      }
    };
    if (entries.length) {
      for (const entry of entries) {
        await walk(entry, "");
      }
    } else {
      for (const f of e.dataTransfer.files) {
        list.push([f.name, f]);
      }
    }
    await openPaths(addFiles(list));
  });

  // ------------------------------------------------------------ the extension

  const context = {
    subscriptions: [],
    extensionPath: "/ext",
    extensionUri: vscode.Uri.file("/ext"),
    globalState: V.memento(),
    workspaceState: V.memento(),
    asAbsolutePath: (p) => N.path.join("/ext", p),
    extensionMode: 1,
  };
  const extension = N.require("./extension.js");
  extension.activate(context);

  $("open-button").addEventListener("click", () => vscode.commands.executeCommand("hexcavator.open"));
  $("diff-button").addEventListener("click", () => vscode.commands.executeCommand("hexcavator.openDiff"));
  $("welcome-open").addEventListener("click", (e) => {
    e.preventDefault();
    vscode.commands.executeCommand("hexcavator.open");
  });

  // The sidebar's width: dragged.
  const sash = $("sidebar-sash");
  sash.addEventListener("pointerdown", (e) => {
    sash.setPointerCapture(e.pointerId);
    const move = (ev) => document.documentElement.style.setProperty("--sidebar-width", `${Math.max(180, ev.clientX)}px`);
    sash.addEventListener("pointermove", move);
    sash.addEventListener("pointerup", () => sash.removeEventListener("pointermove", move), { once: true });
  });

  // (For tests and links within the site: ?open=<url>,<url> fetches those
  // files, same-origin, and opens them as if dropped.)
  const open = params.get("open");
  if (open) {
    const list = [];
    for (const u of open.split(",")) {
      const url = new URL(u, location.href);
      if (url.origin !== location.origin) {
        continue;
      }
      const blob = await (await fetch(url)).blob();
      const name = url.pathname.split("/").pop();
      list.push([name, new File([blob], name)]);
    }
    await openPaths(addFiles(list));
  }
  window.hexcavatorReady = true;
  document.dispatchEvent(new Event("hexcavator-ready"));
})();
