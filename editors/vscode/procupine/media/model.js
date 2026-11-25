// Procupine's model of the processes: what `procupine serve`'s samples say,
// kept with each process's history, and turned into the rows the view
// shows (a tree, or a flat list; sorted; filtered). No DOM: the view
// (view.js) draws it, and the tests (test/model.test.js) run it in Node.
"use strict";

(function (exports) {
  /** Samples of history kept for each process (and the system). */
  const HISTORY = 120;

  /** A ring of the last HISTORY numbers. */
  class Ring {
    constructor(n = HISTORY) {
      this.values = new Float64Array(n);
      this.count = 0;
      this.next = 0;
    }
    push(v) {
      this.values[this.next] = Number.isFinite(v) ? v : 0;
      this.next = (this.next + 1) % this.values.length;
      this.count = Math.min(this.count + 1, this.values.length);
    }
    /** The values, oldest first. */
    list() {
      const n = this.values.length;
      const out = new Array(this.count);
      for (let i = 0; i < this.count; i++) {
        out[i] = this.values[(this.next - this.count + i + n) % n];
      }
      return out;
    }
    last() {
      return this.count ? this.values[(this.next - 1 + this.values.length) % this.values.length] : 0;
    }
    max() {
      let m = 0;
      for (let i = 0; i < this.count; i++) {
        m = Math.max(m, this.values[i]);
      }
      return m;
    }
  }

  const list = (x) => (Array.isArray(x) ? x : []);

  // What a sample may hold, by type: anything else is dropped (null), so
  // the view never shows a value of a type it doesn't expect (markup where
  // a number belongs, say).
  const num = (x) => (typeof x === "number" && Number.isFinite(x) ? x : null);
  const str = (x) => (typeof x === "string" ? x : null);
  const bool = (x) => (typeof x === "boolean" ? x : null);
  const DETAILS = {
    id: num,
    pid: num,
    parent: num,
    parentPid: num,
    name: str,
    start: num,
    path: str,
    commandLine: str,
    user: str,
    architecture: str,
    elevated: bool,
    integrity: str,
    kernel: bool,
    session: num,
  };
  /** The rows' columns that aren't numbers. */
  const TEXT_COLUMNS = new Set(["state"]);
  const SYSTEM = ["cpus", "cpu", "memoryTotal", "memoryAvailable", "commit", "commitLimit", "swapTotal", "swapUsed", "uptime", "processes", "threads", "handles"];

  /** A process's details, as sent, with only what they may hold. */
  function cleanDetails(d) {
    const out = {};
    for (const k in DETAILS) {
      out[k] = DETAILS[k](d && d[k]);
    }
    out.name = out.name || "";
    return out;
  }

  /** The system's numbers, as sent, with only what they may hold. */
  function cleanSystem(s) {
    const out = {};
    for (const k of SYSTEM) {
      out[k] = num(s[k]);
    }
    return out;
  }

  /**
   * The processes, from `procupine serve`'s sample events. `columns` are
   * the rows' columns (the server's `columns`).
   */
  class Model {
    constructor(columns) {
      this.columns = list(columns).map((c) => String(c));
      columns = this.columns;
      this.index = {};
      columns.forEach((c, i) => (this.index[c] = i));
      /** Processes by session ID. */
      this.processes = new Map();
      /** Samples applied. */
      this.samples = 0;
      this.system = null;
      this.systemHistory = { cpu: new Ring(), memory: new Ring(), commit: new Ring() };
    }

    /**
     * Applies a sample event: processes started (with their details),
     * exited (kept, marked, for one sample), and their rows.
     */
    apply(event) {
      this.samples++;
      const now = this.samples;
      // Those that exited a sample ago are gone now.
      for (const [id, p] of this.processes) {
        if (p.exited !== null && p.exited < now) {
          this.processes.delete(id);
        }
      }
      for (const sent of list(event.added)) {
        const d = cleanDetails(sent);
        if (d.id === null) {
          continue;
        }
        const old = this.processes.get(d.id);
        if (old) {
          // (A refresh: everything again.)
          Object.assign(old, d, { v: old.v, history: old.history, seen: old.seen, exited: null });
          continue;
        }
        this.processes.set(d.id, {
          ...d,
          v: {},
          history: { cpu: new Ring(), private: new Ring(), io: new Ring() },
          seen: now,
          exited: null,
        });
      }
      for (const id of list(event.removed)) {
        const p = this.processes.get(num(id));
        if (p) {
          p.exited = now;
        }
      }
      const cols = this.columns;
      const updated = new Set();
      for (const row of list(event.rows)) {
        const p = Array.isArray(row) ? this.processes.get(num(row[0])) : null;
        if (!p) {
          continue;
        }
        for (let i = 1; i < cols.length; i++) {
          p.v[cols[i]] = TEXT_COLUMNS.has(cols[i]) ? str(row[i]) : num(row[i]);
        }
        p.parent = p.v.parent;
        updated.add(p);
      }
      // Every live process's history moves on, whether its row was sent or
      // not (unsent: unchanged).
      for (const p of this.processes.values()) {
        if (p.exited !== null) {
          continue;
        }
        p.history.cpu.push(p.v.cpu || 0);
        p.history.private.push(p.v.private || 0);
        p.history.io.push((p.v.readRate || 0) + (p.v.writeRate || 0));
      }
      if (event.system && typeof event.system === "object") {
        const s = cleanSystem(event.system);
        this.system = s;
        this.systemHistory.cpu.push(s.cpu);
        this.systemHistory.memory.push(s.memoryTotal ? 1 - s.memoryAvailable / s.memoryTotal : 0);
        this.systemHistory.commit.push(s.commitLimit ? s.commit / s.commitLimit : 0);
      }
      return updated;
    }

    get(id) {
      return this.processes.get(id);
    }

    /** Whether `p` started since the last sample (not in the first). */
    isNew(p) {
      return this.samples > 1 && p.seen === this.samples && p.exited === null;
    }

    /** Whether `p` exited in the last sample. */
    isExited(p) {
      return p.exited !== null;
    }

    /** Each process's children, by ID (roots under `null`). */
    children() {
      const kids = new Map();
      kids.set(null, []);
      for (const p of this.processes.values()) {
        const parent = p.parent != null && this.processes.has(p.parent) ? p.parent : null;
        if (!kids.has(parent)) {
          kids.set(parent, []);
        }
        kids.get(parent).push(p);
      }
      return kids;
    }

    /** `id` and every process descended from it, each before its children. */
    descendants(id) {
      const kids = this.children();
      const out = [];
      const stack = [id];
      const seen = new Set();
      while (stack.length) {
        const i = stack.pop();
        if (seen.has(i)) {
          continue;
        }
        seen.add(i);
        out.push(i);
        const c = kids.get(i) || [];
        for (let k = c.length - 1; k >= 0; k--) {
          stack.push(c[k].id);
        }
      }
      return out;
    }

    /** The ancestors of `id`, nearest first. */
    ancestors(id) {
      const out = [];
      const seen = new Set([id]);
      let p = this.processes.get(id);
      while (p && p.parent != null && !seen.has(p.parent) && this.processes.has(p.parent)) {
        seen.add(p.parent);
        out.push(p.parent);
        p = this.processes.get(p.parent);
      }
      return out;
    }

    /**
     * The rows to show: `{ p, depth, kids, open, match }`.
     *
     * - `tree`: by parent, each process's children after it, siblings in
     *   `sort`'s order; else every process, in `sort`'s order.
     * - `sort`: `{ key, desc }` (a column, by `sortValue`); none: by start.
     * - `filter`: text a process's name, ID, user, path, or command line
     *   holds (any case); in a tree, the ancestors of those that match are
     *   shown too (`match` false).
     * - `collapsed`: the IDs whose children are hidden (in a tree).
     * - `hide`: a function of a process, true for those not to show.
     */
    rows({ tree = true, sort = null, filter = "", collapsed = new Set(), hide = null } = {}) {
      const f = filter.trim().toLowerCase();
      const matches = (p) => !f || matchesFilter(p, f);
      const visible = (p) => !hide || !hide(p);
      const cmp = comparator(sort);
      if (!tree) {
        const list = [...this.processes.values()].filter((p) => visible(p) && matches(p));
        list.sort(cmp);
        return list.map((p) => ({ p, depth: 0, kids: false, open: false, match: true }));
      }
      const kids = this.children();
      // With a filter: what matches, and its ancestors.
      let keep = null;
      if (f) {
        keep = new Map();
        for (const p of this.processes.values()) {
          if (visible(p) && matches(p)) {
            keep.set(p.id, true);
            for (const a of this.ancestors(p.id)) {
              if (!keep.has(a)) {
                keep.set(a, false);
              }
            }
          }
        }
      }
      const shown = (p) => (keep ? keep.has(p.id) : visible(p));
      const out = [];
      const seen = new Set();
      const walk = (list, depth) => {
        const sorted = list.filter(shown).sort(cmp);
        for (const p of sorted) {
          if (seen.has(p.id)) {
            continue;
          }
          seen.add(p.id);
          const children = (kids.get(p.id) || []).filter(shown);
          // (Filtered: every match shown, collapsed or not.)
          const open = !collapsed.has(p.id) || !!f;
          out.push({ p, depth, kids: children.length > 0, open, match: keep ? keep.get(p.id) : true });
          if (open && children.length) {
            walk(children, depth + 1);
          }
        }
      };
      walk(kids.get(null) || [], 0);
      return out;
    }

    /** The processes' totals. */
    totals() {
      let cpu = 0;
      let privateBytes = 0;
      for (const p of this.processes.values()) {
        if (p.exited === null) {
          cpu += p.v.cpu || 0;
          privateBytes += p.v.private || 0;
        }
      }
      return { cpu, private: privateBytes };
    }
  }

  function matchesFilter(p, f) {
    return (
      String(p.pid) === f ||
      (p.name || "").toLowerCase().includes(f) ||
      (p.user || "").toLowerCase().includes(f) ||
      (p.path || "").toLowerCase().includes(f) ||
      (p.commandLine || "").toLowerCase().includes(f)
    );
  }

  /** A process's value for sorting by `key`. */
  function sortValue(p, key) {
    const v = p.v;
    switch (key) {
      case "name":
        return (p.name || "").toLowerCase();
      case "pid":
        return p.pid;
      case "user":
        return (p.user || "").toLowerCase();
      case "path":
        return (p.path || "").toLowerCase();
      case "commandLine":
        return (p.commandLine || "").toLowerCase();
      case "start":
        return p.start || 0;
      case "io":
        return (v.readRate || 0) + (v.writeRate || 0);
      case "cpuTime":
        return (v.userTime || 0) + (v.kernelTime || 0);
      case "cpuHistory":
        return p.history.cpu.max();
      case "architecture":
      case "integrity":
        return p[key] || "";
      default: {
        const x = v[key];
        return x == null ? -Infinity : x;
      }
    }
  }

  /** A comparator of processes for `sort` (`{ key, desc }`); none: by start, then ID. */
  function comparator(sort) {
    const byStart = (a, b) => (a.start || 0) - (b.start || 0) || a.pid - b.pid;
    if (!sort || !sort.key) {
      return byStart;
    }
    const dir = sort.desc ? -1 : 1;
    return (a, b) => {
      const x = sortValue(a, sort.key);
      const y = sortValue(b, sort.key);
      if (x < y) {
        return -dir;
      }
      if (x > y) {
        return dir;
      }
      return byStart(a, b);
    };
  }

  // Formatting.

  function bytes(n) {
    if (n == null) {
      return "";
    }
    const units = ["B", "KiB", "MiB", "GiB", "TiB", "PiB"];
    let v = n;
    let u = 0;
    while (Math.abs(v) >= 1024 && u + 1 < units.length) {
      v /= 1024;
      u++;
    }
    return u === 0 ? `${n} B` : `${v.toFixed(v < 10 ? 2 : v < 100 ? 1 : 0)} ${units[u]}`;
  }

  /** A rate in bytes per second; empty for none (or nothing). */
  function rate(n) {
    if (n == null || n === 0) {
      return "";
    }
    return `${bytes(Math.round(n))}/s`;
  }

  /** A share (0 to 1) as Process Explorer shows CPU: "< 0.01" for a trace. */
  function percent(share) {
    if (share == null || share === 0) {
      return "";
    }
    const pct = share * 100;
    return pct < 0.01 ? "< 0.01" : pct.toFixed(2);
  }

  /** Milliseconds as h:mm:ss.mmm. */
  function duration(ms) {
    if (ms == null) {
      return "";
    }
    const h = Math.floor(ms / 3600000);
    const m = Math.floor((ms % 3600000) / 60000);
    const s = Math.floor((ms % 60000) / 1000);
    const r = Math.floor(ms % 1000);
    const two = (x) => String(x).padStart(2, "0");
    return `${h}:${two(m)}:${two(s)}.${String(r).padStart(3, "0")}`;
  }

  function count(n) {
    return n == null ? "" : n.toLocaleString("en-US");
  }

  /** A start time: the time today, else the date and time. */
  function startTime(ms, now = Date.now()) {
    if (!ms) {
      return "";
    }
    const d = new Date(ms);
    const today = new Date(now);
    const two = (x) => String(x).padStart(2, "0");
    const time = `${two(d.getHours())}:${two(d.getMinutes())}:${two(d.getSeconds())}`;
    if (d.toDateString() === today.toDateString()) {
      return time;
    }
    return `${d.getFullYear()}-${two(d.getMonth() + 1)}-${two(d.getDate())} ${time}`;
  }

  /** An SVG polyline's points for `values` in a `w` by `h` box, scaled to `max`. */
  function sparkPoints(values, w, h, max, slots = HISTORY) {
    if (!values.length) {
      return "";
    }
    const top = max > 0 ? max : 1;
    const step = w / Math.max(1, slots - 1);
    const x0 = w - step * (values.length - 1);
    return values
      .map((v, i) => `${(x0 + i * step).toFixed(1)},${(h - (Math.min(v, top) / top) * (h - 1) - 0.5).toFixed(1)}`)
      .join(" ");
  }

  /** Whether `user` (as the server names it) is `me` (this OS user's name). */
  function isUser(user, me) {
    if (!user || !me) {
      return false;
    }
    const name = user.includes("\\") ? user.slice(user.lastIndexOf("\\") + 1) : user;
    return name.toLowerCase() === me.toLowerCase();
  }

  Object.assign(exports, {
    HISTORY,
    Ring,
    Model,
    sortValue,
    comparator,
    matchesFilter,
    bytes,
    rate,
    percent,
    duration,
    count,
    startTime,
    sparkPoints,
    isUser,
  });
})(typeof module !== "undefined" && module.exports ? module.exports : (window.ProcupineModel = {}));
