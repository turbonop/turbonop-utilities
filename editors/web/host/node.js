// The Node modules the Hexcavator extension requires (path, fs, crypto,
// child_process) and a CommonJS `require` to load its files with, for the
// web version.
//
// - fs: the files given to the page (dropped or picked), as the page's
//   file list has them (sizes, directories); reading one reads its File.
// - child_process.spawn: hexcavator, as the WebAssembly process
//   (wasi/process.js), whatever program is named.
"use strict";

(function () {
  // ------------------------------------------------------------ path (POSIX)

  const normalize = (p) => {
    const abs = p.startsWith("/");
    const out = [];
    for (const part of p.split("/")) {
      if (part === "" || part === ".") {
        continue;
      }
      if (part === "..") {
        if (out.length && out[out.length - 1] !== "..") {
          out.pop();
        } else if (!abs) {
          out.push("..");
        }
        continue;
      }
      out.push(part);
    }
    const s = (abs ? "/" : "") + out.join("/");
    return s || (abs ? "/" : ".");
  };
  const path = {
    sep: "/",
    delimiter: ":",
    normalize,
    join: (...parts) => normalize(parts.filter((p) => p !== "").join("/")),
    resolve: (...parts) => {
      let p = "";
      for (const part of parts) {
        p = part.startsWith("/") ? part : `${p}/${part}`;
      }
      return normalize(p.startsWith("/") ? p : `/${p}`);
    },
    dirname: (p) => {
      const s = String(p).replace(/\/+$/, "");
      const i = s.lastIndexOf("/");
      return i < 0 ? "." : i === 0 ? "/" : s.slice(0, i);
    },
    basename: (p, ext) => {
      const s = String(p).replace(/\/+$/, "");
      let b = s.slice(s.lastIndexOf("/") + 1);
      if (ext && b.endsWith(ext) && b !== ext) {
        b = b.slice(0, -ext.length);
      }
      return b;
    },
    extname: (p) => {
      const b = path.basename(p);
      const i = b.lastIndexOf(".");
      return i > 0 ? b.slice(i) : "";
    },
    isAbsolute: (p) => String(p).startsWith("/"),
    relative: (from, to) => {
      const a = normalize(from).split("/").filter(Boolean);
      const b = normalize(to).split("/").filter(Boolean);
      let i = 0;
      while (i < a.length && i < b.length && a[i] === b[i]) {
        i++;
      }
      return [...a.slice(i).map(() => ".."), ...b.slice(i)].join("/");
    },
  };
  path.posix = path;

  // ------------------------------------------------------------ fs

  /** The page's files: path -> File, and the directories they're in. */
  const files = new Map();
  const dirs = new Set(["/"]);
  function addFile(p, file) {
    files.set(p, file);
    for (let d = path.dirname(p); !dirs.has(d); d = path.dirname(d)) {
      dirs.add(d);
    }
  }
  function enoent(p) {
    return Object.assign(new Error(`ENOENT: no such file or directory, '${p}'`), { code: "ENOENT" });
  }
  function stat(p) {
    p = normalize(p);
    const f = files.get(p);
    if (f) {
      return { size: f.size, mtimeMs: f.lastModified || 0, mtime: new Date(f.lastModified || 0), isFile: () => true, isDirectory: () => false };
    }
    if (dirs.has(p)) {
      return { size: 0, mtimeMs: 0, mtime: new Date(0), isFile: () => false, isDirectory: () => true };
    }
    throw enoent(p);
  }
  const fs = {
    existsSync: (p) => {
      try {
        stat(p);
        return true;
      } catch {
        return false;
      }
    },
    statSync: stat,
    readdirSync(p, options) {
      p = normalize(p);
      if (!dirs.has(p)) {
        throw enoent(p);
      }
      const prefix = p === "/" ? "/" : `${p}/`;
      const names = new Map();
      for (const f of files.keys()) {
        if (f.startsWith(prefix)) {
          const rest = f.slice(prefix.length);
          const name = rest.split("/")[0];
          names.set(name, !rest.includes("/"));
        }
      }
      for (const d of dirs) {
        if (d.startsWith(prefix) && d !== p) {
          const name = d.slice(prefix.length).split("/")[0];
          if (!names.has(name)) {
            names.set(name, false);
          }
        }
      }
      return [...names].map(([name, isFile]) =>
        options && options.withFileTypes ? { name, isFile: () => isFile, isDirectory: () => !isFile } : name
      );
    },
    promises: {
      async stat(p) {
        return stat(p);
      },
      async readFile(p, encoding) {
        const f = files.get(normalize(p));
        if (!f) {
          throw enoent(p);
        }
        return encoding ? f.text() : new Uint8Array(await f.arrayBuffer());
      },
    },
    writeFile(_p, _data, cb) {
      if (cb) {
        cb(new Error("the web version doesn't write files"));
      }
    },
  };

  // ------------------------------------------------------------ crypto, Buffer, process

  const crypto = {
    randomBytes(n) {
      const b = new Uint8Array(n);
      self.crypto.getRandomValues(b);
      return {
        toString(enc) {
          if (enc === "base64") {
            return btoa(String.fromCharCode(...b));
          }
          return [...b].map((x) => x.toString(16).padStart(2, "0")).join("");
        },
      };
    },
    createHash() {
      throw new Error("no hashes in the web version");
    },
  };
  const Buffer = {
    from: (data, enc) => (typeof data === "string" ? new TextEncoder().encode(data) : new Uint8Array(data)),
    isBuffer: (b) => b instanceof Uint8Array,
  };
  const process = { platform: "linux", env: {}, arch: "wasm32", versions: {}, cwd: () => "/" };

  // ------------------------------------------------------------ child_process

  let runtime = null;
  const child_process = {
    spawn(_exe, args, options) {
      const env = Object.assign({}, (options && options.env) || {});
      // Every core (hexcavator can't ask the browser how many there are).
      env.HEXC_THREADS = env.HEXC_THREADS || String(navigator.hardwareConcurrency || 4);
      return runtime.spawn(args || [], env);
    },
  };

  // ------------------------------------------------------------ require

  const builtins = { path, fs, crypto, child_process, os: { platform: () => "linux", tmpdir: () => "/tmp", homedir: () => "/" } };
  const cache = new Map();
  /** Loads the extension's modules (fetched first: see `preload`). */
  const sources = new Map();

  function makeRequire(from) {
    return function require(name) {
      if (name === "vscode") {
        return window.HexcavatorVscode.api;
      }
      if (builtins[name]) {
        return builtins[name];
      }
      let full = normalize(name.startsWith(".") ? `${path.dirname(from)}/${name}` : name);
      if (!full.endsWith(".js") && !sources.has(full)) {
        full += ".js";
      }
      if (cache.has(full)) {
        return cache.get(full).exports;
      }
      const src = sources.get(full);
      if (src === undefined) {
        throw new Error(`Cannot find module '${name}' (from ${from})`);
      }
      const module = { exports: {}, id: full, filename: full };
      cache.set(full, module);
      const fn = new Function("module", "exports", "require", "__filename", "__dirname", "process", "Buffer", `${src}\n//# sourceURL=${location.origin}/ext${full}`);
      fn(module, module.exports, makeRequire(full), `/ext${full}`, path.dirname(`/ext${full}`), process, Buffer);
      return module.exports;
    };
  }

  window.HexcavatorNode = {
    path,
    fs,
    process,
    addFile,
    addDir: (d) => {
      for (let x = normalize(d); !dirs.has(x); x = path.dirname(x)) {
        dirs.add(x);
      }
    },
    removeFile: (p) => files.delete(normalize(p)),
    setRuntime: (r) => {
      runtime = r;
    },
    /** Fetches the extension's modules (relative to `base`) for require. */
    async preload(base, names) {
      await Promise.all(
        names.map(async (n) => {
          const res = await fetch(new URL(n, base).href);
          if (!res.ok) {
            throw new Error(`${n}: ${res.status}`);
          }
          sources.set(normalize(`/${n}`), await res.text());
        })
      );
    },
    require: makeRequire("/main.js"),
  };
})();
