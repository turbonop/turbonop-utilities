// hexcavator's WebAssembly build (wasm32-wasip1-threads) run as a process,
// with the interface of Node's child_process.spawn as the extension uses
// it: stdin.write and end, stdout's and stderr's "data", "exit", kill.
//
// A process is a shared memory, a main thread (which runs _start), and a
// pool of threads for those it spawns, each a Web Worker
// (thread-worker.js), with the file system worker (fs-worker.js) every
// process shares, which holds the files given to it. Threads need
// SharedArrayBuffer, which needs the page cross-origin isolated (see
// sw.js).
"use strict";

(function () {
  const HEADER = 4;
  const SLOT = 4;
  const STARTING = 4;
  /** The most threads a process has. */
  const MOST = 256;

  let compiled = null;

  /** The module, compiled once, and its memory's limits (from its import). */
  function module(url) {
    if (!compiled) {
      compiled = (async () => {
        const res = await fetch(url);
        if (!res.ok) {
          throw new Error(`${url}: ${res.status} ${res.statusText}`);
        }
        const bytes = new Uint8Array(await res.arrayBuffer());
        const limits = memoryLimits(bytes);
        return { module: await WebAssembly.compile(bytes), limits };
      })();
      compiled.catch(() => {
        compiled = null;
      });
    }
    return compiled;
  }

  /** The `env.memory` import's limits (pages): { initial, maximum }. */
  function memoryLimits(b) {
    let p = 8;
    const leb = () => {
      let r = 0;
      let s = 0;
      for (;;) {
        const x = b[p++];
        r += (x & 0x7f) * 2 ** s;
        s += 7;
        if (x < 0x80) {
          return r;
        }
      }
    };
    const name = () => {
      const n = leb();
      const v = new TextDecoder().decode(b.subarray(p, p + n));
      p += n;
      return v;
    };
    while (p < b.length) {
      const id = b[p++];
      const size = leb();
      const end = p + size;
      if (id === 2) {
        for (let i = leb(); i > 0; i--) {
          const m = name();
          const f = name();
          const kind = b[p++];
          if (kind === 0) {
            leb();
          } else if (kind === 1) {
            p++;
            const flags = b[p++];
            leb();
            if (flags & 1) {
              leb();
            }
          } else if (kind === 2) {
            const flags = b[p++];
            const initial = leb();
            const maximum = flags & 1 ? leb() : 65536;
            if (m === "env" && f === "memory") {
              return { initial, maximum };
            }
          } else if (kind === 3) {
            p++;
            p++;
          } else {
            break;
          }
        }
      }
      p = end;
    }
    return { initial: 64, maximum: 65536 };
  }

  /** A shared memory as large as the browser allows, up to the module's. */
  function sharedMemory(limits) {
    for (const maximum of [limits.maximum, 49152, 32768, 16384]) {
      if (maximum > limits.maximum || maximum < limits.initial) {
        continue;
      }
      try {
        return new WebAssembly.Memory({ initial: limits.initial, maximum, shared: true });
      } catch {
        // (Too much address space asked for: less.)
      }
    }
    throw new Error("this browser can't give hexcavator a shared memory");
  }

  class Emitter {
    constructor() {
      this.handlers = new Map();
    }
    on(event, fn) {
      if (!this.handlers.has(event)) {
        this.handlers.set(event, []);
      }
      this.handlers.get(event).push(fn);
      return this;
    }
    emit(event, ...args) {
      for (const fn of this.handlers.get(event) || []) {
        try {
          fn(...args);
        } catch (e) {
          console.error(e);
        }
      }
    }
  }

  class Stream extends Emitter {
    setEncoding() {
      return this;
    }
  }

  let nextPid = 1;

  /**
   * A running hexcavator: `fs` the file system worker, `url` the module's,
   * `args` its arguments (without the program's name), `env` its
   * environment.
   */
  class WasmProcess extends Emitter {
    constructor(fs, url, args, env) {
      super();
      this.fs = fs;
      this.pid = nextPid++;
      this.workers = [];
      this.done = false;
      this.held = [];
      this.stdout = new Stream();
      this.stderr = new Stream();
      this.decoders = { stdout: new TextDecoder(), stderr: new TextDecoder() };
      const self = this;
      this.stdin = Object.assign(new Emitter(), {
        // (Held until the process has started: starting resets input.)
        write(text) {
          if (!self.done) {
            self.input(() => fs.postMessage({ kind: "stdin", text }));
          }
          return true;
        },
        end() {
          if (!self.done) {
            self.input(() => fs.postMessage({ kind: "stdin-end" }));
          }
        },
      });
      this.onFs = (e) => {
        const m = e.data;
        if (this.done || (m.kind !== "stdout" && m.kind !== "stderr")) {
          return;
        }
        const t = this.decoders[m.kind].decode(m.bytes, { stream: true });
        if (t) {
          this[m.kind].emit("data", t);
        }
      };
      fs.addEventListener("message", this.onFs);
      this.start(url, args, env).catch((e) => {
        this.emit("error", e);
        this.finish(null);
      });
    }

    /** Sends input now, or once the process has started. */
    input(send) {
      if (this.held) {
        this.held.push(send);
      } else {
        send();
      }
    }

    async start(url, args, env) {
      const { module: mod, limits } = await module(url);
      if (this.done) {
        return;
      }
      this.memory = sharedMemory(limits);
      this.pool = new SharedArrayBuffer((HEADER + MOST * SLOT) * 4);
      this.sys = new SharedArrayBuffer(MOST * 2 * 4);
      const pool = new Int32Array(this.pool);
      pool[0] = 1;
      for (let i = 0; i < MOST; i++) {
        pool[HEADER + i * SLOT] = STARTING;
      }
      this.fs.postMessage({ kind: "start", memory: this.memory, sys: this.sys });
      const held = this.held;
      this.held = null;
      held.forEach((f) => f());
      this.init = { module: mod, memory: this.memory, pool: this.pool, sys: this.sys, args: ["hexcavator", ...args], env };
      const threads = Math.min(MOST - 1, (navigator.hardwareConcurrency || 4) * 2 + 4);
      this.grow(threads);
      this.thread(0);
    }

    /** `n` more pool threads. */
    grow(n) {
      const pool = new Int32Array(this.pool);
      const from = Atomics.load(pool, 2) || 1;
      const to = Math.min(MOST, from + n);
      for (let i = from; i < to; i++) {
        this.thread(i);
      }
      Atomics.store(pool, 2, to);
    }

    message(m) {
      if (this.done) {
        return;
      }
      if (m.kind === "grow") {
        this.grow(8);
      } else if (m.kind === "exit") {
        this.finish(m.code);
      } else if (m.kind === "trap") {
        this.stderr.emit("data", `hexcavator stopped: ${m.message}\n`);
        this.finish(134);
      }
    }

    finish(code) {
      if (this.done) {
        return;
      }
      this.done = true;
      for (const w of this.workers) {
        w.terminate();
      }
      this.workers = [];
      this.fs.removeEventListener("message", this.onFs);
      this.fs.postMessage({ kind: "stop" });
      this.emit("exit", code);
    }

    kill() {
      this.finish(null);
    }
  }

  // (Resolved against this script's own URL: the workers beside it.)
  const here = document.currentScript.src;

  /** The file system worker and the processes run with it. */
  class Runtime {
    constructor(wasmUrl) {
      this.url = wasmUrl;
      this.fs = new Worker(new URL("fs-worker.js", here).href);
    }
    /** Adds `file` (a File) at `path`, or a directory if no file. */
    add(path, file) {
      this.fs.postMessage({ kind: "add", path, file: file || null });
    }
    remove(path) {
      this.fs.postMessage({ kind: "remove", path });
    }
    spawn(args, env) {
      return new WasmProcess(this.fs, this.url, args, env || {});
    }
    /** Whether this browser can run it: threads need cross-origin isolation. */
    static supported() {
      return typeof SharedArrayBuffer === "function" && self.crossOriginIsolated === true;
    }
    /** Compiles the module ahead of the first request. */
    warm() {
      return module(this.url);
    }
  }

  /** A thread: the main one (slot 0), or a pool thread. */
  WasmProcess.prototype.thread = function (slot) {
    const w = new Worker(new URL("thread-worker.js", here).href);
    const channel = new MessageChannel();
    this.fs.postMessage({ kind: "port", port: channel.port1 }, [channel.port1]);
    w.onmessage = (e) => this.message(e.data);
    w.onerror = (e) => this.message({ kind: "trap", message: e.message });
    w.postMessage({ kind: "init", ...this.init, slot, main: slot === 0, port: channel.port2 }, [channel.port2]);
    this.workers.push(w);
  };

  window.HexcavatorRuntime = Runtime;
})();
