// `procupine serve`, run and spoken to: requests answered by ID, sample
// events passed on. No VS Code here, so that the tests run it in Node.
"use strict";

const { spawn } = require("child_process");
const path = require("path");
const fs = require("fs");

/**
 * The procupine to run: `configured` (the procupine.path setting) if set;
 * else the one the extension's package carries (bin/, in a release VSIX);
 * else the build in the repository this extension is in (release, then
 * debug); else `procupine` on PATH. `exists` is for testing.
 */
function procupinePath(configured, exists = fs.existsSync, platform = process.platform) {
  if (configured) {
    return configured;
  }
  const exe = platform === "win32" ? "procupine.exe" : "procupine";
  const bundled = path.join(__dirname, "..", "bin", exe);
  if (exists(bundled)) {
    return bundled;
  }
  for (const build of ["release", "debug"]) {
    const candidate = path.join(__dirname, "..", "..", "..", "..", "target", build, exe);
    if (exists(candidate)) {
      return candidate;
    }
  }
  return "procupine";
}

/** One `procupine serve`. */
class Server {
  /**
   * `onEvent(event)` gets each sample (and error) event; `onExit(error)`
   * is called if it ends; `log(text)` its messages.
   */
  constructor(exe, { onEvent = () => {}, onExit = () => {}, log = () => {} } = {}) {
    this.exe = exe;
    this.onEvent = onEvent;
    this.onExit = onExit;
    this.log = log;
    this.child = null;
    this.next = 1;
    this.pending = new Map();
  }

  start() {
    const child = spawn(this.exe, ["serve"], { windowsHide: true, stdio: ["pipe", "pipe", "pipe"] });
    this.child = child;
    let parts = [];
    child.stdout.setEncoding("utf8");
    child.stdout.on("data", (chunk) => {
      let start = 0;
      let end;
      while ((end = chunk.indexOf("\n", start)) >= 0) {
        parts.push(chunk.slice(start, end));
        const line = parts.join("");
        parts = [];
        start = end + 1;
        if (line.trim()) {
          this.line(line);
        }
      }
      if (start < chunk.length) {
        parts.push(chunk.slice(start));
      }
    });
    child.stderr.setEncoding("utf8");
    child.stderr.on("data", (t) => this.log(`procupine serve: ${t.trimEnd()}`));
    child.stdin.on("error", (e) => this.log(`procupine serve's input: ${e.message}`));
    const gone = (why) => {
      if (this.child !== child) {
        return;
      }
      this.child = null;
      for (const { reject } of this.pending.values()) {
        reject(why);
      }
      this.pending.clear();
      this.onExit(why);
    };
    child.on("error", (e) =>
      gone(new Error(e.code === "ENOENT" ? `procupine was not found ("${this.exe}"); set procupine.path` : e.message))
    );
    child.on("exit", (code, signal) => gone(new Error(`procupine serve exited (${signal || code})`)));
  }

  line(text) {
    let v;
    try {
      v = JSON.parse(text);
    } catch {
      this.log(`procupine serve said: ${text.slice(0, 200)}`);
      return;
    }
    if (v.event) {
      this.onEvent(v);
      return;
    }
    const waiting = this.pending.get(v.id);
    if (!waiting) {
      return;
    }
    this.pending.delete(v.id);
    if (v.error !== undefined) {
      waiting.reject(new Error(v.error));
    } else {
      waiting.resolve(v.result);
    }
  }

  /** `method` with `params`: its result. */
  request(method, params = {}) {
    if (!this.child) {
      this.start();
    }
    const id = this.next++;
    return new Promise((resolve, reject) => {
      this.pending.set(id, { resolve, reject });
      this.child.stdin.write(JSON.stringify({ id, method, ...params }) + "\n");
    });
  }

  stop() {
    const child = this.child;
    this.child = null;
    if (child) {
      for (const { reject } of this.pending.values()) {
        reject(new Error("stopped"));
      }
      this.pending.clear();
      // (The end of its input ends it.)
      child.stdin.end();
    }
  }
}

module.exports = { Server, procupinePath };
