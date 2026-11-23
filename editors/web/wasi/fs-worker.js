// The file system of hexcavator's WebAssembly process, and its standard
// input and output: the WASI calls that touch them, made for every thread
// (thread-worker.js posts each call here and waits for its answer).
//
// Files are the browser's (File objects, from a drop or a file picker),
// read a slice at a time with FileReaderSync as hexcavator asks for them:
// a file is never held whole here, only in hexcavator's memory, if it
// reads it whole. Files hexcavator writes (an index, say) are kept in
// memory. The page adds files under /files.
//
// Standard input is the page's requests (hexcavator serve's), queued: a
// read with none waiting is answered when one comes. Standard output and
// error go to the page as they are written.
"use strict";

const ESUCCESS = 0;
const EBADF = 8;
const EEXIST = 20;
const EINVAL = 28;
const EISDIR = 31;
const ENOENT = 44;
const ENOTDIR = 54;
const ENOTEMPTY = 55;
const ESPIPE = 70;

const CHAR = 2;
const DIR = 3;
const FILE = 4;

/** The most read at once from a File (a read may return less). */
const MOST = 64 << 20;

const RIGHTS = 0x1fffffffn;

let ino = 1;
function dir() {
  return { type: DIR, ino: ino++, children: new Map(), mtime: Date.now() };
}
const root = dir();

let memory = null;
let sys = null;
let fds = new Map();
let nextFd = 4;
let stdin = [];
let stdinEnded = false;
/** fd_read(0) calls waiting for input: [slot, iovs, n, nread]. */
let waiting = [];

function reset() {
  fds = new Map([
    [0, { kind: "stdin" }],
    [1, { kind: "stdout" }],
    [2, { kind: "stderr" }],
    [3, { kind: "dir", node: root, preopen: "/" }],
  ]);
  nextFd = 4;
  stdin = [];
  stdinEnded = false;
  waiting = [];
}
reset();

function mem() {
  return new Uint8Array(memory.buffer);
}
function view() {
  return new DataView(memory.buffer);
}
function text(ptr, len) {
  return new TextDecoder().decode(mem().slice(ptr, ptr + len));
}

/** The node at `path` (components) under `from`; or an errno. */
function walk(from, parts) {
  let node = from;
  const stack = [];
  for (const p of parts) {
    if (p === "" || p === ".") {
      continue;
    }
    if (p === "..") {
      node = stack.pop() || root;
      continue;
    }
    if (node.type !== DIR) {
      return ENOTDIR;
    }
    const next = node.children.get(p);
    if (!next) {
      return ENOENT;
    }
    stack.push(node);
    node = next;
  }
  return node;
}

function split(path) {
  return path.split("/").filter((p) => p !== "");
}

/** A directory descriptor's node and the path under it: [dir, name, parts]. */
function at(fd, ptr, len) {
  const d = fds.get(fd);
  if (!d || d.kind !== "dir") {
    return EBADF;
  }
  const parts = split(text(ptr, len));
  return { base: d.node, parts };
}

function add(path, file) {
  const parts = split(path);
  let node = root;
  for (const p of parts.slice(0, -1)) {
    let next = node.children.get(p);
    if (!next) {
      next = dir();
      node.children.set(p, next);
    }
    node = next;
  }
  const name = parts[parts.length - 1];
  node.children.set(name, file ? { type: FILE, ino: ino++, file, data: null, mtime: file.lastModified || Date.now() } : dir());
}

function size(node) {
  if (node.type === FILE) {
    return node.data ? node.data.length : node.file.size;
  }
  return 0;
}

function stat(node, ptr) {
  const v = view();
  for (let i = 0; i < 64; i += 8) {
    v.setBigUint64(ptr + i, 0n, true);
  }
  v.setBigUint64(ptr + 8, BigInt(node.ino || 0), true);
  v.setUint8(ptr + 16, node.type);
  v.setBigUint64(ptr + 24, 1n, true);
  v.setBigUint64(ptr + 32, BigInt(size(node)), true);
  const t = BigInt(node.mtime || 0) * 1000000n;
  v.setBigUint64(ptr + 40, t, true);
  v.setBigUint64(ptr + 48, t, true);
  v.setBigUint64(ptr + 56, t, true);
}

/** The iovecs' [ptr, len]. */
function iovecs(iovs, n) {
  const v = view();
  const out = [];
  for (let i = 0; i < n; i++) {
    out.push([v.getUint32(iovs + i * 8, true), v.getUint32(iovs + i * 8 + 4, true)]);
  }
  return out;
}

/** `len` bytes of a file node from `pos`, as a Uint8Array. */
function readNode(node, pos, len) {
  if (node.data) {
    return node.data.subarray(pos, pos + len);
  }
  const end = Math.min(node.file.size, pos + len);
  if (end <= pos) {
    return new Uint8Array(0);
  }
  return new Uint8Array(new FileReaderSync().readAsArrayBuffer(node.file.slice(pos, end)));
}

/** A file node's bytes as written to (copied out of its File first). */
function writable(node) {
  if (!node.data) {
    node.data = node.file ? readNode(node, 0, node.file.size).slice() : new Uint8Array(0);
    node.file = null;
  }
  return node;
}

function write(node, pos, chunk) {
  writable(node);
  const end = pos + chunk.length;
  if (end > node.data.length) {
    const grown = new Uint8Array(Math.max(end, node.data.length * 2));
    grown.set(node.data);
    node.data = grown.subarray(0, end);
  }
  node.data.set(chunk, pos);
  node.mtime = Date.now();
}

/** Standard input given to the readers waiting for it. */
function feed() {
  while (waiting.length && (stdin.length || stdinEnded)) {
    const [slot, iovs, n, nread] = waiting.shift();
    let total = 0;
    const m = mem();
    for (const [ptr, len] of iovecs(iovs, n)) {
      let room = len;
      let at = ptr;
      while (room > 0 && stdin.length) {
        const chunk = stdin[0];
        const take = Math.min(room, chunk.length);
        m.set(chunk.subarray(0, take), at);
        at += take;
        room -= take;
        total += take;
        if (take === chunk.length) {
          stdin.shift();
        } else {
          stdin[0] = chunk.subarray(take);
        }
      }
    }
    view().setUint32(nread, total, true);
    answer(slot, ESUCCESS);
  }
}

const calls = {
  fd_prestat_get(fd, ptr) {
    const d = fds.get(fd);
    if (!d || !d.preopen) {
      return EBADF;
    }
    view().setUint8(ptr, 0);
    view().setUint32(ptr + 4, new TextEncoder().encode(d.preopen).length, true);
    return ESUCCESS;
  },
  fd_prestat_dir_name(fd, ptr, len) {
    const d = fds.get(fd);
    if (!d || !d.preopen) {
      return EBADF;
    }
    mem().set(new TextEncoder().encode(d.preopen).subarray(0, len), ptr);
    return ESUCCESS;
  },
  fd_fdstat_get(fd, ptr) {
    const d = fds.get(fd);
    if (!d) {
      return EBADF;
    }
    const v = view();
    const type = d.kind === "dir" ? DIR : d.kind === "file" ? FILE : CHAR;
    v.setUint8(ptr, type);
    v.setUint16(ptr + 2, d.append ? 1 : 0, true);
    v.setBigUint64(ptr + 8, RIGHTS, true);
    v.setBigUint64(ptr + 16, RIGHTS, true);
    return ESUCCESS;
  },
  fd_filestat_get(fd, ptr) {
    const d = fds.get(fd);
    if (!d) {
      return EBADF;
    }
    stat(d.node || { type: CHAR, ino: 0 }, ptr);
    return ESUCCESS;
  },
  path_filestat_get(fd, _flags, ptr, len, out) {
    const a = at(fd, ptr, len);
    if (typeof a === "number") {
      return a;
    }
    const node = walk(a.base, a.parts);
    if (typeof node === "number") {
      return node;
    }
    stat(node, out);
    return ESUCCESS;
  },
  path_open(fd, _dirflags, ptr, len, oflags, _rights, _inheriting, fdflags, out) {
    const a = at(fd, ptr, len);
    if (typeof a === "number") {
      return a;
    }
    const CREAT = 1;
    const DIRECTORY = 2;
    const EXCL = 4;
    const TRUNC = 8;
    let node = walk(a.base, a.parts);
    if (typeof node === "number") {
      if (node !== ENOENT || !(oflags & CREAT) || !a.parts.length) {
        return node;
      }
      const parent = walk(a.base, a.parts.slice(0, -1));
      if (typeof parent === "number") {
        return parent;
      }
      if (parent.type !== DIR) {
        return ENOTDIR;
      }
      node = { type: FILE, ino: ino++, file: null, data: new Uint8Array(0), mtime: Date.now() };
      parent.children.set(a.parts[a.parts.length - 1], node);
    } else if (oflags & CREAT && oflags & EXCL) {
      return EEXIST;
    }
    if (oflags & DIRECTORY && node.type !== DIR) {
      return ENOTDIR;
    }
    if (oflags & TRUNC && node.type === FILE) {
      writable(node).data = new Uint8Array(0);
    }
    const handle = node.type === DIR ? { kind: "dir", node } : { kind: "file", node, pos: 0, append: (fdflags & 1) !== 0 };
    const n = nextFd++;
    fds.set(n, handle);
    view().setUint32(out, n, true);
    return ESUCCESS;
  },
  fd_close(fd) {
    if (!fds.has(fd)) {
      return EBADF;
    }
    if (fd > 3) {
      fds.delete(fd);
    }
    return ESUCCESS;
  },
  fd_seek(fd, offset, whence, out) {
    const d = fds.get(fd);
    if (!d) {
      return EBADF;
    }
    if (d.kind !== "file") {
      return ESPIPE;
    }
    const base = whence === 0 ? 0 : whence === 1 ? d.pos : size(d.node);
    const pos = base + Number(offset);
    if (pos < 0) {
      return EINVAL;
    }
    d.pos = pos;
    view().setBigUint64(out, BigInt(pos), true);
    return ESUCCESS;
  },
  fd_read(fd, iovs, n, nread, slot) {
    const d = fds.get(fd);
    if (!d) {
      return EBADF;
    }
    if (d.kind === "stdin") {
      waiting.push([slot, iovs, n, nread]);
      feed();
      return undefined; // (Answered when input comes.)
    }
    if (d.kind === "dir") {
      return EISDIR;
    }
    if (d.kind !== "file") {
      return EBADF;
    }
    let total = 0;
    const m = mem();
    for (const [ptr, len] of iovecs(iovs, n)) {
      const want = Math.min(len, MOST - total);
      if (want <= 0) {
        break;
      }
      const got = readNode(d.node, d.pos, want);
      m.set(got, ptr);
      d.pos += got.length;
      total += got.length;
      if (got.length < want) {
        break;
      }
    }
    view().setUint32(nread, total, true);
    return ESUCCESS;
  },
  fd_write(fd, iovs, n, nwritten) {
    const d = fds.get(fd);
    if (!d) {
      return EBADF;
    }
    let total = 0;
    const m = mem();
    const parts = [];
    for (const [ptr, len] of iovecs(iovs, n)) {
      parts.push(m.slice(ptr, ptr + len));
      total += len;
    }
    if (d.kind === "stdout" || d.kind === "stderr") {
      const all = new Uint8Array(total);
      let at = 0;
      for (const p of parts) {
        all.set(p, at);
        at += p.length;
      }
      postMessage({ kind: d.kind, bytes: all }, [all.buffer]);
    } else if (d.kind === "file") {
      if (d.append) {
        d.pos = size(d.node);
      }
      for (const p of parts) {
        write(d.node, d.pos, p);
        d.pos += p.length;
      }
    } else {
      return EBADF;
    }
    view().setUint32(nwritten, total, true);
    return ESUCCESS;
  },
  fd_readdir(fd, buf, len, cookie, used) {
    const d = fds.get(fd);
    if (!d || d.kind !== "dir") {
      return EBADF;
    }
    const entries = [...d.node.children.entries()];
    const m = mem();
    const v = view();
    let at = 0;
    for (let i = Number(cookie); i < entries.length && at < len; i++) {
      const [name, node] = entries[i];
      const enc = new TextEncoder().encode(name);
      const head = new Uint8Array(24);
      const hv = new DataView(head.buffer);
      hv.setBigUint64(0, BigInt(i + 1), true);
      hv.setBigUint64(8, BigInt(node.ino || 0), true);
      hv.setUint32(16, enc.length, true);
      hv.setUint8(20, node.type);
      const entry = new Uint8Array(24 + enc.length);
      entry.set(head);
      entry.set(enc, 24);
      const take = Math.min(entry.length, len - at);
      m.set(entry.subarray(0, take), buf + at);
      at += take;
    }
    v.setUint32(used, at, true);
    return ESUCCESS;
  },
  path_create_directory(fd, ptr, len) {
    const a = at(fd, ptr, len);
    if (typeof a === "number") {
      return a;
    }
    if (typeof walk(a.base, a.parts) !== "number") {
      return EEXIST;
    }
    const parent = walk(a.base, a.parts.slice(0, -1));
    if (typeof parent === "number") {
      return parent;
    }
    parent.children.set(a.parts[a.parts.length - 1], dir());
    return ESUCCESS;
  },
  path_readlink(fd, ptr, len) {
    const a = at(fd, ptr, len);
    if (typeof a === "number") {
      return a;
    }
    const node = walk(a.base, a.parts);
    return typeof node === "number" ? node : EINVAL;
  },
  path_rename(fd, ptr, len, fd2, ptr2, len2) {
    const a = at(fd, ptr, len);
    const b = at(fd2, ptr2, len2);
    if (typeof a === "number") {
      return a;
    }
    if (typeof b === "number") {
      return b;
    }
    const from = walk(a.base, a.parts.slice(0, -1));
    const to = walk(b.base, b.parts.slice(0, -1));
    if (typeof from === "number" || typeof to === "number") {
      return ENOENT;
    }
    const name = a.parts[a.parts.length - 1];
    const node = from.children.get(name);
    if (!node) {
      return ENOENT;
    }
    from.children.delete(name);
    to.children.set(b.parts[b.parts.length - 1], node);
    return ESUCCESS;
  },
  path_unlink_file(fd, ptr, len) {
    const a = at(fd, ptr, len);
    if (typeof a === "number") {
      return a;
    }
    const parent = walk(a.base, a.parts.slice(0, -1));
    if (typeof parent === "number") {
      return parent;
    }
    const name = a.parts[a.parts.length - 1];
    const node = parent.children.get(name);
    if (!node) {
      return ENOENT;
    }
    if (node.type === DIR) {
      return node.children.size ? ENOTEMPTY : EISDIR;
    }
    parent.children.delete(name);
    return ESUCCESS;
  },
};

function answer(slot, errno) {
  if (!sys) {
    return;
  }
  Atomics.store(sys, slot * 2 + 1, errno);
  Atomics.store(sys, slot * 2, 1);
  Atomics.notify(sys, slot * 2);
}

/** A thread's call (through its port): answered now, or (stdin) later. */
function onCall(e) {
  const { op, args, slot } = e.data;
  let errno;
  try {
    const f = calls[op];
    errno = f ? f(...args, slot) : 52;
  } catch (err) {
    postMessage({ kind: "stderr", bytes: new TextEncoder().encode(`[web] ${op}: ${err}\n`) });
    errno = 29; // EIO
  }
  if (errno !== undefined) {
    answer(slot, errno);
  }
}

onmessage = (e) => {
  const m = e.data;
  switch (m.kind) {
    case "add":
      add(m.path, m.file);
      break;
    case "remove": {
      const parts = split(m.path);
      const parent = walk(root, parts.slice(0, -1));
      if (typeof parent !== "number") {
        parent.children.delete(parts[parts.length - 1]);
      }
      break;
    }
    case "start":
      memory = m.memory;
      sys = new Int32Array(m.sys);
      reset();
      break;
    case "stop":
      memory = null;
      sys = null;
      reset();
      break;
    case "port":
      m.port.onmessage = onCall;
      break;
    case "stdin":
      stdin.push(new TextEncoder().encode(m.text));
      feed();
      break;
    case "stdin-end":
      stdinEnded = true;
      feed();
      break;
  }
};
