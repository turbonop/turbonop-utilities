// One thread of hexcavator's WebAssembly process (wasm32-wasip1-threads):
// the main thread (which runs _start) or a pool thread (which runs the
// threads the program spawns, one after another, as wasi-threads'
// wasi_thread_start). Every thread shares the process's memory.
//
// The system calls that touch files or standard input and output go to
// the file system worker (fs-worker.js), which owns them: this thread
// posts the call and waits on its slot in `sys` (a SharedArrayBuffer)
// until the file system worker has written the answer into memory. The
// rest (arguments, environment, clocks, randomness) are answered here.
//
// Spawning a thread takes an idle pool thread through `pool` (a
// SharedArrayBuffer too): its state from idle (0) to claimed (3) to run
// (1), with the thread's ID and start argument. Pool threads start before
// any are needed, so a spawn never waits on a browser starting a worker
// (which needs the spawner's event loop, which a blocked thread doesn't
// run); when none are idle, the page is asked for more, and the spawn
// waits for one.
"use strict";

/** Pool slot layout: Int32 words per slot, after the header's. */
const HEADER = 4; // [next thread ID, ready count, slots, unused]
const SLOT = 4; // [state, tid, arg, unused]
const IDLE = 0;
const RUN = 1;
const RUNNING = 2;
const CLAIMED = 3;

// WASI errno values used here.
const ESUCCESS = 0;
const EAGAIN = 6;
const EBADF = 8;
const EINVAL = 28;
const ENOSYS = 52;

class Exit extends Error {
  constructor(code) {
    super(`exit ${code}`);
    this.code = code;
  }
}

let memory;
let instance;
let pool;
let sys;
let slot;
let port;
let argv = [];
let envv = [];

function view() {
  return new DataView(memory.buffer);
}

/** Bytes of memory, copied out (TextDecoder takes no shared views). */
function bytes(ptr, len) {
  return new Uint8Array(memory.buffer).slice(ptr, ptr + len);
}

/** A file-system call, made by the file system worker; its errno. */
function call(op, ...args) {
  const at = slot * 2;
  Atomics.store(sys, at, 0);
  port.postMessage({ op, args, slot });
  Atomics.wait(sys, at, 0);
  return Atomics.load(sys, at + 1);
}

/** The arguments' or environment's strings, NUL-terminated, as WASI lays them out. */
function strings(list) {
  return list.map((s) => new TextEncoder().encode(s + "\0"));
}

function sizes(list, countPtr, sizePtr) {
  const v = view();
  const enc = strings(list);
  v.setUint32(countPtr, enc.length, true);
  v.setUint32(sizePtr, enc.reduce((n, e) => n + e.length, 0), true);
  return ESUCCESS;
}

function lay(list, ptrs, buf) {
  const v = view();
  const mem = new Uint8Array(memory.buffer);
  let at = buf;
  strings(list).forEach((e, i) => {
    v.setUint32(ptrs + i * 4, at, true);
    mem.set(e, at);
    at += e.length;
  });
  return ESUCCESS;
}

/** A monotonic clock shared by every worker: the page's time origin, in ns. */
function now() {
  return BigInt(Math.round((performance.timeOrigin + performance.now()) * 1e6));
}

function spawn(arg) {
  const tid = Atomics.add(pool, 0, 1);
  for (let tries = 0; tries < 6000; tries++) {
    // (Read each time: the page adds threads while this waits.)
    const slots = Atomics.load(pool, 2);
    for (let i = 1; i < slots; i++) {
      const b = HEADER + i * SLOT;
      if (Atomics.compareExchange(pool, b, IDLE, CLAIMED) === IDLE) {
        Atomics.store(pool, b + 1, tid);
        Atomics.store(pool, b + 2, arg);
        Atomics.store(pool, b, RUN);
        Atomics.notify(pool, b);
        return tid;
      }
    }
    if (tries === 0) {
      postMessage({ kind: "grow" });
    }
    // (Until a thread finishes, or the page's new ones are ready.)
    Atomics.wait(pool, 1, Atomics.load(pool, 1), 10);
  }
  return -EAGAIN;
}

const wasi = {
  args_sizes_get: (c, s) => sizes(argv, c, s),
  args_get: (p, b) => lay(argv, p, b),
  environ_sizes_get: (c, s) => sizes(envv, c, s),
  environ_get: (p, b) => lay(envv, p, b),
  clock_time_get(id, _precision, out) {
    const t = id === 0 ? BigInt(Date.now()) * 1000000n : now();
    view().setBigUint64(out, t, true);
    return ESUCCESS;
  },
  random_get(buf, len) {
    const mem = new Uint8Array(memory.buffer);
    for (let at = 0; at < len; at += 65536) {
      const n = Math.min(65536, len - at);
      const chunk = new Uint8Array(n);
      crypto.getRandomValues(chunk);
      mem.set(chunk, buf + at);
    }
    return ESUCCESS;
  },
  sched_yield: () => ESUCCESS,
  proc_exit(code) {
    throw new Exit(code);
  },
};
for (const op of [
  "fd_close",
  "fd_fdstat_get",
  "fd_filestat_get",
  "fd_prestat_get",
  "fd_prestat_dir_name",
  "fd_read",
  "fd_readdir",
  "fd_seek",
  "fd_write",
  "path_create_directory",
  "path_filestat_get",
  "path_open",
  "path_readlink",
  "path_rename",
  "path_unlink_file",
]) {
  wasi[op] = (...args) => call(op, ...args);
}

/** Any other WASI call a newer build imports: not supported. */
function imports(module) {
  const w = {};
  for (const imp of WebAssembly.Module.imports(module)) {
    if (imp.module === "wasi_snapshot_preview1" && imp.kind === "function") {
      w[imp.name] = wasi[imp.name] || (() => ENOSYS);
    }
  }
  return {
    env: { memory },
    wasi_snapshot_preview1: w,
    wasi: { "thread-spawn": spawn },
  };
}

/** What ended a thread: an exit, or a trap (a panic aborts). */
function ended(e) {
  if (e instanceof Exit) {
    postMessage({ kind: "exit", code: e.code });
  } else {
    postMessage({ kind: "trap", message: String((e && e.stack) || e) });
  }
}

function poolLoop() {
  const b = HEADER + slot * SLOT;
  Atomics.store(pool, b, IDLE);
  Atomics.add(pool, 1, 1);
  Atomics.notify(pool, 1);
  for (;;) {
    Atomics.wait(pool, b, IDLE);
    if (Atomics.load(pool, b) !== RUN) {
      continue;
    }
    Atomics.store(pool, b, RUNNING);
    const tid = Atomics.load(pool, b + 1);
    const arg = Atomics.load(pool, b + 2);
    try {
      instance.exports.wasi_thread_start(tid, arg);
    } catch (e) {
      ended(e);
      return;
    }
    Atomics.store(pool, b, IDLE);
    // (A spawner waiting for an idle thread.)
    Atomics.notify(pool, 1);
  }
}

onmessage = async (e) => {
  const m = e.data;
  if (m.kind !== "init") {
    return;
  }
  ({ memory, pool, sys, slot, port } = m);
  pool = new Int32Array(pool);
  sys = new Int32Array(sys);
  argv = m.args;
  envv = Object.entries(m.env).map(([k, v]) => `${k}=${v}`);
  try {
    instance = await WebAssembly.instantiate(m.module, imports(m.module));
  } catch (err) {
    postMessage({ kind: "trap", message: `instantiate: ${err}` });
    return;
  }
  if (m.main) {
    postMessage({ kind: "started" });
    try {
      instance.exports._start();
      postMessage({ kind: "exit", code: 0 });
    } catch (err) {
      ended(err);
    }
  } else {
    poolLoop();
  }
};
